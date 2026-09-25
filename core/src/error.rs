//! Error types shared across the core engine.

use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by the `tpt-focus` core engine.
#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to parse configuration: {0}")]
    ConfigParse(#[from] toml::de::Error),

    #[error("failed to serialize configuration: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),

    #[error("invalid configuration: {0}")]
    ConfigValidation(String),

    #[error("failed to read configuration from {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write configuration to {path}: {source}")]
    ConfigWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to create storage directory {path}: {source}")]
    StorageDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("notification source error: {0}")]
    Source(String),

    #[error("notification source `{0}` is not running")]
    SourceNotRunning(String),

    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),

    #[error("failed to encode or decode stored JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub fn validation(msg: impl Into<String>) -> Self {
        Error::ConfigValidation(msg.into())
    }
}

/// Convenience result alias for core operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;
