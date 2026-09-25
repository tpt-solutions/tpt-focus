//! `tpt-focus daemon` — run the notification engine headlessly (no GUI).
//!
//! Starts the platform notification source (Windows WinRT listener / Linux
//! D-Bus daemon), evaluates every notification against the rules, records
//! history and schedules profile switches. The tray app wraps the same
//! machinery with a UI.

use anyhow::{Context as _, Result};
use clap::Parser;

use crate::paths;
use crate::Ctx;

use std::sync::Arc;
use std::time::Duration;

use tpt_focus_core::schedule::Schedule;
use tpt_focus_core::{AccessState, ActiveProfile, Config, FocusRuntime};

#[derive(Parser)]
pub struct DaemonArgs {
    /// Request Windows notification consent on first run.
    #[arg(long)]
    consent: bool,

    /// Run the source even when consent is missing (degraded: history off).
    #[arg(long)]
    degraded: bool,
}

pub fn run(ctx: &Ctx, args: DaemonArgs) -> Result<()> {
    let config_path = paths::config_path(ctx.config.as_deref());
    let db_path = paths::db_path(ctx.db.as_deref());
    paths::ensure_parent(&db_path)?;

    let config = if config_path.exists() {
        Config::load(&config_path)
            .with_context(|| format!("invalid config {}", config_path.display()))?
    } else {
        eprintln!(
            "no config at {} — using defaults (run `tpt-focus config init`)",
            config_path.display()
        );
        Config::default()
    };

    let storage = tpt_focus_core::open_shared(&db_path)?;
    let active_profile = Arc::new(ActiveProfile::new());
    let context = tpt_focus_core::platform::default_context(Arc::clone(&active_profile));
    let runtime = FocusRuntime::new(config, context, Arc::clone(&storage), active_profile);

    // Load schedules from the database and let the runtime switch profiles.
    let schedules: Vec<Schedule> = storage
        .lock()
        .map_err(|_| anyhow::anyhow!("database locked"))?
        .list_schedules()?;
    runtime.set_schedules(schedules);
    let _ = runtime.tick_schedules();

    // Consent (Windows only in practice; other platforms pass through).
    let access = match tpt_focus_core::platform::listener_access_state() {
        AccessState::Granted => AccessState::Granted,
        AccessState::Unknown if args.consent => tpt_focus_core::platform::request_listener_access(),
        other => other,
    };

    let mut source = tpt_focus_core::platform::default_source()?;
    if access.can_listen() {
        let sink_runtime = runtime.clone();
        source.start(Arc::new(move |notification| {
            let processed = sink_runtime.process(notification);
            tracing::info!(
                id = %processed.notification.id,
                decision = processed.evaluation.decision.as_str(),
                "processed notification"
            );
        }))?;
        eprintln!(
            "listening via `{}` (access: {})",
            source.id(),
            access.as_str()
        );
    } else if args.degraded {
        eprintln!(
            "running degraded (access: {}): rules and history stay usable, \
             live ingestion is off",
            access.as_str()
        );
    } else {
        if let Some(banner) = access.banner() {
            eprintln!("{}: {}", level_word(&banner), banner.message);
            eprintln!(
                "see {}",
                tpt_focus_core::platform::access::PACKAGE_IDENTITY_DOC
            );
        }
        anyhow::bail!(
            "notification access unavailable ({}); pass --degraded to run without live ingestion",
            access.as_str()
        );
    }

    // Persist the access state for the settings UI.
    if let Ok(guard) = storage.lock() {
        let _ = guard.set_setting(
            tpt_focus_core::platform::access::ACCESS_SETTING_KEY,
            access.as_str(),
        );
    }

    eprintln!("daemon ready — ctrl-c to stop");
    let mut tick = 0u64;
    loop {
        std::thread::sleep(Duration::from_secs(30));
        tick += 1;
        let _ = runtime.tick_schedules();

        // Hourly retention pass.
        if tick % 120 == 0 {
            match runtime.prune() {
                Ok(report) if report.total() > 0 => {
                    tracing::info!(removed = report.total(), "retention prune");
                }
                Err(error) => tracing::warn!(%error, "retention prune failed"),
                _ => {}
            }
        }
    }
}

fn level_word(banner: &tpt_focus_core::Banner) -> &'static str {
    match banner.level {
        tpt_focus_core::BannerLevel::Info => "info",
        tpt_focus_core::BannerLevel::Warning => "warning",
        tpt_focus_core::BannerLevel::Error => "error",
    }
}
