#[tauri::command]
fn gui_ready() -> Result<(), GuiError> {
    ccl_gui::self_update::acknowledge_health()?;
    if let (Some(marker), Some(data)) = (
        std::env::var_os("CCL_GUI_SMOKE_MARKER"),
        std::env::var_os("CCL_GUI_DATA_DIR"),
    ) {
        let marker = PathBuf::from(marker);
        let data = PathBuf::from(data);
        if marker.parent().and_then(|p| p.canonicalize().ok()) != data.canonicalize().ok()
            || !marker.is_absolute()
        {
            return Err(GuiError::new(
                "smoke_marker",
                "Readiness marker must be inside the explicit isolated GUI data directory",
            ));
        }
        let build = ccl_gui::self_update::current_identity();
        std::fs::write(
            marker,
            serde_json::to_vec(
                &json!({"status":"ui_ready","pid":std::process::id(),"build":build}),
            )
            .map_err(|e| GuiError::new("smoke_marker", e.to_string()))?,
        )
        .map_err(|e| GuiError::new("smoke_marker", e.to_string()))?;
    }
    Ok(())
}

#[tauri::command]
async fn gui_self_update(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
    request: ccl_gui::self_update::Request,
) -> Result<ccl_gui::self_update::Response, GuiError> {
    use ccl_gui::self_update::{Request, Service};
    let mutates = matches!(
        request,
        Request::Apply { .. } | Request::Rollback | Request::Recover
    );
    let installs_new = matches!(request, Request::Apply { .. });
    ensure_idle(&state)?;
    if mutates {
        let zh = state.store.load().map_or(true, |p| p.language == "zh-CN");
        let dialog_app = app.clone();
        let accepted = tauri::async_runtime::spawn_blocking(move || {
            dialog_app.dialog().message(if zh {
                "即将更新或恢复启动器应用包。启动器会退出并重新启动；更新后无法正常启动时将自动恢复旧版本。是否继续？"
            } else {
                "The launcher package will be updated or restored. The launcher will quit and restart; startup failure after an update restores the previous version. Continue?"
            }).title("CialloCraftLauncher").kind(MessageDialogKind::Warning).buttons(MessageDialogButtons::YesNo).blocking_show()
        }).await.map_err(|_|GuiError::new("worker_error","Update confirmation stopped"))?;
        if !accepted {
            return Err(GuiError::new("cancelled", "Desktop update cancelled"));
        }
    }
    let config_path = state.config_path();
    let config = match LauncherConfig::read_or_default(&config_path) {
        Ok(config) => config,
        Err(_)
            if matches!(
                request,
                Request::Status | Request::Rollback | Request::Recover
            ) =>
        {
            LauncherConfig::default()
        }
        Err(e) => return Err(GuiError::new("configuration_error", e.to_string())),
    };
    if mutates {
        let _gate = try_operation_gate(&state.operation_gate)?;
        check_idle(&state.closing, &state.tasks)?;
        state.closing.store(true, Ordering::Release);
        state.update_in_progress.store(true, Ordering::Release);
    }
    let result = async {
        if installs_new {
            let root = resolved_root(&state.store, &config_path, state.explicit_root.clone())?;
            let report = ccl_core::compatibility::inspect_compatibility(&config_path, &root);
            if !report.compatible {
                return Err(GuiError::new(
                    "update_compatibility",
                    "Existing data is incompatible; inspect diagnostics before updating",
                ));
            }
        }
        Service::new(&config.downloads)?
            .execute(
                request,
                state.store.directory().join("update"),
                ccl_core::CancellationToken::default(),
            )
            .await
    }
    .await;
    if mutates {
        state.update_in_progress.store(false, Ordering::Release);
        if result.as_ref().is_ok_and(|r| r.restart_required) {
            // Let the response reach the visible UI, then release the process
            // guard by exiting. The helper waits for that guard before replacing.
            let exit_app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                exit_app.exit(0);
            });
        } else {
            state.closing.store(false, Ordering::Release);
        }
    }
    result
}
