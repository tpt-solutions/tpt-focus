//! Consent/access boundary between tpt-focus and the OS notification sink.
//!
//! Windows requires package identity plus an explicit `RequestAccessAsync`
//! consent before `UserNotificationListener` will hand out notifications.
//! The state machine below is platform-independent so tests and Linux CI can
//! exercise it through [`MockAccess`] without any package identity.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Settings key persisting the last known access state across restarts.
pub const ACCESS_SETTING_KEY: &str = "windows.listener_access";

/// Documentation shown when access cannot be granted.
pub const PACKAGE_IDENTITY_DOC: &str =
    "https://github.com/tpt-solutions/tpt-focus/blob/master/docs/windows-package-identity.md";

/// Outcome of the OS consent flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessState {
    /// Consent has not been determined yet this session.
    #[default]
    Unknown,
    /// The consent prompt is showing / a request is in flight.
    Requesting,
    /// Consent granted — the listener may start.
    Granted,
    /// The user declined.
    Denied,
    /// The API cannot be used at all (no package identity, OS too old, …).
    Unavailable,
}

impl AccessState {
    /// May the notification listener be started?
    pub fn can_listen(&self) -> bool {
        matches!(self, AccessState::Granted)
    }

    /// Stable string used in settings and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            AccessState::Unknown => "unknown",
            AccessState::Requesting => "requesting",
            AccessState::Granted => "granted",
            AccessState::Denied => "denied",
            AccessState::Unavailable => "unavailable",
        }
    }

    /// Parse the value written by [`AccessState::as_str`].
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "granted" | "allowed" => AccessState::Granted,
            "denied" => AccessState::Denied,
            "unavailable" => AccessState::Unavailable,
            "requesting" => AccessState::Requesting,
            _ => AccessState::Unknown,
        }
    }

    /// Restore from a persisted settings value (`None` = never asked).
    pub fn from_setting(raw: Option<&str>) -> Self {
        raw.map(Self::parse).unwrap_or_default()
    }

    /// Banner shown by the tray/settings UI, if this state needs one.
    ///
    /// Both degraded modes keep rules, profiles, history search and the tray
    /// fully functional — only live ingestion is off.
    pub fn banner(&self) -> Option<Banner> {
        match self {
            AccessState::Unknown | AccessState::Requesting | AccessState::Granted => None,
            AccessState::Denied => Some(Banner {
                level: BannerLevel::Warning,
                message: "Windows has not granted tpt-focus permission to read notifications, \
                          so live history is paused."
                    .to_string(),
                action: Some(BannerAction::RequestAgain),
            }),
            AccessState::Unavailable => Some(Banner {
                level: BannerLevel::Warning,
                message: "tpt-focus has no Windows package identity, so it cannot read \
                          system notifications. Rules, profiles and history search still work."
                    .to_string(),
                action: Some(BannerAction::OpenDocs),
            }),
        }
    }
}

/// Severity of a user-facing banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BannerLevel {
    Info,
    Warning,
    Error,
}

/// What the user can do about a banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BannerAction {
    /// Re-run the consent prompt.
    RequestAgain,
    /// Open the setup documentation.
    OpenDocs,
}

/// User-facing notice attached to a particular [`AccessState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Banner {
    pub level: BannerLevel,
    pub message: String,
    pub action: Option<BannerAction>,
}

/// Platform boundary for obtaining notification access.
pub trait NotificationAccess: Send + Sync {
    /// Last known state without prompting the user.
    fn state(&self) -> AccessState;

    /// Show the OS consent prompt if needed and return the resulting state.
    ///
    /// Implementations must be safe to call from any thread and must never
    /// panic when the underlying API is missing.
    fn request(&self) -> AccessState;

    /// Where to send the user when access cannot be granted.
    fn help_url(&self) -> Option<&'static str> {
        None
    }
}

/// Test double for the consent flow (no package identity required).
#[derive(Debug)]
pub struct MockAccess {
    state: Mutex<AccessState>,
    next_state: Mutex<AccessState>,
    requests: AtomicUsize,
}

impl MockAccess {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(AccessState::Unknown),
            next_state: Mutex::new(AccessState::Granted),
            requests: AtomicUsize::new(0),
        }
    }

    /// Mock that reports granted access after the first request.
    pub fn granted() -> Self {
        Self::new()
    }

    /// Mock where the user always declines.
    pub fn denied() -> Self {
        Self {
            state: Mutex::new(AccessState::Denied),
            next_state: Mutex::new(AccessState::Denied),
            requests: AtomicUsize::new(0),
        }
    }

    /// Mock where the API is entirely unavailable.
    pub fn unavailable() -> Self {
        Self {
            state: Mutex::new(AccessState::Unavailable),
            next_state: Mutex::new(AccessState::Unavailable),
            requests: AtomicUsize::new(0),
        }
    }

    /// What the next `request()` call should resolve to.
    pub fn set_next_state(&self, state: AccessState) {
        *self.next_state.lock().expect("mock access poisoned") = state;
    }

    /// Force the reported state (simulates a persisted value).
    pub fn set_state(&self, state: AccessState) {
        *self.state.lock().expect("mock access poisoned") = state;
    }

    /// How many times consent has been requested.
    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Default for MockAccess {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationAccess for MockAccess {
    fn state(&self) -> AccessState {
        *self.state.lock().expect("mock access poisoned")
    }

    fn request(&self) -> AccessState {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let next = *self.next_state.lock().expect("mock access poisoned");
        *self.state.lock().expect("mock access poisoned") = next;
        next
    }

    fn help_url(&self) -> Option<&'static str> {
        Some(PACKAGE_IDENTITY_DOC)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_granted_allows_listening() {
        assert!(AccessState::Granted.can_listen());
        assert!(!AccessState::Unknown.can_listen());
        assert!(!AccessState::Denied.can_listen());
        assert!(!AccessState::Unavailable.can_listen());
        assert!(!AccessState::Requesting.can_listen());
    }

    #[test]
    fn state_round_trips_through_settings() {
        for state in [
            AccessState::Unknown,
            AccessState::Requesting,
            AccessState::Granted,
            AccessState::Denied,
            AccessState::Unavailable,
        ] {
            assert_eq!(AccessState::parse(state.as_str()), state);
        }
        assert_eq!(AccessState::from_setting(None), AccessState::Unknown);
        assert_eq!(
            AccessState::from_setting(Some("Allowed")),
            AccessState::Granted
        );
        assert_eq!(
            AccessState::from_setting(Some("garbage")),
            AccessState::Unknown
        );
    }

    #[test]
    fn unavailable_and_denied_states_produce_banners() {
        let banner = AccessState::Unavailable.banner().expect("banner");
        assert_eq!(banner.level, BannerLevel::Warning);
        assert_eq!(banner.action, Some(BannerAction::OpenDocs));
        assert!(banner.message.contains("package identity"));

        let banner = AccessState::Denied.banner().expect("banner");
        assert_eq!(banner.level, BannerLevel::Warning);
        assert_eq!(banner.action, Some(BannerAction::RequestAgain));
        assert!(banner.message.contains("permission"));

        assert!(AccessState::Granted.banner().is_none());
        assert!(AccessState::Unknown.banner().is_none());
    }

    #[test]
    fn mock_access_flows_through_the_consent_states() {
        let access = MockAccess::new();
        assert_eq!(access.state(), AccessState::Unknown);
        assert_eq!(access.request(), AccessState::Granted);
        assert_eq!(access.request_count(), 1);

        access.set_next_state(AccessState::Denied);
        assert_eq!(access.request(), AccessState::Denied);
        assert!(!access.state().can_listen());
        assert_eq!(access.request_count(), 2);
    }

    #[test]
    fn denied_and_unavailable_mocks_never_grant() {
        let denied = MockAccess::denied();
        assert!(!denied.request().can_listen());

        let unavailable = MockAccess::unavailable();
        assert!(!unavailable.request().can_listen());
        assert_eq!(unavailable.help_url(), Some(PACKAGE_IDENTITY_DOC));
    }

    #[test]
    fn access_state_serialises_snake_case() {
        let json = serde_json::to_string(&AccessState::Granted).unwrap();
        assert_eq!(json, "\"granted\"");
        let back: AccessState = serde_json::from_str("\"denied\"").unwrap();
        assert_eq!(back, AccessState::Denied);
    }
}
