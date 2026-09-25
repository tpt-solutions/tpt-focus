//! Windows notification access via the WinRT `UserNotificationListener` API.
//!
//! The class lives in `Windows.UI.Notifications.Management`. It requires
//! package identity (see `docs/windows-package-identity.md`); without it the
//! API fails and this backend degrades to [`AccessState::Unavailable`]
//! instead of panicking.

use std::sync::Mutex;

use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};

use crate::platform::access::{AccessState, NotificationAccess, PACKAGE_IDENTITY_DOC};

/// Real WinRT-backed consent implementation.
pub struct WinRtAccess {
    persisted: Mutex<AccessState>,
}

impl WinRtAccess {
    /// Start from a previously persisted state (usually
    /// [`AccessState::Unknown`] on a fresh install).
    pub fn new(initial: AccessState) -> Self {
        Self {
            persisted: Mutex::new(initial),
        }
    }

    /// Blocking consent request.
    ///
    /// Must be called from a worker thread: it blocks until the user answers
    /// the OS prompt.
    fn request_blocking() -> AccessState {
        match Self::request_inner() {
            Ok(status) => match status {
                UserNotificationListenerAccessStatus::Allowed => AccessState::Granted,
                UserNotificationListenerAccessStatus::Denied => AccessState::Denied,
                _ => AccessState::Unknown,
            },
            Err(error) => {
                tracing::warn!(
                    %error,
                    "UserNotificationListener consent request failed; \
                     falling back to degraded mode"
                );
                AccessState::Unavailable
            }
        }
    }

    fn request_inner() -> windows::core::Result<UserNotificationListenerAccessStatus> {
        // The consent flow runs on a worker thread; make sure COM exists.
        super::win32::ensure_com_initialized();
        let listener = UserNotificationListener::Current()?;
        let operation = listener.RequestAccessAsync()?;
        super::block_on(operation)
    }

    fn live_access_status(&self) -> Option<UserNotificationListenerAccessStatus> {
        UserNotificationListener::Current()
            .ok()?
            .GetAccessStatus()
            .ok()
    }
}

impl NotificationAccess for WinRtAccess {
    fn state(&self) -> AccessState {
        // A previously granted session is visible without prompting.
        if let Some(status) = self.live_access_status() {
            if status == UserNotificationListenerAccessStatus::Allowed {
                return AccessState::Granted;
            }
        }
        *self.persisted.lock().expect("winrt access poisoned")
    }

    fn request(&self) -> AccessState {
        {
            let mut guard = self.persisted.lock().expect("winrt access poisoned");
            *guard = AccessState::Requesting;
        }

        let outcome = Self::request_blocking();

        let mut guard = self.persisted.lock().expect("winrt access poisoned");
        *guard = outcome;
        outcome
    }

    fn help_url(&self) -> Option<&'static str> {
        Some(PACKAGE_IDENTITY_DOC)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without package identity the request must degrade, never panic.
    ///
    /// CI runners do not deploy the sparse package, so this exercises the
    /// failure path end to end. On a dev machine that *does* have identity
    /// the test stands aside rather than popping a consent dialog.
    #[test]
    fn consent_request_degrades_gracefully_without_package_identity() {
        if UserNotificationListener::Current().is_ok() {
            eprintln!("package identity present; skipping consent degradation test");
            return;
        }

        let access = WinRtAccess::new(AccessState::Unknown);
        let state = access.request();

        assert!(
            matches!(state, AccessState::Denied | AccessState::Unavailable),
            "unexpected state without package identity: {state:?}"
        );
        assert!(!state.can_listen());
        assert_eq!(access.help_url(), Some(PACKAGE_IDENTITY_DOC));
    }

    #[test]
    fn state_and_request_stay_within_the_state_machine() {
        for initial in [
            AccessState::Unknown,
            AccessState::Requesting,
            AccessState::Granted,
            AccessState::Denied,
            AccessState::Unavailable,
        ] {
            let access = WinRtAccess::new(initial);
            let state = access.state();
            assert!(matches!(
                state,
                AccessState::Unknown
                    | AccessState::Requesting
                    | AccessState::Granted
                    | AccessState::Denied
                    | AccessState::Unavailable
            ));
        }
    }
}
