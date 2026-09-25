//! End-to-end pipeline tests using mocked platform boundaries.

use std::sync::{Arc, Mutex};

use tpt_focus_core::{
    AppIdentity, Condition, Config, Decision, MockContextProvider, MockNotificationSource,
    Notification, NotificationSource, Pipeline, Rule,
};

fn mute_slack_when_ide_fullscreen() -> Config {
    Config {
        rules: vec![
            Rule::new("mute-slack-when-ide-fullscreen", Decision::Mute)
                .named("Mute Slack when the IDE is fullscreen")
                .with_priority(10)
                .with_conditions(vec![
                    Condition::App {
                        apps: vec!["slack".into()],
                    },
                    Condition::Fullscreen { equals: true },
                ]),
            Rule::new("mute-mail", Decision::Mute)
                .with_priority(1)
                .with_condition(Condition::App {
                    apps: vec!["mail".into()],
                }),
        ],
        ..Config::default()
    }
}

#[test]
fn mute_slack_when_ide_is_fullscreen() {
    let context = Arc::new(MockContextProvider::new());
    let pipeline = Pipeline::from_refs(&mute_slack_when_ide_fullscreen(), context.clone());

    let mut source = MockNotificationSource::new("mock");
    let decisions: Arc<Mutex<Vec<(String, Decision)>>> = Arc::new(Mutex::new(Vec::new()));

    let sink = decisions.clone();
    pipeline.set_listener(Arc::new(move |processed| {
        sink.lock().unwrap().push((
            processed.notification.source.app_id.clone(),
            processed.evaluation.decision,
        ));
    }));
    pipeline.attach(&mut source).unwrap();

    context.set_foreground_app(Some(AppIdentity::new("code")));
    context.set_fullscreen(false);
    source.push(Notification::new(
        AppIdentity::new("slack"),
        "Ping",
        "hello",
    ));

    context.set_fullscreen(true);
    source.push(Notification::new(
        AppIdentity::new("slack"),
        "Ping",
        "hello",
    ));
    source.push(Notification::new(
        AppIdentity::new("mail"),
        "News",
        "digest",
    ));

    let decisions = decisions.lock().unwrap().clone();
    assert_eq!(
        decisions,
        vec![
            ("slack".to_string(), Decision::Allow),
            ("slack".to_string(), Decision::Mute),
            ("mail".to_string(), Decision::Mute),
        ]
    );
}

#[test]
fn dismissed_notifications_reach_the_source() {
    let context = Arc::new(MockContextProvider::new());
    let pipeline = Pipeline::from_refs(&Config::default(), context);

    let mut source = MockNotificationSource::new("mock");
    pipeline.attach(&mut source).unwrap();

    let notification = Notification::new(AppIdentity::new("mail"), "Subject", "Body");
    source.push(notification.clone());
    source.dismiss(&notification.id).unwrap();

    assert_eq!(source.dismissed(), vec![notification.id]);
    assert_eq!(source.delivered().len(), 1);
}

#[test]
fn mute_decision_does_not_drop_history_record() {
    let context = Arc::new(MockContextProvider::new());
    let config = Config {
        rules: vec![
            Rule::new("mute-mail", Decision::Mute).with_condition(Condition::App {
                apps: vec!["mail".into()],
            }),
        ],
        ..Config::default()
    };

    let pipeline = Pipeline::from_refs(&config, context);
    let processed = pipeline.process(Notification::new(AppIdentity::new("mail"), "s", "b"));

    assert!(processed.suppressed());
    assert_eq!(processed.notification.source.app_id, "mail");
    assert_eq!(
        processed.evaluation.matched_rule.as_deref(),
        Some("mute-mail")
    );
}
