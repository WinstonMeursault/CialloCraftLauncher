#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ccl_gui::{Bootstrap, GuiError, Preferences, PreferencesStore, TaskRegistry, TaskSnapshot};
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
    config: PathBuf,
    explicit_root: Option<PathBuf>,
    tasks: TaskRegistry,
    closing: AtomicBool,
}

#[tauri::command]
async fn bootstrap(state: State<'_, DesktopState>) -> Result<Bootstrap, GuiError> {
    let store = state.store.clone();
    let config = state.config.clone();
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
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || store.save(preferences))
        .await
        .map_err(|_| GuiError::new("worker_error", "Preferences worker stopped"))?
}
#[tauri::command]
async fn select_root(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<Bootstrap>, GuiError> {
    if state.explicit_root.is_some() {
        return Err(GuiError::new(
            "root_pinned",
            "Root is pinned by CCL_GUI_GAME_DIR; restart without it to choose another root",
        ));
    }
    let store = state.store.clone();
    let config = state.config.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let selected = app.dialog().file().blocking_pick_folder();
        let Some(selected) = selected else {
            return Ok(None);
        };
        let root = selected.into_path().map_err(|_| {
            GuiError::new("invalid_root", "Selected location is not a local directory")
        })?;
        store.select_root(root)?;
        ccl_gui::bootstrap(&store, &config, None).map(Some)
    })
    .await
    .map_err(|_| GuiError::new("worker_error", "Directory worker stopped"))?
}
#[tauri::command]
fn start_probe(state: State<'_, DesktopState>) -> Result<TaskSnapshot, GuiError> {
    state.tasks.start_probe()
}
#[tauri::command]
fn cancel_task(state: State<'_, DesktopState>, id: u64) -> Result<TaskSnapshot, GuiError> {
    state.tasks.cancel(id)
}
#[tauri::command]
fn task_snapshot(state: State<'_, DesktopState>) -> Option<TaskSnapshot> {
    state.tasks.snapshot()
}
#[tauri::command]
fn set_visibility(state: State<'_, DesktopState>, visible: bool) {
    state.tasks.set_visible(visible);
}

fn main() {
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
            let handle = app.handle().clone();
            let tasks = TaskRegistry::new(Arc::new(move |snapshot| {
                let _ = handle.emit("gui-task", snapshot);
            }));
            app.manage(DesktopState {
                store,
                config,
                explicit_root: std::env::var_os("CCL_GUI_GAME_DIR").map(PathBuf::from),
                tasks,
                closing: AtomicBool::new(false),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            save_preferences,
            select_root,
            start_probe,
            cancel_task,
            task_snapshot,
            set_visibility
        ])
        .on_window_event(|window, event| {
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
                if !state.tasks.active() {
                    return;
                }
                api.prevent_close();
                if state.closing.swap(true, Ordering::AcqRel) {
                    return;
                }
                let zh = state.store.load().map_or(true, |p| p.language == "zh-CN");
                let app = window.app_handle().clone();
                let message = if zh {
                    "任务仍在运行。是否取消任务并退出？选择“否”继续保留窗口。"
                } else {
                    "A task is running. Cancel it and quit? Choose No to keep the window open."
                };
                window
                    .dialog()
                    .message(message)
                    .title("CCL G0")
                    .kind(MessageDialogKind::Warning)
                    .buttons(MessageDialogButtons::YesNo)
                    .show(move |yes| {
                        if !yes {
                            app.state::<DesktopState>()
                                .closing
                                .store(false, Ordering::Release);
                            return;
                        }
                        let tasks = app.state::<DesktopState>().tasks.clone();
                        if let Some(snapshot) = tasks.snapshot() {
                            let _ = tasks.cancel(snapshot.id);
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
                                app.state::<DesktopState>()
                                    .closing
                                    .store(false, Ordering::Release);
                            }
                        });
                    });
            }
        })
        .build(tauri::generate_context!())
        .expect("CCL desktop initialization failed")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event
                && app.state::<DesktopState>().tasks.active()
            {
                api.prevent_exit();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.close();
                }
            }
        });
}
