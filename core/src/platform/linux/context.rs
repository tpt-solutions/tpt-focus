//! Linux context signals (Phase 6): foreground application and fullscreen
//! state on X11 and wlroots Wayland sessions.
//!
//! Backend selection follows the session:
//!
//! | Session                             | Backend                                        |
//! |-------------------------------------|------------------------------------------------|
//! | X11 (`DISPLAY`)                     | `_NET_ACTIVE_WINDOW` + `_NET_WM_STATE` polling |
//! | wlroots Wayland (`WAYLAND_DISPLAY`) | `wlr-foreign-toplevel-management`              |
//! | GNOME/KDE Wayland                   | static provider (documented limitation)        |
//!
//! GNOME and KDE deliberately expose no window-introspection protocol to
//! Wayland clients, so on those compositors fullscreen context is unknown
//! and rules should rely on profiles and schedules instead. Poller threads
//! keep a cache that the synchronous [`ContextProvider::snapshot`] reads,
//! so rule evaluation never blocks on the display server.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::{Local, Utc};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::context::{ContextProvider, ContextSnapshot};
use crate::model::AppIdentity;
use crate::profile::ActiveProfile;

/// How often the pollers refresh their cache.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Build the best context provider available in this session.
pub fn best_provider(active_profile: Arc<ActiveProfile>) -> Arc<dyn ContextProvider> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Some(provider) = WaylandContextProvider::new(Arc::clone(&active_profile)) {
            return Arc::new(provider);
        }
        // XWayland still serves the X11 atoms, so try X11 before giving up.
    }
    if std::env::var_os("DISPLAY").is_some() {
        if let Some(provider) = X11ContextProvider::new(Arc::clone(&active_profile)) {
            return Arc::new(provider);
        }
    }

    tracing::info!(
        "no window introspection available in this session; fullscreen \
         rules need X11 or a wlroots compositor"
    );
    Arc::new(StaticContextProvider::new(active_profile))
}

/// Cache shared between a poller thread and `snapshot`.
#[derive(Debug, Clone, Default)]
struct ToplevelCache {
    foreground_app: Option<AppIdentity>,
    fullscreen: bool,
}

/// Cache + poller thread, stopped on drop.
struct Poller {
    cache: Arc<Mutex<ToplevelCache>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Poller {
    /// Spawn `body(cache, stop)` on a named thread; `None` when the thread
    /// could not be spawned.
    fn spawn(
        name: &str,
        body: impl FnOnce(Arc<Mutex<ToplevelCache>>, Arc<AtomicBool>) + Send + 'static,
    ) -> Option<Self> {
        let cache = Arc::new(Mutex::new(ToplevelCache::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let cache_thread = Arc::clone(&cache);
        let stop_thread = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name(name.to_string())
            .spawn(move || body(cache_thread, stop_thread))
            .ok()?;

        Some(Self {
            cache,
            stop,
            handle: Some(handle),
        })
    }

    fn snapshot(&self) -> ToplevelCache {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Assemble a snapshot from the poller cache and the profile toggle.
fn build_snapshot(active_profile: &ActiveProfile, cached: ToplevelCache) -> ContextSnapshot {
    let now = Utc::now();
    ContextSnapshot {
        foreground_app: cached.foreground_app,
        fullscreen: cached.fullscreen,
        active_profile: active_profile.active(),
        captured_at: now,
        local_time: now.with_timezone(&Local).naive_local(),
    }
}

// ---------------------------------------------------------------- X11

/// X11 backend: polls `_NET_ACTIVE_WINDOW` and `_NET_WM_STATE_FULLSCREEN`.
pub struct X11ContextProvider {
    active_profile: Arc<ActiveProfile>,
    poller: Option<Poller>,
}

impl X11ContextProvider {
    pub fn new(active_profile: Arc<ActiveProfile>) -> Option<Self> {
        let poller = Poller::spawn("tpt-focus-x11-context", x11_poll_loop);
        Some(Self {
            active_profile,
            poller,
        })
    }
}

impl ContextProvider for X11ContextProvider {
    fn snapshot(&self) -> ContextSnapshot {
        let cached = self
            .poller
            .as_ref()
            .map(|poller| poller.snapshot())
            .unwrap_or_default();
        build_snapshot(&self.active_profile, cached)
    }

    fn name(&self) -> &str {
        "linux-x11"
    }
}

fn x11_poll_loop(cache: Arc<Mutex<ToplevelCache>>, stop: Arc<AtomicBool>) {
    // (Re)connect until stopped; the display server may start after us.
    while !stop.load(Ordering::SeqCst) {
        let Ok((connection, screen)) = x11rb::connect(None) else {
            thread::sleep(Duration::from_secs(2));
            continue;
        };
        let Some(root) = connection
            .setup()
            .roots
            .get(screen)
            .map(|screen| screen.root)
        else {
            return;
        };

        let atom = |name: &[u8]| {
            connection
                .intern_atom(false, name)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| reply.atom)
                .unwrap_or(0)
        };
        let active_atom = atom(b"_NET_ACTIVE_WINDOW");
        let state_atom = atom(b"_NET_WM_STATE");
        let fullscreen_atom = atom(b"_NET_WM_STATE_FULLSCREEN");
        let class_atom = atom(b"WM_CLASS");

        while !stop.load(Ordering::SeqCst) {
            let state = read_x11_state(
                &connection,
                root,
                active_atom,
                state_atom,
                fullscreen_atom,
                class_atom,
            );
            if let Ok(mut guard) = cache.lock() {
                *guard = state;
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

fn read_x11_state<C: x11rb::connection::Connection>(
    connection: &C,
    root: u32,
    active_atom: u32,
    state_atom: u32,
    fullscreen_atom: u32,
    class_atom: u32,
) -> ToplevelCache {
    use x11rb::protocol::xproto::AtomEnum;

    let mut cache = ToplevelCache::default();

    let active = connection
        .get_property(false, root, active_atom, AtomEnum::WINDOW, 0, 1)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| {
            let mut values = reply.value32()?;
            values.next()
        });

    let Some(window) = active else {
        return cache;
    };

    // `_NET_WM_STATE` is a list of atoms; fullscreen is one of them.
    cache.fullscreen = connection
        .get_property(false, window, state_atom, AtomEnum::ATOM, 0, 64)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| {
            let mut atoms = reply.value32()?;
            Some(atoms.any(|atom| atom == fullscreen_atom))
        })
        .unwrap_or(false);

    // `WM_CLASS` holds two NUL-terminated strings: instance, then class.
    cache.foreground_app = connection
        .get_property(false, window, class_atom, AtomEnum::STRING, 0, 256)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| parse_wm_class(&reply.value));

    cache
}

/// `WM_CLASS` = `"instance\0class\0"`; the class name is the stable id.
fn parse_wm_class(value: &[u8]) -> Option<AppIdentity> {
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_string();
    let mut parts = value
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty());

    let instance = parts.next()?;
    let class = parts.next().unwrap_or(instance);

    let class = text(class);
    if class.is_empty() {
        return None;
    }

    let instance = text(instance);
    let display_name = (!instance.is_empty() && instance != class).then_some(instance);

    Some(AppIdentity {
        app_id: class,
        display_name,
    })
}

// ------------------------------------------------------------- Wayland

/// wlroots Wayland backend via `wlr-foreign-toplevel-management`.
pub struct WaylandContextProvider {
    active_profile: Arc<ActiveProfile>,
    poller: Option<Poller>,
}

impl WaylandContextProvider {
    pub fn new(active_profile: Arc<ActiveProfile>) -> Option<Self> {
        let poller = Poller::spawn("tpt-focus-wayland-context", wayland_poll_loop);
        Some(Self {
            active_profile,
            poller,
        })
    }
}

impl ContextProvider for WaylandContextProvider {
    fn snapshot(&self) -> ContextSnapshot {
        let cached = self
            .poller
            .as_ref()
            .map(|poller| poller.snapshot())
            .unwrap_or_default();
        build_snapshot(&self.active_profile, cached)
    }

    fn name(&self) -> &str {
        "linux-wayland"
    }
}

fn wayland_poll_loop(cache: Arc<Mutex<ToplevelCache>>, stop: Arc<AtomicBool>) {
    if let Err(error) = wayland::run(cache, &stop) {
        tracing::info!(
            error = %error,
            "wlr-foreign-toplevel-management unavailable; GNOME/KDE \
             Wayland expose no window protocol by design"
        );
    }
}

/// All the `wayland-client` specifics, kept in one place.
mod wayland {
    use super::ToplevelCache;
    use crate::model::AppIdentity;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::{Connection, Dispatch, QueueHandle};
    use wayland_protocols_wlr::foreign_toplevel::v1::client::{
        zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
        zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
    };

    /// `zwlr_foreign_toplevel_handle_v1.state` wire values.
    mod state {
        pub const MAXIMIZED: u32 = 0;
        pub const MINIMIZED: u32 = 1;
        pub const ACTIVATED: u32 = 2;
        pub const FULLSCREEN: u32 = 3;
    }

    #[derive(Debug, Default, Clone)]
    struct Toplevel {
        app_id: String,
        title: String,
        states: HashSet<u32>,
    }

    impl Toplevel {
        fn activated(&self) -> bool {
            self.states.contains(&state::ACTIVATED)
        }

        fn fullscreen(&self) -> bool {
            self.states.contains(&state::FULLSCREEN)
        }

        fn identity(&self) -> Option<AppIdentity> {
            let app_id = self.app_id.trim();
            if app_id.is_empty() {
                return None;
            }
            let mut app = AppIdentity::new(app_id);
            if !self.title.is_empty() {
                app.display_name = Some(self.title.clone());
            }
            Some(app)
        }
    }

    /// Dispatch state: everything the event handlers touch.
    pub(super) struct Session {
        cache: Arc<Mutex<ToplevelCache>>,
        toplevels: HashMap<ZwlrForeignToplevelHandleV1, Toplevel>,
        manager: Option<ZwlrForeignToplevelManagerV1>,
    }

    impl Session {
        /// Publish the focused toplevel into the shared cache.
        fn publish(&mut self) {
            let focused = self
                .toplevels
                .values()
                .find(|toplevel| toplevel.activated());

            let fullscreen = focused.is_some_and(Toplevel::fullscreen);
            let foreground_app = focused.and_then(Toplevel::identity);

            let mut guard = match self.cache.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.foreground_app = foreground_app;
            guard.fullscreen = fullscreen;
        }
    }

    pub(super) fn run(cache: Arc<Mutex<ToplevelCache>>, stop: &AtomicBool) -> Result<(), String> {
        let connection =
            Connection::connect_to_env().map_err(|error| format!("connect: {error}"))?;
        let display = connection.display();
        let mut queue = connection.new_event_queue();
        let queue_handle = queue.handle();

        let mut session = Session {
            cache,
            toplevels: HashMap::new(),
            manager: None,
        };

        // The first roundtrip delivers the registry globals.
        let _registry = display.get_registry(&queue_handle, ());
        queue
            .roundtrip(&mut session)
            .map_err(|error| format!("registry roundtrip: {error}"))?;

        let Some(manager) = &session.manager else {
            return Err(
                "compositor does not advertise wlr-foreign-toplevel-management".to_string(),
            );
        };
        // This protocol version pushes handles via the `toplevel` event
        // (new_id); no explicit request is needed. One more roundtrip lets
        // the initial batch arrive.
        let _ = &manager;
        queue
            .roundtrip(&mut session)
            .map_err(|error| format!("toplevel roundtrip: {error}"))?;

        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            queue
                .roundtrip(&mut session)
                .map_err(|error| format!("dispatch: {error}"))?;
            session.publish();
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    impl Dispatch<WlRegistry, ()> for Session {
        fn event(
            state: &mut Self,
            registry: &WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            queue_handle: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                if interface == "zwlr_foreign_toplevel_manager_v1" {
                    state.manager = Some(registry.bind(name, version.min(3), queue_handle, ()));
                }
            }
        }
    }

    impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Session {
        fn event(
            state: &mut Self,
            _: &ZwlrForeignToplevelManagerV1,
            event: zwlr_foreign_toplevel_manager_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
                state.toplevels.entry(toplevel).or_default();
            }
        }
    }

    impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for Session {
        fn event(
            state: &mut Self,
            handle: &ZwlrForeignToplevelHandleV1,
            event: zwlr_foreign_toplevel_handle_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let Some(toplevel) = state.toplevels.get_mut(handle) else {
                return;
            };

            match event {
                zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                    toplevel.title = title;
                }
                zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                    toplevel.app_id = app_id;
                }
                zwlr_foreign_toplevel_handle_v1::Event::State { state: array } => {
                    toplevel.states = array
                        .chunks_exact(std::mem::size_of::<u32>())
                        .map(|bytes| u32::from_ne_bytes(bytes.try_into().unwrap()))
                        .collect();
                    let _ = (state::MAXIMIZED, state::MINIMIZED);
                }
                zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                    state.toplevels.remove(handle);
                    state.publish();
                }
                _ => {}
            }
        }
    }

    #[cfg(test)]
    fn probe_states(states: &[u32]) -> (bool, bool) {
        let set: HashSet<u32> = states.iter().copied().collect();
        (
            set.contains(&state::ACTIVATED),
            set.contains(&state::FULLSCREEN),
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn state_bits_map_to_flags() {
            assert_eq!(probe_states(&[2]), (true, false));
            assert_eq!(probe_states(&[0, 2, 3]), (true, true));
            assert_eq!(probe_states(&[]), (false, false));
            assert_eq!(probe_states(&[1]), (false, false));
        }

        #[test]
        fn identity_requires_an_app_id() {
            let mut toplevel = Toplevel::default();
            assert!(toplevel.identity().is_none());

            toplevel.app_id = "  ".into();
            assert!(toplevel.identity().is_none());

            toplevel.app_id = "slack".into();
            toplevel.title = " #general".into();
            let app = toplevel.identity().expect("identity");
            assert_eq!(app.app_id, "slack");
            assert_eq!(app.display_name.as_deref(), Some(" #general"));
        }
    }
}

// -------------------------------------------------------------- Static

/// Degraded provider for sessions that expose no window state
/// (GNOME/KDE Wayland). Rules still see profiles and schedules.
pub struct StaticContextProvider {
    active_profile: Arc<ActiveProfile>,
}

impl StaticContextProvider {
    pub fn new(active_profile: Arc<ActiveProfile>) -> Self {
        Self { active_profile }
    }
}

impl ContextProvider for StaticContextProvider {
    fn snapshot(&self) -> ContextSnapshot {
        build_snapshot(&self.active_profile, ToplevelCache::default())
    }

    fn name(&self) -> &str {
        "linux-static"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wm_class_prefers_the_class_name() {
        let app = parse_wm_class(b"slack\0Slack\0").expect("app");
        assert_eq!(app.app_id, "Slack");
        assert_eq!(app.display_name.as_deref(), Some("slack"));
    }

    #[test]
    fn wm_class_survives_a_missing_instance() {
        let app = parse_wm_class(b"\0code-oss\0").expect("app");
        assert_eq!(app.app_id, "code-oss");
        assert_eq!(app.display_name, None);
    }

    #[test]
    fn wm_class_rejects_garbage() {
        assert!(parse_wm_class(b"").is_none());
        assert!(parse_wm_class(b"\0\0").is_none());
    }

    #[test]
    fn static_provider_reports_only_the_profile() {
        let profile = Arc::new(ActiveProfile::new());
        profile.activate("Deep Work");
        let provider = StaticContextProvider::new(profile);

        let snapshot = provider.snapshot();
        assert_eq!(provider.name(), "linux-static");
        assert!(snapshot.foreground_app.is_none());
        assert!(!snapshot.fullscreen);
        assert_eq!(snapshot.active_profile.as_deref(), Some("Deep Work"));
    }

    #[test]
    fn x11_provider_exists_headless() {
        // CI has no X server: constructing must not panic, and `new` may
        // still succeed because the poller connects lazily.
        let provider = X11ContextProvider::new(Arc::new(ActiveProfile::new()));
        let snapshot = provider.as_ref().map(|provider| provider.snapshot());
        if let Some(snapshot) = snapshot {
            assert_eq!(snapshot.foreground_app, None);
            assert!(!snapshot.fullscreen);
        }
    }
}
