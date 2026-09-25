//! Notification data model shared by every platform backend.

use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Stable identifier for a single notification.
///
/// Platform backends map their native handles (WinRT tag/group, D-Bus
/// notification id) into this string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NotificationId(String);

impl NotificationId {
    /// Generate a fresh random identifier.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for NotificationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for NotificationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for NotificationId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for NotificationId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Identity of the application that produced a notification.
///
/// `app_id` is the stable, platform-native key: the AUMID on Windows, the
/// `.desktop` id on Linux. Rules match against this value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppIdentity {
    pub app_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

impl AppIdentity {
    pub fn new(app_id: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            display_name: None,
        }
    }

    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = Some(name.into());
        self
    }
}

impl fmt::Display for AppIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.display_name {
            Some(name) => write!(f, "{} ({})", name, self.app_id),
            None => f.write_str(&self.app_id),
        }
    }
}

/// How urgent the operating system considers a notification.
///
/// Mirrors the freedesktop.org `urgency` hint so Linux notifications map 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

/// A single button/inline action attached to a notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationAction {
    /// Platform action key, passed back to the source on invocation.
    pub id: String,
    /// Human-readable label shown on the button.
    pub label: String,
}

impl NotificationAction {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// Reference to a notification icon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconRef {
    /// Theme icon name, e.g. `"slack"` or `"dialog-information"`.
    Name(String),
    /// Absolute path to an image file on disk.
    Path(PathBuf),
    /// Raw image bytes carried with the notification.
    Data { mime_type: String, bytes: Vec<u8> },
}

/// A notification as normalised by the core engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub id: NotificationId,
    pub source: AppIdentity,
    pub title: String,
    pub body: String,
    pub urgency: Urgency,
    pub timestamp: DateTime<Utc>,

    #[serde(default)]
    pub actions: Vec<NotificationAction>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<IconRef>,
}

impl Notification {
    /// Build a notification with sensible defaults (normal urgency, now,
    /// random id, no actions or icon).
    pub fn new(
        source: impl Into<AppIdentity>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            id: NotificationId::new(),
            source: source.into(),
            title: title.into(),
            body: body.into(),
            urgency: Urgency::Normal,
            timestamp: Utc::now(),
            actions: Vec::new(),
            icon: None,
        }
    }

    pub fn with_urgency(mut self, urgency: Urgency) -> Self {
        self.urgency = urgency;
        self
    }

    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = timestamp;
        self
    }

    pub fn with_actions(mut self, actions: Vec<NotificationAction>) -> Self {
        self.actions = actions;
        self
    }

    pub fn with_icon(mut self, icon: IconRef) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// Case-insensitive glob matching supporting `*` and `?`.
///
/// Used for app-identity rule patterns such as `"slack*"` or `"*chrome*"`.
pub fn pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let value: Vec<char> = value.to_lowercase().chars().collect();

    let (mut p, mut v) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut resume = 0usize;

    while v < value.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            resume = v;
        } else if let Some(s) = star {
            p = s + 1;
            resume += 1;
            v = resume;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_matching_is_case_insensitive() {
        assert!(pattern_matches("Slack", "slack"));
        assert!(pattern_matches("SLACK", "Slack"));
    }

    #[test]
    fn pattern_supports_wildcards() {
        assert!(pattern_matches("*chrome*", "Google Chrome"));
        assert!(pattern_matches("slack*", "slack.desktop"));
        assert!(pattern_matches("code-?", "code-x"));
        assert!(!pattern_matches("code?", "code-x"));
        assert!(!pattern_matches("discord", "discord-ptb"));
    }

    #[test]
    fn notification_round_trips_through_serde() {
        let n = Notification::new(
            AppIdentity::new("com.slack.Slack").with_display_name("Slack"),
            "Standup in 5",
            "Daily standup starts soon",
        )
        .with_urgency(Urgency::Critical)
        .with_actions(vec![NotificationAction::new("join", "Join")]);

        let json = serde_json::to_string(&n).unwrap();
        let back: Notification = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn urgency_defaults_to_normal() {
        assert_eq!(Urgency::default(), Urgency::Normal);
    }
}
