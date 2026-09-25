# Contributing to tpt-focus

Thanks for helping build the Unified Notification & Focus Center.

## Development setup

1. Install Rust 1.85 or later — `rust-toolchain.toml` pins the exact toolchain,
   so `rustup` will pick it up automatically on first build.
2. Clone the repo and build the workspace:

   ```sh
   git clone https://github.com/tpt-solutions/tpt-focus
   cd tpt-focus
   cargo build --workspace
   ```

## Making changes

Run the full local gate before pushing:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks on a Windows and a Linux matrix.

### Code conventions

- `#![forbid(unsafe_code)]` in `core/`. Platform FFI lives behind the
  platform-specific crates and must be reviewed carefully.
- No `unwrap()`/`expect()` on user- or platform-supplied data — return a
  typed error instead (`thiserror` in the core, `anyhow` at the edges).
- Public API of `tpt-focus-core` needs rustdoc comments and unit tests.
- Keep platform-specific code behind `cfg(...)` or dedicated modules so the
  core stays portable.

### Tests

- Unit tests live next to the code in `core/src/**`.
- Tests must not require package identity, a D-Bus session, or a display
  server — those boundaries are mocked (see `NotificationSource` /
  `ContextProvider` mocks).

## Commit messages

Short, imperative summaries ("Add FTS5 history search"), with a body
explaining *why* when the change isn't self-evident.

## Reporting bugs

Open an issue at <https://github.com/tpt-solutions/tpt-focus/issues> with:

- OS and version (Windows 10/11, GNOME/KDE/Sway, etc.)
- `tpt-focus --version` output
- Relevant logs (`RUST_LOG=debug tpt-focus ...`)

## Licensing

By contributing you agree that your contributions are dual licensed under
either the Apache-2.0 or MIT license, at the option of the recipient, matching
the rest of the project (see `LICENSE-APACHE` / `LICENSE-MIT`).
