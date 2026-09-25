//! Linux backend — the `org.freedesktop.Notifications` D-Bus daemon plus the
//! X11/Wayland context signals that feed rule conditions.
//!
//! There is no consent prompt on Linux: tpt-focus either claims the
//! well-known bus name (becoming *the* notification daemon) or reports a
//! conflict with the existing daemon (GNOME Shell, dunst, mako, …).

pub mod context;
pub mod dbus;

/// Well-known bus name of the freedesktop notification service.
pub const NOTIFICATIONS_BUS_NAME: &str = "org.freedesktop.Notifications";

/// Object path implementing the notification interface.
pub const NOTIFICATIONS_OBJECT_PATH: &str = "/org/freedesktop/Notifications";

/// D-Bus interface implemented by the daemon.
pub const NOTIFICATIONS_INTERFACE: &str = "org.freedesktop.Notifications";

/// `NotificationClosed` reason codes mandated by the specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ClosedReason {
    /// The notification timed out.
    Expired = 1,
    /// The user dismissed it.
    Dismissed = 2,
    /// `CloseNotification` was called.
    ByRequest = 3,
    /// Undefined / implementation-specific.
    Other = 4,
}
