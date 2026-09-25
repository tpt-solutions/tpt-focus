//! `tpt-focus-tray` — tray icon, global hotkey and the popup UI, hosting the
//! platform notification source.

mod app;
mod source;
mod state;
#[cfg(target_os = "linux")]
mod tray_linux;
#[cfg(windows)]
mod tray_windows;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use clap::Parser;

use state::{AppState, UiHandle};
use tpt_focus_core::{AccessState, ActiveProfile, Config, FocusRuntime};

#[derive(Parser)]
#[command(
    name = "tpt-focus-tray",
    version,
    about = "tpt-focus tray and popup UI"
)]
struct Args {
    /// Path to the TOML configuration file.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Path to the SQLite history database.
    #[arg(long, value_name = "FILE")]
    db: Option<PathBuf>,

    /// Initialise state, start the source, then exit (CI smoke test).
    #[arg(long)]
    smoke: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let config_path = args.config.unwrap_or_else(default_config_path);
    let db_path = args.db.unwrap_or_else(default_db_path);
    std::fs::create_dir_all(config_path.parent().context("config parent")?)?;
    std::fs::create_dir_all(db_path.parent().context("db parent")?)?;

    let config = match Config::load(&config_path) {
        Ok(config) => config,
        Err(_) => {
            tracing::info!(
                path = %config_path.display(),
                "no valid config found; writing the starter configuration"
            );
            let starter = Config::starter();
            starter.save(&config_path)?;
            starter
        }
    };

    let storage = tpt_focus_core::open_shared(&db_path)?;

    // Restore the persisted consent state so Settings shows it immediately.
    let persisted_access = storage
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .get_setting(tpt_focus_core::platform::access::ACCESS_SETTING_KEY)
                .ok()
                .flatten()
        })
        .map(|raw| AccessState::parse(&raw))
        .unwrap_or_default();

    let active_profile = Arc::new(ActiveProfile::new());
    let context = tpt_focus_core::platform::default_context(Arc::clone(&active_profile));
    let runtime = FocusRuntime::new(config.clone(), context, storage.clone(), active_profile);
    runtime.set_retention(tpt_focus_core::Retention {
        max_age: Some(chrono::Duration::days(30)),
        max_count: Some(20_000),
    });

    // Schedules drive profile switches; re-evaluate twice a minute and prune
    // hourly.
    if let Ok(schedules) = storage.lock().map(|guard| guard.list_schedules()) {
        runtime.set_schedules(schedules?);
    }
    let _ = runtime.prune();
    {
        let runtime_for_schedules = runtime.clone();
        std::thread::Builder::new()
            .name("tpt-focus-schedules".into())
            .spawn(move || loop {
                let _ = runtime_for_schedules.tick_schedules();
                std::thread::sleep(Duration::from_secs(30));
            })
            .expect("spawn schedule ticker");
    }

    let (sender, receiver) = std::sync::mpsc::channel();
    let live_access = tpt_focus_core::platform::listener_access_state();
    let initial_access =
        if persisted_access == AccessState::Granted || live_access == AccessState::Granted {
            AccessState::Granted
        } else if persisted_access == AccessState::Denied {
            AccessState::Denied
        } else {
            live_access
        };

    let app_state = Arc::new(Mutex::new(AppState {
        access: initial_access,
        ..AppState::new(config_path.clone())
    }));
    let handle = UiHandle::new(Arc::clone(&app_state), sender);
    let source_slot: source::SourceSlot = Arc::new(Mutex::new(None));

    source::start_background(runtime.clone(), handle.clone(), Arc::clone(&source_slot));

    #[cfg(windows)]
    tray_windows::spawn(handle.clone());
    #[cfg(target_os = "linux")]
    tray_linux::spawn(handle.clone());

    if args.smoke {
        // CI smoke: let the source thread settle, report, exit 0.
        std::thread::sleep(Duration::from_millis(1500));
        let state = app_state.lock().expect("app state poisoned");
        eprintln!(
            "smoke ok: access={}, bubbles={}, rules={}, profiles={}",
            state.access.as_str(),
            state.bubbles.len(),
            state.rules.len(),
            state.profiles.len(),
        );
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([560.0, 640.0])
            .with_visible(false)
            .with_icon(icon()),
        ..Default::default()
    };
    eframe::run_native(
        "tpt-focus",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::FocusApp::new(
                cc,
                receiver,
                handle,
                runtime,
                source_slot,
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!("UI failed: {error}"))?;

    Ok(())
}

fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tpt-focus")
        .join("focus.toml")
}

fn default_db_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tpt-focus")
        .join("history.db")
}

/// Programmatic 32×32 app icon (purple dot + white bar) so no image assets
/// are needed.
fn icon() -> egui::IconData {
    const SIZE: usize = 32;
    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let inside = dx * dx + dy * dy <= 13.5 * 13.5;
            let bar = inside && (17..=23).contains(&y);
            rgba.extend_from_slice(if bar {
                &[255, 255, 255, 255]
            } else if inside {
                &[110, 70, 160, 255]
            } else {
                &[0, 0, 0, 0]
            });
        }
    }
    egui::IconData {
        width: SIZE as u32,
        height: SIZE as u32,
        rgba,
    }
}
