# tpt-focus user guide

tpt-focus is a **unified notification & focus center**: it ingests system
notifications, evaluates each one against your rules, and allows, mutes or
batches it — with a searchable local history.

## Concepts

| Concept | Meaning |
|---------|---------|
| **Rule** | Conditions + a decision (`allow`, `mute`, `batch`). Higher `priority` wins. |
| **Focus profile** | A named mode (e.g. *Deep Work*) you switch into manually; rules can match the active profile. |
| **Schedule** | A recurring time window that can auto-activate a profile. |
| **Digest** | Muted/batched notifications held per app; nothing is re-issued as a toast. |
| **History** | Every notification + decision, stored locally in SQLite with full-text search. |

## Install & first run

### Windows

1. Build or unpack a release (see `docs/windows-package-identity.md`).
2. Run `tpt-focus-tray.exe`. On first launch the OS asks for permission to
   read notifications (`RequestAccessAsync`).
   - **Allow** → live ingestion starts.
   - **Deny** → everything except live ingestion still works; the tray shows
     a banner and you can retry from *Settings*.
3. A tray icon appears. **Ctrl+Alt+N** (or left-click) opens the popup.

### Linux

1. Run `tpt-focus-tray` (systemd user service available, see below).
2. tpt-focus tries to claim `org.freedesktop.Notifications`:
   - If another daemon (GNOME Shell, dunst, mako, …) owns the name, the
     status line in *Settings* tells you; disable that daemon first
     (e.g. `systemctl --user disable --now mako.service`).
   - Otherwise tpt-focus *becomes* the notification daemon.
3. **Fullscreen context**: works on X11 and wlroots Wayland compositors.
   GNOME/KDE Wayland expose no window protocol by design; fullscreen rules
   will not fire there — use profiles and schedules instead.

## Rules

Rules live in the TOML config (`tpt-focus config path`), editable by hand or
via the CLI/UI:

```toml
[[rules]]
id = "mute-slack-when-fullscreen"
name = "Mute Slack while fullscreen"
action = "mute"
priority = 10

[[rules.conditions]]
type = "app"
apps = ["slack*", "teams"]

[[rules.conditions]]
type = "fullscreen"
equals = true

[[rules.conditions]]
type = "time"
start = "09:00"
end = "17:00"
weekdays = ["mon", "tue", "wed", "thu", "fri"]

[[rules.conditions]]
type = "profile"
profiles = ["Deep Work"]

[[rules.conditions]]
type = "urgency"
urgencies = ["critical"]
```

- Conditions combine with **AND** by default (`match_mode = "any"` for OR).
- A rule with **no conditions** matches everything (catch-all).
- An empty `weekdays` list means every day; `start > end` crosses midnight.
- Time conditions use the local wall clock (DST-safe).
- Critical notifications usually deserve a high-priority allow rule — the
  starter config ships one.

### Testing a rule

```sh
tpt-focus evaluate --app slack --fullscreen        # what would happen now?
tpt-focus evaluate --app mail --time 23:30         # as if it were 23:30
```

## CLI cheat sheet

```sh
tpt-focus config init                 # starter configuration
tpt-focus config validate             # sanity-check the TOML
tpt-focus rules list                  # rules in evaluation order
tpt-focus rules add quick --action mute --app "discord*"
tpt-focus rules disable quick
tpt-focus profiles list
tpt-focus profiles add Deep --default mute
tpt-focus schedules add lunch --start 12:00 --end 13:00 --profile Meetings
tpt-focus history list --app slack --limit 20
tpt-focus history search "standup"
tpt-focus history digest --minutes 60 # what got held back recently
tpt-focus history export backup.json  # / import backup.json
tpt-focus daemon                      # headless engine (no UI)
```

Global overrides: `--config <file>` and `--db <file>`.

## Tray & popup

- **Tray menu**: open/hide popup, History, Digest, Settings, focus-profile
  submenu, Quit.
- **History page**: full-text search, per-entry mark-read/delete, and
  dismiss-through to the OS where supported.
- **Digest page**: per-app groups of currently held notifications; *open*
  marks the group read.
- **Rules/Profiles pages**: quick editors for app-pattern rules and profiles.
- **Settings page**: Windows consent banner, config path, capabilities.

## systemd user service (Linux)

`packaging/linux/tpt-focus.service` (installed by the .deb/.rpm, or copy to
`~/.config/systemd/user/`):

```sh
systemctl --user enable --now tpt-focus
journalctl --user -u tpt-focus -f
```

## Data locations

| What | Where |
|------|-------|
| Config | `<config dir>/tpt-focus/focus.toml` |
| History | `<data dir>/tpt-focus/history.db` |

(`<config dir>` / `<data dir>` are the platform standard user dirs.)

Retention defaults: 30 days, 20 000 notifications, applied hourly.

## Privacy

Everything stays on your machine: one SQLite database and one TOML file.
No telemetry, no network access.
