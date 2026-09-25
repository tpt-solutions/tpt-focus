//! Environment/context signals that rules evaluate against.

use std::sync::Mutex;

use chrono::{Local, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::AppIdentity;

/// A point-in-time view of the user's environment.
///
/// Captured by a [`ContextProvider`] for every notification so rule
/// evaluation is deterministic and replayable from history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSnapshot {
    /// Currently focused application, when the platform can observe it.
    pub foreground_app: Option<AppIdentity>,
    /// Whether the foreground window covers an entire monitor.
    pub fullscreen: bool,
    /// Name of the manually activated focus profile, if any.
    pub active_profile: Option<String>,
    /// Instant the snapshot was captured (UTC).
    pub captured_at: chrono::DateTime<Utc>,
    /// Wall-clock local time used for schedule rules. Kept explicit so
    /// evaluation does not depend on the host timezone.
    pub local_time: NaiveDateTime,
}

impl ContextSnapshot {
    /// Snapshot "now" on this machine.
    pub fn now() -> Self {
        let now = Utc::now();
        Self {
            foreground_app: None,
            fullscreen: false,
            active_profile: None,
            captured_at: now,
            local_time: now.with_timezone(&Local).naive_local(),
        }
    }
}

impl Default for ContextSnapshot {
    fn default() -> Self {
        Self::now()
    }
}

/// Supplies [`ContextSnapshot`]s to the rule engine.
///
/// Real implementations land in Phase 4 (Windows) and Phase 6 (Linux);
/// tests use [`MockContextProvider`].
pub trait ContextProvider: Send + Sync {
    /// Capture the current environment.
    fn snapshot(&self) -> ContextSnapshot;

    /// Human-readable provider name for diagnostics.
    fn name(&self) -> &str {
        "unnamed"
    }
}

/// Thread-safe, manually driven context provider for tests.
#[derive(Debug, Default)]
pub struct MockContextProvider {
    snapshot: Mutex<ContextSnapshot>,
}

impl MockContextProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_snapshot(snapshot: ContextSnapshot) -> Self {
        Self {
            snapshot: Mutex::new(snapshot),
        }
    }

    /// Replace the snapshot returned by [`ContextProvider::snapshot`].
    pub fn set_snapshot(&self, snapshot: ContextSnapshot) {
        *self.snapshot.lock().expect("mock context poisoned") = snapshot;
    }

    pub fn set_foreground_app(&self, app: Option<AppIdentity>) {
        let mut guard = self.snapshot.lock().expect("mock context poisoned");
        guard.foreground_app = app;
    }

    pub fn set_fullscreen(&self, fullscreen: bool) {
        self.snapshot
            .lock()
            .expect("mock context poisoned")
            .fullscreen = fullscreen;
    }

    pub fn set_active_profile(&self, profile: Option<&str>) {
        let mut guard = self.snapshot.lock().expect("mock context poisoned");
        guard.active_profile = profile.map(str::to_string);
    }
}

impl ContextProvider for MockContextProvider {
    fn snapshot(&self) -> ContextSnapshot {
        self.snapshot.lock().expect("mock context poisoned").clone()
    }

    fn name(&self) -> &str {
        "mock"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_provider_reflects_mutations() {
        let provider = MockContextProvider::new();
        assert!(provider.snapshot().foreground_app.is_none());

        provider.set_foreground_app(Some(AppIdentity::new("code")));
        provider.set_fullscreen(true);
        provider.set_active_profile(Some("Deep Work"));

        let snapshot = provider.snapshot();
        assert_eq!(
            snapshot.foreground_app.as_ref().map(|a| a.app_id.as_str()),
            Some("code")
        );
        assert!(snapshot.fullscreen);
        assert_eq!(snapshot.active_profile.as_deref(), Some("Deep Work"));
    }

    #[test]
    fn snapshot_captures_local_wall_clock() {
        let snapshot = ContextSnapshot::now();
        assert!(!snapshot.local_time.to_string().is_empty());
    }
}
