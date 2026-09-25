//! `tpt-focus schedules` — recurring time windows stored in the history
//! database (they may activate a focus profile while live).

use anyhow::{Context as _, Result};
use clap::{Subcommand, ValueEnum};

use crate::paths;
use crate::print;
use crate::Ctx;

use chrono::{NaiveTime, Weekday};
use tpt_focus_core::Schedule;

#[derive(Subcommand)]
pub enum SchedulesArgs {
    /// List stored schedules.
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Add a schedule.
    Add {
        /// Unique schedule id.
        id: String,
        /// Window start, `HH:MM`.
        #[arg(long, value_name = "HH:MM")]
        start: String,
        /// Window end, `HH:MM` (may be before the start to cross midnight).
        #[arg(long, value_name = "HH:MM")]
        end: String,
        /// Human-readable label.
        #[arg(short, long)]
        name: Option<String>,
        /// Focus profile to activate while the schedule is live.
        #[arg(short, long)]
        profile: Option<String>,
        /// Days the schedule applies to (default: every day).
        #[arg(short, long = "day")]
        weekdays: Vec<String>,
    },
    /// Remove a schedule by id.
    Remove { id: String },
    /// Enable or disable a schedule without deleting it.
    #[group(skip)]
    Toggle {
        /// Schedule id.
        id: String,
        /// Desired state.
        #[arg(long, value_enum, default_value_t = ToggleState::On)]
        state: ToggleState,
    },
}

#[derive(Copy, Clone, ValueEnum)]
pub enum ToggleState {
    On,
    Off,
}

pub fn run(ctx: &Ctx, args: SchedulesArgs) -> Result<()> {
    let db_path = paths::db_path(ctx.db.as_deref());
    paths::ensure_parent(&db_path)?;
    let storage = tpt_focus_core::open_shared(&db_path)?;

    match args {
        SchedulesArgs::List { json } => {
            let schedules = storage
                .lock()
                .map_err(|_| anyhow::anyhow!("database locked"))?
                .list_schedules()?;
            if json {
                print::print_json(&schedules)?;
            } else if schedules.is_empty() {
                println!("no schedules stored ({})", db_path.display());
            } else {
                for schedule in &schedules {
                    print::print_schedule(schedule);
                }
            }
        }

        SchedulesArgs::Add {
            id,
            start,
            end,
            name,
            profile,
            weekdays,
        } => {
            let mut schedule = Schedule::new(&id, parse_hm(&start)?, parse_hm(&end)?);
            if let Some(name) = name {
                schedule.name = name;
            }
            if let Some(profile) = profile {
                schedule.profile = Some(profile);
            }
            if !weekdays.is_empty() {
                schedule.weekdays = weekdays
                    .iter()
                    .map(|raw| parse_weekday(raw))
                    .collect::<Result<Vec<_>>>()?;
            }
            schedule.validate()?;

            let guard = storage
                .lock()
                .map_err(|_| anyhow::anyhow!("database locked"))?;
            guard.save_schedule(&schedule)?;
            println!("schedule `{id}` stored");
        }

        SchedulesArgs::Remove { id } => {
            let removed = storage
                .lock()
                .map_err(|_| anyhow::anyhow!("database locked"))?
                .delete_schedule(&id)
                .context("failed to remove schedule")?;
            if removed {
                println!("schedule `{id}` removed");
            } else {
                anyhow::bail!("no schedule with id `{id}`");
            }
        }

        SchedulesArgs::Toggle { id, state } => {
            let guard = storage
                .lock()
                .map_err(|_| anyhow::anyhow!("database locked"))?;
            let mut schedules = guard.list_schedules()?;
            let schedule = schedules
                .iter_mut()
                .find(|schedule| schedule.id == id)
                .ok_or_else(|| anyhow::anyhow!("no schedule with id `{id}`"))?;

            let new_state = matches!(state, ToggleState::On);
            if schedule.enabled == new_state {
                println!("schedule `{id}` is already {}", on_off(new_state));
            } else {
                schedule.enabled = new_state;
                guard.save_schedule(schedule)?;
                println!("schedule `{id}` {}", on_off(new_state));
            }
        }
    }
    Ok(())
}

fn on_off(on: bool) -> &'static str {
    if on {
        "enabled"
    } else {
        "disabled"
    }
}

fn parse_hm(raw: &str) -> Result<NaiveTime> {
    let mut parts = raw.trim().split(':');
    let hour: u32 = parts
        .next()
        .context("missing hour")?
        .parse()
        .context("bad hour")?;
    let minute: u32 = match parts.next() {
        Some(value) => value.parse().context("bad minute")?,
        None => 0,
    };
    NaiveTime::from_hms_opt(hour, minute, 0).ok_or_else(|| anyhow::anyhow!("bad time `{raw}`"))
}

fn parse_weekday(raw: &str) -> Result<Weekday> {
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
