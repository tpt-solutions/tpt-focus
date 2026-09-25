//! `tpt-focus evaluate` — run a synthetic notification through the rules and
//! print the decision. Handy for testing a rule before saving it.

use anyhow::Result;
use clap::Parser;

use crate::paths;
use crate::Ctx;

use chrono::NaiveTime;
use std::sync::Arc;
use tpt_focus_core::{
    ActiveProfile, AppIdentity, Config, ContextSnapshot, Decision, MockContextProvider, Pipeline,
    Urgency,
};

#[derive(Parser)]
pub struct EvaluateArgs {
    /// Source app id (as shown by `history list`, e.g. `slack`, `code`).
    #[arg(short, long, default_value = "test")]
    app: String,

    /// Notification title.
    #[arg(short, long, default_value = "Test notification")]
    title: String,

    /// Notification body.
    #[arg(short, long, default_value = "")]
    body: String,

    /// Urgency level of the synthetic notification.
    #[arg(short, long, value_enum, default_value_t = UrgencyArg::Normal)]
    urgency: UrgencyArg,

    /// Override fullscreen state (default: real platform state).
    #[arg(long)]
    fullscreen: bool,

    /// Pretend this focus profile is active.
    #[arg(long)]
    profile: Option<String>,

    /// Evaluate as if it were this wall-clock time, `HH:MM`.
    #[arg(long, value_name = "HH:MM")]
    time: Option<String>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum UrgencyArg {
    Low,
    Normal,
    Critical,
}

impl From<UrgencyArg> for Urgency {
    fn from(value: UrgencyArg) -> Self {
        match value {
            UrgencyArg::Low => Urgency::Low,
            UrgencyArg::Normal => Urgency::Normal,
            UrgencyArg::Critical => Urgency::Critical,
        }
    }
}

pub fn run(ctx: &Ctx, args: EvaluateArgs) -> Result<()> {
    let path = paths::config_path(ctx.config.as_deref());
    let config = Config::load(&path)?;

    // Start from the real platform context when we can observe it, then
    // apply explicit overrides on top.
    let active_profile = Arc::new(ActiveProfile::new());
    let provider = tpt_focus_core::platform::default_context(Arc::clone(&active_profile));
    let mut snapshot: ContextSnapshot = provider.snapshot();
    snapshot.fullscreen = args.fullscreen;
    snapshot.active_profile = args.profile.clone();
    if let Some(raw) = &args.time {
        let time = parse_hm(raw)?;
        snapshot.local_time = chrono::NaiveDate::from_ymd_opt(2000, 1, 1)
            .expect("valid date")
            .and_time(time);
    }

    // The pipeline reads the context from its provider, so hand it a mock
    // pre-loaded with the composed snapshot.
    let mock = Arc::new(MockContextProvider::with_snapshot(snapshot));
    let pipeline = Pipeline::from_refs(&config, mock);

    let urgency: Urgency = args.urgency.into();
    let notification = tpt_focus_core::Notification::new(
        AppIdentity::new(&args.app),
        args.title.clone(),
        args.body.clone(),
    )
    .with_urgency(urgency);

    let processed = pipeline.process(notification);
    let evaluation = &processed.evaluation;

    println!(
        "decision: {}",
        match evaluation.decision {
            Decision::Allow => "ALLOW",
            Decision::Mute => "MUTE",
            Decision::Batch => "BATCH",
        }
    );
    if let Some(rule) = &evaluation.matched_rule {
        println!("matched rule: {rule}");
    }
    println!("reason: {}", evaluation.reason);
    Ok(())
}

fn parse_hm(raw: &str) -> Result<NaiveTime> {
    let mut parts = raw.trim().split(':');
    let hour: u32 = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing hour"))?
        .parse()?;
    let minute: u32 = match parts.next() {
        Some(value) => value.parse()?,
        None => 0,
    };
    NaiveTime::from_hms_opt(hour, minute, 0).ok_or_else(|| anyhow::anyhow!("bad time `{raw}`"))
}
