//! Platform notification source startup: consent flow, pipeline routing and
//! the Linux bubble lifecycle.

use std::sync::{Arc, Mutex};

#[cfg(target_os = "linux")]
use tpt_focus_core::Notification;
use tpt_focus_core::{AccessState, FocusRuntime, NotificationSource};

use crate::state::UiHandle;
#[cfg(target_os = "linux")]
use crate::state::{Bubble, UiCommand};

/// Type of the shared slot holding the started source (dismiss-through).
pub type SourceSlot = Arc<Mutex<Option<Box<dyn NotificationSource>>>>;

/// Start the platform source in the background; updates `handle` with the
/// resulting access state. Never blocks the UI.
pub fn start_background(runtime: FocusRuntime, handle: UiHandle, slot: SourceSlot) {
    std::thread::Builder::new()
        .name("tpt-focus-source".into())
        .spawn(move || {
            let access = platform_source_flow(runtime, handle.clone(), slot);
            handle.update(|state| {
                state.access = access;
                if !access.can_listen() {
                    state.status = match access {
                        AccessState::Denied => "notification access denied".into(),
                        _ => "live ingestion unavailable".into(),
                    };
                }
            });
        })
        .expect("spawn source thread");
}

#[cfg(windows)]
fn platform_source_flow(runtime: FocusRuntime, handle: UiHandle, slot: SourceSlot) -> AccessState {
    use tpt_focus_core::platform::access::NotificationAccess as _;
    use tpt_focus_core::platform::access::ACCESS_SETTING_KEY;
    use tpt_focus_core::platform::windows::access::WinRtAccess;
    use tpt_focus_core::platform::windows::listener::WindowsNotificationSource;

    let access = WinRtAccess::new(AccessState::Unknown);
    let current = access.state();
    handle.update(|state| state.access = current);

    let effective = match current {
        AccessState::Granted => current,
        AccessState::Unknown => access.request(),
        other => other,
    };

    // Persist the consent result for the next launch.
    if let Ok(storage) = runtime.storage().lock() {
        let _ = storage.set_setting(ACCESS_SETTING_KEY, effective.as_str());
    }

    if effective.can_listen() {
        let runtime_for_source = runtime.clone();
        let handle_for_source = handle.clone();
        let mut source = WindowsNotificationSource::new();
        let started = source.start(Arc::new(move |notification| {
            // The OS already displayed this toast; we only observe.
            let processed = runtime_for_source.process(notification);
            tracing::debug!(
                id = %processed.notification.id,
                decision = processed.evaluation.decision.as_str(),
                "notification processed"
            );
            handle_for_source.update(|state| state.history_stale = true);
        }));
        match started {
            Ok(()) => {
                *slot.lock().expect("source slot poisoned") = Some(Box::new(source));
                tracing::info!("WinRT notification listener started");
            }
            Err(error) => {
                tracing::warn!(%error, "listener start failed; running degraded");
            }
        }
    }
    effective
}

#[cfg(target_os = "linux")]
fn platform_source_flow(runtime: FocusRuntime, handle: UiHandle, slot: SourceSlot) -> AccessState {
    use tpt_focus_core::platform::linux::dbus::DbusNotificationSource;

    let runtime_for_source = runtime.clone();
    let handle_for_source = handle.clone();
    let mut source = DbusNotificationSource::new();
    let started = source.start(Arc::new(move |notification| {
        route_linux(&runtime_for_source, &handle_for_source, notification);
    }));

    match started {
        Ok(()) => {
            *slot.lock().expect("source slot poisoned") = Some(Box::new(source));
            tracing::info!("D-Bus notification daemon started");
            AccessState::Granted
        }
        Err(error) => {
            tracing::warn!(%error, "D-Bus daemon start failed");
            handle.update(|state| {
                state.status = format!("could not become the notification daemon: {error}");
            });
            AccessState::Unavailable
        }
    }
}

#[cfg(target_os = "linux")]
fn route_linux(runtime: &FocusRuntime, handle: &UiHandle, notification: Notification) {
    let processed = runtime.process(notification.clone());
    if processed.evaluation.suppressed() {
        tracing::debug!(
            id = %processed.notification.id,
            decision = processed.evaluation.decision.as_str(),
            "notification suppressed"
        );
    } else {
        // Allowed: we are the daemon, so render the bubble.
        handle.update(|state| {
            state.bubbles.push(Bubble::new(notification, 5_000));
            state.history_stale = true;
        });
        handle.send(UiCommand::Show(crate::state::Page::History));
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn platform_source_flow(
    _runtime: FocusRuntime,
    handle: UiHandle,
    _slot: SourceSlot,
) -> AccessState {
    handle.update(|state| {
        state.status = "no notification source for this platform".into();
    });
    AccessState::Unavailable
}

/// Re-run the consent prompt from the settings page.
pub fn request_access_background(handle: UiHandle) {
    std::thread::Builder::new()
        .name("tpt-focus-consent".into())
        .spawn(move || {
            #[cfg(windows)]
            {
                use tpt_focus_core::platform::access::NotificationAccess as _;
                use tpt_focus_core::platform::windows::access::WinRtAccess;
                let access = WinRtAccess::new(AccessState::Unknown);
                let state = access.request();
                handle.update(|ui| ui.access = state);
            }
            #[cfg(not(windows))]
            {
                handle.update(|state| {
                    state.status = "no consent prompt on this platform".into();
                });
            }
        })
        .expect("spawn consent thread");
}

/// User activated an action on a bubble → emit `ActionInvoked`.
pub fn bubble_action(id: &tpt_focus_core::NotificationId, key: &str) {
    #[cfg(target_os = "linux")]
    if let Err(error) = tpt_focus_core::platform::linux::dbus::emit_action(id, key) {
        tracing::warn!(%error, "ActionInvoked failed");
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (id, key);
}

/// A bubble disappeared → emit `NotificationClosed`.
pub fn bubble_closed(id: &tpt_focus_core::NotificationId, by_user: bool) {
    #[cfg(target_os = "linux")]
    {
        use tpt_focus_core::platform::linux::dbus::emit_closed;
        use tpt_focus_core::platform::linux::ClosedReason;
        let reason = if by_user {
            ClosedReason::Dismissed
        } else {
            ClosedReason::Expired
        };
        if let Err(error) = emit_closed(id, reason) {
            tracing::warn!(%error, "NotificationClosed failed");
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (id, by_user);
}
