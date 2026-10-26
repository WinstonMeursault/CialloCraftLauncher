use ccl_gui::instance_service::{InstanceOperations, InstanceRequest as InstanceAction};
use ccl_gui::maintenance::{Maintenance, MaintenanceRequest};
use ccl_gui::online_service::{OnlineMutation, OnlineQuery, OnlineService};

fn require_grant(
    state: &DesktopState,
    purpose: &str,
    path: &std::path::Path,
) -> Result<(), GuiError> {
    if state
        .path_grants
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(purpose)
        .is_some_and(|p| p == path)
    {
        Ok(())
    } else {
        Err(GuiError::new(
            "path_not_selected",
            "Choose this path in the native file picker first",
        ))
    }
}
fn validate_instance_paths(state: &DesktopState, request: &InstanceAction) -> Result<(), GuiError> {
    match request {
        InstanceAction::PackInspect { archive }
        | InstanceAction::ContentRestore { archive, .. } => {
            require_grant(state, "archive", archive)
        }
        InstanceAction::PackExport { output, .. } => require_grant(state, "export", output),
        InstanceAction::ContentBackup { output, .. } => require_grant(state, "backup", output),
        InstanceAction::ContentAdd { source, .. } => require_grant(state, "content_file", source)
            .or_else(|_| require_grant(state, "content_directory", source)),
        _ => Ok(()),
    }
}
async fn root_config(state: &DesktopState) -> Result<(PathBuf, LauncherConfig), GuiError> {
    let store = state.store.clone();
    let config = state.config_path();
    let explicit = state.explicit_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        Ok((
            resolved_root(&store, &config, explicit)?,
            LauncherConfig::read_or_default(&config)
                .map_err(|e| GuiError::new("configuration_error", e.to_string()))?,
        ))
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Directory worker stopped"))?
}
#[tauri::command]
async fn choose_path(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
    purpose: String,
) -> Result<Option<PathBuf>, GuiError> {
    ensure_idle(&state)?;
    if !matches!(
        purpose.as_str(),
        "content_file"
            | "content_directory"
            | "archive"
            | "export"
            | "backup"
            | "diagnostic"
            | "installation"
    ) {
        return Err(GuiError::new("invalid_purpose", "Unknown picker purpose"));
    }
    let p = purpose.clone();
    let selected = tauri::async_runtime::spawn_blocking(move || {
        let picker = app.dialog().file();
        let selected = match p.as_str() {
            "content_directory" | "installation" => picker.blocking_pick_folder(),
            "export" => picker
                .add_filter("Modrinth pack", &["mrpack"])
                .blocking_save_file(),
            "backup" => picker.add_filter("Backup", &["zip"]).blocking_save_file(),
            "diagnostic" => picker
                .add_filter("Diagnostic", &["json"])
                .blocking_save_file(),
            "archive" => picker
                .add_filter("Archive", &["zip", "mrpack"])
                .blocking_pick_file(),
            _ => picker.blocking_pick_file(),
        };
        selected
            .map(|p| {
                p.into_path()
                    .map_err(|_| GuiError::new("invalid_path", "Choose a local path"))
            })
            .transpose()
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "File picker stopped"))??;
    if let Some(path) = &selected {
        let _gate = try_operation_gate(&state.operation_gate)?;
        check_idle(&state.closing, &state.tasks)?;
        state
            .path_grants
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(purpose, path.clone());
    }
    Ok(selected)
}
#[tauri::command]
async fn instance_query(
    state: State<'_, DesktopState>,
    request: InstanceAction,
) -> Result<Value, GuiError> {
    if request.mutates() {
        return Err(GuiError::new(
            "invalid_query",
            "Use a task for instance changes",
        ));
    }
    validate_instance_paths(&state, &request)?;
    let (root, config) = root_config(&state).await?;
    tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(
            InstanceOperations::new(root, config.downloads).execute(request, None),
        )
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Instance worker stopped"))?
}
#[tauri::command]
async fn instance_mutate(
    state: State<'_, DesktopState>,
    request: InstanceAction,
) -> Result<TaskSnapshot, GuiError> {
    ensure_idle(&state)?;
    validate_instance_paths(&state, &request)?;
    if !request.mutates() {
        return Err(GuiError::new(
            "invalid_mutation",
            "Use a query for read-only requests",
        ));
    }
    let (root, config) = root_config(&state).await?;
    start_task_policy(
        &state,
        "instance",
        matches!(
            &request,
            InstanceAction::Install { .. }
                | InstanceAction::Update { .. }
                | InstanceAction::UninstallLoader { .. }
        ),
        move |h| async move {
            tauri::async_runtime::spawn_blocking(move || {
                tauri::async_runtime::block_on(
                    InstanceOperations::new(root, config.downloads)
                        .execute(request, Some(h.context)),
                )
            })
            .await
            .map_err(|_| GuiError::new("worker_error", "Instance worker stopped"))?
        },
    )
}
#[tauri::command]
async fn online_query(
    state: State<'_, DesktopState>,
    request: OnlineQuery,
) -> Result<Value, GuiError> {
    let (root, _) = root_config(&state).await?;
    let config = state.config_path();
    OnlineService::new(root, config)?.query(request).await
}
#[tauri::command]
async fn online_mutate(
    state: State<'_, DesktopState>,
    request: OnlineMutation,
) -> Result<TaskSnapshot, GuiError> {
    ensure_idle(&state)?;
    let (root, _) = root_config(&state).await?;
    let config = state.config_path();
    start_task(&state, "online", move |h| async move {
        OnlineService::new(root, config)?
            .mutate(request, h.context)
            .await
    })
}
#[tauri::command]
async fn maintenance(
    state: State<'_, DesktopState>,
    request: MaintenanceRequest,
) -> Result<Value, GuiError> {
    ensure_idle(&state)?;
    if let MaintenanceRequest::ExportDiagnostics { output, .. } = &request {
        require_grant(&state, "diagnostic", output)?;
    }
    let store = state.store.clone();
    let config = state.config_path();
    let explicit = state.explicit_root.clone();
    let gate = state.operation_gate.clone();
    let closing = state.closing.clone();
    let tasks = state.tasks.clone();
    tauri::async_runtime::spawn_blocking(move || {
        with_idle_gate(&gate, &closing, &tasks, || {
            // Settings reset is available even when malformed config blocks library bootstrap.
            let root = explicit.or(store.root()?).unwrap_or_else(|| {
                config
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join(".minecraft")
            });
            Maintenance { root, config }.execute(request)
        })
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Maintenance worker stopped"))?
}
#[tauri::command]
async fn inspect_dropped(
    state: State<'_, DesktopState>,
    archive: PathBuf,
) -> Result<Value, GuiError> {
    ensure_idle(&state)?;
    require_grant(&state, "drop", &archive)?;
    let service = operations_blocking(&state).await?;
    let path = archive.clone();
    let summary = tauri::async_runtime::spawn_blocking(move || service.inspect_import(&path))
        .await
        .map_err(|_| GuiError::new("worker_error", "Archive worker stopped"))??;
    let _gate = try_operation_gate(&state.operation_gate)?;
    check_idle(&state.closing, &state.tasks)?;
    *state
        .selected_archive
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(archive.clone());
    Ok(json!({"path":archive,"summary":summary}))
}

#[tauri::command]
async fn update_installation(
    state: State<'_, DesktopState>,
    request: ccl_gui::update_service::UpdateRequest,
) -> Result<TaskSnapshot, GuiError> {
    use ccl_gui::update_service::{UpdateGuiService, UpdateRequest};
    ensure_idle(&state)?;
    let installation = match &request {
        UpdateRequest::Status { installation_root }
        | UpdateRequest::Check {
            installation_root, ..
        }
        | UpdateRequest::Apply {
            installation_root, ..
        }
        | UpdateRequest::Rollback { installation_root }
        | UpdateRequest::Recover { installation_root } => installation_root,
    };
    require_grant(&state, "installation", installation)?;
    let (root, config) = root_config(&state).await?;
    let path = state.config_path();
    start_task_policy(
        &state,
        "update",
        matches!(
            &request,
            UpdateRequest::Check { .. } | UpdateRequest::Apply { .. }
        ),
        move |h| async move {
            let result = UpdateGuiService::new(config.downloads, path, root)
                .execute_with_context(request, h.context)
                .await?;
            serde_json::to_value(result)
                .map_err(|_| GuiError::new("update_error", "Cannot serialize update result"))
        },
    )
}

#[tauri::command]
async fn select_config(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<Bootstrap>, GuiError> {
    ensure_idle(&state)?;
    if state.config_pinned {
        return Err(GuiError::new(
            "config_pinned",
            "Configuration is pinned by CCL_GUI_CONFIG_PATH",
        ));
    }
    let selected = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("CCL configuration", &["json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "File picker stopped"))?;
    let Some(path) = selected else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|_| GuiError::new("invalid_path", "Choose a local configuration"))?;
    let store = state.store.clone();
    let explicit = state.explicit_root.clone();
    let gate = state.operation_gate.clone();
    let closing = state.closing.clone();
    let tasks = state.tasks.clone();
    let current_config = state.config.clone();
    let grants = state.path_grants.clone();
    let archive = state.selected_archive.clone();
    tauri::async_runtime::spawn_blocking(move || {
        with_idle_gate(&gate, &closing, &tasks, || {
            LauncherConfig::read(&path)
                .map_err(|e| GuiError::new("configuration_error", e.to_string()))?;
            let result = ccl_gui::bootstrap(&store, &path, explicit)?;
            store.select_config(path.clone())?;
            *current_config.lock().unwrap_or_else(|e| e.into_inner()) = path;
            grants.lock().unwrap_or_else(|e| e.into_inner()).clear();
            *archive.lock().unwrap_or_else(|e| e.into_inner()) = None;
            Ok(Some(result))
        })
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Configuration worker stopped"))?
}
