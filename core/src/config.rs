//! TOML configuration: rules, focus profiles and defaults.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::profile::{validate_profiles, FocusProfile};
use crate::rules::{Condition, Decision, Rule};

/// Current configuration schema version written by this build.
pub const CONFIG_VERSION: u32 = 1;

fn current_version() -> u32 {
    CONFIG_VERSION
}

/// Fallback behaviour when no rule matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default)]
    pub decision: Decision,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            decision: Decision::Allow,
        }
    }
}

/// On-disk configuration document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Schema version; bumped when the format changes incompatibly.
    #[serde(default = "current_version")]
    pub version: u32,

    #[serde(default)]
    pub defaults: Defaults,

    #[serde(default)]
    pub profiles: Vec<FocusProfile>,

    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            defaults: Defaults::default(),
            profiles: Vec::new(),
            rules: Vec::new(),
        }
    }
}

impl Config {
    /// First-run configuration with a couple of sensible starter rules.
    pub fn starter() -> Self {
        Self {
            profiles: vec![
                FocusProfile::new("Deep Work")
                    .with_description("Notifications muted unless critical")
                    .with_default_decision(Decision::Mute),
                FocusProfile::new("Meetings").with_description("Everything batched into a digest"),
            ],
            rules: vec![
                Rule::new("critical-always", Decision::Allow)
                    .named("Critical alerts always show")
                    .with_priority(100)
                    .with_condition(Condition::Urgency {
                        urgencies: vec![crate::model::Urgency::Critical],
                    }),
                Rule::new("mute-chat-when-fullscreen", Decision::Mute)
                    .named("Mute chat apps while a window is fullscreen")
                    .with_priority(10)
                    .with_conditions(vec![
                        Condition::App {
                            apps: vec![
                                "slack".into(),
                                "discord".into(),
                                "teams".into(),
                                "telegram".into(),
                            ],
                        },
                        Condition::Fullscreen { equals: true },
                    ]),
            ],
            ..Self::default()
        }
    }

    /// Parse and validate a configuration document.
    pub fn from_toml(raw: &str) -> Result<Self> {
        let config: Config = toml::from_str(raw)?;
        config.validate()?;
        Ok(config)
    }

    /// Serialize to a TOML document (not validated — call [`validate`] first
    /// if the value may be hand-built).
    ///
    /// [`validate`]: Config::validate
    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Read + validate a configuration file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path).map_err(|source| Error::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml(&raw)
    }

    /// Validate, then write the configuration file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let path = path.as_ref();
        let raw = self.to_toml()?;
        std::fs::write(path, raw).map_err(|source| Error::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Structural validation of the whole document.
    pub fn validate(&self) -> Result<()> {
        if self.version == 0 || self.version > CONFIG_VERSION {
            return Err(Error::validation(format!(
                "unsupported config version {} (supported: 1..={})",
                self.version, CONFIG_VERSION
            )));
        }

        validate_profiles(&self.profiles)?;
        self.validate_rules()?;
        Ok(())
    }

    fn validate_rules(&self) -> Result<()> {
        let mut seen_ids: Vec<&str> = Vec::new();

        for rule in &self.rules {
            let id = rule.id.trim();
            if id.is_empty() {
                return Err(Error::validation("rule id must not be empty"));
            }
            if seen_ids
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(id))
            {
                return Err(Error::validation(format!("duplicate rule id `{id}`")));
            }
            seen_ids.push(id);

            if rule.name.trim().is_empty() {
                return Err(Error::validation(format!("rule `{id}` has an empty name")));
            }

            for condition in &rule.conditions {
                self.validate_condition(id, condition)?;
            }
        }

        Ok(())
    }

    fn validate_condition(&self, rule_id: &str, condition: &Condition) -> Result<()> {
        match condition {
            Condition::App { apps } if apps.is_empty() => Err(Error::validation(format!(
                "rule `{rule_id}`: app condition must list at least one application"
            ))),
            Condition::Profile { profiles } => {
                if profiles.is_empty() {
                    return Err(Error::validation(format!(
                        "rule `{rule_id}`: profile condition must list at least one profile"
                    )));
                }
                for name in profiles {
                    if crate::profile::find_profile(&self.profiles, name).is_none() {
                        return Err(Error::validation(format!(
                            "rule `{rule_id}` references unknown focus profile `{name}`"
                        )));
                    }
                }
                Ok(())
            }
            Condition::TimeOfDay { start, end, .. } => {
                if start == end {
                    return Err(Error::validation(format!(
                        "rule `{rule_id}`: time-of-day range must have a distinct start and end"
                    )));
                }
                Ok(())
            }
            Condition::Urgency { urgencies } if urgencies.is_empty() => Err(Error::validation(
                format!("rule `{rule_id}`: urgency condition must list at least one level"),
            )),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Urgency;
    use crate::rules::MatchMode;
    use chrono::{NaiveTime, Weekday};

    fn sample_config() -> Config {
        Config {
            profiles: vec![
                FocusProfile::new("Deep Work").with_default_decision(Decision::Mute),
                FocusProfile::new("Meetings"),
            ],
            rules: vec![
                Rule::new("quiet", Decision::Batch)
                    .named("Quiet hours")
                    .with_priority(50)
                    .with_match_mode(MatchMode::All)
                    .with_conditions(vec![
                        Condition::TimeOfDay {
                            start: NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
                            end: NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
                            weekdays: vec![Weekday::Sat, Weekday::Sun],
                        },
                        Condition::Profile {
                            profiles: vec!["Deep Work".into()],
                        },
                    ]),
                Rule::new("critical", Decision::Allow)
                    .named("Critical always allowed")
                    .with_priority(100)
                    .with_condition(Condition::Urgency {
                        urgencies: vec![Urgency::Critical],
                    }),
            ],
            ..Config::default()
        }
    }

    #[test]
    fn config_round_trips_through_toml() {
        let config = sample_config();
        let raw = config.to_toml().unwrap();
        let back = Config::from_toml(&raw).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn config_round_trips_through_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("focus.toml");

        let config = sample_config();
        config.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let err = Config::from_toml("version = 1\nbogus = true\n").unwrap_err();
        assert!(err.to_string().contains("bogus") || err.to_string().contains("parse"));
    }

    #[test]
    fn duplicate_rule_ids_are_rejected() {
        let raw = r#"
            [[rules]]
            id = "a"
            name = "A"
            action = "mute"

            [[rules]]
            id = "A"
            name = "B"
            action = "batch"
        "#;
        let err = Config::from_toml(raw).unwrap_err();
        assert!(err.to_string().contains("duplicate rule id"));
    }

    #[test]
    fn unknown_profile_reference_is_rejected() {
        let raw = r#"
            [[rules]]
            id = "a"
            name = "A"
            action = "mute"

            [[rules.conditions]]
            type = "profile"
            profiles = ["Missing"]
        "#;
        let err = Config::from_toml(raw).unwrap_err();
        assert!(err.to_string().contains("unknown focus profile"));
    }

    #[test]
    fn empty_app_list_is_rejected() {
        let raw = r#"
            [[rules]]
            id = "a"
            name = "A"
            action = "mute"

            [[rules.conditions]]
            type = "app"
            apps = []
        "#;
        let err = Config::from_toml(raw).unwrap_err();
        assert!(err.to_string().contains("at least one application"));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let err = Config::from_toml("version = 99\n").unwrap_err();
        assert!(err.to_string().contains("unsupported config version"));
    }

    #[test]
    fn starter_config_is_valid() {
        let starter = Config::starter();
        starter.validate().unwrap();
        let raw = starter.to_toml().unwrap();
        let back = Config::from_toml(&raw).unwrap();
        assert_eq!(starter, back);
    }

    #[test]
    fn missing_file_reports_path() {
        let err = Config::load("definitely-not-here.toml").unwrap_err();
        assert!(matches!(err, Error::ConfigRead { .. }));
    }
}
