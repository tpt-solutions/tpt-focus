//! `tpt-focus history` — query and maintain the local notification history.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::Subcommand;

use crate::paths;
use crate::print;
use crate::{Ctx, HistoryFilter};

use chrono::{Duration, Utc};
use tpt_focus_core::{Decision, HistoryQuery, Retention};

#[derive(Subcommand)]
pub enum HistoryArgs {
    /// Show recent notifications, newest first.
    List {
        #[command(flatten)]
        filter: HistoryFilter,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Full-text search over notification titles and bodies.
    Search {
        /// Query text (all terms must match).
        query: String,
        #[command(flatten)]
        filter: HistoryFilter,
        #[arg(long)]
        json: bool,
    },
    /// Mark one notification read (or unread with --undo).
    Read {
        /// Notification id (`tpt-focus history list` prints them).
        id: String,
        /// Mark unread again.
        #[arg(long)]
        undo: bool,
    },
    /// Delete history older than / beyond the retention limits.
    Prune {
        /// Maximum age in days.
        #[arg(long, value_name = "DAYS")]
        max_age_days: Option<u64>,
        /// Keep only the newest N notifications.
        #[arg(long, value_name = "N")]
        max_count: Option<u64>,
    },
    /// Export the whole history (with decisions) as JSON.
    Export { file: PathBuf },
    /// Import a previously exported JSON file.
    Import { file: PathBuf },
    /// Delete every notification from history.
    Clear {
        /// Skip the confirmation prompt.
        #[arg(long, short)]
        yes: bool,
    },
    /// Show a per-app digest of suppressed notifications.
    Digest {
        /// Window to look back on, in minutes.
        #[arg(long, default_value_t = 60, value_name = "MINUTES")]
        minutes: u64,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(ctx: &Ctx, args: HistoryArgs) -> Result<()> {
    let db_path = paths::db_path(ctx.db.as_deref());
    paths::ensure_parent(&db_path)?;
    let storage = tpt_focus_core::open_shared(&db_path)?;
    let guard = storage
        .lock()
        .map_err(|_| anyhow::anyhow!("history database locked"))?;

    match args {
        HistoryArgs::List { filter, json } => {
            let query = build_query(&filter)?;
            let entries = guard.query(&query)?;
            if json {
                print::print_json(&entries)?;
            } else if entries.is_empty() {
                println!("no notifications matched");
            } else {
                println!(
                    "   {:<19}  {:<5}  {:<20}  {:<40}  ID",
                    "TIME", "OUT", "APP", "TITLE"
                );
                for entry in &entries {
                    print::print_history_entry(entry);
                }
            }
        }

        HistoryArgs::Search {
            query,
            filter,
            json,
        } => {
            let filter_query = build_query(&filter)?;
            let entries = guard.search(&query, &filter_query)?;
            if json {
                print::print_json(&entries)?;
            } else if entries.is_empty() {
                println!("`{query}` did not match any notifications");
            } else {
                for entry in &entries {
                    print::print_history_entry(entry);
                }
            }
        }

        HistoryArgs::Read { id, undo } => {
            let changed = guard.set_read(&tpt_focus_core::NotificationId::from(id), !undo)?;
            if changed {
                println!("notification updated");
            } else {
                anyhow::bail!("no notification with that id");
            }
        }

        HistoryArgs::Prune {
            max_age_days,
            max_count,
        } => {
            if max_age_days.is_none() && max_count.is_none() {
                anyhow::bail!("pass --max-age-days and/or --max-count");
            }
            let retention = Retention {
                max_age: max_age_days.map(|days| Duration::days(days as i64)),
                max_count,
            };
            let report = guard.prune(&retention)?;
            println!(
                "pruned {} notifications ({} by age, {} by count)",
                report.total(),
                report.removed_by_age,
                report.removed_by_count
            );
        }

        HistoryArgs::Export { file } => {
            let raw = guard.export_json()?;
            std::fs::write(&file, raw)
                .with_context(|| format!("failed to write {}", file.display()))?;
            println!("history exported to {}", file.display());
        }

        HistoryArgs::Import { file } => {
            let raw = std::fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            let imported = guard.import_json(&raw)?;
            println!("imported {imported} notifications");
        }

        HistoryArgs::Clear { yes } => {
            if !yes {
                anyhow::bail!("refusing to clear history without --yes");
            }
            let removed = guard.clear_history()?;
            println!("deleted {removed} notifications");
        }

        HistoryArgs::Digest { minutes, json } => {
            let since = Utc::now() - Duration::minutes(minutes as i64);
            let groups = guard.digest_groups(since)?;
            if json {
                print::print_json(&groups)?;
            } else if groups.is_empty() {
                println!("nothing was suppressed in the last {minutes} minutes");
            } else {
                for group in &groups {
                    println!(
                        "{:22} {:>3} held   latest: {}",
                        print::truncate(&group.app_id, 22),
                        group.entries.len(),
                        group.headline()
                    );
                }
            }
        }
    }
    Ok(())
}

fn build_query(filter: &HistoryFilter) -> Result<HistoryQuery> {
    let mut query = HistoryQuery::new();
    if let Some(app) = &filter.app {
        query = query.with_app(app);
    }
    if let Some(raw) = &filter.decision {
        query = query.with_decision(parse_decision(raw)?);
    }
    if filter.unread {
        query = query.unread_only();
    }
    Ok(query.with_limit(filter.limit))
}

fn parse_decision(raw: &str) -> Result<Decision> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "allow" | "show" => Ok(Decision::Allow),
        "mute" | "muted" => Ok(Decision::Mute),
        "batch" | "batched" => Ok(Decision::Batch),
        _ => anyhow::bail!("unknown decision `{raw}` (use allow, mute or batch)"),
    }
}
