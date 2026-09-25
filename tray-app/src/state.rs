//! Shared UI state and the command channel between the tray, the platform
//! source and the egui window.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};

use tpt_focus_core::{AccessState, FocusProfile, HistoryEntry, Notification, Rule};

/// Which tab the popup window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    History,
    Digest,
    Rules,
    Profiles,
    Settings,
}

/// Commands from the tray / source threads into the UI.
#[derive(Debug, Clone)]
pub enum UiCommand {
    /// Show/hide the popup window.
    ToggleVisible,
    /// Show a specific page.
    Show(Page),
    /// Quick profile switch from the tray menu.
    ActivateProfile(Option<String>),
    /// Tear down.
    Quit,
}

/// One allowed notification awaiting user attention (Linux bubbles; on
/// Windows the OS already displayed a toast and this stays empty).
#[derive(Debug, Clone)]
pub struct Bubble {
    pub notification: Notification,
    /// When the bubble should close; `None` = sticky until dismissed.
    pub expires: Option<DateTime<Utc>>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl Bubble {
    /// Spec policy: `expire_timeout <= 0` and critical urgency stick.
    pub fn new(notification: Notification, expire_timeout_ms: i64) -> Self {
        let sticky =
            expire_timeout_ms <= 0 || notification.urgency == tpt_focus_core::Urgency::Critical;
        let expires = (!sticky).then(|| {
            Utc::now()
                + chrono::Duration::milliseconds(if expire_timeout_ms > 0 {
                    expire_timeout_ms
                } else {
                    5_000
                })
        });
        Self {
            notification,
            expires,
        }
    }

    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        self.expires.is_some_and(|expires| expires <= now)
    }

    /// Whole seconds left before expiry; `None` = sticky.
    pub fn remaining_seconds(&self) -> Option<i64> {
        self.expires
            .map(|expires| (expires - Utc::now()).num_seconds().max(0))
    }
}

/// Draft state for the "add rule" form.
#[derive(Debug, Clone, Default)]
pub struct RuleDraft {
    pub id: String,
    pub app: String,
    pub action: usize, // index into ACTIONS
    pub fullscreen: bool,
    pub priority: String,
}

impl RuleDraft {
    pub const ACTIONS: [&'static str; 3] = ["allow", "mute", "batch"];

    /// Build a rule from the form; `Err` carries a user-facing message.
    pub fn build(&self) -> Result<Rule, String> {
        let id = self.id.trim().to_string();
        if id.is_empty() {
            return Err("rule id must not be empty".into());
        }
        let app = self.app.trim().to_string();
        if app.is_empty() {
            return Err("app pattern must not be empty".into());
        }

        let action = match self.action {
            0 => tpt_focus_core::Decision::Allow,
            1 => tpt_focus_core::Decision::Mute,
            _ => tpt_focus_core::Decision::Batch,
        };

        let mut rule = Rule::new(id.clone(), action);
        rule.priority = self.priority.trim().parse().unwrap_or(0);
        rule.conditions
            .push(tpt_focus_core::Condition::App { apps: vec![app] });
        if self.fullscreen {
            rule.conditions
                .push(tpt_focus_core::Condition::Fullscreen { equals: true });
        }
        Ok(rule)
    }
}

/// Everything the egui app renders.
#[derive(Debug, Clone)]
pub struct AppState {
    pub page: Page,
    pub visible: bool,
    pub search: String,
    pub history: Vec<HistoryEntry>,
    pub history_stale: bool,
    pub profiles: Vec<FocusProfile>,
    pub active_profile: Option<String>,
    pub rules: Vec<Rule>,
    pub access: AccessState,
    pub bubbles: Vec<Bubble>,
    pub rule_draft: RuleDraft,
    pub new_profile: String,
    pub status: String,
    pub config_path: PathBuf,
}

impl AppState {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            page: Page::History,
            visible: false,
            search: String::new(),
            history: Vec::new(),
            history_stale: true,
            profiles: Vec::new(),
            active_profile: None,
            rules: Vec::new(),
            access: AccessState::Unknown,
            bubbles: Vec::new(),
            rule_draft: RuleDraft::default(),
            new_profile: String::new(),
            status: String::new(),
            config_path,
        }
    }
}

/// The handle shared with tray / source threads.
#[derive(Clone)]
pub struct UiHandle {
    state: Arc<Mutex<AppState>>,
    commands: std::sync::mpsc::Sender<UiCommand>,
    context: Arc<Mutex<Option<egui::Context>>>,
}

impl UiHandle {
    pub fn new(state: Arc<Mutex<AppState>>, commands: std::sync::mpsc::Sender<UiCommand>) -> Self {
        Self {
            state,
            commands,
            context: Arc::new(Mutex::new(None)),
        }
    }

    /// The egui context, once the window exists.
    pub fn context_slot(&self) -> Option<egui::Context> {
        self.context
            .lock()
            .expect("ui context slot poisoned")
            .clone()
    }

    /// Called by the eframe app once the context exists.
    pub fn set_context(&self, context: egui::Context) {
        *self.context.lock().expect("ui context slot poisoned") = Some(context);
    }

    pub fn send(&self, command: UiCommand) {
        let _ = self.commands.send(command);
        self.wake();
    }

    pub fn wake(&self) {
        if let Some(context) = self
            .context
            .lock()
            .expect("ui context slot poisoned")
            .as_ref()
        {
            context.request_repaint();
        }
    }

    /// Mutate the state and repaint afterwards.
    pub fn update(&self, mutate: impl FnOnce(&mut AppState)) {
        let mut guard = self.state.lock().expect("app state poisoned");
        mutate(&mut guard);
        drop(guard);
        self.wake();
    }

    /// Snapshot for rendering.
    pub fn snapshot(&self) -> AppState {
        self.state.lock().expect("app state poisoned").clone()
    }

    pub fn state(&self) -> &Arc<Mutex<AppState>> {
        &self.state
    }

    /// A sender clone for a tray / hotkey thread.
    pub fn command_sender(&self) -> std::sync::mpsc::Sender<UiCommand> {
        self.commands.clone()
    }

    /// Bubble expiry polling interval for `request_repaint_after`.
    pub fn repaint_interval(&self) -> Duration {
        Duration::from_millis(500)
    }
}
