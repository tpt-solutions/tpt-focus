# tpt-focus — Project TODO

Unified Notification & Focus Center. Rust core engine + CLI, native tray+popup
GUI (Windows & Linux), dual-licensed MIT/Apache-2.0, by TPT Solutions.
Targets Windows and Linux for v1 — Windows via WinRT UserNotificationListener
(package identity required), Linux by becoming the org.freedesktop.Notifications
D-Bus daemon. Archon (capability-gated kernel IPC notification sink) is
design-only until the core is solid.

Reference: Archon platform — https://github.com/tpt-solutions/tpt-archon

---

## Status Summary

| Phase | Area | Progress |
|-------|------|----------|
| 0 | Project Setup & Licensing | ❌ Not Started |
| 1 | Core Engine: Data Model, Pipeline & Rules | ❌ Not Started |
| 2 | Local Storage & History (SQLite + FTS) | ❌ Not Started |
| 3 | Windows Package Identity & Signing | ❌ Not Started |
| 4 | Windows Notification Listener & Context Signals | ❌ Not Started |
| 5 | Linux D-Bus Notification Daemon | ❌ Not Started |
| 6 | Linux Context Signals (X11/Wayland) | ❌ Not Started |
| 7 | Focus Profiles & Scheduling | ❌ Not Started |
| 8 | Tray + Popup GUI | ❌ Not Started |
| 9 | Archon Support (Design Only) | ❌ Not Started |
| 10 | Testing, Hardening & Release | ❌ Not Started |

---

## Phase 0 — Project Setup & Licensing
- [ ] Init git repo, `.gitignore` (Rust + OS-specific + Windows signing/packaging artifacts)
- [ ] Dual license: `LICENSE-MIT` + `LICENSE-APACHE`, dual-license note
- [ ] Cargo workspace: `core/` (engine + platform backends), `tray-app/` (Windows+Linux tray/popup UI + OS sink host), `cli/` (rule/profile scripting), shared `workspace.package`
- [ ] `rust-toolchain.toml` (MSRV pin), `rustfmt.toml`, `clippy.toml`
- [ ] README.md, CONTRIBUTING.md, CI (GitHub Actions, Windows/Linux build+test matrix)

## Phase 1 — Core Engine: Data Model, Pipeline & Rules
- [ ] Notification model (id, source app, title/body, urgency, timestamp, actions, icon ref)
- [ ] `trait NotificationSource` (start/stop, on_notification callback, dismiss(id), capability query) + mock source for tests
- [ ] `trait ContextProvider` (foreground app identity, fullscreen bool, active profile) + mock provider — real impls land in Phases 4 & 6
- [ ] Rule model: conditions (app identity, fullscreen-context, active-profile, time-of-day/schedule), actions (allow, mute, batch)
- [ ] Rule evaluation engine (precedence/ordering), Focus Profile model (named, manual toggle)
- [ ] Config format (TOML) for rules/profiles/schedules — load/save/validate
- [ ] Core unit tests: rule matching against mock context/source, config round-trip

## Phase 2 — Local Storage & History (SQLite + FTS)
- [ ] rusqlite (bundled) schema: notifications, rule_decisions, profiles, schedule config
- [ ] FTS5 index on title/body for search
- [ ] Retention/pruning (max-age, max-count), history query API (filter by app/date/decision)
- [ ] Backup/export (JSON), schema-version migrations
- [ ] Storage unit tests: insert/query/prune/FTS round-trip

## Phase 3 — Windows Package Identity & Signing
- [ ] Choose sparse package vs. full MSIX for `UserNotificationListener` eligibility (unpackaged Win32 processes cannot call this API — gates all of Phase 4)
- [ ] Code-signing cert (self-signed for dev vs. trusted CA for distribution), documented dev-cert trust flow for contributors/CI
- [ ] Sparse package manifest (AppxManifest.xml) or MSIX project, package identity registration
- [ ] `RequestAccessAsync` consent-flow UX (first-run prompt)
- [ ] Fallback UX when consent denied or listener unavailable (degrade gracefully, no crash)

## Phase 4 — Windows Notification Listener & Context Signals
- [ ] `UserNotificationListener.Current` via `windows-rs`, subscribe to `NotificationChanged`, map WinRT toast → core model
- [ ] Dismiss-through (`RemoveNotification`) wired to core dismiss action
- [ ] Foreground+fullscreen detection: `GetForegroundWindow` + `GetMonitorInfo` bounds heuristic
- [ ] Wire real Windows `ContextProvider` into rule engine
- [ ] Mockable WinRT boundary for CI (no real package identity in CI)
- [ ] End-to-end test: "mute Slack when IDE fullscreen" on Windows

## Phase 5 — Linux D-Bus Notification Daemon
- [ ] Implement `org.freedesktop.Notifications`: `Notify`, `CloseNotification`, `GetCapabilities`, `GetServerInformation`, signals `NotificationClosed`/`ActionInvoked`
- [ ] Claim well-known bus name; detect existing owner (GNOME Shell/dunst/mako)
- [ ] Conflict-resolution UX: per-DE instructions to disable the competing daemon (docs + in-app banner)
- [ ] Popup notification rendering (egui bubble: position, timeout, action buttons)
- [ ] Route incoming `Notify` calls through rule engine before render/history
- [ ] systemd user unit (autostart, restart-on-crash)

## Phase 6 — Linux Context Signals (X11/Wayland)
- [ ] X11: `_NET_ACTIVE_WINDOW` + `_NET_WM_STATE_FULLSCREEN`/geometry heuristic via `x11rb`
- [ ] Wayland: `wlr-foreign-toplevel-management` for wlroots compositors (Sway)
- [ ] GNOME/KDE Wayland fallback: no window-introspection protocol exists by design — document limitation, fall back to manual profiles
- [ ] Wire real Linux `ContextProvider` into rule engine (runtime backend selection per session type)
- [ ] End-to-end test: "mute Slack when IDE fullscreen" on X11; documented Wayland fallback path

## Phase 7 — Focus Profiles & Scheduling
- [ ] Manual focus profile CRUD (named, user-defined), active-profile quick-switch
- [ ] Time-of-day/schedule rules: weekday + time-range parser, recurring activation, timezone/DST handling
- [ ] Batch/digest accumulation: muted/batched notifications grouped by app+time-window in history (no OS toast re-issued)
- [ ] Rule precedence tests across combined condition types (profile + schedule + fullscreen)

## Phase 8 — Tray + Popup GUI (Windows & Linux)
- [ ] Tray icon + context menu (quick profile switch, pause/mute-all, open history)
- [ ] Global hotkey to open popup
- [ ] Popup history view: scrollable, FTS search, per-app category colors, batch/digest grouping, dismiss/open/mark-read
- [ ] Rule & profile editor UI (app-identity builder, schedule builder, profile management)
- [ ] Settings window: Windows consent-status banner, Linux daemon-conflict banner, general prefs
- [ ] egui/eframe app shell shared across platform entry points

## Phase 9 — Archon Support (Design Only for v1)
- [ ] `docs/archon-design.md`: `NotificationSink` trait design, tpt-archon-bridge IPC message shapes, capability-gated sink concept
- [ ] Open Questions (pending tpt-archon team): bridge IPC API shape, capability grant/revocation model, performance targets
- [ ] Post-v1 `archon` Cargo feature flag plan (`ArchonNotificationSource`/`ArchonContextProvider` stubs)

## Phase 10 — Testing, Hardening & Release
- [ ] Core/storage/rule-engine unit test suite; mocked WinRT/D-Bus boundaries for CI
- [ ] Manual QA matrix: Windows 10/11 (consent granted + denied), GNOME/KDE/Sway Linux (conflict + no-conflict)
- [ ] Windows packaging: MSIX/sparse package build + signing pipeline, install/uninstall
- [ ] Linux packaging: .deb/.rpm/AppImage, systemd user unit install
- [ ] docs/user-guide.md, docs/CHANGELOG.md, CI (clippy/fmt/test matrix)
- [ ] Cut v1.0.0, publish binaries to GitHub Releases
