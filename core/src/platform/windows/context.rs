//! Win32 context signals: foreground application and fullscreen state.

use std::sync::Arc;

use chrono::{Local, Utc};

use crate::context::{ContextProvider, ContextSnapshot};
use crate::model::AppIdentity;
use crate::profile::ActiveProfile;

use super::win32;

/// Captures the environment rules evaluate against on Windows.
///
/// Foreground app and fullscreen state are read fresh for every snapshot;
/// each call is cheap (a handful of Win32 calls) and runs before pipeline
/// evaluation, so no caching layer is needed.
pub struct WindowsContextProvider {
    active_profile: Arc<ActiveProfile>,
}

impl WindowsContextProvider {
    pub fn new(active_profile: Arc<ActiveProfile>) -> Self {
        Self { active_profile }
    }

    /// Foreground window identity plus whether it covers its monitor.
    fn window_state(&self) -> (Option<AppIdentity>, bool) {
        let Some(hwnd) = win32::foreground_window() else {
            return (None, false);
        };

        let fullscreen = match (win32::window_rect(hwnd), win32::monitor_rect(hwnd)) {
            (Some(window), Some(monitor)) => {
                win32::covers_monitor(&window, &monitor, win32::FULLSCREEN_TOLERANCE)
            }
            _ => false,
        };

        let app = win32::process_id(hwnd)
            .and_then(win32::process_name)
            .map(AppIdentity::new);

        (app, fullscreen)
    }
}

impl ContextProvider for WindowsContextProvider {
    fn snapshot(&self) -> ContextSnapshot {
        let now = Utc::now();
        let (foreground_app, fullscreen) = self.window_state();

        ContextSnapshot {
            foreground_app,
            fullscreen,
            active_profile: self.active_profile.active(),
            captured_at: now,
            local_time: now.with_timezone(&Local).naive_local(),
        }
    }

    fn name(&self) -> &str {
        "windows"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_structurally_sound_headless() {
        let provider = WindowsContextProvider::new(Arc::new(ActiveProfile::new()));

        // CI runners have no interactive desktop; the provider must still
        // produce a usable snapshot instead of panicking.
        let snapshot = provider.snapshot();
        assert_eq!(provider.name(), "windows");
        assert!(snapshot.captured_at <= Utc::now());
        assert_eq!(
            snapshot.local_time,
            snapshot.captured_at.with_timezone(&Local).naive_local()
        );
        assert!(
            snapshot.foreground_app.is_none()
                || !snapshot.foreground_app.unwrap().app_id.is_empty()
        );
    }

    #[test]
    fn active_profile_is_visible_to_rules() {
        let profile = Arc::new(ActiveProfile::new());
        profile.activate("deep-work");

        let provider = WindowsContextProvider::new(Arc::clone(&profile));
        assert_eq!(
            provider.snapshot().active_profile.as_deref(),
            Some("deep-work")
        );
    }
}
