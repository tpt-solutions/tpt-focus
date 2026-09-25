//! SQLite-backed notification history with full-text search.
//!
//! The database holds everything the user can look back on: raw notifications,
//! the decision the rule engine reached for each, focus profiles, schedules
//! and key/value settings. Rules themselves stay in the TOML config file.
//!
//! [`Connection`] is `Send` but not `Sync`, so cross-thread sharing goes
//! through [`SharedStorage`] (an `Arc<Mutex<Storage>>`).

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::engine::ProcessedNotification;
use crate::error::{Error, Result};
use crate::model::{AppIdentity, Notification, NotificationAction, NotificationId, Urgency};
use crate::profile::FocusProfile;
use crate::rules::Decision;
use crate::schedule::Schedule;

/// Storage shared between the tray, CLI and pipeline listener.
pub type SharedStorage = Arc<Mutex<Storage>>;

/// Open (creating if needed) a storage handle suitable for sharing.
pub fn open_shared(path: impl AsRef<Path>) -> Result<SharedStorage> {
    Ok(Arc::new(Mutex::new(Storage::open(path)?)))
}

/// Write every pipeline decision into `storage`.
///
/// Storage failures are logged rather than propagated so a full disk or
/// locked database can never take the notification sink down.
pub fn record_history(pipeline: &crate::engine::Pipeline, storage: SharedStorage) {
    pipeline.set_listener(Arc::new(move |processed| {
        let entry = HistoryEntry::from_processed(processed);
        match storage.lock() {
            Ok(guard) => {
                if let Err(error) = guard.record(&entry) {
                    tracing::error!(%error, id = %entry.notification.id, "failed to persist notification");
                }
            }
            Err(_) => tracing::error!("history storage lock poisoned"),
        }
    }));
}

/// One row of user-visible history: a notification plus its decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub notification: Notification,
    pub decision: Decision,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,

    #[serde(default)]
    pub reason: String,

    #[serde(default)]
    pub read: bool,

    #[serde(default = "Utc::now")]
    pub decided_at: DateTime<Utc>,
}

impl HistoryEntry {
    /// Build an entry from a pipeline decision.
    pub fn from_processed(processed: &ProcessedNotification) -> Self {
        Self {
            notification: processed.notification.clone(),
            decision: processed.evaluation.decision,
            rule_id: processed.evaluation.matched_rule.clone(),
            reason: processed.evaluation.reason.clone(),
            read: false,
            decided_at: Utc::now(),
        }
    }
}

/// Filter set for [`Storage::query`] / [`Storage::search`].
#[derive(Debug, Clone, Default)]
pub struct HistoryQuery {
    /// Exact `app_id` (case-insensitive).
    pub app: Option<String>,
    /// Only this decision.
    pub decision: Option<Decision>,
    /// `timestamp >= since`.
    pub since: Option<DateTime<Utc>>,
    /// `timestamp <= until`.
    pub until: Option<DateTime<Utc>>,
    /// Only unread notifications.
    pub unread_only: bool,
    /// Maximum rows returned.
    pub limit: Option<u32>,
    /// Rows skipped (for pagination).
    pub offset: Option<u32>,
    /// Oldest-first instead of newest-first.
    pub ascending: bool,
}

impl HistoryQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_app(mut self, app: impl Into<String>) -> Self {
        self.app = Some(app.into());
        self
    }

    pub fn with_decision(mut self, decision: Decision) -> Self {
        self.decision = Some(decision);
        self
    }

    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn since(mut self, since: DateTime<Utc>) -> Self {
        self.since = Some(since);
        self
    }

    pub fn until(mut self, until: DateTime<Utc>) -> Self {
        self.until = Some(until);
        self
    }

    pub fn unread_only(mut self) -> Self {
        self.unread_only = true;
        self
    }

    pub fn ascending(mut self) -> Self {
        self.ascending = true;
        self
    }
}

/// Retention policy applied by [`Storage::prune`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Retention {
    /// Delete notifications older than this.
    pub max_age: Option<Duration>,
    /// Keep only the newest N notifications.
    pub max_count: Option<u64>,
}

/// What a prune run actually removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub removed_by_age: u64,
    pub removed_by_count: u64,
}

impl PruneReport {
    pub fn total(&self) -> u64 {
        self.removed_by_age + self.removed_by_count
    }
}

/// Portable backup document produced by [`Storage::export_json`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryExport {
    pub schema_version: u32,
    pub exported_at: DateTime<Utc>,
    pub entries: Vec<HistoryEntry>,
}

/// SQLite connection wrapper owning schema migrations.
pub struct Storage {
    conn: Connection,
}

/// Columns + join shared by every history read.
const ENTRY_COLUMNS: &str = "n.id, n.app_id, n.app_display_name, n.title, n.body, n.urgency, \
     n.timestamp_millis, n.actions_json, n.icon_json, n.read, \
     d.decision, d.rule_id, d.reason, d.decided_millis";

const DECISION_JOIN: &str = "LEFT JOIN rule_decisions d ON d.id = ( \
     SELECT d2.id FROM rule_decisions d2 \
     WHERE d2.notification_id = n.id \
     ORDER BY d2.id DESC LIMIT 1 )";

/// Schema migrations; index + 1 is the resulting `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[
    // v1 — initial schema
    "
    CREATE TABLE IF NOT EXISTS notifications (
        id              TEXT PRIMARY KEY NOT NULL,
        app_id          TEXT NOT NULL,
        app_display_name TEXT,
        title           TEXT NOT NULL,
        body            TEXT NOT NULL,
        urgency         TEXT NOT NULL,
        timestamp_millis INTEGER NOT NULL,
        actions_json    TEXT NOT NULL DEFAULT '[]',
        icon_json       TEXT,
        read            INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS idx_notifications_timestamp ON notifications(timestamp_millis);
    CREATE INDEX IF NOT EXISTS idx_notifications_app ON notifications(app_id);

    CREATE TABLE IF NOT EXISTS rule_decisions (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        notification_id TEXT NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
        decision        TEXT NOT NULL,
        rule_id         TEXT,
        reason          TEXT NOT NULL DEFAULT '',
        decided_millis  INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_decisions_notification
        ON rule_decisions(notification_id);

    CREATE VIRTUAL TABLE IF NOT EXISTS notifications_fts USING fts5(
        notification_id UNINDEXED,
        title,
        body,
        tokenize = 'unicode61'
    );

    CREATE TABLE IF NOT EXISTS profiles (
        name             TEXT PRIMARY KEY NOT NULL,
        description      TEXT,
        default_decision TEXT,
        position         INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE IF NOT EXISTS schedules (
        id           TEXT PRIMARY KEY NOT NULL,
        name         TEXT NOT NULL,
        weekdays     TEXT NOT NULL DEFAULT '[]',
        start_time   TEXT NOT NULL,
        end_time     TEXT NOT NULL,
        profile      TEXT,
        enabled      INTEGER NOT NULL DEFAULT 1
    );

    CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY NOT NULL,
        value TEXT NOT NULL
    );
    ",
];

impl Storage {
    /// Open (creating parents and applying migrations) a database file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|source| Error::StorageDir {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
        }

        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// In-memory database for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> Result<()> {
        let current: u32 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;

        for (index, sql) in MIGRATIONS.iter().enumerate() {
            let target = (index + 1) as u32;
            if current < target {
                self.conn.execute_batch(sql)?;
                self.conn
                    .execute_batch(&format!("PRAGMA user_version = {target}"))?;
            }
        }
        Ok(())
    }

    /// Schema version currently applied.
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }

    // ---------------------------------------------------------------- writes

    /// Store a notification together with the decision reached for it.
    ///
    /// Re-recording the same id updates the notification and appends a new
    /// decision (the audit trail is preserved).
    pub fn record(&self, entry: &HistoryEntry) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        Self::store_notification(&tx, &entry.notification, entry.read)?;
        Self::store_decision(&tx, entry)?;
        tx.commit()?;
        Ok(())
    }

    /// Store a notification without a decision (used by imports).
    pub fn insert_notification(&self, notification: &Notification) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        Self::store_notification(&tx, notification, false)?;
        tx.commit()?;
        Ok(())
    }

    fn store_notification(conn: &Connection, n: &Notification, read: bool) -> Result<()> {
        let display_name = n.source.display_name.as_deref();
        let actions = serde_json::to_string(&n.actions)?;
        let icon = n.icon.as_ref().map(serde_json::to_string).transpose()?;

        conn.execute(
            "INSERT INTO notifications
             (id, app_id, app_display_name, title, body, urgency, timestamp_millis,
              actions_json, icon_json, read)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                 app_id = excluded.app_id,
                 app_display_name = excluded.app_display_name,
                 title = excluded.title,
                 body = excluded.body,
                 urgency = excluded.urgency,
                 timestamp_millis = excluded.timestamp_millis,
                 actions_json = excluded.actions_json,
                 icon_json = excluded.icon_json,
                 read = excluded.read",
            params![
                n.id.as_str(),
                n.source.app_id,
                display_name,
                n.title,
                n.body,
                urgency_str(n.urgency),
                n.timestamp.timestamp_millis(),
                actions,
                icon,
                read as i64,
            ],
        )?;

        conn.execute(
            "DELETE FROM notifications_fts WHERE notification_id = ?1",
            params![n.id.as_str()],
        )?;
        conn.execute(
            "INSERT INTO notifications_fts (notification_id, title, body)
             VALUES (?1, ?2, ?3)",
            params![n.id.as_str(), n.title, n.body],
        )?;
        Ok(())
    }

    fn store_decision(conn: &Connection, entry: &HistoryEntry) -> Result<()> {
        conn.execute(
            "INSERT INTO rule_decisions
             (notification_id, decision, rule_id, reason, decided_millis)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry.notification.id.as_str(),
                entry.decision.as_str(),
                entry.rule_id,
                entry.reason,
                entry.decided_at.timestamp_millis(),
            ],
        )?;
        Ok(())
    }

    /// Set/clear the read flag. Returns `false` when the id is unknown.
    pub fn set_read(&self, id: &NotificationId, read: bool) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE notifications SET read = ?1 WHERE id = ?2",
            params![read as i64, id.as_str()],
        )?;
        Ok(changed > 0)
    }

    /// Remove one notification (and its decisions/search entry).
    pub fn delete_notification(&self, id: &NotificationId) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let removed = tx.execute(
            "DELETE FROM notifications WHERE id = ?1",
            params![id.as_str()],
        )?;
        tx.execute(
            "DELETE FROM notifications_fts WHERE notification_id = ?1",
            params![id.as_str()],
        )?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Remove every notification. Returns how many were deleted.
    pub fn clear_history(&self) -> Result<u64> {
        let tx = self.conn.unchecked_transaction()?;
        let removed = tx.execute("DELETE FROM notifications", [])?;
        tx.execute("DELETE FROM notifications_fts", [])?;
        tx.execute("DELETE FROM rule_decisions", [])?;
        tx.commit()?;
        Ok(removed as u64)
    }

    /// Apply a retention policy, returning what was removed.
    pub fn prune(&self, retention: &Retention) -> Result<PruneReport> {
        self.prune_at(retention, Utc::now())
    }

    /// [`Storage::prune`] with an explicit clock (for tests).
    pub fn prune_at(&self, retention: &Retention, now: DateTime<Utc>) -> Result<PruneReport> {
        let mut report = PruneReport::default();
        let tx = self.conn.unchecked_transaction()?;

        if let Some(max_age) = retention.max_age {
            let cutoff = (now - max_age).timestamp_millis();
            let ids: Vec<String> = {
                let mut stmt =
                    tx.prepare("SELECT id FROM notifications WHERE timestamp_millis < ?1")?;
                let rows = stmt.query_map(params![cutoff], |row| row.get::<_, String>(0))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            report.removed_by_age = Self::delete_ids(&tx, &ids)?;
        }

        if let Some(max_count) = retention.max_count {
            let total: i64 =
                tx.query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))?;
            let overflow = total - max_count as i64;
            if overflow > 0 {
                let ids: Vec<String> = {
                    let mut stmt = tx.prepare(
                        "SELECT id FROM notifications
                         ORDER BY timestamp_millis DESC, id DESC
                         LIMIT -1 OFFSET ?1",
                    )?;
                    let rows = stmt.query_map(params![overflow], |row| row.get::<_, String>(0))?;
                    rows.collect::<std::result::Result<Vec<_>, _>>()?
                };
                report.removed_by_count = Self::delete_ids(&tx, &ids)?;
            }
        }

        tx.commit()?;
        Ok(report)
    }

    fn delete_ids(conn: &Connection, ids: &[String]) -> Result<u64> {
        let mut removed = 0u64;
        for id in ids {
            removed += conn.execute(
                "DELETE FROM notifications WHERE id = ?1",
                params![id.as_str()],
            )? as u64;
            conn.execute(
                "DELETE FROM notifications_fts WHERE notification_id = ?1",
                params![id.as_str()],
            )?;
        }
        Ok(removed)
    }

    // ----------------------------------------------------------------- reads

    /// Number of stored notifications.
    pub fn count(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))?;
        Ok(count as u64)
    }

    /// Number of notifications not yet marked read.
    pub fn unread_count(&self) -> Result<u64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE read = 0",
            [],
            |r| r.get(0),
        )?;
        Ok(count as u64)
    }

    /// Fetch a single notification by id.
    pub fn get_notification(&self, id: &NotificationId) -> Result<Option<Notification>> {
        let raw = self
            .conn
            .query_row(
                "SELECT id, app_id, app_display_name, title, body, urgency,
                        timestamp_millis, actions_json, icon_json
                 FROM notifications WHERE id = ?1",
                params![id.as_str()],
                map_notification_row,
            )
            .optional()?;
        raw.map(decode_notification).transpose()
    }

    /// Filtered history query.
    pub fn query(&self, filter: &HistoryQuery) -> Result<Vec<HistoryEntry>> {
        let mut conditions = Vec::new();
        let mut args = Vec::new();
        push_filters(filter, &mut conditions, &mut args);

        let mut sql = format!("SELECT {ENTRY_COLUMNS} FROM notifications n {DECISION_JOIN}");
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(&order_clause(filter));
        push_limit(filter, &mut sql, &mut args);

        self.fetch_entries(&sql, args)
    }

    /// Full-text search over title and body, ranked by BM25.
    ///
    /// Each whitespace-separated term is quoted, so user input cannot inject
    /// FTS5 syntax.
    pub fn search(&self, text: &str, filter: &HistoryQuery) -> Result<Vec<HistoryEntry>> {
        let fts_query = fts_quote(text);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut args = vec![rusqlite::types::Value::Text(fts_query)];

        let mut conditions = vec!["notifications_fts MATCH ?1".to_string()];
        let mut filter_args = Vec::new();
        push_filters(filter, &mut conditions, &mut filter_args);
        args.extend(filter_args);

        let mut sql = format!(
            "SELECT {ENTRY_COLUMNS}
             FROM notifications_fts
             JOIN notifications n ON n.id = notifications_fts.notification_id
             {DECISION_JOIN}
             WHERE {}",
            conditions.join(" AND ")
        );
        if filter.ascending {
            sql.push_str(" ORDER BY n.timestamp_millis ASC, n.id ASC, bm25(notifications_fts) ASC");
        } else {
            sql.push_str(
                " ORDER BY bm25(notifications_fts) ASC, n.timestamp_millis DESC, n.id DESC",
            );
        }
        push_limit(filter, &mut sql, &mut args);

        self.fetch_entries(&sql, args)
    }

    fn fetch_entries(
        &self,
        sql: &str,
        args: Vec<rusqlite::types::Value>,
    ) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params_from_iter(args), map_entry_row)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(decode_entry(row?)?);
        }
        Ok(entries)
    }

    // ---------------------------------------------------------------- digest

    /// Per-app digest groups over suppressed history (mute/batch) newer than
    /// `since`, oldest app first. This is the historical view backing the
    /// live [`crate::digest::DigestCollector`].
    pub fn digest_groups(&self, since: DateTime<Utc>) -> Result<Vec<crate::digest::DigestGroup>> {
        use crate::digest::{DigestEntry, DigestGroup};
        use std::collections::BTreeMap;

        let mut groups: BTreeMap<String, DigestGroup> = BTreeMap::new();
        for decision in [Decision::Mute, Decision::Batch] {
            let filter = HistoryQuery::new()
                .with_decision(decision)
                .since(since)
                .ascending();
            for entry in self.query(&filter)? {
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
                group.entries.push(DigestEntry {
                    notification: entry.notification,
                    matched_rule: entry.rule_id,
                    reason: entry.reason,
                });
            }
        }
        Ok(groups.into_values().collect())
    }

    // -------------------------------------------------------------- profiles

    /// Insert or update a focus profile.
    pub fn save_profile(&self, profile: &FocusProfile, position: u32) -> Result<()> {
        self.conn.execute(
            "INSERT INTO profiles (name, description, default_decision, position)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET
                 description = excluded.description,
                 default_decision = excluded.default_decision,
                 position = excluded.position",
            params![
                profile.name,
                profile.description,
                profile.default_decision.map(|d| d.as_str().to_string()),
                position as i64,
            ],
        )?;
        Ok(())
    }

    /// All stored profiles, in `position` order.
    pub fn list_profiles(&self) -> Result<Vec<FocusProfile>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, description, default_decision FROM profiles ORDER BY position, name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;

        let mut profiles = Vec::new();
        for row in rows {
            let (name, description, default_decision) = row?;
            profiles.push(FocusProfile {
                name,
                description,
                default_decision: default_decision.as_deref().and_then(parse_decision),
            });
        }
        Ok(profiles)
    }

    /// Remove a profile. Returns `false` when the name is unknown.
    pub fn delete_profile(&self, name: &str) -> Result<bool> {
        let removed = self
            .conn
            .execute("DELETE FROM profiles WHERE name = ?1", params![name])?;
        Ok(removed > 0)
    }

    // ------------------------------------------------------------- schedules

    /// Insert or update a schedule.
    pub fn save_schedule(&self, schedule: &Schedule) -> Result<()> {
        schedule.validate()?;
        let weekdays = serde_json::to_string(
            &schedule
                .weekdays
                .iter()
                .map(|d| format!("{d:?}").to_lowercase())
                .collect::<Vec<_>>(),
        )?;
        self.conn.execute(
            "INSERT INTO schedules
             (id, name, weekdays, start_time, end_time, profile, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 weekdays = excluded.weekdays,
                 start_time = excluded.start_time,
                 end_time = excluded.end_time,
                 profile = excluded.profile,
                 enabled = excluded.enabled",
            params![
                schedule.id,
                schedule.name,
                weekdays,
                schedule.start_time.to_string(),
                schedule.end_time.to_string(),
                schedule.profile,
                schedule.enabled as i64,
            ],
        )?;
        Ok(())
    }

    /// All stored schedules.
    pub fn list_schedules(&self) -> Result<Vec<Schedule>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, weekdays, start_time, end_time, profile, enabled
             FROM schedules ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ScheduleRow {
                id: row.get(0)?,
                name: row.get(1)?,
                weekdays: row.get(2)?,
                start_time: row.get(3)?,
                end_time: row.get(4)?,
                profile: row.get(5)?,
                enabled: row.get(6)?,
            })
        })?;

        let mut schedules = Vec::new();
        for row in rows {
            schedules.push(decode_schedule(row?)?);
        }
        Ok(schedules)
    }

    /// Remove a schedule. Returns `false` when the id is unknown.
    pub fn delete_schedule(&self, id: &str) -> Result<bool> {
        let removed = self
            .conn
            .execute("DELETE FROM schedules WHERE id = ?1", params![id])?;
        Ok(removed > 0)
    }

    // -------------------------------------------------------------- settings

    /// Read a settings value.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Write a settings value.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- backup

    /// Serialise the whole history to JSON.
    pub fn export_json(&self) -> Result<String> {
        let entries = self.query(&HistoryQuery {
            ascending: true,
            ..HistoryQuery::default()
        })?;
        let export = HistoryExport {
            schema_version: self.schema_version()?,
            exported_at: Utc::now(),
            entries,
        };
        Ok(serde_json::to_string_pretty(&export)?)
    }

    /// Restore entries from [`Storage::export_json`] output.
    ///
    /// Existing ids are updated in place; returns how many entries were
    /// written.
    pub fn import_json(&self, raw: &str) -> Result<usize> {
        let export: HistoryExport = serde_json::from_str(raw)?;
        if export.schema_version == 0 || export.schema_version > MIGRATIONS.len() as u32 {
            return Err(Error::validation(format!(
                "backup schema version {} is not supported by this build",
                export.schema_version
            )));
        }
        for entry in &export.entries {
            self.record(entry)?;
        }
        Ok(export.entries.len())
    }
}

// -------------------------------------------------------------- row mapping

struct ScheduleRow {
    id: String,
    name: String,
    weekdays: String,
    start_time: String,
    end_time: String,
    profile: Option<String>,
    enabled: i64,
}

fn map_entry_row(row: &Row<'_>) -> rusqlite::Result<RawEntry> {
    Ok(RawEntry {
        id: row.get(0)?,
        app_id: row.get(1)?,
        app_display_name: row.get(2)?,
        title: row.get(3)?,
        body: row.get(4)?,
        urgency: row.get(5)?,
        timestamp_millis: row.get(6)?,
        actions_json: row.get(7)?,
        icon_json: row.get(8)?,
        read: row.get(9)?,
        decision: row.get(10)?,
        rule_id: row.get(11)?,
        reason: row.get(12)?,
        decided_millis: row.get(13)?,
    })
}

fn map_notification_row(row: &Row<'_>) -> rusqlite::Result<RawNotification> {
    Ok(RawNotification {
        id: row.get(0)?,
        app_id: row.get(1)?,
        app_display_name: row.get(2)?,
        title: row.get(3)?,
        body: row.get(4)?,
        urgency: row.get(5)?,
        timestamp_millis: row.get(6)?,
        actions_json: row.get(7)?,
        icon_json: row.get(8)?,
    })
}

struct RawNotification {
    id: String,
    app_id: String,
    app_display_name: Option<String>,
    title: String,
    body: String,
    urgency: String,
    timestamp_millis: i64,
    actions_json: String,
    icon_json: Option<String>,
}

struct RawEntry {
    id: String,
    app_id: String,
    app_display_name: Option<String>,
    title: String,
    body: String,
    urgency: String,
    timestamp_millis: i64,
    actions_json: String,
    icon_json: Option<String>,
    read: i64,
    decision: Option<String>,
    rule_id: Option<String>,
    reason: Option<String>,
    decided_millis: Option<i64>,
}

fn decode_notification(raw: RawNotification) -> Result<Notification> {
    let actions: Vec<NotificationAction> = serde_json::from_str(&raw.actions_json)?;
    let icon = raw
        .icon_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;

    Ok(Notification {
        id: NotificationId::from(raw.id),
        source: AppIdentity {
            app_id: raw.app_id,
            display_name: raw.app_display_name,
        },
        title: raw.title,
        body: raw.body,
        urgency: parse_urgency(&raw.urgency).unwrap_or_default(),
        timestamp: millis_to_datetime(raw.timestamp_millis),
        actions,
        icon,
    })
}

fn decode_entry(raw: RawEntry) -> Result<HistoryEntry> {
    let notification = decode_notification(RawNotification {
        id: raw.id,
        app_id: raw.app_id,
        app_display_name: raw.app_display_name,
        title: raw.title,
        body: raw.body,
        urgency: raw.urgency,
        timestamp_millis: raw.timestamp_millis,
        actions_json: raw.actions_json,
        icon_json: raw.icon_json,
    })?;

    let decided_at = raw
        .decided_millis
        .map(millis_to_datetime)
        .unwrap_or(notification.timestamp);

    Ok(HistoryEntry {
        decision: raw
            .decision
            .as_deref()
            .and_then(parse_decision)
            .unwrap_or(Decision::Allow),
        rule_id: raw.rule_id,
        reason: raw.reason.unwrap_or_default(),
        read: raw.read != 0,
        decided_at,
        notification,
    })
}

fn decode_schedule(raw: ScheduleRow) -> Result<Schedule> {
    let weekdays: Vec<String> = serde_json::from_str(&raw.weekdays)?;
    let mut parsed = Vec::new();
    for name in weekdays {
        match name.as_str() {
            "mon" => parsed.push(chrono::Weekday::Mon),
            "tue" => parsed.push(chrono::Weekday::Tue),
            "wed" => parsed.push(chrono::Weekday::Wed),
            "thu" => parsed.push(chrono::Weekday::Thu),
            "fri" => parsed.push(chrono::Weekday::Fri),
            "sat" => parsed.push(chrono::Weekday::Sat),
            "sun" => parsed.push(chrono::Weekday::Sun),
            other => {
                return Err(Error::validation(format!(
                    "stored schedule `{}` has unknown weekday `{other}`",
                    raw.id
                )))
            }
        }
    }

    let start_time = raw
        .start_time
        .parse()
        .map_err(|_| Error::validation(format!("schedule `{}` has invalid start_time", raw.id)))?;
    let end_time = raw
        .end_time
        .parse()
        .map_err(|_| Error::validation(format!("schedule `{}` has invalid end_time", raw.id)))?;

    Ok(Schedule {
        id: raw.id,
        name: raw.name,
        weekdays: parsed,
        start_time,
        end_time,
        profile: raw.profile,
        enabled: raw.enabled != 0,
    })
}

// ------------------------------------------------------------------ helpers

fn order_clause(filter: &HistoryQuery) -> String {
    if filter.ascending {
        " ORDER BY n.timestamp_millis ASC, n.id ASC".to_string()
    } else {
        " ORDER BY n.timestamp_millis DESC, n.id DESC".to_string()
    }
}

fn push_filters(
    filter: &HistoryQuery,
    conditions: &mut Vec<String>,
    args: &mut Vec<rusqlite::types::Value>,
) {
    use rusqlite::types::Value;

    if let Some(app) = &filter.app {
        conditions.push(format!("LOWER(n.app_id) = LOWER(?{})", args.len() + 1));
        args.push(Value::Text(app.clone()));
    }
    if let Some(decision) = filter.decision {
        conditions.push(format!("d.decision = ?{}", args.len() + 1));
        args.push(Value::Text(decision.as_str().to_string()));
    }
    if let Some(since) = filter.since {
        conditions.push(format!("n.timestamp_millis >= ?{}", args.len() + 1));
        args.push(Value::Integer(since.timestamp_millis()));
    }
    if let Some(until) = filter.until {
        conditions.push(format!("n.timestamp_millis <= ?{}", args.len() + 1));
        args.push(Value::Integer(until.timestamp_millis()));
    }
    if filter.unread_only {
        conditions.push(format!("n.read = ?{}", args.len() + 1));
        args.push(Value::Integer(0));
    }
}

fn push_limit(filter: &HistoryQuery, sql: &mut String, args: &mut Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value;
    if let Some(limit) = filter.limit {
        sql.push_str(&format!(" LIMIT ?{}", args.len() + 1));
        args.push(Value::Integer(limit as i64));
        if let Some(offset) = filter.offset {
            sql.push_str(&format!(" OFFSET ?{}", args.len() + 1));
            args.push(Value::Integer(offset as i64));
        }
    }
}

/// Wrap every whitespace-separated term in double quotes so arbitrary user
/// input is safe to pass to FTS5.
fn fts_quote(text: &str) -> String {
    text.split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn urgency_str(urgency: Urgency) -> &'static str {
    match urgency {
        Urgency::Low => "low",
        Urgency::Normal => "normal",
        Urgency::Critical => "critical",
    }
}

fn parse_urgency(raw: &str) -> Option<Urgency> {
    match raw {
        "low" => Some(Urgency::Low),
        "critical" => Some(Urgency::Critical),
        _ => Some(Urgency::Normal),
    }
}

fn parse_decision(raw: &str) -> Option<Decision> {
    match raw {
        "allow" => Some(Decision::Allow),
        "mute" => Some(Decision::Mute),
        "batch" => Some(Decision::Batch),
        _ => None,
    }
}

fn millis_to_datetime(millis: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(millis).unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Notification;

    fn sample(app: &str, title: &str, body: &str) -> HistoryEntry {
        // SQLite stores millisecond precision; align so round-trips compare equal.
        let timestamp = DateTime::from_timestamp_millis(Utc::now().timestamp_millis())
            .expect("valid timestamp");
        HistoryEntry {
            notification: Notification::new(AppIdentity::new(app), title, body)
                .with_timestamp(timestamp),
            decision: Decision::Allow,
            rule_id: None,
            reason: "test".to_string(),
            read: false,
            decided_at: timestamp,
        }
    }

    #[test]
    fn insert_and_query_round_trip() {
        let storage = Storage::open_in_memory().unwrap();
        let mut entry = sample("slack", "Standup", "in 5 minutes");
        entry.decision = Decision::Mute;
        entry.rule_id = Some("mute-slack".into());

        storage.record(&entry).unwrap();
        assert_eq!(storage.count().unwrap(), 1);

        let rows = storage.query(&HistoryQuery::new()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], entry);
    }

    #[test]
    fn query_filters_by_app_and_decision() {
        let storage = Storage::open_in_memory().unwrap();

        let mut slack = sample("slack", "Ping", "hello");
        slack.decision = Decision::Mute;
        storage.record(&slack).unwrap();
        storage.record(&sample("mail", "Digest", "today")).unwrap();

        assert_eq!(
            storage
                .query(&HistoryQuery::new().with_app("SLACK"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            storage
                .query(&HistoryQuery::new().with_decision(Decision::Mute))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            storage
                .query(&HistoryQuery::new().with_decision(Decision::Allow))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(storage.query(&HistoryQuery::new()).unwrap().len(), 2);
    }

    #[test]
    fn query_filters_by_date_range() {
        let storage = Storage::open_in_memory().unwrap();
        let mut storage_entries = sample("app", "old", "b");
        storage_entries.notification.timestamp = Utc::now() - Duration::days(10);
        let mut newer = sample("app", "new", "b");
        newer.notification.timestamp = Utc::now();

        storage.record(&storage_entries).unwrap();
        storage.record(&newer).unwrap();

        let since = Utc::now() - Duration::days(1);
        let rows = storage.query(&HistoryQuery::new().since(since)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].notification.title, "new");

        let until = Utc::now() - Duration::days(5);
        let rows = storage.query(&HistoryQuery::new().until(until)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].notification.title, "old");
    }

    #[test]
    fn full_text_search_matches_title_and_body() {
        let storage = Storage::open_in_memory().unwrap();
        storage
            .record(&sample("slack", "Standup", "daily meeting"))
            .unwrap();
        storage
            .record(&sample("mail", "Invoice", "payment received"))
            .unwrap();
        storage
            .record(&sample("jira", "Bug report", "standup blocker"))
            .unwrap();

        let rows = storage.search("standup", &HistoryQuery::new()).unwrap();
        assert_eq!(rows.len(), 2);

        let rows = storage.search("payment", &HistoryQuery::new()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].notification.source.app_id, "mail");

        // Multi-term AND semantics.
        let rows = storage
            .search("daily meeting", &HistoryQuery::new())
            .unwrap();
        assert_eq!(rows.len(), 1);

        assert!(storage.search("", &HistoryQuery::new()).unwrap().is_empty());
    }

    #[test]
    fn search_survives_fts_operator_input() {
        let storage = Storage::open_in_memory().unwrap();
        storage.record(&sample("app", "Bug", "crash")).unwrap();

        // Operator-looking input must not error or match everything.
        assert!(storage.search("AND OR NOT", &HistoryQuery::new()).is_ok());
        assert!(storage.search("*)(", &HistoryQuery::new()).is_ok());
        assert_eq!(
            storage.search("crash", &HistoryQuery::new()).unwrap().len(),
            1
        );
    }

    #[test]
    fn read_state_is_persisted_and_counted() {
        let storage = Storage::open_in_memory().unwrap();
        let entry = sample("app", "t", "b");
        let id = entry.notification.id.clone();
        storage.record(&entry).unwrap();

        assert_eq!(storage.unread_count().unwrap(), 1);
        assert!(storage.set_read(&id, true).unwrap());
        assert_eq!(storage.unread_count().unwrap(), 0);
        assert!(storage
            .query(&HistoryQuery::new().unread_only())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn pruning_respects_max_age_and_max_count() {
        let storage = Storage::open_in_memory().unwrap();
        let now = Utc::now();

        for days in [10, 5, 1] {
            let mut entry = sample("app", &format!("day {days}"), "b");
            entry.notification.timestamp = now - Duration::days(days);
            storage.record(&entry).unwrap();
        }
        assert_eq!(storage.count().unwrap(), 3);

        let report = storage
            .prune_at(
                &Retention {
                    max_age: Some(Duration::days(7)),
                    max_count: None,
                },
                now,
            )
            .unwrap();
        assert_eq!(report.removed_by_age, 1);
        assert_eq!(storage.count().unwrap(), 2);

        let report = storage
            .prune_at(
                &Retention {
                    max_age: None,
                    max_count: Some(1),
                },
                now,
            )
            .unwrap();
        assert_eq!(report.removed_by_count, 1);
        assert_eq!(storage.count().unwrap(), 1);
        assert_eq!(
            storage.query(&HistoryQuery::new()).unwrap()[0]
                .notification
                .title,
            "day 1"
        );
    }

    #[test]
    fn prune_removes_search_index_too() {
        let storage = Storage::open_in_memory().unwrap();
        storage.record(&sample("app", "unique-term", "b")).unwrap();
        assert_eq!(
            storage
                .search("unique-term", &HistoryQuery::new())
                .unwrap()
                .len(),
            1
        );

        storage.clear_history().unwrap();
        assert_eq!(
            storage
                .search("unique-term", &HistoryQuery::new())
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn export_and_import_restore_history() {
        let storage = Storage::open_in_memory().unwrap();
        let mut entry = sample("slack", "Title", "Body");
        entry.decision = Decision::Batch;
        storage.record(&entry).unwrap();

        let json = storage.export_json().unwrap();

        let restored = Storage::open_in_memory().unwrap();
        let imported = restored.import_json(&json).unwrap();
        assert_eq!(imported, 1);
        assert_eq!(restored.query(&HistoryQuery::new()).unwrap()[0], entry);
    }

    #[test]
    fn import_rejects_future_schema() {
        let storage = Storage::open_in_memory().unwrap();
        let json =
            r#"{"schema_version": 999, "exported_at": "2026-01-01T00:00:00Z", "entries": []}"#;
        assert!(storage.import_json(json).is_err());
    }

    #[test]
    fn profiles_and_schedules_round_trip() {
        let storage = Storage::open_in_memory().unwrap();

        let profile = FocusProfile::new("Deep Work")
            .with_description("No chat")
            .with_default_decision(Decision::Mute);
        storage.save_profile(&profile, 0).unwrap();
        assert_eq!(storage.list_profiles().unwrap(), vec![profile.clone()]);
        assert!(storage.delete_profile("Deep Work").unwrap());
        assert!(storage.list_profiles().unwrap().is_empty());

        let schedule = Schedule::new(
            "quiet",
            chrono::NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
            chrono::NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
        )
        .named("Quiet hours")
        .with_weekdays(vec![chrono::Weekday::Sat, chrono::Weekday::Sun])
        .activating_profile("Deep Work");
        storage.save_schedule(&schedule).unwrap();

        let listed = storage.list_schedules().unwrap();
        assert_eq!(listed, vec![schedule]);
        assert!(storage.delete_schedule("quiet").unwrap());
        assert!(storage.list_schedules().unwrap().is_empty());
    }

    #[test]
    fn settings_are_upserted() {
        let storage = Storage::open_in_memory().unwrap();
        assert_eq!(storage.get_setting("theme").unwrap(), None);
        storage.set_setting("theme", "dark").unwrap();
        storage.set_setting("theme", "light").unwrap();
        assert_eq!(
            storage.get_setting("theme").unwrap().as_deref(),
            Some("light")
        );
    }

    #[test]
    fn file_backed_storage_recreates_missing_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("history.db");

        {
            let storage = Storage::open(&path).unwrap();
            storage.record(&sample("app", "t", "b")).unwrap();
        }

        let storage = Storage::open(&path).unwrap();
        assert_eq!(storage.count().unwrap(), 1);
        assert_eq!(storage.schema_version().unwrap(), 1);
    }

    #[test]
    fn fts_quote_escapes_user_input() {
        assert_eq!(fts_quote("hello world"), "\"hello\" \"world\"");
        assert_eq!(fts_quote("say \"hi\""), "\"say\" \"\"\"hi\"\"\"");
        assert_eq!(fts_quote("   "), "");
    }

    #[test]
    fn pipeline_decisions_are_recorded_to_history() {
        use crate::config::Config;
        use crate::context::MockContextProvider;
        use crate::engine::Pipeline;
        use crate::rules::Rule;

        let config = Config {
            rules: vec![Rule::new("mute-mail", Decision::Mute).with_condition(
                crate::rules::Condition::App {
                    apps: vec!["mail".into()],
                },
            )],
            ..Config::default()
        };

        let shared = Arc::new(Mutex::new(Storage::open_in_memory().unwrap()));

        let pipeline = Pipeline::from_refs(&config, Arc::new(MockContextProvider::new()));
        record_history(&pipeline, shared.clone());

        pipeline.process(Notification::new(AppIdentity::new("mail"), "s", "b"));
        pipeline.process(Notification::new(AppIdentity::new("slack"), "s", "b"));

        let guard = shared.lock().unwrap();
        let entries = guard.query(&HistoryQuery::new()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            guard
                .query(&HistoryQuery::new().with_decision(Decision::Mute))
                .unwrap()
                .len(),
            1
        );
    }
}
