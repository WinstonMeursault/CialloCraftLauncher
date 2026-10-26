#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ccl_core::{accounts::OsSecretStore, config::LauncherConfig, loaders::LoaderKind};
use ccl_gui::account_service::{AccountListing, AccountService};
use ccl_gui::operations::{GuiOperations, ImportRequest, InstallRequest, LaunchRequest};
use ccl_gui::{Bootstrap, GuiError, Preferences, PreferencesStore, TaskRegistry, TaskSnapshot};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

struct DesktopState {
    store: PreferencesStore,
    config: Arc<Mutex<PathBuf>>,
    config_pinned: bool,
    explicit_root: Option<PathBuf>,
    tasks: TaskRegistry,
    closing: Arc<AtomicBool>,
    operation_gate: Arc<Mutex<()>>,
    selected_archive: Arc<Mutex<Option<PathBuf>>>,
    path_grants: Arc<Mutex<std::collections::BTreeMap<String, PathBuf>>>,
    update_in_progress: AtomicBool,
    _update_process_guard: Option<std::fs::File>,
}

impl DesktopState {
    fn config_path(&self) -> PathBuf {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[tauri::command]
async fn bootstrap(state: State<'_, DesktopState>) -> Result<Bootstrap, GuiError> {
    let store = state.store.clone();
    let config = state.config_path();
    let root = state.explicit_root.clone();
    tauri::async_runtime::spawn_blocking(move || ccl_gui::bootstrap(&store, &config, root))
        .await
        .map_err(|_| GuiError::new("worker_error", "Library worker stopped"))?
}
#[tauri::command]
async fn save_preferences(
    state: State<'_, DesktopState>,
    preferences: Preferences,
) -> Result<Preferences, GuiError> {
    ensure_idle(&state)?;
    let store = state.store.clone();
    let gate = state.operation_gate.clone();
    let closing = state.closing.clone();
    let tasks = state.tasks.clone();
    tauri::async_runtime::spawn_blocking(move || {
        with_idle_gate(&gate, &closing, &tasks, || store.save(preferences))
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Preferences worker stopped"))?
}
#[tauri::command]
async fn select_root(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<Bootstrap>, GuiError> {
    ensure_idle(&state)?;
    if state.explicit_root.is_some() {
        return Err(GuiError::new(
            "root_pinned",
            "Root is pinned by CCL_GUI_GAME_DIR; restart without it to choose another root",
        ));
    }
    let store = state.store.clone();
    let config = state.config_path();
    let gate = state.operation_gate.clone();
    let closing = state.closing.clone();
    let tasks = state.tasks.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let selected = app.dialog().file().blocking_pick_folder();
        let Some(selected) = selected else {
            return Ok(None);
        };
        let root = selected.into_path().map_err(|_| {
            GuiError::new("invalid_root", "Selected location is not a local directory")
        })?;
        with_idle_gate(&gate, &closing, &tasks, || {
            store.select_root(root)?;
            ccl_gui::bootstrap(&store, &config, None).map(Some)
        })
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Directory worker stopped"))?
}
#[tauri::command]
fn cancel_task(state: State<'_, DesktopState>, id: u64) -> Result<TaskSnapshot, GuiError> {
    state
        .tasks
        .cancel(id)
        .map(|snapshot| snapshot.for_display())
}
#[tauri::command]
fn task_snapshot(state: State<'_, DesktopState>) -> Option<TaskSnapshot> {
    state
        .tasks
        .snapshot()
        .map(|snapshot| snapshot.for_display())
}
#[tauri::command]
fn set_visibility(state: State<'_, DesktopState>, visible: bool) {
    state.tasks.set_visible(visible);
}

#[cfg(feature = "gui-validation")]
#[tauri::command]
fn gui_log_pressure(state: State<'_, DesktopState>) -> Result<TaskSnapshot, GuiError> {
    let _gate = try_operation_gate(&state.operation_gate)?;
    check_idle(&state.closing, &state.tasks)?;
    ccl_gui::validation::start_log_pressure(&state.tasks).map(|snapshot| snapshot.for_display())
}

#[cfg(feature = "gui-validation")]
#[tauri::command]
fn gui_blank_baseline(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), GuiError> {
    let _gate = try_operation_gate(&state.operation_gate)?;
    check_idle(&state.closing, &state.tasks)?;
    app.get_webview_window("main").ok_or_else(||GuiError::new("validation_window","Validation window is unavailable"))?
        .eval("document.getElementById('root')?.replaceChildren(); document.title = 'CCL blank rendering control';")
        .map_err(|_|GuiError::new("validation_window","Could not prepare blank rendering control"))
}

include!("commands.rs");
include!("advanced_commands.rs");
include!("self_update_commands.rs");

fn main() {
    if let Some(code) = ccl_gui::self_update::helper_entry() {
        std::process::exit(code);
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let inputs = ccl_core::directory::DirectoryInputs::from_environment(None)?;
            let config = std::env::var_os("CCL_GUI_CONFIG_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    ccl_core::config::default_config_path(
                        &inputs.home_directory,
                        std::env::var_os("APPDATA")
                            .as_deref()
                            .map(std::path::Path::new),
                        std::env::var_os("XDG_CONFIG_HOME")
                            .as_deref()
                            .map(std::path::Path::new),
                    )
                });
            let directory = std::env::var_os("CCL_GUI_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    config
                        .parent()
                        .unwrap_or(std::path::Path::new("."))
                        .join("gui")
                });
            let store = PreferencesStore::new(directory);
            let update_process_guard = ccl_gui::self_update::Installation::current(&store.directory().join("update"))
                .ok().map(|install| install.process_guard()).transpose()?;
            let handle = app.handle().clone();
            let tasks = TaskRegistry::new(Arc::new(move |snapshot| {
                let _ = handle.emit("gui-task", snapshot);
            }));
            let selected_config = if std::env::var_os("CCL_GUI_CONFIG_PATH").is_some() { config } else { store.config_path()?.unwrap_or(config) };
            app.manage(DesktopState {
                store,
                config: Arc::new(Mutex::new(selected_config)),
                config_pinned: std::env::var_os("CCL_GUI_CONFIG_PATH").is_some(),
                explicit_root: std::env::var_os("CCL_GUI_GAME_DIR").map(PathBuf::from),
                tasks,
                closing: Arc::new(AtomicBool::new(false)),
                operation_gate: Arc::new(Mutex::new(())),
                selected_archive: Arc::new(Mutex::new(None)),
                path_grants: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
                update_in_progress: AtomicBool::new(false),
                _update_process_guard: update_process_guard,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            save_preferences,
            select_root,
            cancel_task,
            task_snapshot,
            set_visibility,
            initialize_root,
            list_versions,
            list_loaders,
            install_instance,
            inspect_import,
            import_instance,
            launch_game,
            task_history,
            account_list,
            account_add_offline,
            account_select,
            account_logout,
            account_login,
            account_refresh,
            account_check_storage,
            open_login_browser,
            log_runs,
            read_run_log,
            set_network_offline,
            choose_path, instance_query, instance_mutate, online_query, online_mutate, maintenance, inspect_dropped, update_installation, select_config,
            gui_self_update, gui_ready,
            #[cfg(feature = "gui-validation")]
            gui_log_pressure,
            #[cfg(feature = "gui-validation")]
            gui_blank_baseline
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event
                && let Some(state)=window.try_state::<DesktopState>() {
                    if paths.len()==1 {
                        let path=&paths[0];
                        if path.extension().and_then(|e|e.to_str()).is_some_and(|e|e.eq_ignore_ascii_case("zip")||e.eq_ignore_ascii_case("mrpack")) {
                            state.path_grants.lock().unwrap_or_else(|e|e.into_inner()).insert("drop".into(),path.clone());
                            let _=window.emit("ccl-archive-drop",path);
                        } else { let _=window.emit("ccl-drop-error","Unsupported archive. Drop one .zip or .mrpack file."); }
                    } else { let _=window.emit("ccl-drop-error","Drop one archive at a time."); }
            }

            if matches!(
                event,
                tauri::WindowEvent::Focused(_) | tauri::WindowEvent::Resized(_)
            ) && let Some(state) = window.try_state::<DesktopState>()
            {
                state.tasks.set_visible(
                    window.is_visible().unwrap_or(true) && !window.is_minimized().unwrap_or(false),
                );
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<DesktopState>();
                if state.update_in_progress.load(Ordering::Acquire) {
                    api.prevent_close();
                    return;
                }
                let _gate = match state.operation_gate.try_lock() {
                    Ok(guard) => guard,
                    Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
                    Err(std::sync::TryLockError::WouldBlock) => {
                        api.prevent_close();
                        let zh = state.store.load().map_or(true, |p| p.language == "zh-CN");
                        let message = if zh {
                            "账户或设置操作仍在完成，请稍后重试关闭窗口。"
                        } else {
                            "An account/settings operation is still finishing; retry close shortly."
                        };
                        window
                            .dialog()
                            .message(message)
                            .parent(window)
                            .title("CCL")
                            .kind(MessageDialogKind::Info)
                            .buttons(MessageDialogButtons::Ok)
                            .show(|_| {});
                        return;
                    }
                };
                let (prompted_task, game) = {
                    if state.closing.swap(true, Ordering::AcqRel) {
                        api.prevent_close();
                        return;
                    }
                    let snapshot = state.tasks.snapshot().filter(|s| {
                        matches!(s.state, ccl_gui::TaskState::Running | ccl_gui::TaskState::Cancelling)
                    });
                    let Some(snapshot) = snapshot else {
                        // Block new IPC work after this close request has been accepted.
                        return;
                    };
                    (snapshot.clone(), snapshot.kind == "launch" && snapshot.pid.is_some())
                };
                drop(_gate);
                api.prevent_close();
                let zh = state.store.load().map_or(true, |p| p.language == "zh-CN");
                let app = window.app_handle().clone();
                if !prompted_task.cancellable {
                    state.closing.store(false, Ordering::Release);
                    window.dialog().message(if zh {"文件事务正在提交，不能安全中断。请等待完成后退出。"} else {"A filesystem transaction is committing and cannot be interrupted safely. Wait for it to finish before quitting."}).parent(window).title("CCL").kind(MessageDialogKind::Info).show(|_|{});
                    return;
                }
                let message = if game && zh {
                    "游戏仍在运行。是否停止游戏并退出启动器？选择“否”继续保留窗口和游戏。"
                } else if game {
                    "A game is running. Stop the game and quit the launcher? Choose No to keep both running."
                } else if zh {
                    "任务仍在运行。是否取消任务并退出？选择“否”继续保留窗口。"
                } else {
                    "A task is running. Cancel it and quit? Choose No to keep the window open."
                };
                window
                    .dialog()
                    .message(message)
                    .parent(window)
                    .title("CCL")
                    .kind(MessageDialogKind::Warning)
                    .buttons(MessageDialogButtons::YesNo)
                    .show(move |yes| {
                        if !yes {
                            let state = app.state::<DesktopState>();
                            state.closing.store(false, Ordering::Release);
                            return;
                        }
                        let tasks = app.state::<DesktopState>().tasks.clone();
                        if tasks.snapshot().is_some_and(|snapshot| {
                            snapshot.id == prompted_task.id
                                && matches!(snapshot.state, ccl_gui::TaskState::Running | ccl_gui::TaskState::Cancelling)
                        }) {
                            let _ = tasks.cancel(prompted_task.id);
                        } else if tasks.active() {
                            let state = app.state::<DesktopState>();
                            state.closing.store(false, Ordering::Release);
                            return;
                        }
                        std::thread::spawn(move || {
                            let deadline =
                                std::time::Instant::now() + std::time::Duration::from_secs(10);
                            while tasks.active() && std::time::Instant::now() < deadline {
                                std::thread::sleep(std::time::Duration::from_millis(20));
                            }
                            if !tasks.active() {
                                app.exit(0);
                            } else {
                                let state = app.state::<DesktopState>();
                                state.closing.store(false, Ordering::Release);
                            }
                        });
                    });
            }
        })
        .build(tauri::generate_context!())
        .expect("CCL desktop initialization failed")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                let state = app.state::<DesktopState>();
                if state.update_in_progress.load(Ordering::Acquire) {
                    api.prevent_exit();
                    return;
                }
                if ccl_gui::lifecycle::begin_exit(&state.operation_gate, &state.closing, &state.tasks)
                    != ccl_gui::lifecycle::ExitDecision::Ready
                {
                    api.prevent_exit();
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.close();
                    }
                }
            }
        });
}
