//! The notification ingestion trait plus a mock backend for tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{Notification, NotificationId};

/// Callback invoked by a [`NotificationSource`] for every incoming alert.
pub type NotificationCallback = Arc<dyn Fn(Notification) + Send + Sync>;

/// Feature flags reported by a notification source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SourceCapabilities {
    /// The source can remove a notification from the OS surface on request.
    pub can_dismiss: bool,
    /// The source can report that the user dismissed a notification natively.
    pub observes_user_dismiss: bool,
    /// Actions (buttons) are forwarded with the notification.
    pub supports_actions: bool,
    /// The source supports idempotent updates of an existing notification.
    pub supports_replace: bool,
}

/// A platform feed of notifications (WinRT listener, D-Bus daemon, Archon
/// bridge, …) feeding the focus engine.
pub trait NotificationSource: Send {
    /// Stable identifier, e.g. `"winrt"`, `"dbus"`, `"mock"`.
    fn id(&self) -> &str;

    /// Query what this backend is able to do.
    fn capabilities(&self) -> SourceCapabilities;

    /// Begin delivering notifications to `on_notification`.
    ///
    /// Calling `start` twice without `stop` is an error.
    fn start(&mut self, on_notification: NotificationCallback) -> Result<()>;

    /// Stop delivering notifications and release platform resources.
    fn stop(&mut self) -> Result<()>;

    /// Ask the platform to remove `id` from the notification surface.
    fn dismiss(&self, id: &NotificationId) -> Result<()>;
}

/// Mock source used by unit tests and CI (no package identity, no D-Bus).
pub struct MockNotificationSource {
    id: String,
    capabilities: SourceCapabilities,
    running: AtomicBool,
    callback: Mutex<Option<NotificationCallback>>,
    dismissed: Mutex<Vec<NotificationId>>,
    delivered: Mutex<Vec<Notification>>,
}

impl std::fmt::Debug for MockNotificationSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockNotificationSource")
            .field("id", &self.id)
            .field("capabilities", &self.capabilities)
            .field("running", &self.is_running())
            .field("dismissed", &self.dismissed())
            .field("delivered", &self.delivered().len())
            .finish()
    }
}

impl MockNotificationSource {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            capabilities: SourceCapabilities {
                can_dismiss: true,
                observes_user_dismiss: true,
                supports_actions: true,
                supports_replace: true,
            },
            running: AtomicBool::new(false),
            callback: Mutex::new(None),
            dismissed: Mutex::new(Vec::new()),
            delivered: Mutex::new(Vec::new()),
        }
    }

    pub fn with_capabilities(mut self, capabilities: SourceCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Feed a notification through the started callback, recording it as
    /// delivered.
    pub fn push(&self, notification: Notification) {
        self.delivered
            .lock()
            .expect("mock source poisoned")
            .push(notification.clone());

        let callback = self.callback.lock().expect("mock source poisoned");
        if let Some(callback) = callback.as_ref() {
            callback(notification);
        }
    }

    /// Ids removed via [`NotificationSource::dismiss`].
    pub fn dismissed(&self) -> Vec<NotificationId> {
        self.dismissed.lock().expect("mock source poisoned").clone()
    }

    /// Notifications handed to `push`.
    pub fn delivered(&self) -> Vec<Notification> {
        self.delivered.lock().expect("mock source poisoned").clone()
    }
}

impl NotificationSource for MockNotificationSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self) -> SourceCapabilities {
        self.capabilities
    }

    fn start(&mut self, on_notification: NotificationCallback) -> Result<()> {
        if self.is_running() {
            return Err(Error::Source(format!(
                "notification source `{}` is already running",
                self.id
            )));
        }
        *self.callback.lock().expect("mock source poisoned") = Some(on_notification);
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if !self.is_running() {
            return Err(Error::SourceNotRunning(self.id.clone()));
        }
        *self.callback.lock().expect("mock source poisoned") = None;
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn dismiss(&self, id: &NotificationId) -> Result<()> {
        if !self.is_running() {
            return Err(Error::SourceNotRunning(self.id.clone()));
        }
        self.dismissed
            .lock()
            .expect("mock source poisoned")
            .push(id.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppIdentity, Notification};

    #[test]
    fn start_delivers_and_stop_halts() {
        let mut source = MockNotificationSource::new("mock");
        let seen: Arc<Mutex<Vec<Notification>>> = Arc::new(Mutex::new(Vec::new()));

        let sink = seen.clone();
        source
            .start(Arc::new(move |n| sink.lock().unwrap().push(n)))
            .unwrap();
        assert!(source.is_running());

        source.push(Notification::new(AppIdentity::new("app"), "t", "b"));
        assert_eq!(seen.lock().unwrap().len(), 1);

        source.stop().unwrap();
        assert!(!source.is_running());

        source.push(Notification::new(AppIdentity::new("app"), "t", "b"));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn double_start_is_rejected() {
        let mut source = MockNotificationSource::new("mock");
        source.start(Arc::new(|_| {})).unwrap();
        assert!(source.start(Arc::new(|_| {})).is_err());
    }

    #[test]
    fn dismiss_requires_running_source() {
        let mut source = MockNotificationSource::new("mock");
        let id = NotificationId::new();
        assert!(source.dismiss(&id).is_err());

        source.start(Arc::new(|_| {})).unwrap();
        source.dismiss(&id).unwrap();
        assert_eq!(source.dismissed(), vec![id]);
    }

    #[test]
    fn capabilities_are_queryable() {
        let source = MockNotificationSource::new("mock");
        assert!(source.capabilities().can_dismiss);
        assert_eq!(source.id(), "mock");
    }
}
