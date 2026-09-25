//! The egui popup window: history search, digest, rule/profile editors and
//! settings, plus the Linux notification bubbles.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tpt_focus_core::{
    AccessState, Decision, FocusRuntime, HistoryQuery, NotificationId, NotificationSource,
};

use crate::state::{AppState, Page, RuleDraft, UiCommand, UiHandle};

pub struct FocusApp {
    receiver: std::sync::mpsc::Receiver<UiCommand>,
    handle: UiHandle,
    runtime: FocusRuntime,
    source: Arc<Mutex<Option<Box<dyn NotificationSource>>>>,
    last_refresh: std::time::Instant,
}

impl FocusApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        receiver: std::sync::mpsc::Receiver<UiCommand>,
        handle: UiHandle,
        runtime: FocusRuntime,
        source: Arc<Mutex<Option<Box<dyn NotificationSource>>>>,
    ) -> Self {
        handle.set_context(cc.egui_ctx.clone());
        Self {
            receiver,
            handle,
            runtime,
            source,
            last_refresh: std::time::Instant::now() - Duration::from_secs(10),
        }
    }

    /// Drain commands sent by the tray and source threads.
    fn poll_commands(&mut self) {
        while let Ok(command) = self.receiver.try_recv() {
            match command {
                UiCommand::ToggleVisible => {
                    let visible = !self.handle.snapshot().visible;
                    self.set_visible(visible);
                }
                UiCommand::Show(page) => {
                    self.handle.update(|state| {
                        state.page = page;
                        if page == Page::History {
                            state.history_stale = true;
                        }
                    });
                    self.set_visible(true);
                }
                UiCommand::ActivateProfile(name) => {
                    self.activate_profile(name);
                    #[cfg(target_os = "linux")]
                    crate::tray_linux::refresh();
                }
                UiCommand::Quit => {
                    if let Some(context) = self.handle.context_slot() {
                        context.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }
    }

    fn set_visible(&mut self, visible: bool) {
        self.handle.update(|state| {
            state.visible = visible;
            if visible {
                state.history_stale = true;
            }
        });
        if let Some(context) = self.handle.context_slot() {
            context.send_viewport_cmd(egui::ViewportCommand::Visible(visible));
            if visible {
                context.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.place_bottom_right();
            }
        }
    }

    /// Park the popup in the bottom-right corner once egui knows the monitor
    /// size.
    fn place_bottom_right(&mut self) {
        let Some(context) = self.handle.context_slot() else {
            return;
        };
        let Some(monitor) = context.input(|input| input.viewport().monitor_size) else {
            return;
        };
        let window = egui::vec2(560.0, 640.0);
        let position = egui::pos2(
            (monitor.x - window.x - 24.0).max(0.0),
            (monitor.y - window.y - 48.0).max(0.0),
        );
        context.send_viewport_cmd(egui::ViewportCommand::OuterPosition(position));
    }

    fn activate_profile(&mut self, name: Option<String>) {
        match &name {
            Some(name) => {
                self.runtime.active_profile().activate(name.clone());
            }
            None => {
                self.runtime.active_profile().clear();
            }
        }
        self.handle.update(|state| state.active_profile = name);
    }

    /// Pull fresh history from storage while the window is visible.
    fn refresh_history(&mut self, state: &mut AppState) {
        if !state.visible && !state.history_stale {
            return;
        }
        if self.last_refresh.elapsed() < Duration::from_millis(800) && !state.history_stale {
            return;
        }
        self.last_refresh = std::time::Instant::now();
        state.history_stale = false;

        let query = HistoryQuery::new().with_limit(200);
        if let Ok(storage) = self.runtime.storage().lock() {
            state.history = if state.search.trim().is_empty() {
                storage.query(&query).unwrap_or_default()
            } else {
                storage.search(&state.search, &query).unwrap_or_default()
            };
        }
        let config = self.runtime.pipeline().config();
        state.rules = config.rules.clone();
        state.profiles = config.profiles.clone();
    }

    fn dismiss_through(&self, id: &NotificationId) {
        let guard = self.source.lock().expect("source slot poisoned");
        if let Some(source) = guard.as_ref() {
            if let Err(error) = source.dismiss(id) {
                tracing::warn!(%error, "dismiss-through failed");
            }
        }
    }
}

impl eframe::App for FocusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_commands();

        // Drop expired bubbles.
        let now = chrono::Utc::now();
        self.handle.update(|state| {
            state.bubbles.retain(|bubble| !bubble.expired(now));
        });

        // Lazy refresh while visible.
        {
            let state_guard = self.handle.state().lock().expect("app state poisoned");
            let mut state_view = state_guard.clone();
            drop(state_guard);
            self.refresh_history(&mut state_view);
            if let Ok(mut guard) = self.handle.state().lock() {
                *guard = state_view;
            }
        }

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let page = self.handle.snapshot().page;
                for (target, label) in [
                    (Page::History, "History"),
                    (Page::Digest, "Digest"),
                    (Page::Rules, "Rules"),
                    (Page::Profiles, "Profiles"),
                    (Page::Settings, "Settings"),
                ] {
                    if ui.selectable_label(page == target, label).clicked() {
                        self.handle.send(UiCommand::Show(target));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(profile) = self.handle.snapshot().active_profile {
                        ui.colored_label(egui::Color32::LIGHT_BLUE, format!("focus: {profile}"));
                    }
                    if ui.button("Hide").clicked() {
                        self.set_visible(false);
                    }
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let snapshot = self.handle.snapshot();
            match snapshot.page {
                Page::History => self.history_page(ui, &snapshot),
                Page::Digest => self.digest_page(ui),
                Page::Rules => self.rules_page(ui, &snapshot),
                Page::Profiles => self.profiles_page(ui, &snapshot),
                Page::Settings => self.settings_page(ui, &snapshot),
            }
        });

        // Bubble stack pinned to the bottom (Linux).
        let snapshot = self.handle.snapshot();
        if !snapshot.bubbles.is_empty() {
            egui::TopBottomPanel::bottom("bubbles").show(ctx, |ui| {
                self.bubbles_section(ui, &snapshot);
            });
        }

        // Keep countdowns and history refresh moving.
        ctx.request_repaint_after(self.handle.repaint_interval());
    }
}

impl FocusApp {
    fn history_page(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let mut query = state.search.clone();
        let response = ui.add(
            egui::TextEdit::singleline(&mut query)
                .hint_text("Search notifications… (full text)")
                .desired_width(340.0),
        );
        if response.changed() {
            let fresh = query.clone();
            self.handle.update(|state| {
                state.search = fresh;
                state.history_stale = true;
            });
        }
        ui.add_space(6.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            if state.history.is_empty() {
                ui.weak(if state.search.is_empty() {
                    "no notifications recorded yet"
                } else {
                    "no notifications match the search"
                });
                return;
            }
            for entry in &state.history {
                let entry = entry.clone();
                ui.horizontal(|ui| {
                    let (badge, color) = match entry.decision {
                        Decision::Allow => ("show", egui::Color32::GREEN),
                        Decision::Mute => ("muted", egui::Color32::GRAY),
                        Decision::Batch => ("batch", egui::Color32::YELLOW),
                    };
                    ui.monospace(
                        entry
                            .notification
                            .timestamp
                            .with_timezone(&chrono::Local)
                            .format("%m-%d %H:%M")
                            .to_string(),
                    );
                    ui.colored_label(color, badge);
                    ui.strong(truncate(
                        &entry
                            .notification
                            .source
                            .display_name
                            .clone()
                            .unwrap_or_else(|| entry.notification.source.app_id.clone()),
                        18,
                    ));
                    ui.label(truncate(&entry.notification.title, 42));
                    if !entry.read {
                        ui.small("•unread");
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let id = entry.notification.id.clone();
                        if ui
                            .button("✕")
                            .on_hover_text("Delete from history")
                            .clicked()
                        {
                            if let Ok(storage) = self.runtime.storage().lock() {
                                let _ = storage.delete_notification(&id);
                            }
                            self.handle.update(|state| state.history_stale = true);
                        }
                        let id = entry.notification.id.clone();
                        let label = if entry.read {
                            "mark unread"
                        } else {
                            "mark read"
                        };
                        if ui.button(label).clicked() {
                            if let Ok(storage) = self.runtime.storage().lock() {
                                let _ = storage.set_read(&id, !entry.read);
                            }
                            self.handle.update(|state| state.history_stale = true);
                        }
                        if entry.decision == Decision::Allow
                            && ui
                                .button("dismiss")
                                .on_hover_text("Remove from the OS")
                                .clicked()
                        {
                            self.dismiss_through(&id);
                        }
                    });
                });
                ui.separator();
            }
        });
    }

    fn digest_page(&mut self, ui: &mut egui::Ui) {
        let groups = self.runtime.digests().groups();
        if groups.is_empty() {
            ui.weak("nothing is being held back right now");
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for group in &groups {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(&group.app_id);
                        ui.label(format!("{} held", group.entries.len()));
                        let app_id = group.app_id.clone();
                        if ui
                            .button("open")
                            .on_hover_text("Mark the group read")
                            .clicked()
                        {
                            if let Some(group) = self.runtime.digests().take(&app_id) {
                                if let Ok(storage) = self.runtime.storage().lock() {
                                    for entry in &group.entries {
                                        let _ = storage.set_read(&entry.notification.id, true);
                                    }
                                }
                            }
                            self.handle.update(|state| state.history_stale = true);
                        }
                    });
                    for entry in group.entries.iter().rev().take(5) {
                        ui.label(format!("  • {}", entry.notification.title));
                    }
                    if group.entries.len() > 5 {
                        ui.weak(format!("  … and {} more", group.entries.len() - 5));
                    }
                });
            }
        });
    }

    fn rules_page(&mut self, ui: &mut egui::Ui, state: &AppState) {
        ui.heading("Rules");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for rule in &state.rules {
                ui.horizontal(|ui| {
                    let id = rule.id.clone();
                    if ui
                        .button(if rule.enabled { "on" } else { "off" })
                        .on_hover_text("Toggle rule")
                        .clicked()
                    {
                        self.toggle_rule(&id, !rule.enabled);
                    }
                    ui.monospace(&rule.id);
                    ui.label(rule.action.as_str());
                    ui.weak(rule.name.clone());
                    let id = rule.id.clone();
                    if ui.button("✕").clicked() {
                        self.remove_rule(&id);
                    }
                });
            }
        });

        ui.add_space(10.0);
        ui.separator();
        ui.heading("Add rule");
        let mut draft = state.rule_draft.clone();
        egui::Grid::new("rule-draft").show(ui, |ui| {
            ui.label("id");
            if ui.text_edit_singleline(&mut draft.id).changed() {
                self.save_draft(&draft);
            }
            ui.end_row();

            ui.label("app pattern");
            if ui.text_edit_singleline(&mut draft.app).changed() {
                self.save_draft(&draft);
            }
            ui.end_row();

            ui.label("action");
            for (index, action) in RuleDraft::ACTIONS.into_iter().enumerate() {
                if ui.selectable_label(draft.action == index, action).clicked() {
                    draft.action = index;
                    self.save_draft(&draft);
                }
            }
            ui.end_row();

            ui.label("only when fullscreen");
            if ui.checkbox(&mut draft.fullscreen, "").changed() {
                self.save_draft(&draft);
            }
            ui.end_row();

            ui.label("priority");
            if ui.text_edit_singleline(&mut draft.priority).changed() {
                self.save_draft(&draft);
            }
            ui.end_row();
        });

        ui.horizontal(|ui| {
            if ui.button("Add rule").clicked() {
                match state.rule_draft.build() {
                    Ok(rule) => self.add_rule(rule),
                    Err(message) => self.handle.update(|state| state.status = message),
                }
            }
            if !state.status.is_empty() {
                ui.weak(&state.status);
            }
        });
    }

    fn profiles_page(&mut self, ui: &mut egui::Ui, state: &AppState) {
        ui.heading("Focus profiles");
        for profile in &state.profiles {
            ui.horizontal(|ui| {
                let active = state
                    .active_profile
                    .as_deref()
                    .is_some_and(|active| active.eq_ignore_ascii_case(&profile.name));
                if ui
                    .button(if active { "● active" } else { "activate" })
                    .clicked()
                {
                    let name = if active {
                        None
                    } else {
                        Some(profile.name.clone())
                    };
                    self.activate_profile(name);
                }
                ui.strong(&profile.name);
                if let Some(description) = &profile.description {
                    ui.weak(description.clone());
                }
            });
        }

        ui.add_space(8.0);
        let mut new_profile = state.new_profile.clone();
        let response =
            ui.add(egui::TextEdit::singleline(&mut new_profile).hint_text("New profile name"));
        if response.changed() {
            let fresh = new_profile.clone();
            self.handle.update(|state| state.new_profile = fresh);
        }
        if ui.button("Add profile").clicked() {
            self.add_profile(state.new_profile.trim().to_string());
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui, state: &AppState) {
        ui.heading("Settings");
        if let Some(banner) = state.access.banner() {
            let color = match banner.level {
                tpt_focus_core::BannerLevel::Info => egui::Color32::LIGHT_BLUE,
                tpt_focus_core::BannerLevel::Warning => egui::Color32::YELLOW,
                tpt_focus_core::BannerLevel::Error => egui::Color32::LIGHT_RED,
            };
            ui.colored_label(color, banner.message.clone());
            match banner.action {
                Some(tpt_focus_core::BannerAction::RequestAgain) => {
                    if ui.button("Request access again").clicked() {
                        crate::source::request_access_background(self.handle.clone());
                    }
                }
                Some(tpt_focus_core::BannerAction::OpenDocs) => {
                    ui.hyperlink_to(
                        "Open package identity docs",
                        tpt_focus_core::platform::access::PACKAGE_IDENTITY_DOC,
                    );
                }
                None => {}
            }
        } else if state.access == AccessState::Granted {
            ui.label("✔ notification access granted");
        } else {
            ui.label("✔ notifications available");
        }

        ui.add_space(8.0);
        ui.label(format!("config: {}", state.config_path.display()));
        let dismissible = self
            .source
            .lock()
            .expect("source slot poisoned")
            .as_ref()
            .map(|source| source.capabilities().can_dismiss)
            .unwrap_or(false);
        ui.label(format!("dismiss-through: {dismissible}"));
        ui.label(format!("tpt-focus v{}", tpt_focus_core::VERSION));
    }

    fn bubbles_section(&mut self, ui: &mut egui::Ui, state: &AppState) {
        for bubble in &state.bubbles {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.strong(truncate(
                        &bubble
                            .notification
                            .source
                            .display_name
                            .clone()
                            .unwrap_or_else(|| bubble.notification.source.app_id.clone()),
                        18,
                    ));
                    ui.label(truncate(&bubble.notification.title, 44));
                    if let Some(seconds) = bubble.remaining_seconds() {
                        ui.weak(format!("{seconds}s"));
                    }
                });
                if !bubble.notification.body.is_empty() {
                    ui.label(truncate(&bubble.notification.body, 90));
                }
                ui.horizontal(|ui| {
                    for action in &bubble.notification.actions {
                        let id = bubble.notification.id.clone();
                        let key = action.id.clone();
                        let label = action.label.clone();
                        if ui.button(label).clicked() {
                            crate::source::bubble_action(&id, &key);
                            self.close_bubble(&id, false);
                        }
                    }
                    let id = bubble.notification.id.clone();
                    if ui.button("dismiss").clicked() {
                        self.close_bubble(&id, true);
                    }
                });
            });
        }
    }

    fn close_bubble(&mut self, id: &NotificationId, by_user: bool) {
        self.handle.update(|state| {
            state.bubbles.retain(|bubble| bubble.notification.id != *id);
        });
        crate::source::bubble_closed(id, by_user);
    }

    fn save_draft(&mut self, draft: &RuleDraft) {
        self.handle.update(|state| state.rule_draft = draft.clone());
    }

    fn persist_config(&mut self, config: &tpt_focus_core::Config) {
        let path = self.handle.snapshot().config_path;
        if let Err(error) = config.save(&path) {
            tracing::error!(%error, "failed to save config");
            self.handle
                .update(|state| state.status = format!("save failed: {error}"));
            return;
        }
        self.runtime.pipeline().replace_config(config.clone());
        self.handle.update(|state| {
            state.rules = config.rules.clone();
            state.profiles = config.profiles.clone();
            state.history_stale = true;
        });
    }

    fn add_rule(&mut self, rule: tpt_focus_core::Rule) {
        let mut config = (*self.runtime.pipeline().config()).clone();
        config.rules.push(rule);
        self.persist_config(&config);
    }

    fn remove_rule(&mut self, id: &str) {
        let mut config = (*self.runtime.pipeline().config()).clone();
        config.rules.retain(|rule| rule.id != id);
        self.persist_config(&config);
    }

    fn toggle_rule(&mut self, id: &str, on: bool) {
        let mut config = (*self.runtime.pipeline().config()).clone();
        if let Some(rule) = config.rules.iter_mut().find(|rule| rule.id == id) {
            rule.enabled = on;
        }
        self.persist_config(&config);
    }

    fn add_profile(&mut self, name: String) {
        if name.is_empty() {
            return;
        }
        let mut config = (*self.runtime.pipeline().config()).clone();
        if config
            .profiles
            .iter()
            .any(|profile| profile.name.eq_ignore_ascii_case(&name))
        {
            return;
        }
        config
            .profiles
            .push(tpt_focus_core::FocusProfile::new(name));
        self.persist_config(&config);
        self.handle
            .update(|state| state.new_profile = String::new());
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        let cut: String = value.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
