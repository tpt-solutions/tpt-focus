//! Composition layer wiring the rule pipeline, history storage, digest
//! collector and schedule controller into one running focus engine.
//!
//! Both the tray app and the headless daemon build a [`FocusRuntime`];
//! platform crates only supply the [`ContextProvider`] and
//! [`NotificationSource`] implementations.

use std::sync::{Arc, Mutex};

use chrono::{Local, NaiveDateTime, Utc};

use crate::config::Config;
use crate::context::ContextProvider;
use crate::digest::DigestCollector;
use crate::engine::{Pipeline, ProcessedNotification};
use crate::error::{Error, Result};
use crate::model::Notification;
use crate::profile::{ActiveProfile, ScheduleController, ScheduleTick};
use crate::schedule::Schedule;
use crate::storage::{HistoryEntry, PruneReport, Retention, SharedStorage};

/// The running focus engine. Cheap to clone; all state is shared.
#[derive(Clone)]
pub struct FocusRuntime {
    pipeline: Pipeline,
    storage: SharedStorage,
    digests: Arc<DigestCollector>,
    retention: Arc<Mutex<Retention>>,
    active_profile: Arc<ActiveProfile>,
    schedules: Arc<Mutex<Vec<Schedule>>>,
    controller: Arc<Mutex<ScheduleController>>,
}

impl FocusRuntime {
    pub fn new(
        config: Config,
        context: Arc<dyn ContextProvider>,
        storage: SharedStorage,
        active_profile: Arc<ActiveProfile>,
    ) -> Self {
        let pipeline = Pipeline::new(Arc::new(config), context);
        let digests = Arc::new(DigestCollector::default());

        let digest_sink = Arc::clone(&digests);
        let storage_sink = Arc::clone(&storage);
        pipeline.set_listener(Arc::new(move |processed: &ProcessedNotification| {
            digest_sink.add(processed);
            let entry = HistoryEntry::from_processed(processed);
            match storage_sink.lock() {
                Ok(guard) => {
                    if let Err(error) = guard.record(&entry) {
                        tracing::error!(
                            %error,
                            id = %entry.notification.id,
                            "failed to persist notification"
                        );
                    }
                }
                Err(_) => tracing::error!("history storage lock poisoned"),
            }
        }));

        Self {
            pipeline,
            storage,
            digests,
            retention: Arc::new(Mutex::new(Retention::default())),
            active_profile,
            schedules: Arc::new(Mutex::new(Vec::new())),
            controller: Arc::new(Mutex::new(ScheduleController::new())),
        }
    }

    /// Route one notification through the engine: rules decide, history
    /// records, suppressed entries accumulate in the digest.
    pub fn process(&self, notification: Notification) -> ProcessedNotification {
        self.pipeline.process(notification)
    }

    /// Rule pipeline (config hot-reload, ad-hoc evaluation).
    pub fn pipeline(&self) -> &Pipeline {
        &self.pipeline
    }

    /// History store.
    pub fn storage(&self) -> &SharedStorage {
        &self.storage
    }

    /// Digest accumulator for suppressed notifications.
    pub fn digests(&self) -> &Arc<DigestCollector> {
        &self.digests
    }

    /// Shared manual profile toggle.
    pub fn active_profile(&self) -> &Arc<ActiveProfile> {
        &self.active_profile
    }

    /// Set the retention policy applied by [`prune`](Self::prune).
    pub fn set_retention(&self, retention: Retention) {
        *self.retention.lock().expect("runtime retention poisoned") = retention;
    }

    /// Apply the retention policy now.
    pub fn prune(&self) -> Result<PruneReport> {
        let retention = *self.retention.lock().expect("runtime retention poisoned");
        let storage = self
            .storage
            .lock()
            .map_err(|_| Error::Source("history storage lock poisoned".into()))?;
        storage.prune(&retention)
    }

    /// Install the schedule list evaluated by [`tick_schedules`](Self::tick_schedules).
    pub fn set_schedules(&self, schedules: Vec<Schedule>) {
        *self.schedules.lock().expect("runtime schedules poisoned") = schedules;
    }

    /// Re-evaluate schedules against the current wall clock and switch the
    /// active profile when the winning schedule changed.
    pub fn tick_schedules(&self) -> ScheduleTick {
        let schedules: Vec<Schedule> = self
            .schedules
            .lock()
            .expect("runtime schedules poisoned")
            .clone();

        let now_local: NaiveDateTime = Utc::now().with_timezone(&Local).naive_local();
        self.controller
            .lock()
            .expect("schedule controller poisoned")
            .tick(&schedules, &self.active_profile, now_local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::MockContextProvider;
    use crate::model::AppIdentity;
    use crate::rules::{Condition, Decision, Rule};
    use crate::storage::{HistoryQuery, Storage};
    use chrono::Duration;

    fn runtime() -> FocusRuntime {
        let config = Config {
            rules: vec![
                Rule::new("mute-slack", Decision::Mute).with_condition(Condition::App {
                    apps: vec!["slack".into()],
                }),
            ],
            ..Config::default()
        };
        let context = Arc::new(MockContextProvider::new());
        let storage = Arc::new(Mutex::new(Storage::open_in_memory().unwrap()));
        FocusRuntime::new(config, context, storage, Arc::new(ActiveProfile::new()))
    }

    #[test]
    fn processed_notifications_land_in_history_and_digest() {
        let runtime = runtime();

        let allowed = runtime.process(Notification::new(AppIdentity::new("mail"), "t", "b"));
        let muted = runtime.process(Notification::new(AppIdentity::new("slack"), "t", "b"));
        assert_eq!(allowed.evaluation.decision, Decision::Allow);
        assert_eq!(muted.evaluation.decision, Decision::Mute);

        let storage = runtime.storage().lock().unwrap();
        assert_eq!(storage.count().unwrap(), 2);
        assert_eq!(
            storage
                .query(&HistoryQuery::new().with_decision(Decision::Mute))
                .unwrap()
                .len(),
            1
        );
        drop(storage);

        assert_eq!(runtime.digests().held_count(), 1);
        assert!(runtime.digests().holds(&muted.notification.id));
    }

    #[test]
    fn retention_prunes_history() {
        let runtime = runtime();
        runtime.process(Notification::new(AppIdentity::new("mail"), "t", "b"));
        assert_eq!(runtime.storage().lock().unwrap().count().unwrap(), 1);

        runtime.set_retention(Retention {
            max_age: Some(Duration::seconds(0)),
            max_count: None,
        });
        let report = runtime.prune().unwrap();
        assert_eq!(report.removed_by_age, 1);
        assert_eq!(runtime.storage().lock().unwrap().count().unwrap(), 0);
    }

    #[test]
    fn empty_schedule_list_never_touched_the_manual_toggle() {
        let runtime = runtime();
        runtime.set_schedules(Vec::new());

        runtime.tick_schedules();
        assert!(runtime.active_profile().active().is_none());

        // With no schedule claiming the slot the manual toggle stays put.
        runtime.active_profile().activate("Deep Work");
        runtime.tick_schedules();
        assert_eq!(
            runtime.active_profile().active().as_deref(),
            Some("Deep Work")
        );
    }
}
