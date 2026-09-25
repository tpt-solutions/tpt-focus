# tpt-focus

**Unified Notification & Focus Center** — a granular, rule-based notification
manager that batches alerts, mutes specific apps based on context (e.g. *"mute
Slack when my IDE is fullscreen"*), and gives you a searchable notification
history.

Notifications are chaotic across every platform. Apps spam users, and OS-level
"Focus"/"Do Not Disturb" modes are usually far too blunt — they block
everything or nothing. `tpt-focus` sits between apps and your screen as a
single, capability-gated **notification sink**: every alert is ingested,
evaluated against your rules, then allowed, muted, or batched.

## Platform support

| Platform | v1 status | How it works |
|----------|-----------|--------------|
| **Windows 10/11** | Supported | WinRT `UserNotificationListener` (requires package identity), plus foreground/fullscreen context signals |
| **Linux (X11/Wayland)** | Supported | Becomes the `org.freedesktop.Notifications` D-Bus daemon, aggregating the freedesktop notification standard into one history center |
| **Archon** | Design only | Notifications passed via secure `tpt-archon-bridge` IPC; rules enforced natively at the kernel IPC level — apps physically cannot bypass the sink |

Archon reference: <https://github.com/tpt-solutions/tpt-archon>

## Repository layout

```
tpt-focus/
├── core/       # Engine: data model, rule pipeline, profiles, storage
├── cli/        # `tpt-focus` command-line tool (rules, profiles, history)
├── tray-app/   # Tray icon + popup UI, platform notification sink host
└── docs/       # Design docs, user guide, Archon design
```

## Building

Requires Rust 1.85+ (pinned in `rust-toolchain.toml`).

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Configuration

Rules, focus profiles and schedules live in a TOML file; notification history
is kept in a local SQLite database with an FTS5 index for full-text search.

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
