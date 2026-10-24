//! Toolkit-independent desktop adapter. No terminal, WebView, or credential access.
mod library;
mod preferences;
mod tasks;

pub use library::{Bootstrap, LibraryEntry, bootstrap};
pub use preferences::{Preferences, PreferencesStore, Theme};
pub use tasks::{TaskRegistry, TaskSnapshot, TaskState};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct GuiError {
    pub code: &'static str,
    pub message: String,
}
impl GuiError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
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
