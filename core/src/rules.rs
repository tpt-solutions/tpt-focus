//! Rule model and evaluation engine.

use chrono::{Datelike, NaiveTime, Weekday};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::context::ContextSnapshot;
use crate::model::{Notification, Urgency};
use crate::schedule::{deserialize_weekdays, serialize_weekdays, time_in_range, weekday_selected};

/// What the engine does with a notification once a rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Show it to the user immediately.
    #[default]
    Allow,
    /// Drop it before it reaches the screen; it is still recorded in history.
    Mute,
    /// Hold it and fold it into a per-app digest instead of a popup.
    Batch,
}

impl Decision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Mute => "mute",
            Decision::Batch => "batch",
        }
    }
}

/// How the conditions of a rule combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Every condition must hold (logical AND).
    #[default]
    All,
    /// At least one condition must hold (logical OR).
    Any,
}

/// A single condition evaluated against a notification and its context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    /// Source application identity. Patterns support `*` and `?` and are
    /// matched case-insensitively against `app_id` and `display_name`.
    App { apps: Vec<String> },

    /// Whether the foreground window is fullscreen.
    Fullscreen { equals: bool },

    /// Active focus profile by name.
    Profile { profiles: Vec<String> },

    /// Wall-clock time window. An empty `weekdays` list means every day and a
    /// range that wraps (start > end) crosses midnight.
    TimeOfDay {
        start: NaiveTime,
        end: NaiveTime,
        #[serde(
            default,
            serialize_with = "serialize_weekdays",
            deserialize_with = "deserialize_weekdays"
        )]
        weekdays: Vec<Weekday>,
    },

    /// Notification urgency level(s).
    Urgency { urgencies: Vec<Urgency> },
}

impl Condition {
    /// Evaluate this condition against a notification and its context.
    pub fn matches(&self, notification: &Notification, context: &ContextSnapshot) -> bool {
        match self {
            Condition::App { apps } => apps.iter().any(|pattern| {
                crate::model::pattern_matches(pattern, &notification.source.app_id)
                    || notification
                        .source
                        .display_name
                        .as_deref()
                        .is_some_and(|name| crate::model::pattern_matches(pattern, name))
            }),
            Condition::Fullscreen { equals } => context.fullscreen == *equals,
            Condition::Profile { profiles } => {
                context.active_profile.as_deref().is_some_and(|active| {
                    profiles
                        .iter()
                        .any(|p| crate::model::pattern_matches(p, active))
                })
            }
            Condition::TimeOfDay {
                start,
                end,
                weekdays,
            } => {
                let local = context.local_time;
                weekday_selected(weekdays, local.weekday())
                    && time_in_range(*start, *end, local.time())
            }
            Condition::Urgency { urgencies } => urgencies.contains(&notification.urgency),
        }
    }
}

/// A user-defined rule: conditions under which `action` is applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Unique machine-readable id.
    pub id: String,

    /// Human-readable label shown in the UI/CLI.
    pub name: String,

    /// Disabled rules are skipped entirely.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Higher priority rules are evaluated first. Ties fall back to file
    /// order, so the first listed rule wins.
    #[serde(default)]
    pub priority: i32,

    /// How `conditions` combine.
    #[serde(default)]
    pub match_mode: MatchMode,

    /// Conditions to evaluate. An empty list matches every notification
    /// (a catch-all rule).
    #[serde(default)]
    pub conditions: Vec<Condition>,

    /// What happens on a match.
    pub action: Decision,
}

fn default_enabled() -> bool {
    true
}

impl Rule {
    pub fn new(id: impl Into<String>, action: Decision) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            enabled: true,
            priority: 0,
            match_mode: MatchMode::default(),
            conditions: Vec::new(),
            action,
        }
    }

    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_match_mode(mut self, mode: MatchMode) -> Self {
        self.match_mode = mode;
        self
    }

    pub fn with_conditions(mut self, conditions: Vec<Condition>) -> Self {
        self.conditions = conditions;
        self
    }

    pub fn with_condition(mut self, condition: Condition) -> Self {
        self.conditions.push(condition);
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Evaluate this rule. An empty condition list always matches.
    pub fn matches(&self, notification: &Notification, context: &ContextSnapshot) -> bool {
        if self.conditions.is_empty() {
            return true;
        }
        match self.match_mode {
            MatchMode::All => self
                .conditions
                .iter()
                .all(|c| c.matches(notification, context)),
            MatchMode::Any => self
                .conditions
                .iter()
                .any(|c| c.matches(notification, context)),
        }
    }
}

/// Result of running the rule pipeline over one notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleEvaluation {
    /// The decision to apply.
    pub decision: Decision,

    /// Id of the winning rule, `None` when the fallback was used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,

    /// Human-readable explanation, surfaced in logs and the history UI.
    pub reason: String,
}

impl RuleEvaluation {
    /// True when the notification should not reach the screen immediately.
    pub fn suppressed(&self) -> bool {
        self.decision != Decision::Allow
    }
}

/// Evaluates notifications against a configuration.
///
/// Precedence: enabled rules by descending `priority` (file order breaks
/// ties) → active profile default → global default.
pub struct RuleEngine<'a> {
    config: &'a Config,
}

impl<'a> RuleEngine<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }

    /// Enabled rules in evaluation order.
    pub fn ordered_rules(&self) -> Vec<&'a Rule> {
        let mut rules: Vec<(usize, &Rule)> = self
            .config
            .rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.enabled)
            .collect();
        rules.sort_by(|a, b| b.1.priority.cmp(&a.1.priority).then(a.0.cmp(&b.0)));
        rules.into_iter().map(|(_, rule)| rule).collect()
    }

    /// Decide what to do with `notification`.
    pub fn evaluate(
        &self,
        notification: &Notification,
        context: &ContextSnapshot,
    ) -> RuleEvaluation {
        for rule in self.ordered_rules() {
            if rule.matches(notification, context) {
                return RuleEvaluation {
                    decision: rule.action,
                    matched_rule: Some(rule.id.clone()),
                    reason: format!("matched rule `{}`", rule.name),
                };
            }
        }

        if let Some(profile_name) = context.active_profile.as_deref() {
            if let Some(profile) = crate::profile::find_profile(&self.config.profiles, profile_name)
            {
                if let Some(decision) = profile.default_decision {
                    return RuleEvaluation {
                        decision,
                        matched_rule: None,
                        reason: format!(
                            "no rule matched; profile `{}` default applied",
                            profile.name
                        ),
                    };
                }
            }
        }

        RuleEvaluation {
            decision: self.config.defaults.decision,
            matched_rule: None,
            reason: "no rule matched; global default applied".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Defaults};
    use crate::model::AppIdentity;
    use chrono::NaiveDateTime;

    fn notification(app: &str) -> Notification {
        Notification::new(AppIdentity::new(app), "Title", "Body")
    }

    fn context_at(hour: u32, minute: u32) -> ContextSnapshot {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
        let time = NaiveTime::from_hms_opt(hour, minute, 0).unwrap();
        ContextSnapshot {
            local_time: NaiveDateTime::new(date, time),
            ..ContextSnapshot::now()
        }
    }

    fn config(rules: Vec<Rule>) -> Config {
        Config {
            rules,
            ..Config::default()
        }
    }

    #[test]
    fn higher_priority_rule_wins() {
        let config = config(vec![
            Rule::new("mute-all", Decision::Mute).with_priority(1),
            Rule::new("allow-slack", Decision::Allow)
                .with_priority(10)
                .with_condition(Condition::App {
                    apps: vec!["slack".into()],
                }),
        ]);
        let engine = RuleEngine::new(&config);

        let eval = engine.evaluate(&notification("slack"), &ContextSnapshot::now());
        assert_eq!(eval.decision, Decision::Allow);
        assert_eq!(eval.matched_rule.as_deref(), Some("allow-slack"));

        let eval = engine.evaluate(&notification("mail"), &ContextSnapshot::now());
        assert_eq!(eval.decision, Decision::Mute);
        assert_eq!(eval.matched_rule.as_deref(), Some("mute-all"));
    }

    #[test]
    fn equal_priority_falls_back_to_file_order() {
        let config = config(vec![
            Rule::new("first", Decision::Batch),
            Rule::new("second", Decision::Mute),
        ]);
        let engine = RuleEngine::new(&config);
        let eval = engine.evaluate(&notification("any"), &ContextSnapshot::now());
        assert_eq!(eval.matched_rule.as_deref(), Some("first"));
    }

    #[test]
    fn disabled_rules_are_skipped() {
        let config = config(vec![
            Rule::new("off", Decision::Mute).disabled(),
            Rule::new("on", Decision::Batch),
        ]);
        let engine = RuleEngine::new(&config);
        let eval = engine.evaluate(&notification("any"), &ContextSnapshot::now());
        assert_eq!(eval.matched_rule.as_deref(), Some("on"));
    }

    #[test]
    fn fullscreen_condition_matches_context() {
        let config = config(vec![Rule::new("mute-when-fullscreen", Decision::Mute)
            .with_condition(Condition::Fullscreen { equals: true })]);
        let engine = RuleEngine::new(&config);

        let mut ctx = ContextSnapshot::now();
        ctx.fullscreen = true;
        assert_eq!(
            engine.evaluate(&notification("slack"), &ctx).decision,
            Decision::Mute
        );

        ctx.fullscreen = false;
        assert_eq!(
            engine.evaluate(&notification("slack"), &ctx).decision,
            Decision::Allow
        );
    }

    #[test]
    fn profile_condition_matches_active_profile() {
        let config = config(vec![Rule::new("deep-work", Decision::Mute).with_condition(
            Condition::Profile {
                profiles: vec!["deep work".into()],
            },
        )]);
        let engine = RuleEngine::new(&config);

        let mut ctx = ContextSnapshot::now();
        ctx.active_profile = Some("Deep Work".into());
        assert_eq!(
            engine.evaluate(&notification("x"), &ctx).decision,
            Decision::Mute
        );

        ctx.active_profile = None;
        assert_eq!(
            engine.evaluate(&notification("x"), &ctx).decision,
            Decision::Allow
        );
    }

    #[test]
    fn time_of_day_condition_uses_local_wall_clock() {
        let config = config(vec![Rule::new("quiet-hours", Decision::Batch)
            .with_condition(Condition::TimeOfDay {
                start: NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
                end: NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
                weekdays: Vec::new(),
            })]);
        let engine = RuleEngine::new(&config);

        assert_eq!(
            engine
                .evaluate(&notification("x"), &context_at(23, 30))
                .decision,
            Decision::Batch
        );
        assert_eq!(
            engine
                .evaluate(&notification("x"), &context_at(3, 0))
                .decision,
            Decision::Batch
        );
        assert_eq!(
            engine
                .evaluate(&notification("x"), &context_at(12, 0))
                .decision,
            Decision::Allow
        );
    }

    #[test]
    fn time_of_day_respects_weekdays() {
        let config = config(vec![Rule::new("weekdays-only", Decision::Mute)
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
            })]);
        let engine = RuleEngine::new(&config);

        // 2026-09-25 is a Friday.
        assert_eq!(
            engine
                .evaluate(&notification("x"), &context_at(10, 0))
                .decision,
            Decision::Mute
        );
        // 2026-09-26 is a Saturday.
        let sat = ContextSnapshot {
            local_time: chrono::NaiveDate::from_ymd_opt(2026, 9, 26)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap(),
            ..ContextSnapshot::now()
        };
        assert_eq!(
            engine.evaluate(&notification("x"), &sat).decision,
            Decision::Allow
        );
    }

    #[test]
    fn match_mode_any_requires_only_one_condition() {
        let config = config(vec![Rule::new("either", Decision::Mute)
            .with_match_mode(MatchMode::Any)
            .with_conditions(vec![
                Condition::App {
                    apps: vec!["slack".into()],
                },
                Condition::Fullscreen { equals: true },
            ])]);
        let engine = RuleEngine::new(&config);

        let mut ctx = ContextSnapshot::now();
        ctx.fullscreen = false;
        assert_eq!(
            engine.evaluate(&notification("slack"), &ctx).decision,
            Decision::Mute
        );

        ctx.fullscreen = true;
        assert_eq!(
            engine.evaluate(&notification("mail"), &ctx).decision,
            Decision::Mute
        );

        ctx.fullscreen = false;
        assert_eq!(
            engine.evaluate(&notification("mail"), &ctx).decision,
            Decision::Allow
        );
    }

    #[test]
    fn all_mode_requires_every_condition() {
        let config = config(vec![Rule::new("slack-fullscreen", Decision::Mute)
            .with_conditions(vec![
                Condition::App {
                    apps: vec!["slack".into()],
                },
                Condition::Fullscreen { equals: true },
            ])]);
        let engine = RuleEngine::new(&config);

        let mut ctx = ContextSnapshot::now();
        ctx.fullscreen = true;
        assert_eq!(
            engine.evaluate(&notification("slack"), &ctx).decision,
            Decision::Mute
        );

        ctx.fullscreen = false;
        assert_eq!(
            engine.evaluate(&notification("slack"), &ctx).decision,
            Decision::Allow
        );
    }

    #[test]
    fn fallback_uses_profile_default_then_global_default() {
        let mut config = config(Vec::new());
        config.defaults = Defaults {
            decision: Decision::Batch,
        };
        config.profiles.push(
            crate::profile::FocusProfile::new("Deep Work").with_default_decision(Decision::Mute),
        );

        let engine = RuleEngine::new(&config);

        let mut ctx = ContextSnapshot::now();
        assert_eq!(
            engine.evaluate(&notification("x"), &ctx).decision,
            Decision::Batch
        );

        ctx.active_profile = Some("Deep Work".into());
        let eval = engine.evaluate(&notification("x"), &ctx);
        assert_eq!(eval.decision, Decision::Mute);
        assert!(eval.reason.contains("Deep Work"));
    }

    #[test]
    fn empty_condition_list_is_catch_all() {
        let config = config(vec![Rule::new("catch-all", Decision::Mute)]);
        let engine = RuleEngine::new(&config);
        let eval = engine.evaluate(&notification("anything"), &ContextSnapshot::now());
        assert_eq!(eval.decision, Decision::Mute);
    }

    #[test]
    fn urgency_condition_matches_levels() {
        let config = Config {
            defaults: Defaults {
                decision: Decision::Mute,
            },
            rules: vec![Rule::new("critical", Decision::Allow).with_condition(
                Condition::Urgency {
                    urgencies: vec![Urgency::Critical],
                },
            )],
            ..Config::default()
        };
        let engine = RuleEngine::new(&config);

        let critical = notification("x").with_urgency(Urgency::Critical);
        assert_eq!(
            engine.evaluate(&critical, &ContextSnapshot::now()).decision,
            Decision::Allow
        );

        let low = notification("x").with_urgency(Urgency::Low);
        assert_eq!(
            engine.evaluate(&low, &ContextSnapshot::now()).decision,
            Decision::Mute
        );
    }

    #[test]
    fn suppressed_flags_non_allow_decisions() {
        assert!(!RuleEvaluation {
            decision: Decision::Allow,
            matched_rule: None,
            reason: String::new(),
        }
        .suppressed());
        assert!(RuleEvaluation {
            decision: Decision::Mute,
            matched_rule: None,
            reason: String::new(),
        }
        .suppressed());
        assert!(RuleEvaluation {
            decision: Decision::Batch,
            matched_rule: None,
            reason: String::new(),
        }
        .suppressed());
    }
}
