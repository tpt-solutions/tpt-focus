//! Linux tray via the StatusNotifierItem protocol (`ksni`).
//!
//! GNOME needs an AppIndicator extension for SNI; KDE, Sway (waybar) and
//! most wlroots setups ship SNI support by default. The menu is rebuilt
//! from the shared [`AppState`] whenever `refresh` is called.

use std::sync::{Arc, Mutex, OnceLock};

use ksni::blocking::TrayMethods as _;
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use ksni::Icon;

use crate::state::{AppState, Page, UiCommand};

static SENDER: OnceLock<std::sync::mpsc::Sender<UiCommand>> = OnceLock::new();
static STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();
static HANDLE: OnceLock<Mutex<ksni::blocking::Handle<Tray>>> = OnceLock::new();

/// Spawn the tray service on its own thread.
pub fn spawn(handle: crate::state::UiHandle) {
    SENDER.get_or_init(|| handle.command_sender());
    STATE.get_or_init(|| handle.state().clone());

    std::thread::Builder::new()
        .name("tpt-focus-tray".into())
        .spawn(move || {
            let service = match Tray.spawn() {
                Ok(service) => service,
                Err(error) => {
                    tracing::warn!(%error, "StatusNotifierItem tray unavailable");
                    return;
                }
            };
            let _ = HANDLE.set(Mutex::new(service));
            // Park until the process exits so the service keeps running;
            // `refresh` redraws the menu through the stored handle.
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        })
        .expect("spawn tray thread");
}

/// Ask the SNI host to redraw the menu (after a profile switch).
pub fn refresh() {
    if let Some(handle) = HANDLE.get() {
        if let Ok(handle) = handle.lock() {
            handle.update(|_| {});
        }
    }
}

struct Tray;

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "tpt-focus".into()
    }

    fn title(&self) -> String {
        "tpt-focus".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: String::new(),
            icon_pixmap: icon(),
            title: "tpt-focus".into(),
            description: "Unified Notification & Focus Center".into(),
        }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        icon()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        if let Some(sender) = SENDER.get() {
            let _ = sender.send(UiCommand::ToggleVisible);
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let state = STATE
            .get()
            .and_then(|state| state.lock().ok())
            .map(|guard| guard.clone());

        let profiles = state
            .as_ref()
            .map(|s| s.profiles.clone())
            .unwrap_or_default();
        let active = state.as_ref().and_then(|s| s.active_profile.clone());

        let item = |label: &str, command: UiCommand| {
            MenuItem::Standard(StandardItem {
                label: label.to_string(),
                activate: Box::new(move |_tray| {
                    if let Some(sender) = SENDER.get() {
                        let _ = sender.send(command.clone());
                    }
                }),
                ..Default::default()
            })
        };

        let mut profile_items: Vec<MenuItem<Self>> = vec![MenuItem::Checkmark(CheckmarkItem {
            label: "normal (no profile)".into(),
            checked: active.is_none(),
            enabled: true,
            activate: Box::new(|_tray| {
                if let Some(sender) = SENDER.get() {
                    let _ = sender.send(UiCommand::ActivateProfile(None));
                }
            }),
            ..Default::default()
        })];
        for profile in profiles {
            let is_active = active
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(&profile.name));
            let name = profile.name.clone();
            profile_items.push(MenuItem::Checkmark(CheckmarkItem {
                label: name.clone(),
                checked: is_active,
                enabled: true,
                activate: Box::new(move |_tray| {
                    if let Some(sender) = SENDER.get() {
                        let _ = sender.send(UiCommand::ActivateProfile(Some(name.clone())));
                    }
                }),
                ..Default::default()
            }));
        }

        vec![
            item("Open / hide popup", UiCommand::ToggleVisible),
            item("History", UiCommand::Show(Page::History)),
            item("Digest", UiCommand::Show(Page::Digest)),
            item("Settings", UiCommand::Show(Page::Settings)),
            MenuItem::Separator,
            MenuItem::SubMenu(SubMenu {
                label: "Focus profile".into(),
                submenu: profile_items,
                ..Default::default()
            }),
            MenuItem::Separator,
            item("Quit", UiCommand::Quit),
        ]
    }
}

/// A 16×16 purple dot with a white bar, ARGB little-endian — avoids shipping
/// icon assets.
fn icon() -> Vec<Icon> {
    const SIZE: usize = 16;
    let mut data = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - 7.5;
            let dy = y as f32 - 7.5;
            let inside = dx * dx + dy * dy <= 6.5 * 6.5;
            let bar = inside && (8..=11).contains(&y);
            if bar {
                data.extend_from_slice(&[255, 255, 255, 255]); // ARGB white
            } else if inside {
                data.extend_from_slice(&[255, 110, 70, 160]); // ARGB purple
            } else {
                data.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    vec![Icon {
        width: SIZE as i32,
        height: SIZE as i32,
        data,
    }]
}
