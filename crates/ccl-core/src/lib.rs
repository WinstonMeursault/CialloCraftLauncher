//! Reusable launcher services. This crate has no terminal or graphical interface dependencies.

pub mod config;
pub mod directory;
pub mod download;
pub mod error;
pub mod install;
pub mod java;
pub mod launch;
pub mod launch_plan;
pub mod metadata;
pub mod natives;
pub mod network;
pub mod source;
pub mod task;

pub use error::{Error, Result};
pub use task::{CancellationToken, EventSink, TaskContext, TaskEvent};
