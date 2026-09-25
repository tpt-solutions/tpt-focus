//! Default locations for the configuration file and history database.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// Default TOML configuration path: `<config dir>/tpt-focus/focus.toml`.
pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tpt-focus")
        .join("focus.toml")
}

/// Default history database path: `<data dir>/tpt-focus/history.db`.
pub fn default_db_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tpt-focus")
        .join("history.db")
}

/// Create the parent directories of `path` when missing.
pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    Ok(())
}

/// Resolve the effective configuration path for this invocation.
pub fn config_path(override_path: Option<&Path>) -> PathBuf {
    override_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_path)
}

/// Resolve the effective database path for this invocation.
pub fn db_path(override_path: Option<&Path>) -> PathBuf {
    override_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_db_path)
}
