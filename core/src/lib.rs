//! `tpt-focus-core` — engine for the Unified Notification & Focus Center.
//!
//! This crate hosts the notification data model, the rule evaluation pipeline,
//! focus profiles and scheduling, and (in later phases) the local history
//! store. Platform backends — the Windows WinRT listener, the Linux
//! `org.freedesktop.Notifications` D-Bus daemon and the tray UI — live in the
//! sibling `tpt-focus-cli` / `tpt-focus-tray` crates and depend on the traits
//! defined here.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//! use tpt_focus_core::{
//!     AppIdentity, Config, Condition, ContextSnapshot, Decision, MockContextProvider,
//!     Notification, Pipeline, Rule,
//! };
//!
//! let config = Config {
//!     rules: vec![Rule::new("mute-slack-when-fullscreen", Decision::Mute).with_conditions(
//!         vec![
//!             Condition::App { apps: vec!["slack".into()] },
//!             Condition::Fullscreen { equals: true },
//!         ],
//!     )],
//!     ..Config::default()
//! };
//!
//! let context = Arc::new(MockContextProvider::new());
//! let pipeline = Pipeline::from_refs(&config, context.clone());
//!
//! let notification = Notification::new(AppIdentity::new("slack"), "Standup", "in 5 min");
//! assert_eq!(pipeline.process(notification.clone()).evaluation.decision, Decision::Allow);
//!
//! context.set_fullscreen(true);
//! assert_eq!(pipeline.process(notification).evaluation.decision, Decision::Mute);
//! ```

#![deny(unsafe_code)]
//
// The single exception is platform::windows::win32, whose raw Win32 calls
// opt back in locally.

pub mod config;
pub mod context;
pub mod digest;
pub mod engine;
pub mod error;
pub mod model;
pub mod platform;
pub mod profile;
pub mod rules;
pub mod runtime;
pub mod schedule;
pub mod source;
pub mod storage;

pub use config::{Config, Defaults, CONFIG_VERSION};
pub use context::{ContextProvider, ContextSnapshot, MockContextProvider};
pub use digest::{DigestCollector, DigestEntry, DigestGroup, DEFAULT_WINDOW as DIGEST_WINDOW};
pub use engine::{DecisionListener, Pipeline, ProcessedNotification};
pub use error::{Error, Result};
pub use model::{
    pattern_matches, AppIdentity, IconRef, Notification, NotificationAction, NotificationId,
    Urgency,
};
pub use platform::access::{
    AccessState, Banner, BannerAction, BannerLevel, MockAccess, NotificationAccess,
};
pub use profile::{find_profile, ActiveProfile, FocusProfile, ScheduleController, ScheduleTick};
pub use rules::{Condition, Decision, MatchMode, Rule, RuleEngine, RuleEvaluation};
pub use runtime::FocusRuntime;
pub use schedule::{active_schedules, Schedule};
pub use source::{
    MockNotificationSource, NotificationCallback, NotificationSource, SourceCapabilities,
};
pub use storage::{
    open_shared, record_history, HistoryEntry, HistoryExport, HistoryQuery, PruneReport, Retention,
    SharedStorage, Storage,
};

/// Crate version, surfaced by the CLI and tray about dialogs.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
