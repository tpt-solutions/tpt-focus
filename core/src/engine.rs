//! The notification pipeline: source → context → rules → decision.

use std::sync::{Arc, RwLock};

use crate::config::Config;
use crate::context::ContextProvider;
use crate::error::Result;
use crate::model::Notification;
use crate::rules::{RuleEngine, RuleEvaluation};
use crate::source::NotificationSource;

/// A notification together with the decision the engine reached.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedNotification {
    pub notification: Notification,
    pub evaluation: RuleEvaluation,
}

impl ProcessedNotification {
    /// True when the notification should not be shown immediately.
    pub fn suppressed(&self) -> bool {
        self.evaluation.suppressed()
    }
}

/// Sink notified after every decision (history store, tray, digest builder).
pub type DecisionListener = Arc<dyn Fn(&ProcessedNotification) + Send + Sync>;

/// Wires a [`NotificationSource`] to the rule engine.
///
/// The pipeline owns its configuration behind a lock so rules can be reloaded
/// at runtime without tearing down the source. Cloning a `Pipeline` is cheap
/// and shares that state.
#[derive(Clone)]
pub struct Pipeline {
    inner: Arc<PipelineInner>,
}

struct PipelineInner {
    config: RwLock<Arc<Config>>,
    context: Arc<dyn ContextProvider>,
    listener: RwLock<Option<DecisionListener>>,
}

impl Pipeline {
    pub fn new(config: Arc<Config>, context: Arc<dyn ContextProvider>) -> Self {
        Self {
            inner: Arc::new(PipelineInner {
                config: RwLock::new(config),
                context,
                listener: RwLock::new(None),
            }),
        }
    }

    /// Convenience constructor for borrowing a config.
    pub fn from_refs(config: &Config, context: Arc<dyn ContextProvider>) -> Self {
        Self::new(Arc::new(config.clone()), context)
    }

    /// Register a callback invoked for every processed notification.
    pub fn set_listener(&self, listener: DecisionListener) {
        *self
            .inner
            .listener
            .write()
            .expect("pipeline listener poisoned") = Some(listener);
    }

    /// Hot-reload rules and profiles.
    pub fn replace_config(&self, config: impl Into<Arc<Config>>) {
        *self.inner.config.write().expect("pipeline config poisoned") = config.into();
    }

    /// Current configuration snapshot.
    pub fn config(&self) -> Arc<Config> {
        self.inner
            .config
            .read()
            .expect("pipeline config poisoned")
            .clone()
    }

    /// The context provider backing this pipeline.
    pub fn context_provider(&self) -> &dyn ContextProvider {
        self.inner.context.as_ref()
    }

    /// Run one notification through context capture and rule evaluation.
    pub fn process(&self, notification: Notification) -> ProcessedNotification {
        let config = self.config();
        let context = self.inner.context.snapshot();
        let evaluation = RuleEngine::new(&config).evaluate(&notification, &context);

        let processed = ProcessedNotification {
            notification,
            evaluation,
        };

        let listener = self
            .inner
            .listener
            .read()
            .expect("pipeline listener poisoned")
            .clone();
        if let Some(listener) = listener {
            listener(&processed);
        }

        processed
    }

    /// Start `source`, routing every incoming notification through the
    /// pipeline. The listener registered with [`set_listener`] receives the
    /// outcome.
    pub fn attach(&self, source: &mut dyn NotificationSource) -> Result<()> {
        let pipeline = self.clone();
        source.start(Arc::new(move |notification| {
            pipeline.process(notification);
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::MockContextProvider;
    use crate::model::{AppIdentity, Notification};
    use crate::rules::{Condition, Decision, Rule};
    use crate::source::MockNotificationSource;
    use std::sync::Mutex;

    fn mute_slack_when_fullscreen() -> Config {
        Config {
            rules: vec![
                Rule::new("mute-slack-when-fullscreen", Decision::Mute).with_conditions(vec![
                    Condition::App {
                        apps: vec!["slack".into()],
                    },
                    Condition::Fullscreen { equals: true },
                ]),
            ],
            ..Config::default()
        }
    }

    #[test]
    fn pipeline_applies_rules_with_live_context() {
        let context = Arc::new(MockContextProvider::new());
        let pipeline = Pipeline::from_refs(&mute_slack_when_fullscreen(), context.clone());

        let notification = Notification::new(AppIdentity::new("slack"), "hi", "there");

        assert_eq!(
            pipeline.process(notification.clone()).evaluation.decision,
            Decision::Allow
        );

        context.set_fullscreen(true);
        let processed = pipeline.process(notification);
        assert_eq!(processed.evaluation.decision, Decision::Mute);
        assert!(processed.suppressed());
        assert_eq!(
            processed.evaluation.matched_rule.as_deref(),
            Some("mute-slack-when-fullscreen")
        );
    }

    #[test]
    fn config_can_be_reloaded_at_runtime() {
        let context = Arc::new(MockContextProvider::new());
        let pipeline = Pipeline::from_refs(&Config::default(), context.clone());
        let notification = Notification::new(AppIdentity::new("slack"), "hi", "there");
        assert_eq!(
            pipeline.process(notification.clone()).evaluation.decision,
            Decision::Allow
        );

        context.set_fullscreen(true);
        assert_eq!(
            pipeline.process(notification.clone()).evaluation.decision,
            Decision::Allow
        );

        pipeline.replace_config(mute_slack_when_fullscreen());
        assert_eq!(
            pipeline.process(notification).evaluation.decision,
            Decision::Mute
        );
    }

    #[test]
    fn listener_receives_every_decision() {
        let pipeline =
            Pipeline::from_refs(&Config::default(), Arc::new(MockContextProvider::new()));
        let seen: Arc<Mutex<Vec<Decision>>> = Arc::new(Mutex::new(Vec::new()));

        let sink = seen.clone();
        pipeline.set_listener(Arc::new(move |processed: &ProcessedNotification| {
            sink.lock().unwrap().push(processed.evaluation.decision);
        }));

        pipeline.process(Notification::new(AppIdentity::new("a"), "t", "b"));
        pipeline.process(Notification::new(AppIdentity::new("b"), "t", "b"));

        assert_eq!(
            *seen.lock().unwrap(),
            vec![Decision::Allow, Decision::Allow]
        );
    }

    #[test]
    fn attached_source_routes_through_pipeline() {
        let context = Arc::new(MockContextProvider::new());
        context.set_fullscreen(true);
        let pipeline = Pipeline::from_refs(&mute_slack_when_fullscreen(), context);

        let mut source = MockNotificationSource::new("mock");
        let seen: Arc<Mutex<Vec<Decision>>> = Arc::new(Mutex::new(Vec::new()));

        let sink = seen.clone();
        pipeline.set_listener(Arc::new(move |p: &ProcessedNotification| {
            sink.lock().unwrap().push(p.evaluation.decision);
        }));

        pipeline.attach(&mut source).unwrap();
        assert!(source.is_running());

        source.push(Notification::new(AppIdentity::new("slack"), "hi", "there"));
        source.push(Notification::new(AppIdentity::new("mail"), "hi", "there"));

        assert_eq!(*seen.lock().unwrap(), vec![Decision::Mute, Decision::Allow]);
    }

    #[test]
    fn context_provider_is_exposed() {
        let context = Arc::new(MockContextProvider::new());
        context.set_active_profile(Some("Deep Work"));
        let pipeline = Pipeline::from_refs(&Config::default(), context);

        let snapshot = pipeline.context_provider().snapshot();
        assert_eq!(snapshot.active_profile.as_deref(), Some("Deep Work"));
    }
}
