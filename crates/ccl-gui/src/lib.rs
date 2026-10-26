//! Toolkit-independent desktop adapter using shared core services.
//! A terminal or WebView is not required by this library.
pub mod account_service;
pub mod browser;
pub mod catalog;
pub mod instance_service;
pub mod launcher_log;
mod library;
pub mod lifecycle;
pub mod maintenance;
pub mod online_service;
pub mod operations;
mod preferences;
pub mod self_update;
mod tasks;
pub mod update_service;
#[cfg(feature = "gui-validation")]
pub mod validation;

pub use library::{Bootstrap, LibraryEntry, bootstrap, initialize_root};
pub use preferences::{Preferences, PreferencesStore, Theme};
pub use tasks::{TaskHandle, TaskRegistry, TaskSnapshot, TaskState};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct GuiError {
    pub code: &'static str,
    pub message: String,
}
impl GuiError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        launcher_log::record("ERROR", "operation.failed", Some(code), None);
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for GuiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for GuiError {}
pub type GuiResult<T> = Result<T, GuiError>;
