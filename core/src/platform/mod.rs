//! Platform backends and their capability boundaries.
//!
//! Only the platform-appropriate module is compiled for a given target, so a
//! Windows build never pulls in D-Bus and vice versa. The factory functions
//! at the bottom give the CLI and tray one place to obtain the real
//! implementations.

pub mod access;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(windows)]
pub mod windows;

use std::sync::Arc;

use crate::context::ContextProvider;
#[cfg(windows)]
use crate::platform::access::NotificationAccess as _;
use crate::profile::ActiveProfile;
use crate::source::NotificationSource;
use crate::AccessState;

/// Build the platform's real context provider (foreground app, fullscreen).
///
/// Falls back to the mock provider on platforms without a real backend.
pub fn default_context(active_profile: Arc<ActiveProfile>) -> Arc<dyn ContextProvider> {
    #[cfg(windows)]
    {
        Arc::new(windows::context::WindowsContextProvider::new(
            active_profile,
        ))
    }
    #[cfg(target_os = "linux")]
    {
        linux::context::best_provider(active_profile)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = active_profile;
        Arc::new(crate::context::MockContextProvider::new())
    }
}

/// Build the platform notification source, if the platform can support one.
///
/// The returned source still needs [`NotificationSource::start`]; startup
/// errors (missing consent, bus-name conflict) are reported there so callers
/// can degrade gracefully.
pub fn default_source() -> crate::Result<Box<dyn NotificationSource>> {
    #[cfg(windows)]
    {
        Ok(Box::new(windows::listener::WindowsNotificationSource::new()))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::dbus::DbusNotificationSource::new()))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err(crate::error::Error::Source(
            "no notification source for this platform".into(),
        ))
    }
}

/// Current notification-access state without prompting.
pub fn listener_access_state() -> AccessState {
    #[cfg(windows)]
    {
        windows::access::WinRtAccess::new(AccessState::Unknown).state()
    }
    #[cfg(target_os = "linux")]
    {
        // No consent gate on Linux; availability is decided at source start.
        AccessState::Granted
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        AccessState::Unavailable
    }
}

/// Run the consent prompt when the user has not decided yet.
pub fn request_listener_access() -> AccessState {
    #[cfg(windows)]
    {
        windows::access::WinRtAccess::new(AccessState::Unknown).request()
    }
    #[cfg(target_os = "linux")]
    {
        AccessState::Granted
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        AccessState::Unavailable
    }
}
