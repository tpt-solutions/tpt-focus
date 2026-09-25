//! `tpt-focus rules` — manage notification rules in the TOML configuration.

use anyhow::{Context as _, Result};
use clap::{Subcommand, ValueEnum};

use crate::paths;
use crate::print;
use crate::Ctx;

use tpt_focus_core::{Condition, Decision, MatchMode, Rule};

#[derive(Subcommand)]
pub enum RulesArgs {
    /// List rules in evaluation order.
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Add a new rule.
    Add {
        /// Unique rule id.
        id: String,
        /// Human-readable name (defaults to the id).
        #[arg(short, long)]
        name: Option<String>,

        /// Decision applied on match.
        #[arg(short, long, value_enum)]
        action: ActionArg,

        /// Rule priority (higher wins; default 0).
        #[arg(short, long, default_value_t = 0)]
        priority: i32,

        /// How the conditions combine.
        #[arg(short, long, value_enum, default_value_t = ModeArg::All)]
        mode: ModeArg,

        /// Source app pattern (`*`/`?` wildcards, case-insensitive).
        #[arg(long = "app")]
        apps: Vec<String>,

        /// Match only while this focus profile is active.
        #[arg(long = "profile")]
        profiles: Vec<String>,

        /// Match only these urgency levels.
        #[arg(short, long = "urgency")]
        urgencies: Vec<UrgencyArg>,

        /// Require the desktop to be fullscreen / not fullscreen.
        #[arg(long)]
        fullscreen: bool,
        #[arg(long, conflicts_with = "fullscreen")]
        no_fullscreen: bool,

        /// Wall-clock window, `HH:MM-HH:MM` (may cross midnight).
        #[arg(long, value_name = "START-END")]
        time: Option<String>,

        /// Weekdays for `--time` (mon tue wed thu fri sat sun; default: all).
        #[arg(short, long = "day")]
        weekdays: Vec<String>,
    },
    /// Remove a rule by id.
    Remove { id: String },
    /// Re-enable a rule.
    Enable { id: String },
    /// Disable a rule without deleting it.
    Disable { id: String },
}

#[derive(Copy, Clone, ValueEnum)]
pub enum ActionArg {
    Allow,
    Mute,
    Batch,
}

impl From<ActionArg> for Decision {
    fn from(value: ActionArg) -> Self {
        match value {
            ActionArg::Allow => Decision::Allow,
            ActionArg::Mute => Decision::Mute,
            ActionArg::Batch => Decision::Batch,
        }
    }
}

#[derive(Copy, Clone, ValueEnum)]
pub enum ModeArg {
    All,
    Any,
}

impl From<ModeArg> for MatchMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::All => MatchMode::All,
            ModeArg::Any => MatchMode::Any,
        }
    }
}

#[derive(Copy, Clone, ValueEnum)]
pub enum UrgencyArg {
    Low,
    Normal,
    Critical,
}

impl From<UrgencyArg> for tpt_focus_core::Urgency {
    fn from(value: UrgencyArg) -> Self {
        match value {
            UrgencyArg::Low => tpt_focus_core::Urgency::Low,
            UrgencyArg::Normal => tpt_focus_core::Urgency::Normal,
            UrgencyArg::Critical => tpt_focus_core::Urgency::Critical,
        }
    }
}

pub fn run(ctx: &Ctx, args: RulesArgs) -> Result<()> {
    let path = paths::config_path(ctx.config.as_deref());
    let mut config = load_or_default(&path)?;

    match args {
        RulesArgs::List { json } => {
            config.validate()?;
            if json {
                print::print_json(&config.rules)?;
            } else if config.rules.is_empty() {
                println!("no rules configured ({})", path.display());
            } else {
                println!(
                    "{:<28} {:<6} {:>3}  {:<45}  NAME",
                    "ID", "ACTION", "PRI", "CONDITIONS"
                );
                for rule in &config.rules {
                    print::print_rule(rule);
                }
            }
        }

        RulesArgs::Add {
            id,
            name,
            action,
            priority,
            mode,
            apps,
            profiles,
            urgencies,
            fullscreen,
            no_fullscreen,
            time,
            weekdays,
        } => {
            if config
                .rules
                .iter()
                .any(|rule| rule.id.eq_ignore_ascii_case(&id))
            {
                anyhow::bail!("rule `{id}` already exists");
            }

            let mut conditions = Vec::new();
            if !apps.is_empty() {
                conditions.push(Condition::App { apps });
            }
            if !profiles.is_empty() {
                conditions.push(Condition::Profile { profiles });
            }
            if !urgencies.is_empty() {
                conditions.push(Condition::Urgency {
                    urgencies: urgencies.into_iter().map(Into::into).collect(),
                });
            }
            if fullscreen {
                conditions.push(Condition::Fullscreen { equals: true });
            } else if no_fullscreen {
                conditions.push(Condition::Fullscreen { equals: false });
            }
            if let Some(window) = time {
                conditions.push(parse_time_window(&window, &weekdays)?);
            }

            let rule = Rule::new(&id, action.into())
                .named(name.unwrap_or_else(|| id.clone()))
                .with_priority(priority)
                .with_match_mode(mode.into())
                .with_conditions(conditions);
            config.rules.push(rule);

            paths::ensure_parent(&path)?;
            paths::ensure_parent(&path)?;
            config.save(&path).context("failed to save config")?;
            println!("rule `{id}` added");
        }

        RulesArgs::Remove { id } => {
            let before = config.rules.len();
            config
                .rules
                .retain(|rule| !rule.id.eq_ignore_ascii_case(&id));
            if config.rules.len() == before {
                anyhow::bail!("no rule with id `{id}`");
            }
            paths::ensure_parent(&path)?;
            paths::ensure_parent(&path)?;
            config.save(&path).context("failed to save config")?;
            println!("rule `{id}` removed");
        }

        RulesArgs::Enable { id } => toggle(&mut config, &path, &id, true)?,
        RulesArgs::Disable { id } => toggle(&mut config, &path, &id, false)?,
    }
    Ok(())
}

fn toggle(
    config: &mut tpt_focus_core::Config,
    path: &std::path::Path,
    id: &str,
    on: bool,
) -> Result<()> {
    let rule = config
        .rules
        .iter_mut()
        .find(|rule| rule.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| anyhow::anyhow!("no rule with id `{id}`"))?;

    if rule.enabled == on {
        println!("rule `{id}` is already {}", enabled_word(on));
        return Ok(());
    }
    rule.enabled = on;
    config.save(path).context("failed to save config")?;
    println!("rule `{id}` {}", enabled_word(on));
    Ok(())
}

fn enabled_word(on: bool) -> &'static str {
    if on {
        "enabled"
    } else {
        "disabled"
    }
}

/// Parse `HH:MM-HH:MM` with optional weekday filters into a condition.
fn parse_time_window(window: &str, weekdays: &[String]) -> Result<Condition> {
    let (start_raw, end_raw) = window
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("--time expects `HH:MM-HH:MM`, got `{window}`"))?;

    let parse_hm = |raw: &str| -> Result<chrono::NaiveTime> {
        let mut parts = raw.split(':');
        let hour: u32 = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("bad time `{raw}`"))?
            .parse()?;
        let minute: u32 = match parts.next() {
            Some(value) => value.parse()?,
            None => 0,
        };
        chrono::NaiveTime::from_hms_opt(hour, minute, 0)
            .ok_or_else(|| anyhow::anyhow!("bad time `{raw}`"))
    };

    Ok(Condition::TimeOfDay {
        start: parse_hm(start_raw.trim())?,
        end: parse_hm(end_raw.trim())?,
        weekdays: weekdays
            .iter()
            .map(|raw| parse_weekday(raw))
            .collect::<Result<Vec<_>>>()?,
    })
}

fn parse_weekday(raw: &str) -> Result<chrono::Weekday> {
    use chrono::Weekday::*;
    let value = raw.trim().to_ascii_lowercase();
    Ok(match value.as_str() {
        "mon" | "monday" => Mon,
        "tue" | "tues" | "tuesday" => Tue,
        "wed" | "wednesday" => Wed,
        "thu" | "thur" | "thurs" | "thursday" => Thu,
        "fri" | "friday" => Fri,
        "sat" | "saturday" => Sat,
        "sun" | "sunday" => Sun,
        _ => anyhow::bail!("unknown weekday `{raw}` (use mon tue wed thu fri sat sun)"),
    })
}

fn load_or_default(path: &std::path::Path) -> Result<tpt_focus_core::Config> {
    if path.exists() {
        tpt_focus_core::Config::load(path)
            .with_context(|| format!("failed to load {}", path.display()))
    } else {
        Ok(tpt_focus_core::Config::default())
    }
}
