fn try_operation_gate(gate: &Mutex<()>) -> Result<std::sync::MutexGuard<'_, ()>, GuiError> {
    match gate.try_lock() {
        Ok(guard) => Ok(guard),
        Err(std::sync::TryLockError::Poisoned(error)) => Ok(error.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => Err(GuiError::new(
            "operation_busy",
            "An account or settings operation is still finishing",
        )),
    }
}

fn ensure_idle(state: &DesktopState) -> Result<(), GuiError> {
    let _gate = try_operation_gate(&state.operation_gate)?;
    check_idle(state.closing.as_ref(), &state.tasks)
}

fn check_idle(
    closing: &std::sync::atomic::AtomicBool,
    tasks: &TaskRegistry,
) -> Result<(), GuiError> {
    if closing.load(Ordering::Acquire) {
        Err(GuiError::new("app_closing", "The launcher is closing"))
    } else if tasks.active() {
        Err(GuiError::new(
            "task_busy",
            "Wait for the active task to finish",
        ))
    } else {
        Ok(())
    }
}
fn with_idle_gate<T>(
    gate: &Mutex<()>,
    closing: &std::sync::atomic::AtomicBool,
    tasks: &TaskRegistry,
    operation: impl FnOnce() -> Result<T, GuiError>,
) -> Result<T, GuiError> {
    let _guard = try_operation_gate(gate)?;
    check_idle(closing, tasks)?;
    operation()
}

fn start_task<F, Fut>(state: &DesktopState, kind: &str, work: F) -> Result<TaskSnapshot, GuiError>
where
    F: FnOnce(ccl_gui::TaskHandle) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<Value, GuiError>> + Send + 'static,
{
    let _gate = try_operation_gate(&state.operation_gate)?;
    check_idle(state.closing.as_ref(), &state.tasks)?;
    state.tasks.start(kind, work)
}

fn start_task_policy<F, Fut>(
    state: &DesktopState,
    kind: &str,
    cancellable: bool,
    work: F,
) -> Result<TaskSnapshot, GuiError>
where
    F: FnOnce(ccl_gui::TaskHandle) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<Value, GuiError>> + Send + 'static,
{
    let _gate = try_operation_gate(&state.operation_gate)?;
    check_idle(state.closing.as_ref(), &state.tasks)?;
    state.tasks.start_with_cancellation(kind, cancellable, work)
}

fn resolved_root(
    store: &PreferencesStore,
    config: &std::path::Path,
    explicit: Option<PathBuf>,
) -> Result<PathBuf, GuiError> {
    let selected = explicit
        .or(store.root()?)
        .ok_or_else(|| GuiError::new("setup_required", "Confirm the game directory first"))?;
    let mut inputs = ccl_core::directory::DirectoryInputs::from_environment(Some(selected))
        .map_err(|error| GuiError::new("directory_error", error.to_string()))?;
    inputs.working_directory = inputs.home_directory.clone();
    let resolution = inputs
        .resolve()
        .map_err(|error| GuiError::new("directory_error", error.to_string()))?;
    if !resolution.exists {
        return Err(GuiError::new(
            "setup_required",
            "Select an existing game directory",
        ));
    }
    let root = std::fs::canonicalize(&resolution.path).map_err(|_| {
        GuiError::new(
            "directory_error",
            "Cannot resolve the selected game directory",
        )
    })?;
    if !ccl_core::compatibility::inspect_compatibility(config, &root).compatible {
        return Err(GuiError::new(
            "library_unavailable",
            "Incompatible configuration or instance metadata",
        ));
    }
    Ok(root)
}
fn operations_for(
    store: &PreferencesStore,
    config: &std::path::Path,
    explicit: Option<PathBuf>,
) -> Result<GuiOperations, GuiError> {
    GuiOperations::new(
        resolved_root(store, config, explicit)?,
        config.to_path_buf(),
    )
}
async fn operations_blocking(state: &DesktopState) -> Result<GuiOperations, GuiError> {
    let store = state.store.clone();
    let config = state.config_path();
    let explicit = state.explicit_root.clone();
    tauri::async_runtime::spawn_blocking(move || operations_for(&store, &config, explicit))
        .await
        .map_err(|_| GuiError::new("worker_error", "Directory worker stopped"))?
}
fn account_service(config: &std::path::Path) -> Result<AccountService<OsSecretStore>, GuiError> {
    let accounts = config
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("accounts.json");
    let vault = OsSecretStore::for_account_file(&accounts)
        .map_err(|e| GuiError::new("account_storage", e.to_string()))?;
    let config_data = LauncherConfig::read_or_default(config)
        .map_err(|e| GuiError::new("configuration_error", e.to_string()))?;
    Ok(AccountService::new(config, Arc::new(vault), config_data))
}
#[tauri::command]
async fn initialize_root(state: State<'_, DesktopState>) -> Result<Bootstrap, GuiError> {
    ensure_idle(&state)?;
    let store = state.store.clone();
    let config = state.config_path();
    let explicit = state.explicit_root.clone();
    let gate = state.operation_gate.clone();
    let closing = state.closing.clone();
    let tasks = state.tasks.clone();
    tauri::async_runtime::spawn_blocking(move || {
        with_idle_gate(&gate, &closing, &tasks, || {
            ccl_gui::initialize_root(&store, &config, explicit)
        })
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Directory worker stopped"))?
}
#[tauri::command]
async fn list_versions(state: State<'_, DesktopState>) -> Result<Value, GuiError> {
    operations_blocking(&state).await?.list_versions().await
}
#[tauri::command]
async fn list_loaders(
    state: State<'_, DesktopState>,
    minecraft: String,
    loader: LoaderKind,
) -> Result<Value, GuiError> {
    operations_blocking(&state)
        .await?
        .list_loaders(&minecraft, loader)
        .await
}
#[tauri::command]
async fn install_instance(
    state: State<'_, DesktopState>,
    request: InstallRequest,
) -> Result<TaskSnapshot, GuiError> {
    ensure_idle(&state)?;
    let service = operations_blocking(&state).await?;
    start_task(&state, "install", move |h| async move {
        service.install(request, h.context).await
    })
}
#[tauri::command]
async fn inspect_import(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<Value>, GuiError> {
    ensure_idle(&state)?;
    let selected = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("Modpack", &["zip", "mrpack"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "File picker stopped"))?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|_| GuiError::new("invalid_archive", "Select a local archive"))?;
    let store = state.store.clone();
    let config = state.config_path();
    let explicit = state.explicit_root.clone();
    let summary_path = path.clone();
    let summary = tauri::async_runtime::spawn_blocking(move || {
        operations_for(&store, &config, explicit)?.inspect_import(&summary_path)
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Archive inspection worker stopped"))??;
    let gate = state.operation_gate.clone();
    let closing = state.closing.clone();
    let tasks = state.tasks.clone();
    with_idle_gate(&gate, &closing, &tasks, || {
        *state
            .selected_archive
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
        Ok(())
    })?;
    Ok(Some(json!({"path":path,"summary":summary})))
}
#[tauri::command]
async fn import_instance(
    state: State<'_, DesktopState>,
    request: ImportRequest,
) -> Result<TaskSnapshot, GuiError> {
    if state
        .selected_archive
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        != Some(&request.archive)
    {
        return Err(GuiError::new(
            "archive_not_selected",
            "Select and inspect the archive first",
        ));
    }
    ensure_idle(&state)?;
    let service = operations_blocking(&state).await?;
    start_task(&state, "import", move |h| async move {
        service.import(request, h.context).await
    })
}
#[tauri::command]
async fn launch_game(
    state: State<'_, DesktopState>,
    request: LaunchRequest,
) -> Result<TaskSnapshot, GuiError> {
    ensure_idle(&state)?;
    let service = operations_blocking(&state).await?;
    start_task(&state, "launch", move |h| async move {
        service.launch(request, h.context).await
    })
}
#[tauri::command]
fn task_history(state: State<'_, DesktopState>) -> Vec<TaskSnapshot> {
    state.tasks.history()
}
#[tauri::command]
async fn account_list(state: State<'_, DesktopState>) -> Result<AccountListing, GuiError> {
    let config = state.config_path();
    tauri::async_runtime::spawn_blocking(move || account_service(&config)?.list())
        .await
        .map_err(|_| GuiError::new("worker_error", "Account worker stopped"))?
}
#[tauri::command]
async fn account_select(
    state: State<'_, DesktopState>,
    id: String,
) -> Result<AccountListing, GuiError> {
    ensure_idle(&state)?;
    let config = state.config_path();
    let gate = state.operation_gate.clone();
    let closing = state.closing.clone();
    let tasks = state.tasks.clone();
    tauri::async_runtime::spawn_blocking(move || {
        with_idle_gate(&gate, &closing, &tasks, || {
            let service = account_service(&config)?;
            service.select(&id)?;
            service.list()
        })
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Account worker stopped"))?
}
#[tauri::command]
async fn account_logout(
    state: State<'_, DesktopState>,
    id: String,
) -> Result<AccountListing, GuiError> {
    ensure_idle(&state)?;
    let config = state.config_path();
    let gate = state.operation_gate.clone();
    let closing = state.closing.clone();
    let tasks = state.tasks.clone();
    tauri::async_runtime::spawn_blocking(move || {
        with_idle_gate(&gate, &closing, &tasks, || {
            let service = account_service(&config)?;
            service.logout(&id)?;
            service.list()
        })
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Account worker stopped"))?
}
#[tauri::command]
async fn account_check_storage(state: State<'_, DesktopState>) -> Result<Value, GuiError> {
    ensure_idle(&state)?;
    let config = state.config_path();
    let gate = state.operation_gate.clone();
    let closing = state.closing.clone();
    let tasks = state.tasks.clone();
    tauri::async_runtime::spawn_blocking(move || {
        with_idle_gate(&gate, &closing, &tasks, || {
            ccl_gui::account_service::check_storage(config)?;
            Ok(json!({"available":true}))
        })
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Credential worker stopped"))?
}
#[tauri::command]
fn account_login(
    state: State<'_, DesktopState>,
    client_id: Option<String>,
    open_browser: bool,
) -> Result<TaskSnapshot, GuiError> {
    ensure_idle(&state)?;
    let config = state.config_path();
    start_task(&state, "login", move |h| async move {
        ccl_gui::account_service::check_storage(&config)?;
        let service = account_service(&config)?;
        let (login, authorization) = service
            .begin_login(client_id, &h.context.cancellation)
            .await?;
        let uri = authorization.verification_uri.clone();
        h.authorization(authorization);
        if open_browser
            && ccl_gui::browser::open_verification_page(&uri)
                .await
                .is_err()
        {
            let _ = h.context.emit(ccl_core::TaskEvent::Log {
                stream: "info".into(),
                line: "Browser could not be opened; use the displayed verification URL and code."
                    .into(),
            });
        }
        let account = service
            .complete_login(login, &h.context.cancellation)
            .await?;
        Ok(json!({"account":account}))
    })
}
#[tauri::command]
fn account_refresh(state: State<'_, DesktopState>, id: String) -> Result<TaskSnapshot, GuiError> {
    ensure_idle(&state)?;
    let config = state.config_path();
    start_task(&state, "refresh", move |h| async move {
        let account = account_service(&config)?
            .refresh(&id, &h.context.cancellation)
            .await?;
        Ok(json!({"account":account}))
    })
}
#[tauri::command]
async fn open_login_browser(state: State<'_, DesktopState>, task_id: u64) -> Result<(), GuiError> {
    let snapshot = state
        .tasks
        .snapshot()
        .filter(|s| s.id == task_id && s.kind == "login" && s.state == ccl_gui::TaskState::Running)
        .ok_or_else(|| GuiError::new("task_not_found", "No active login"))?;
    let authorization = snapshot
        .authorization
        .ok_or_else(|| GuiError::new("auth_pending", "Device code is not available yet"))?;
    ccl_gui::browser::open_verification_page(&authorization.verification_uri)
        .await
        .map_err(|e| GuiError::new("browser_error", e))
}
#[tauri::command]
async fn log_runs(state: State<'_, DesktopState>, key: String) -> Result<Value, GuiError> {
    let store = state.store.clone();
    let config = state.config_path();
    let explicit = state.explicit_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        operations_for(&store, &config, explicit)?.log_runs(&key)
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Log worker stopped"))?
}
#[tauri::command]
async fn read_run_log(
    state: State<'_, DesktopState>,
    key: String,
    run_id: Option<String>,
    lines: usize,
) -> Result<Value, GuiError> {
    let store = state.store.clone();
    let config = state.config_path();
    let explicit = state.explicit_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        operations_for(&store, &config, explicit)?.read_run_log(
            &key,
            run_id.as_deref(),
            lines.min(1000),
        )
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Log worker stopped"))?
}
#[tauri::command]
async fn set_network_offline(
    state: State<'_, DesktopState>,
    offline: bool,
) -> Result<Bootstrap, GuiError> {
    ensure_idle(&state)?;
    let config = state.config_path();
    let store = state.store.clone();
    let root = state.explicit_root.clone();
    let gate = state.operation_gate.clone();
    let closing = state.closing.clone();
    let tasks = state.tasks.clone();
    tauri::async_runtime::spawn_blocking(move || {
        with_idle_gate(&gate, &closing, &tasks, || {
            ccl_core::config::ConfigStore::new(&config)
                .set("downloads.offline", json!(offline))
                .map_err(|e| GuiError::new("configuration_error", e.to_string()))?;
            ccl_gui::bootstrap(&store, &config, root)
        })
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Configuration worker stopped"))?
}
