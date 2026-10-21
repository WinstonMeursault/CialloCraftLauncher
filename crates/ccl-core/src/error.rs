use std::path::PathBuf;

/// Errors returned by reusable launcher services.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid JSON configuration at {path}: {source}")]
    ConfigJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("game directory path is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("task cancelled")]
    Cancelled,
    #[error(transparent)]
    Install(#[from] crate::install::InstallError),
}

pub type Result<T> = std::result::Result<T, Error>;
