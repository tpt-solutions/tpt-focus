//! Batch/digest accumulation for muted and batched notifications.
//!
//! Suppressed notifications are never re-issued as toasts; they accumulate
//! here, grouped by source application, until their time window matures and
//! the tray/CLI presents them as a per-app digest. The history store remains
//! the source of truth — this module is the live, in-memory view.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::engine::ProcessedNotification;
use crate::model::{Notification, NotificationId};

/// How long after its last notification an app's digest matures.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// One suppressed notification inside a [`DigestGroup`].
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DigestEntry {
    pub notification: Notification,
    /// Rule that suppressed it, if any.
    pub matched_rule: Option<String>,
    /// Why it was suppressed (rule name or fallback explanation).
    pub reason: String,
}

impl DigestEntry {
    /// Build from a pipeline outcome. `Allow` decisions return `None` —
    /// allowed notifications are shown immediately, never digested.
    pub fn from_processed(processed: &ProcessedNotification) -> Option<Self> {
        if !processed.suppressed() {
            return None;
        }
        Some(Self {
            notification: processed.notification.clone(),
            matched_rule: processed.evaluation.matched_rule.clone(),
            reason: processed.evaluation.reason.clone(),
        })
    }
}

/// Suppressed notifications from one app, oldest first.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DigestGroup {
    pub app_id: String,
    pub app_display_name: Option<String>,
    pub entries: Vec<DigestEntry>,
}

impl DigestGroup {
    /// First notification in the group.
    pub fn first_seen(&self) -> DateTime<Utc> {
        self.entries
            .first()
            .map(|e| e.notification.timestamp)
            .unwrap_or_else(Utc::now)
    }

    /// Most recent notification in the group.
    pub fn last_seen(&self) -> DateTime<Utc> {
        self.entries
            .last()
            .map(|e| e.notification.timestamp)
            .unwrap_or_else(Utc::now)
    }

    /// Title of the newest entry — the digest headline.
    pub fn headline(&self) -> &str {
        self.entries
            .last()
            .map(|e| e.notification.title.as_str())
            .unwrap_or("")
    }
}

/// Accumulates suppressed notifications per app until their window matures.
#[derive(Debug)]
pub struct DigestCollector {
    window: Duration,
    groups: Mutex<HashMap<String, DigestGroup>>,
}

impl Default for DigestCollector {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW)
    }
}

impl DigestCollector {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            groups: Mutex::new(HashMap::new()),
        }
    }

    /// Configured maturity window.
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Fold a pipeline outcome into its app group. Allowed notifications are
    /// ignored.
    pub fn add(&self, processed: &ProcessedNotification) {
        let Some(entry) = DigestEntry::from_processed(processed) else {
            return;
        };

        let mut groups = self.groups.lock().expect("digest collector poisoned");
        let group = groups
            .entry(entry.notification.source.app_id.clone())
            .or_insert_with(|| DigestGroup {
                app_id: entry.notification.source.app_id.clone(),
                app_display_name: entry.notification.source.display_name.clone(),
                entries: Vec::new(),
            });
        if group.app_display_name.is_none() {
            group.app_display_name = entry.notification.source.display_name.clone();
        }
        group.entries.push(entry);
    }

    /// Snapshot of every open group, oldest entry first.
    pub fn groups(&self) -> Vec<DigestGroup> {
        let mut groups: Vec<DigestGroup> = self
            .groups
            .lock()
            .expect("digest collector poisoned")
            .values()
            .cloned()
            .collect();
        groups.sort_by_key(|g| g.first_seen());
        groups
    }

    /// Number of notifications currently held back.
    pub fn held_count(&self) -> usize {
        self.groups
            .lock()
            .expect("digest collector poisoned")
            .values()
            .map(|g| g.entries.len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.groups
            .lock()
            .expect("digest collector poisoned")
            .is_empty()
    }

    /// Groups quiet for at least the configured window (`now` injected for
    /// tests).
    pub fn mature(&self, now: DateTime<Utc>) -> Vec<DigestGroup> {
        self.groups()
            .into_iter()
            .filter(|group| (now - group.last_seen()).to_std().unwrap_or_default() >= self.window)
            .collect()
    }

    /// Remove and return the mature groups.
    pub fn drain_mature(&self, now: DateTime<Utc>) -> Vec<DigestGroup> {
        let mature_ids: Vec<String> = self.mature(now).into_iter().map(|g| g.app_id).collect();
        let mut groups = self.groups.lock().expect("digest collector poisoned");
        mature_ids
            .into_iter()
            .filter_map(|id| groups.remove(&id))
            .collect()
    }

    /// Drop one specific app's group (user opened the digest). Returns the
    /// removed group.
    pub fn take(&self, app_id: &str) -> Option<DigestGroup> {
        self.groups
            .lock()
            .expect("digest collector poisoned")
            .remove(app_id)
    }

    /// Whether `id` is being held back in any group.
    pub fn holds(&self, id: &NotificationId) -> bool {
        self.groups
            .lock()
            .expect("digest collector poisoned")
            .values()
            .any(|group| group.entries.iter().any(|e| &e.notification.id == id))
    }

    /// Drop everything.
    pub fn clear(&self) {
        self.groups
            .lock()
            .expect("digest collector poisoned")
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::MockContextProvider;
    use crate::model::{AppIdentity, Notification};
    use crate::rules::{Condition, Decision, Rule};
    use crate::{Config, Pipeline};
    use std::sync::Arc;

    fn pipeline_muting(app: &str) -> Pipeline {
        let config = Config {
            rules: vec![
                Rule::new("mute-app", Decision::Mute).with_condition(Condition::App {
                    apps: vec![app.into()],
                }),
            ],
            ..Config::default()
        };
        Pipeline::from_refs(&config, Arc::new(MockContextProvider::new()))
    }

    fn notification(app: &str, title: &str) -> Notification {
        Notification::new(AppIdentity::new(app), title, "body")
    }

    #[test]
    fn allowed_notifications_are_never_collected() {
        let collector = DigestCollector::default();
        let pipeline = pipeline_muting("slack");

        collector.add(&pipeline.process(notification("mail", "hello")));
        assert!(collector.is_empty());
        assert_eq!(collector.held_count(), 0);
    }

    #[test]
    fn suppressed_notifications_group_by_app() {
        let collector = DigestCollector::default();
        let pipeline = pipeline_muting("slack");

        collector.add(&pipeline.process(notification("slack", "one")));
        collector.add(&pipeline.process(notification("slack", "two")));
        collector.add(&pipeline.process(notification("slack", "three")));

        let groups = collector.groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].app_id, "slack");
        assert_eq!(groups[0].entries.len(), 3);
        assert_eq!(groups[0].headline(), "three");
        assert_eq!(
            groups[0].entries[0].matched_rule.as_deref(),
            Some("mute-app")
        );
        assert_eq!(collector.held_count(), 3);
    }

    #[test]
    fn display_name_survives_when_first_entry_has_none() {
        let collector = DigestCollector::default();
        let pipeline = pipeline_muting("slack");

        let named = Notification::new(
            AppIdentity::new("slack").with_display_name("Slack"),
            "t",
            "b",
        );
        collector.add(&pipeline.process(named));
        collector.add(&pipeline.process(notification("slack", "second")));

        let groups = collector.groups();
        assert_eq!(groups[0].app_display_name.as_deref(), Some("Slack"));
    }

    #[test]
    fn mature_groups_are_those_quiet_past_the_window() {
        let collector = DigestCollector::new(Duration::from_secs(60));
        let pipeline = pipeline_muting("slack");

        let early = Notification::new(AppIdentity::new("slack"), "early", "b")
            .with_timestamp(DateTime::from_timestamp_millis(0).unwrap());
        collector.add(&pipeline.process(early));

        let now = DateTime::from_timestamp_millis(30_000).unwrap();
        assert!(collector.mature(now).is_empty());

        let later = now + chrono::Duration::seconds(31);
        let matured = collector.mature(later);
        assert_eq!(matured.len(), 1);
        assert_eq!(matured[0].app_id, "slack");
    }

    #[test]
    fn drain_mature_removes_only_those_groups() {
        let collector = DigestCollector::new(Duration::from_secs(60));
        let pipeline = pipeline_muting("*");

        // slack matures at t=60s; teams at t=120s.
        let slack = Notification::new(AppIdentity::new("slack"), "s", "b")
            .with_timestamp(DateTime::from_timestamp_millis(0).unwrap());
        let teams = Notification::new(AppIdentity::new("teams"), "t", "b")
            .with_timestamp(DateTime::from_timestamp_millis(60_000).unwrap());
        collector.add(&pipeline.process(slack));
        collector.add(&pipeline.process(teams));

        let drained = collector.drain_mature(DateTime::from_timestamp_millis(61_000).unwrap());
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].app_id, "slack");

        assert!(collector.take("teams").is_some());
        assert!(collector.is_empty());
    }

    #[test]
    fn holds_reports_suppressed_ids() {
        let collector = DigestCollector::default();
        let pipeline = pipeline_muting("slack");

        let processed = pipeline.process(notification("slack", "held"));
        collector.add(&processed);
        assert!(collector.holds(&processed.notification.id));
        assert!(!collector.holds(&NotificationId::new()));

        collector.clear();
        assert!(!collector.holds(&processed.notification.id));
    }

    #[test]
    fn batch_decision_is_collected_like_mute() {
        let config = Config {
            rules: vec![Rule::new("batch-all", Decision::Batch)],
            ..Config::default()
        };
        let pipeline = Pipeline::from_refs(&config, Arc::new(MockContextProvider::new()));
        let collector = DigestCollector::default();

        collector.add(&pipeline.process(notification("any", "held")));
        assert_eq!(collector.held_count(), 1);
    }
}
