//! Output helpers shared by the command implementations.

use anyhow::Result;

use tpt_focus_core::{Decision, FocusProfile, HistoryEntry, Rule, Schedule};

/// Serialization wrapper so `--json` can dump mixed shapes.
pub fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn print_rule(rule: &Rule) {
    let conditions = if rule.conditions.is_empty() {
        "always".to_string()
    } else {
        rule.conditions
            .iter()
            .map(condition_label)
            .collect::<Vec<_>>()
            .join(
                if matches!(rule.match_mode, tpt_focus_core::MatchMode::Any) {
                    " OR "
                } else {
                    " AND "
                },
            )
    };

    let status = if rule.enabled { "" } else { " [disabled]" };
    println!(
        "{:28} {:6} {:>3}  {:<45} {}{}",
        rule.id,
        rule.action.as_str(),
        rule.priority,
        conditions,
        rule.name,
        status
    );
}

fn condition_label(condition: &tpt_focus_core::Condition) -> String {
    use tpt_focus_core::Condition::*;
    match condition {
        App { apps } => format!("app in [{}]", apps.join(", ")),
        Fullscreen { equals } => {
            if *equals {
                "fullscreen".into()
            } else {
                "not fullscreen".into()
            }
        }
        Profile { profiles } => format!("profile in [{}]", profiles.join(", ")),
        TimeOfDay {
            start,
            end,
            weekdays,
        } => {
            let days = if weekdays.is_empty() {
                "every day".to_string()
            } else {
                weekdays
                    .iter()
                    .map(|d| format!("{d:?}"))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            format!("time {start}-{end} {days}")
        }
        Urgency { urgencies } => format!(
            "urgency in [{}]",
            urgencies
                .iter()
                .map(|u| format!("{u:?}"))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

pub fn print_profile(profile: &FocusProfile) {
    let default = profile
        .default_decision
        .map(|d| d.as_str().to_string())
        .unwrap_or_else(|| "global default".into());
    let description = profile.description.clone().unwrap_or_default();
    println!(
        "  {:24} default: {:16} {}",
        profile.name, default, description
    );
}

pub fn print_schedule(schedule: &Schedule) {
    let days = if schedule.weekdays.is_empty() {
        "every day".to_string()
    } else {
        schedule
            .weekdays
            .iter()
            .map(|d| format!("{d:?}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let status = if schedule.enabled { "" } else { " [disabled]" };
    let profile = schedule.profile.clone().unwrap_or_default();
    println!(
        "{:20} {:>5}-{:<5} {:<45} profile: {}{}",
        schedule.id, schedule.start_time, schedule.end_time, days, profile, status
    );
}

pub fn print_history_entry(entry: &HistoryEntry) {
    let notification = &entry.notification;
    let time = notification
        .timestamp
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M:%S");
    let app = notification
        .source
        .display_name
        .clone()
        .unwrap_or_else(|| notification.source.app_id.clone());
    let title = if notification.title.chars().count() > 40 {
        let truncated: String = notification.title.chars().take(37).collect();
        format!("{truncated}...")
    } else {
        notification.title.clone()
    };
    let read = if entry.read { " " } else { "*" };
    println!(
        "{read} {time}  {:5}  {:20}  {:40}  {}",
        decision_mark(entry.decision),
        truncate(&app, 20),
        truncate(&title, 40),
        notification.id
    );
}

fn decision_mark(decision: Decision) -> &'static str {
    match decision {
        Decision::Allow => "show",
        Decision::Mute => "muted",
        Decision::Batch => "batch",
    }
}

pub fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        let cut: String = value.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
