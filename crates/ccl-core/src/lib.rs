//! Reusable launcher services. This crate has no terminal or graphical interface dependencies.

pub mod config;
pub mod directory;
pub mod error;
pub mod network;
pub mod task;

pub use error::{Error, Result};
