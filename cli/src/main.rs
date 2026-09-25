//! `tpt-focus` — command line interface for rules, profiles, schedules and
//! notification history.

mod cmd_config;
mod cmd_daemon;
mod cmd_evaluate;
mod cmd_history;
mod cmd_profiles;
mod cmd_rules;
mod cmd_schedules;
mod paths;
mod print;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Global options shared by every subcommand.
pub struct Ctx {
    pub config: Option<PathBuf>,
    pub db: Option<PathBuf>,
}

#[derive(Parser)]
#[command(
    name = "tpt-focus",
    version,
    about = "Unified Notification & Focus Center — manage rules, profiles and history",
    after_help = "Run `tpt-focus <command> --help` for details."
)]
pub struct Cli {
    /// Path to the TOML configuration file.
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Path to the SQLite history database.
    #[arg(long, global = true, value_name = "FILE")]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Inspect, create or validate the TOML configuration.
    #[command(subcommand)]
    Config(cmd_config::ConfigArgs),

    /// Manage notification rules.
    #[command(subcommand)]
    Rules(cmd_rules::RulesArgs),

    /// Manage focus profiles.
    #[command(subcommand)]
    Profiles(cmd_profiles::ProfilesArgs),

    /// Manage recurring schedules (stored in the history database).
    #[command(subcommand)]
    Schedules(cmd_schedules::SchedulesArgs),

    /// Query and maintain the local notification history.
    #[command(subcommand)]
    History(cmd_history::HistoryArgs),

    /// Evaluate a synthetic notification against the current rules.
    Evaluate(cmd_evaluate::EvaluateArgs),

    /// Run the headless notification engine (no GUI).
    Daemon(cmd_daemon::DaemonArgs),
}

/// Shared filter flags for `history list` / `history search`.
#[derive(clap::Args, Clone, Default)]
pub struct HistoryFilter {
    /// Only notifications from this app (case-insensitive).
    #[arg(long, value_name = "APP")]
    pub app: Option<String>,

    /// Only notifications with this decision.
    #[arg(long, value_name = "ALLOW|MUTE|BATCH")]
    pub decision: Option<String>,

    /// Only unread notifications.
    #[arg(long)]
    pub unread: bool,

    /// Maximum rows to print.
    #[arg(long, default_value_t = 50, value_name = "N")]
    pub limit: u32,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let Cli {
        config,
        db,
        command,
    } = Cli::parse();
    let ctx = Ctx { config, db };

    match command {
        Command::Config(args) => cmd_config::run(&ctx, args),
        Command::Rules(args) => cmd_rules::run(&ctx, args),
        Command::Profiles(args) => cmd_profiles::run(&ctx, args),
        Command::Schedules(args) => cmd_schedules::run(&ctx, args),
        Command::History(args) => cmd_history::run(&ctx, args),
        Command::Evaluate(args) => cmd_evaluate::run(&ctx, args),
        Command::Daemon(args) => cmd_daemon::run(&ctx, args),
    }
}
