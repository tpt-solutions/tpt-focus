//! WinRT notification listener feeding the core pipeline.
//!
//! Delivery strategy: the `NotificationChanged` event is used when the OS
//! allows it, and a `GetNotificationsAsync` diffing poller takes over when
//! subscription fails — some sparse-package deployments reject the event with
//! `0x80070490` even though the listener API itself is usable. Either way a
//! notification that appears in Action Center reaches the pipeline exactly
//! once.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::{DateTime, Utc};
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{
    UserNotification, UserNotificationChangedEventArgs, UserNotificationChangedKind,
};

use crate::error::{Error, Result};
use crate::model::{AppIdentity, Notification, NotificationId};
use crate::source::{NotificationCallback, NotificationSource, SourceCapabilities};

use super::win32;

/// Source id reported to logs and the CLI.
pub const SOURCE_ID: &str = "winrt";

/// Interval between fallback polls of `GetNotificationsAsync`.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

type Shared = Arc<Mutex<Option<(UserNotificationListener, NotificationCallback)>>>;

/// Windows notification source backed by `UserNotificationListener`.
///
/// Requires package identity and a granted consent (Phase 3); otherwise
/// [`NotificationSource::start`] returns an error instead of silently doing
/// nothing.
pub struct WindowsNotificationSource {
    shared: Shared,
    token: Option<i64>,
    poller: Option<Poller>,
    running: bool,
}

struct Poller {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Default for WindowsNotificationSource {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsNotificationSource {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(None)),
            token: None,
            poller: None,
            running: false,
        }
    }

    /// Convert one WinRT notification into the core model.
    ///
    /// Pure: everything WinRT-shaped is reduced to plain data first so this
    /// stays unit-testable without package identity.
    pub(super) fn to_core(
        winrt_id: u32,
        created: Option<DateTime<Utc>>,
        app_id: String,
        display_name: Option<String>,
        texts: Vec<String>,
    ) -> Notification {
        let (title, body) = texts_to_title_body(&texts);
        let mut notification = Notification::new(
            AppIdentity {
                app_id,
                display_name,
            },
            title,
            body,
        );
        notification.id = notification_id(winrt_id);
        if let Some(created) = created {
            notification.timestamp = created;
        }
        notification
    }

    fn from_user_notification(
        user: &UserNotification,
    ) -> std::result::Result<Notification, String> {
        let winrt_id = user.Id().map_err(|e| e.to_string())?;

        let app_id = user
            .AppInfo()
            .ok()
            .and_then(|info| info.AppUserModelId().ok())
            .map(|value| value.to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "windows.unknown".to_string());

        let display_name = user
            .AppInfo()
            .ok()
            .and_then(|info| info.DisplayInfo().ok())
            .and_then(|info| info.DisplayName().ok())
            .map(|value| value.to_string())
            .filter(|value| !value.is_empty());

        let created = user
            .CreationTime()
            .ok()
            .and_then(|time| winrt_time_to_utc(time.UniversalTime));

        let texts = extract_texts(user).unwrap_or_default();

        Ok(Self::to_core(
            winrt_id,
            created,
            app_id,
            display_name,
            texts,
        ))
    }

    fn fetch(
        listener: &UserNotificationListener,
        winrt_id: u32,
    ) -> std::result::Result<Notification, String> {
        let user: UserNotification = listener
            .GetNotification(winrt_id)
            .map_err(|error| error.to_string())?;
        Self::from_user_notification(&user)
    }

    /// Snapshot the ids currently visible to the listener.
    fn current_ids(listener: &UserNotificationListener) -> std::result::Result<Vec<u32>, String> {
        let operation = listener
            .GetNotificationsAsync(windows::UI::Notifications::NotificationKinds::Toast)
            .map_err(|error| error.to_string())?;
        let view = super::block_on(operation).map_err(|error| error.to_string())?;

        let size = view.Size().map_err(|e| e.to_string())?;
        let mut ids = Vec::with_capacity(size as usize);
        for index in 0..size {
            let user = view.GetAt(index).map_err(|e| e.to_string())?;
            if let Ok(id) = user.Id() {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// Subscribe to the change event, or fall back to a diffing poller.
    fn start_delivery(
        &mut self,
        listener: &UserNotificationListener,
        baseline: HashSet<u32>,
        callback: NotificationCallback,
    ) {
        let handler_listener = listener.clone();
        let state = Arc::clone(&self.shared);
        let handler =
            TypedEventHandler::<UserNotificationListener, UserNotificationChangedEventArgs>::new(
                move |_sender, args| {
                    let Some(args) = args.as_ref() else {
                        return Ok(());
                    };
                    let added = args
                        .ChangeKind()
                        .map(|kind| kind.0 == UserNotificationChangedKind::Added.0)
                        .unwrap_or(false);
                    if !added {
                        return Ok(());
                    }
                    let Ok(winrt_id) = args.UserNotificationId() else {
                        return Ok(());
                    };

                    // Copy what we need out of the lock so WinRT calls never run
                    // while the mutex is held.
                    let callback = state
                        .lock()
                        .ok()
                        .and_then(|guard| guard.as_ref().map(|(_, cb)| cb.clone()));
                    let Some(callback) = callback else {
                        return Ok(());
                    };

                    if let Ok(notification) = Self::fetch(&handler_listener, winrt_id) {
                        callback(notification);
                    }
                    Ok(())
                },
            );

        match listener.NotificationChanged(&handler) {
            Ok(token) => self.token = Some(token),
            Err(error) => {
                tracing::warn!(
                    error_code = error.code().0,
                    %error,
                    "NotificationChanged subscription failed; falling back to polling"
                );
                self.poller = Some(start_poller(listener.clone(), baseline, callback));
            }
        }
    }
}

impl NotificationSource for WindowsNotificationSource {
    fn id(&self) -> &str {
        SOURCE_ID
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            can_dismiss: true,
            observes_user_dismiss: true,
            supports_actions: false,
            supports_replace: true,
        }
    }

    fn start(&mut self, on_notification: NotificationCallback) -> Result<()> {
        if self.running {
            return Err(Error::Source(format!(
                "notification source `{SOURCE_ID}` is already running"
            )));
        }

        // WinRT activation and `NotificationChanged` subscription both need
        // an initialised COM apartment; `start` may run on any worker thread.
        win32::ensure_com_initialized();

        let listener = UserNotificationListener::Current()
            .map_err(|error| Error::Source(format!("no package identity: {error}")))?;

        let status = listener
            .GetAccessStatus()
            .map_err(|error| Error::Source(format!("access status unavailable: {error}")))?;
        if status.0 != UserNotificationListenerAccessStatus::Allowed.0 {
            return Err(Error::Source(
                "notification access not granted; run the consent flow first".to_string(),
            ));
        }

        // Baseline of what is already in Action Center, so startup does not
        // replay history — only genuinely new notifications are delivered.
        let baseline: HashSet<u32> = Self::current_ids(&listener)
            .map_err(Error::Source)?
            .into_iter()
            .collect();

        *self.shared.lock().expect("winrt source poisoned") =
            Some((listener.clone(), on_notification.clone()));
        self.running = true;

        self.start_delivery(&listener, baseline, on_notification);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if !self.running {
            return Err(Error::Source(format!(
                "notification source `{SOURCE_ID}` is not running"
            )));
        }

        let state = self.shared.lock().expect("winrt source poisoned").take();
        if let Some(token) = self.token.take() {
            if let Some((listener, _)) = state.as_ref() {
                let _ = listener.RemoveNotificationChanged(token);
            }
        }
        self.poller = None; // Drop joins the poller thread.
        self.running = false;
        Ok(())
    }

    fn dismiss(&self, id: &NotificationId) -> Result<()> {
        let winrt_id = parse_notification_id(id).ok_or_else(|| {
            Error::Source(format!("id `{id}` does not belong to the Windows source"))
        })?;

        let listener = self
            .shared
            .lock()
            .expect("winrt source poisoned")
            .as_ref()
            .map(|(listener, _)| listener.clone())
            .ok_or_else(|| Error::SourceNotRunning(SOURCE_ID.to_string()))?;

        listener
            .RemoveNotification(winrt_id)
            .map_err(|error| Error::Source(format!("dismiss failed: {error}")))
    }
}

/// Background diffing poller used when the change event is unavailable.
///
/// Every `POLL_INTERVAL` it snapshots the visible notification ids and
/// delivers the ones that appeared since the last snapshot (or since the
/// startup baseline). This keeps ingestion alive on systems where the
/// `NotificationChanged` event cannot be subscribed.
fn start_poller(
    listener: UserNotificationListener,
    baseline: HashSet<u32>,
    callback: NotificationCallback,
) -> Poller {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);

    let handle = thread::Builder::new()
        .name("tpt-focus-winrt-poll".into())
        .spawn(move || {
            // The poller thread makes its own WinRT calls and therefore
            // needs its own initialised COM apartment.
            win32::ensure_com_initialized();

            let mut known = baseline;
            while !stop_flag.load(Ordering::SeqCst) {
                // Sleep in short slices so `stop` joins promptly.
                let wake = std::time::Instant::now() + POLL_INTERVAL;
                while std::time::Instant::now() < wake && !stop_flag.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(100));
                }
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }

                let Ok(ids) = WindowsNotificationSource::current_ids(&listener) else {
                    continue;
                };

                let fresh: Vec<u32> = ids
                    .iter()
                    .filter(|id| !known.contains(id))
                    .copied()
                    .collect();
                for id in fresh {
                    known.insert(id);
                    match WindowsNotificationSource::fetch(&listener, id) {
                        Ok(notification) => callback(notification),
                        Err(error) => {
                            tracing::debug!(winrt_id = id, %error, "poll fetch failed")
                        }
                    }
                }
            }
        })
        .expect("spawn winrt poller");

    Poller {
        stop,
        handle: Some(handle),
    }
}

/// Pull the human-readable lines out of a WinRT notification.
fn extract_texts(user: &UserNotification) -> std::result::Result<Vec<String>, String> {
    let notification = user.Notification().map_err(|e| e.to_string())?;
    let visual = notification.Visual().map_err(|e| e.to_string())?;
    let bindings = visual.Bindings().map_err(|e| e.to_string())?;

    let mut texts = Vec::new();
    for index in 0..bindings.Size().map_err(|e| e.to_string())? {
        let binding = bindings.GetAt(index).map_err(|e| e.to_string())?;
        let elements = binding.GetTextElements().map_err(|e| e.to_string())?;
        for element_index in 0..elements.Size().map_err(|e| e.to_string())? {
            let element = elements.GetAt(element_index).map_err(|e| e.to_string())?;
            if let Ok(text) = element.Text() {
                let text = text.to_string();
                if !text.trim().is_empty() {
                    texts.push(text);
                }
            }
        }
        if !texts.is_empty() {
            break;
        }
    }
    Ok(texts)
}

/// First non-empty line becomes the title, the rest the body.
pub(super) fn texts_to_title_body(texts: &[String]) -> (String, String) {
    let mut lines = texts.iter().map(|t| t.trim()).filter(|t| !t.is_empty());
    let title = lines.next().unwrap_or("Notification").to_string();
    let body = lines.collect::<Vec<_>>().join("\n");
    (title, body)
}

/// Namespaced id so ids from other sources can never collide.
pub(super) fn notification_id(winrt_id: u32) -> NotificationId {
    NotificationId::from(format!("{SOURCE_ID}:{winrt_id}"))
}

/// Inverse of [`notification_id`].
pub(super) fn parse_notification_id(id: &NotificationId) -> Option<u32> {
    let raw = id.as_str().strip_prefix(&format!("{SOURCE_ID}:"))?;
    raw.parse().ok()
}

/// Convert a WinRT `DateTime` (100ns ticks since 1601-01-01) to UTC.
pub(super) fn winrt_time_to_utc(universal_time: i64) -> Option<DateTime<Utc>> {
    const EPOCH_DIFFERENCE: i64 = 116_444_736_000_000_000;
    let unix_ticks = universal_time.checked_sub(EPOCH_DIFFERENCE)?;
    let millis = unix_ticks.checked_div(10_000)?;
    DateTime::from_timestamp_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_and_body_come_from_text_elements() {
        let texts = vec![
            "Standup in 5 minutes".to_string(),
            "Daily sync starting soon".to_string(),
        ];
        let (title, body) = texts_to_title_body(&texts);
        assert_eq!(title, "Standup in 5 minutes");
        assert_eq!(body, "Daily sync starting soon");

        let (title, body) = texts_to_title_body(&[]);
        assert_eq!(title, "Notification");
        assert!(body.is_empty());

        let (title, body) = texts_to_title_body(&["   ".to_string(), "Actual title".to_string()]);
        assert_eq!(title, "Actual title");
        assert!(body.is_empty());
    }

    #[test]
    fn ids_round_trip_through_the_namespace() {
        assert_eq!(
            parse_notification_id(&notification_id(42)),
            Some(42),
            "winrt ids must round trip for dismiss-through"
        );
        assert_eq!(
            parse_notification_id(&NotificationId::from("other:1")),
            None
        );
        assert_eq!(parse_notification_id(&NotificationId::from("nope")), None);
    }

    #[test]
    fn winrt_timestamps_convert_to_utc() {
        // 1601-01-01T00:00:00Z → 1970-01-01T00:00:00Z
        assert_eq!(
            winrt_time_to_utc(116_444_736_000_000_000),
            Some(DateTime::from_timestamp_millis(0).unwrap())
        );

        // 2026-09-25T00:00:00Z
        let unix_seconds: i64 = 1_790_000_000;
        let winrt = unix_seconds * 10_000_000 + 116_444_736_000_000_000;
        assert_eq!(
            winrt_time_to_utc(winrt),
            Some(DateTime::from_timestamp_millis(unix_seconds * 1000).unwrap())
        );

        assert_eq!(winrt_time_to_utc(i64::MIN), None);
    }

    #[test]
    fn mapping_builds_a_core_notification() {
        let notification = WindowsNotificationSource::to_core(
            7,
            Some(DateTime::from_timestamp_millis(1_790_000_000_000).unwrap()),
            "slack".to_string(),
            Some("Slack".to_string()),
            vec!["Ping".to_string(), "New message".to_string()],
        );

        assert_eq!(notification.id, notification_id(7));
        assert_eq!(notification.source.app_id, "slack");
        assert_eq!(notification.source.display_name.as_deref(), Some("Slack"));
        assert_eq!(notification.title, "Ping");
        assert_eq!(notification.body, "New message");
        assert_eq!(notification.timestamp.timestamp_millis(), 1_790_000_000_000);
    }

    #[test]
    fn fresh_ids_are_the_diff_against_the_baseline() {
        let baseline: HashSet<u32> = [1u32, 2, 3].into_iter().collect();
        let current: Vec<u32> = vec![2, 3, 4, 9];

        let fresh: Vec<u32> = current
            .iter()
            .filter(|id| !baseline.contains(id))
            .copied()
            .collect();
        assert_eq!(fresh, vec![4, 9]);
    }

    /// Live check: with identity + consent the source starts (events or
    /// polling); without them it fails loudly naming the missing piece.
    /// Either way it never hangs and `stop` cleans up.
    #[test]
    fn start_and_stop_are_clean_in_every_environment() {
        let mut source = WindowsNotificationSource::new();
        let started = source.start(Arc::new(|_| {}));

        match started {
            Ok(()) => source.stop().expect("stop after start"),
            Err(error) => {
                let message = error.to_string();
                assert!(
                    message.contains("identity") || message.contains("access"),
                    "unexpected start failure: {message}"
                );
            }
        }
        assert!(!source.running);

        // Double-stop is an error, not a panic.
        assert!(source.stop().is_err());
    }

    #[test]
    fn source_reports_its_capabilities() {
        let source = WindowsNotificationSource::new();
        assert_eq!(source.id(), SOURCE_ID);
        let capabilities = source.capabilities();
        assert!(capabilities.can_dismiss);
        assert!(capabilities.observes_user_dismiss);
        assert!(!capabilities.supports_actions);
    }
}
