//! The `org.freedesktop.Notifications` daemon source (Phase 5).
//!
//! On startup the source claims `org.freedesktop.Notifications` on the
//! session bus; if another daemon (GNOME Shell, dunst, mako, …) already owns
//! the name, `start` fails with a message telling the user how to resolve
//! the conflict. Every incoming `Notify` call is normalised to the core
//! model and handed to the pipeline — the host decides from the decision
//! whether to render a popup. `NotificationClosed` / `ActionInvoked`
//! signals are emitted through the source's helpers once the host closes a
//! bubble or the user activates an action.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use zbus::object_server::SignalEmitter;
use zbus::zvariant::Value;
use zbus::Connection;

use crate::error::{Error, Result};
use crate::model::{
    AppIdentity, IconRef, Notification, NotificationAction, NotificationId, Urgency,
};
use crate::source::{NotificationCallback, NotificationSource, SourceCapabilities};

use super::{ClosedReason, NOTIFICATIONS_BUS_NAME, NOTIFICATIONS_OBJECT_PATH};

/// Source id reported to logs and the CLI.
pub const SOURCE_ID: &str = "dbus";

/// Prefix used for [`NotificationId`]s produced by this source.
pub const ID_PREFIX: &str = "dbus:";

type SharedCallback = Arc<Mutex<Option<NotificationCallback>>>;
type SharedCloseHook = Arc<Mutex<Option<Arc<dyn Fn(u32) + Send + Sync>>>>;
type SharedCounter = Arc<AtomicU32>;

/// Linux notification source backed by the session bus.
pub struct DbusNotificationSource {
    callback: SharedCallback,
    close_hook: SharedCloseHook,
    next_id: SharedCounter,
    connection: Mutex<Option<Connection>>,
    running: AtomicBool,
}

impl Default for DbusNotificationSource {
    fn default() -> Self {
        Self::new()
    }
}

impl DbusNotificationSource {
    pub fn new() -> Self {
        Self {
            callback: Arc::new(Mutex::new(None)),
            close_hook: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU32::new(1)),
            connection: Mutex::new(None),
            running: AtomicBool::new(false),
        }
    }

    /// Register a hook fired when a client calls `CloseNotification(id)`;
    /// the popup host removes the matching bubble.
    pub fn set_close_hook(&self, hook: Arc<dyn Fn(u32) + Send + Sync>) {
        *self.close_hook.lock().expect("dbus close hook poisoned") = Some(hook);
    }

    /// Emit `NotificationClosed(id, reason)`.
    pub fn emit_closed(&self, id: &NotificationId, reason: ClosedReason) -> Result<()> {
        let dbus_id = parse_id(id)?;
        let context = self.signal_emitter()?;
        zbus::block_on(async move {
            Daemon::notification_closed(&context, dbus_id, reason as u32)
                .await
                .map_err(|error| Error::Source(format!("NotificationClosed failed: {error}")))
        })
    }

    /// Emit `ActionInvoked(id, key)` followed by
    /// `NotificationClosed(id, Dismissed)`, as the specification requires
    /// after an action is activated.
    pub fn emit_action(&self, id: &NotificationId, action_key: &str) -> Result<()> {
        let dbus_id = parse_id(id)?;
        let context = self.signal_emitter()?;
        let key = action_key.to_string();
        zbus::block_on(async move {
            Daemon::action_invoked(&context, dbus_id, key)
                .await
                .map_err(|error| Error::Source(format!("ActionInvoked failed: {error}")))?;
            Daemon::notification_closed(&context, dbus_id, ClosedReason::Dismissed as u32)
                .await
                .map_err(|error| Error::Source(format!("NotificationClosed failed: {error}")))
        })
    }

    fn signal_emitter(&self) -> Result<SignalEmitter<'static>> {
        let connection = self
            .connection
            .lock()
            .expect("dbus connection poisoned")
            .clone()
            .ok_or_else(|| Error::SourceNotRunning(SOURCE_ID.to_string()))?;
        SignalEmitter::new(&connection, NOTIFICATIONS_OBJECT_PATH)
            .map_err(|error| Error::Source(format!("signal context failed: {error}")))
    }
}

/// Process-wide signal emitter installed by `start`, so the popup host can
/// emit bubble signals without holding the source instance.
static EMITTER: Mutex<Option<SignalEmitter>> = Mutex::new(None);

fn install_emitter(connection: &Connection) -> Result<()> {
    let emitter = SignalEmitter::new(connection, NOTIFICATIONS_OBJECT_PATH)
        .map_err(|error| Error::Source(format!("signal emitter failed: {error}")))?;
    *EMITTER.lock().expect("dbus emitter poisoned") = Some(emitter);
    Ok(())
}

fn take_emitter() {
    EMITTER.lock().expect("dbus emitter poisoned").take();
}

fn emitter() -> Result<SignalEmitter<'static>> {
    EMITTER
        .lock()
        .expect("dbus emitter poisoned")
        .clone()
        .ok_or_else(|| Error::SourceNotRunning(SOURCE_ID.to_string()))
}

/// Emit `NotificationClosed(id, reason)` from the host side.
pub fn emit_closed(id: &NotificationId, reason: ClosedReason) -> Result<()> {
    let dbus_id = parse_id(id)?;
    let emitter = emitter()?;
    zbus::block_on(async move {
        Daemon::notification_closed(&emitter, dbus_id, reason as u32)
            .await
            .map_err(|error| Error::Source(format!("NotificationClosed failed: {error}")))
    })
}

/// Emit `ActionInvoked(id, key)` then `NotificationClosed(id, Dismissed)`.
pub fn emit_action(id: &NotificationId, action_key: &str) -> Result<()> {
    let dbus_id = parse_id(id)?;
    let emitter = emitter()?;
    let key = action_key.to_string();
    zbus::block_on(async move {
        Daemon::action_invoked(&emitter, dbus_id, key)
            .await
            .map_err(|error| Error::Source(format!("ActionInvoked failed: {error}")))?;
        Daemon::notification_closed(&emitter, dbus_id, ClosedReason::Dismissed as u32)
            .await
            .map_err(|error| Error::Source(format!("NotificationClosed failed: {error}")))
    })
}

fn parse_id(id: &NotificationId) -> Result<u32> {
    id.as_str()
        .strip_prefix(ID_PREFIX)
        .and_then(|raw| raw.parse().ok())
        .ok_or_else(|| Error::Source(format!("id `{id}` does not belong to the Linux source")))
}

impl NotificationSource for DbusNotificationSource {
    fn id(&self) -> &str {
        SOURCE_ID
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            can_dismiss: true,
            observes_user_dismiss: true,
            supports_actions: true,
            supports_replace: true,
        }
    }

    fn start(&mut self, on_notification: NotificationCallback) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Err(Error::Source(format!(
                "notification source `{SOURCE_ID}` is already running"
            )));
        }

        *self.callback.lock().expect("dbus callback poisoned") = Some(on_notification);

        let iface = Daemon {
            callback: Arc::clone(&self.callback),
            close_hook: Arc::clone(&self.close_hook),
            next_id: Arc::clone(&self.next_id),
        };

        let connection = zbus::block_on(async {
            zbus::connection::Builder::session()?
                .name(NOTIFICATIONS_BUS_NAME)?
                .serve_at(NOTIFICATIONS_OBJECT_PATH, iface)?
                .build()
                .await
        })
        .map_err(|error| match &error {
            zbus::Error::NameTaken => Error::Source(format!(
                "another notification daemon already owns {NOTIFICATIONS_BUS_NAME} \
                 (GNOME Shell, dunst, mako, …); disable it to use tpt-focus"
            )),
            other => Error::Source(format!("failed to claim {NOTIFICATIONS_BUS_NAME}: {other}")),
        })?;

        install_emitter(&connection)?;
        *self.connection.lock().expect("dbus connection poisoned") = Some(connection);
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(Error::SourceNotRunning(SOURCE_ID.to_string()));
        }

        take_emitter();
        let connection = self
            .connection
            .lock()
            .expect("dbus connection poisoned")
            .take();
        if let Some(connection) = connection {
            let _ = zbus::block_on(
                async move { connection.release_name(NOTIFICATIONS_BUS_NAME).await },
            );
        }
        *self.callback.lock().expect("dbus callback poisoned") = None;
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn dismiss(&self, id: &NotificationId) -> Result<()> {
        self.emit_closed(id, ClosedReason::ByRequest)
    }
}

/// The zbus service object served at `/org/freedesktop/Notifications`.
struct Daemon {
    callback: SharedCallback,
    close_hook: SharedCloseHook,
    next_id: SharedCounter,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Daemon {
    #[allow(clippy::too_many_arguments)] // mandated by the spec signature
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<&str>,
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        let _ = expire_timeout; // popup policy lives with the host
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::SeqCst)
        };

        let owned_hints: HashMap<String, Value> = hints
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect();

        let notification = map_notify(
            id,
            app_name,
            app_icon,
            summary,
            body,
            &actions,
            &owned_hints,
        );

        let callback = self
            .callback
            .lock()
            .expect("dbus callback poisoned")
            .clone();
        if let Some(callback) = callback {
            callback(notification);
        }

        Ok(id)
    }

    fn close_notification(&self, id: u32) -> zbus::fdo::Result<()> {
        if let Some(hook) = self
            .close_hook
            .lock()
            .expect("dbus close hook poisoned")
            .as_ref()
        {
            hook(id);
        }
        Ok(())
    }

    fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["actions", "body", "body-markup", "icon-static"]
    }

    fn get_server_information(&self) -> (&'static str, &'static str, String, &'static str) {
        (
            "tpt-focus",
            "TPT Solutions",
            crate::VERSION.to_string(),
            "1.2",
        )
    }

    #[zbus(signal)]
    pub async fn notification_closed(
        signal_emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn action_invoked(
        signal_emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: String,
    ) -> zbus::Result<()>;
}

/// Normalise one `Notify` call into the core model. Pure and unit-testable.
fn map_notify(
    id: u32,
    app_name: &str,
    app_icon: &str,
    summary: &str,
    body: &str,
    actions: &[&str],
    hints: &HashMap<String, Value<'_>>,
) -> Notification {
    let desktop_entry = hints
        .get("desktop-entry")
        .and_then(|value| String::try_from(value).ok())
        .filter(|value| !value.is_empty());

    let app_id = desktop_entry
        .clone()
        .or_else(|| (!app_name.is_empty()).then(|| app_name.to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    let display_name = (!app_name.is_empty()).then(|| app_name.to_string());

    let urgency = hints
        .get("urgency")
        .and_then(|value| u8::try_from(value).ok())
        .map(|byte| match byte {
            0 => Urgency::Low,
            2 => Urgency::Critical,
            _ => Urgency::Normal,
        })
        .unwrap_or(Urgency::Normal);

    let icon = image_hint(hints).or_else(|| {
        (!app_icon.is_empty()).then(|| {
            if app_icon.starts_with('/') {
                IconRef::Path(app_icon.into())
            } else {
                IconRef::Name(app_icon.to_string())
            }
        })
    });

    // `actions` alternates key, label: "reply", "Reply", "default", "Open".
    // The `default` pseudo-action is the body click, not a button.
    let buttons = actions
        .chunks(2)
        .filter(|pair| pair.len() == 2 && pair[0] != "default")
        .filter_map(|pair| match pair {
            [key, label] => Some(NotificationAction::new(*key, *label)),
            _ => None,
        })
        .collect();

    let mut notification = Notification::new(
        AppIdentity {
            app_id,
            display_name,
        },
        if summary.is_empty() {
            "Notification"
        } else {
            summary
        },
        body,
    )
    .with_urgency(urgency)
    .with_actions(buttons);
    notification.id = NotificationId::from(format!("{ID_PREFIX}{id}"));
    notification.icon = icon;
    notification
}

fn image_hint(hints: &HashMap<String, Value<'_>>) -> Option<IconRef> {
    for key in ["image-path", "image_path"] {
        if let Some(value) = hints.get(key) {
            if let Ok(path) = String::try_from(value) {
                if !path.is_empty() {
                    return Some(IconRef::Path(path.into()));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints<'v>(entries: &[(&str, Value<'v>)]) -> HashMap<String, Value<'v>> {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn notify_maps_desktop_entry_and_urgency() {
        let notification = map_notify(
            7,
            "Slack",
            "slack-icon",
            "Standup",
            "starts soon",
            &["reply", "Reply", "default", "Open"],
            &hints(&[
                ("desktop-entry", Value::from("com.slack.Slack")),
                ("urgency", Value::U8(2)),
                ("image-path", Value::from("/tmp/img.png")),
            ]),
        );

        assert_eq!(notification.id.as_str(), "dbus:7");
        assert_eq!(notification.source.app_id, "com.slack.Slack");
        assert_eq!(notification.source.display_name.as_deref(), Some("Slack"));
        assert_eq!(notification.urgency, Urgency::Critical);
        assert_eq!(notification.title, "Standup");
        assert_eq!(notification.body, "starts soon");
        assert!(matches!(notification.icon, Some(IconRef::Path(_))));
        // The `default` pseudo-action never becomes a button.
        assert_eq!(
            notification
                .actions
                .iter()
                .map(|action| action.id.as_str())
                .collect::<Vec<_>>(),
            vec!["reply"]
        );
    }

    #[test]
    fn notify_falls_back_to_app_name() {
        let notification = map_notify(1, "", "", "Headline", "", &[], &HashMap::new());
        assert_eq!(notification.source.app_id, "unknown");
        assert_eq!(notification.urgency, Urgency::Normal);
        assert_eq!(notification.title, "Headline");
        assert!(notification.actions.is_empty());
    }

    #[test]
    fn ids_round_trip_through_the_namespace() {
        let id = NotificationId::from("dbus:42");
        assert_eq!(parse_id(&id).unwrap(), 42);
        assert!(parse_id(&NotificationId::from("winrt:1")).is_err());
        assert!(parse_id(&NotificationId::from("nope")).is_err());
    }
}
