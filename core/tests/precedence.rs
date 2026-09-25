//! Rule precedence across combined condition types: app × fullscreen ×
//! profile × schedule × urgency in one configuration.

use std::sync::Arc;

use chrono::{NaiveDateTime, NaiveTime, Weekday};

use tpt_focus_core::{
    active_schedules, AppIdentity, Condition, Config, ContextSnapshot, Decision, FocusProfile,
    MatchMode, MockContextProvider, Notification, Pipeline, Rule, Schedule,
};

fn notification(app: &str) -> Notification {
    Notification::new(AppIdentity::new(app), "Title", "Body")
}

fn context_at(day: u32, hour: u32, minute: u32) -> ContextSnapshot {
    ContextSnapshot {
        local_time: chrono::NaiveDate::from_ymd_opt(2026, 9, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap(),
        ..ContextSnapshot::now()
    }
}

/// The kitchen-sink config every test evaluates against.
///
/// Precedence (high priority first):
///   100  critical-urgency allow         — urgency only
///    80  deep-work mute                 — profile only
///    60  fullscreen chat mute           — app × fullscreen
///    40  office-hours batch             — schedule window
///     0  (fallback: profile default → global default)
fn kitchen_sink() -> Config {
    Config {
        defaults: crate_default(),
        profiles: vec![
            FocusProfile::new("Deep Work").with_default_decision(Decision::Mute),
            FocusProfile::new("Meetings"),
        ],
        rules: vec![
            Rule::new("critical-urgency", Decision::Allow)
                .with_priority(100)
                .with_condition(Condition::Urgency {
                    urgencies: vec![tpt_focus_core::Urgency::Critical],
                }),
            Rule::new("deep-work", Decision::Mute)
                .with_priority(80)
                .with_condition(Condition::Profile {
                    profiles: vec!["Deep Work".into()],
                }),
            Rule::new("fullscreen-chat", Decision::Mute)
                .with_priority(60)
                .with_match_mode(MatchMode::All)
                .with_conditions(vec![
                    Condition::App {
                        apps: vec!["slack".into(), "teams".into()],
                    },
                    Condition::Fullscreen { equals: true },
                ]),
            Rule::new("office-hours", Decision::Batch)
                .with_priority(40)
                .with_condition(Condition::TimeOfDay {
                    start: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                    end: NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
                    weekdays: vec![
                        Weekday::Mon,
                        Weekday::Tue,
                        Weekday::Wed,
                        Weekday::Thu,
                        Weekday::Fri,
                    ],
                }),
        ],
        ..Config::default()
    }
}

fn crate_default() -> tpt_focus_core::Defaults {
    tpt_focus_core::Defaults {
        decision: Decision::Allow,
    }
}

#[test]
fn urgency_rule_beats_everything() {
    let config = kitchen_sink();
    let engine = tpt_focus_core::RuleEngine::new(&config);

    let mut context = context_at(26, 12, 0); // Saturday noon, no fullscreen
    context.active_profile = Some("Deep Work".into());
    context.fullscreen = true;

    let critical = notification("slack").with_urgency(tpt_focus_core::Urgency::Critical);
    let evaluation = engine.evaluate(&critical, &context);
    assert_eq!(evaluation.decision, Decision::Allow);
    assert_eq!(evaluation.matched_rule.as_deref(), Some("critical-urgency"));
}

#[test]
fn profile_rule_beats_app_fullscreen_and_schedule() {
    let config = kitchen_sink();
    let engine = tpt_focus_core::RuleEngine::new(&config);

    // Friday office hours (schedule matches) + fullscreen slack (app×fs
    // matches) + Deep Work profile → the profile rule wins on priority.
    let mut context = context_at(25, 10, 0);
    context.fullscreen = true;
    context.active_profile = Some("Deep Work".into());

    let evaluation = engine.evaluate(&notification("slack"), &context);
    assert_eq!(evaluation.decision, Decision::Mute);
    assert_eq!(evaluation.matched_rule.as_deref(), Some("deep-work"));
}

#[test]
fn app_fullscreen_rule_beats_schedule() {
    let config = kitchen_sink();
    let engine = tpt_focus_core::RuleEngine::new(&config);

    // Friday office hours + fullscreen slack: app×fullscreen (60) beats
    // schedule (40). Both are Mute vs Batch — the *decision* differs, so the
    // winner is observable.
    let mut context = context_at(25, 10, 0);
    context.fullscreen = true;

    let evaluation = engine.evaluate(&notification("slack"), &context);
    assert_eq!(evaluation.decision, Decision::Mute);
    assert_eq!(evaluation.matched_rule.as_deref(), Some("fullscreen-chat"));

    // Same window, not fullscreen: schedule wins.
    let mut context = context_at(25, 10, 0);
    context.fullscreen = false;
    let evaluation = engine.evaluate(&notification("slack"), &context);
    assert_eq!(evaluation.decision, Decision::Batch);
    assert_eq!(evaluation.matched_rule.as_deref(), Some("office-hours"));
}

#[test]
fn app_rule_requires_fullscreen_so_other_apps_fall_through() {
    let config = kitchen_sink();
    let engine = tpt_focus_core::RuleEngine::new(&config);

    let mut context = context_at(25, 10, 0);
    context.fullscreen = true;

    // Fullscreen but not a chat app → office-hours batch applies.
    let evaluation = engine.evaluate(&notification("chrome"), &context);
    assert_eq!(evaluation.matched_rule.as_deref(), Some("office-hours"));

    // Chat app but office hours over (Saturday) → global default.
    let weekend = context_at(26, 10, 0);
    let evaluation = engine.evaluate(&notification("slack"), &weekend);
    assert_eq!(evaluation.decision, Decision::Allow);
    assert_eq!(evaluation.matched_rule, None);
}

#[test]
fn fallback_chain_is_profile_default_then_global_default() {
    let mut config = kitchen_sink();
    // No rules at all: profile default, then global default.
    config.rules.clear();

    let engine = tpt_focus_core::RuleEngine::new(&config);

    // No profile → global default (allow).
    let evaluation = engine.evaluate(&notification("x"), &ContextSnapshot::now());
    assert_eq!(evaluation.decision, Decision::Allow);

    // Meetings profile has no default_decision → global default still.
    let mut context = ContextSnapshot::now();
    context.active_profile = Some("Meetings".into());
    let evaluation = engine.evaluate(&notification("x"), &context);
    assert_eq!(evaluation.decision, Decision::Allow);

    // Deep Work profile defaults to mute.
    let mut context = ContextSnapshot::now();
    context.active_profile = Some("Deep Work".into());
    let evaluation = engine.evaluate(&notification("x"), &context);
    assert_eq!(evaluation.decision, Decision::Mute);
    assert!(evaluation.reason.contains("Deep Work"));
}

#[test]
fn disabled_high_priority_rule_lets_lower_priority_win() {
    let config = Config {
        defaults: crate_default(),
        profiles: Vec::new(),
        rules: vec![
            Rule::new("would-win", Decision::Mute)
                .with_priority(100)
                .disabled(),
            Rule::new("runner-up", Decision::Batch)
                .with_priority(10)
                .with_condition(Condition::App {
                    apps: vec!["slack".into()],
                }),
        ],
        ..Config::default()
    };

    let engine = tpt_focus_core::RuleEngine::new(&config);
    let evaluation = engine.evaluate(&notification("slack"), &ContextSnapshot::now());
    assert_eq!(evaluation.matched_rule.as_deref(), Some("runner-up"));
}

#[test]
fn schedule_activation_drives_the_profile_condition() {
    // The schedule runtime activates "Deep Work" 22:00–06:00 daily; the
    // profile rule then mutes everything except critical alerts.
    let config = Config {
        defaults: crate_default(),
        profiles: vec![FocusProfile::new("Deep Work").with_default_decision(Decision::Mute)],
        rules: vec![
            Rule::new("critical-urgency", Decision::Allow)
                .with_priority(100)
                .with_condition(Condition::Urgency {
                    urgencies: vec![tpt_focus_core::Urgency::Critical],
                }),
            Rule::new("deep-work", Decision::Mute)
                .with_priority(10)
                .with_condition(Condition::Profile {
                    profiles: vec!["Deep Work".into()],
                }),
        ],
        ..Config::default()
    };

    let schedules = vec![Schedule::new(
        "night",
        NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
        NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
    )
    .named("Night focus")
    .activating_profile("Deep Work")];

    let context = Arc::new(MockContextProvider::new());
    let pipeline = Pipeline::from_refs(&config, context.clone());

    let evaluate = |local: NaiveDateTime, urgency: tpt_focus_core::Urgency| {
        let active = active_schedules(&schedules, local);
        context.set_active_profile(active.first().and_then(|s| s.profile.clone()).as_deref());
        context.set_fullscreen(false);
        pipeline
            .process(
                notification("slack")
                    .with_urgency(urgency)
                    .with_timestamp(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            )
            .evaluation
            .decision
    };

    // 23:30 → schedule live → profile active → muted.
    let night = chrono::NaiveDate::from_ymd_opt(2026, 9, 25)
        .unwrap()
        .and_hms_opt(23, 30, 0)
        .unwrap();
    assert_eq!(
        evaluate(night, tpt_focus_core::Urgency::Normal),
        Decision::Mute
    );
    // …but critical notifications still punch through.
    assert_eq!(
        evaluate(night, tpt_focus_core::Urgency::Critical),
        Decision::Allow
    );

    // 12:00 → no schedule → profile inactive → global default (allow).
    let noon = chrono::NaiveDate::from_ymd_opt(2026, 9, 25)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap();
    assert_eq!(
        evaluate(noon, tpt_focus_core::Urgency::Normal),
        Decision::Allow
    );
}
