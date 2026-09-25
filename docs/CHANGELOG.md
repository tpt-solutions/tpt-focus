# Changelog

All notable changes to tpt-focus are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Core engine**: notification model, rule pipeline (app / fullscreen /
  profile / time-window / urgency conditions, AND|OR match modes,
  priority ordering), focus profiles, TOML configuration with validation,
  schedule controller that activates profiles on recurring time windows.
- **History**: SQLite (bundled) storage with FTS5 full-text search,
  retention pruning (max-age / max-count), JSON export/import, per-app
  digest queries.
- **Digest collector**: suppressed notifications grouped per app until
  their window matures; never re-issued as toasts.
- **Windows backend**: WinRT `UserNotificationListener` source with
  `NotificationChanged` events and an automatic diffing-poll fallback
  (some sparse-package deployments reject the event with `0x80070490`),
  consent state machine with persisted outcome and degraded-mode banners,
  Win32 foreground-app + fullscreen context provider, dismiss-through via
  `RemoveNotification`.
- **Windows packaging**: sparse package decision record, `AppxManifest.xml`
  (incl. the required `userNotificationListener` capability), dev-certificate
  and package-deploy scripts, CI-friendly consent-degradation tests.
- **Linux backend**: full `org.freedesktop.Notifications` daemon (Notify,
  CloseNotification, GetCapabilities, GetServerInformation,
  `NotificationClosed`/`ActionInvoked` signals, replaces-id, hints incl.
  urgency/desktop-entry/images) via zbus; bus-name conflict detection with
  actionable error messages; X11 context provider (`_NET_ACTIVE_WINDOW`,
  `_NET_WM_STATE`, `WM_CLASS` polling) and wlroots Wayland provider
  (`wlr-foreign-toplevel-management`) with a documented static fallback for
  GNOME/KDE Wayland.
- **CLI (`tpt-focus`)**: `config init/show/validate/path`, `rules
  list/add/remove/enable/disable`, `profiles list/add/remove`,
  `schedules list/add/remove/toggle`, `history list/search/read/prune/
  export/import/clear/digest`, `evaluate` (dry-run a synthetic notification
  against the live context), `daemon` (headless engine), `--json` output,
  `--config`/`--db` overrides.
- **Tray app (`tpt-focus-tray`)**: egui popup with history search,
  digest, rule/profile editors and settings; tray icon with profile
  quick-switch (native Win32 tray on Windows, SNI via ksni on Linux);
  Ctrl+Alt+N global hotkey (Windows); Linux notification bubbles with
  action buttons and expiry; Windows consent flow in the background.
- **Docs**: user guide, Windows package identity decision record, Archon
  integration design (capability-gated sink, bridge envelopes, open
  questions), this changelog.
- **CI**: format/clippy/test matrix on Windows and Linux, release builds.

### Platform notes

- Windows 10 1809+ is required for `UserNotificationListener`; package
  identity is mandatory and provided via a sparse package.
- Linux builds are pure Rust (no GTK/libappindicator dependency); the tray
  uses StatusNotifierItem, which GNOME only exposes with the AppIndicator
  extension enabled.
