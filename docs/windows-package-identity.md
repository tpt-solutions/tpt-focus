# Windows package identity & signing

`UserNotificationListener` — the WinRT API that lets `tpt-focus` observe
notifications on Windows — **requires package identity**. A plain unpackaged
Win32 process calling `UserNotificationListener::Current()` gets
`E_ACCESSDENIED`. Everything in Phase 4 is gated on the decisions in this
document.

## Decision: sparse package (not a full MSIX)

| Option | Verdict |
|--------|---------|
| Full MSIX wrapping the binaries | ❌ Every release must be re-packaged, binaries live inside the MSIX payload, install/uninstall churn |
| **Sparse package (manifest only)** | ✅ **Chosen** — `Add-AppxPackage -ExternalLocation` maps an identity onto the binaries we already ship |

A sparse package contains only `AppxManifest.xml` (plus logos). The package
identity is registered against an *external* location, so the existing
`cargo build --release` output keeps working unpackaged on disk and still
counts as the packaged app.

Manifest source: [`packaging/windows/AppxManifest.xml`](../packaging/windows/AppxManifest.xml).

### What identity gives us

- `UserNotificationListener::Current()` becomes callable
- `RequestAccessAsync()` shows the OS consent prompt the first time
- Toasts raised by *other* apps are visible to us through the listener

Package identity does **not** require the app to be installed from the Store,
and does not require MSIX containerisation of the process.

## Identity values

| Field | Value |
|-------|-------|
| `Identity/@Name` | `TPT.Focus` |
| `Identity/@Publisher` | `CN=TPT Solutions` |
| Version | `MAJOR.MINOR.PATCH.0` — keep in sync with `Cargo.toml` |

`Publisher` **must** exactly match the subject of the certificate used to
sign the manifest, otherwise `SignTool` validation and `Add-AppxPackage` fail
with `0x80073CF9` / `ERROR_INSTALL_FAILED`.

## Signing certificates

### Development (contributors, CI smoke tests)

A self-signed certificate is enough. Windows trusts it for package
deployment only after it is added to the *Trusted People* (or *Trusted Root*)
store of the user running the install.

```powershell
# One-time: create the dev identity certificate
./scripts/windows/new-dev-cert.ps1

# Then build + sign + deploy the sparse package
./scripts/windows/package-sparse.ps1 -DevCert
```

`new-dev-cert.ps1` writes:

- `build/certs/tpt-focus-dev.cer` — public cert (import for trust)
- `build/certs/tpt-focus-dev.pfx` — private key (**gitignored**), used to sign

Trust flow for a fresh contributor machine:

```powershell
Import-Certificate -FilePath build/certs/tpt-focus-dev.cer `
  -CertStoreLocation Cert:\CurrentUser\TrustedPeople\
```

CI runs the same script; the pfx never leaves the runner.

### Distribution (GitHub Releases)

Ship a package signed by a real code-signing certificate (CA-issued or an
Azure Trusted Signing / EWT account). The publisher string in the manifest is
changed to the CA subject before signing. Certificates are injected as CI
secrets — never committed.

## Consent flow

1. First run → `UserNotificationListener::RequestAccessAsync()`
2. `Allowed` → state persisted (`windows.listener_access = granted`), listener
   starts
3. `Denied` → state persisted as `denied`; the app runs in *degraded* mode:
   rules, profiles, history search and the tray all keep working, only live
   ingestion is off, and the tray shows a banner with a "Request again"
   button
4. No package identity at all → state `unavailable`, same degraded mode plus
   a link to this document

The state machine lives in `core::platform::windows::access` behind the
`NotificationAccess` trait so tests and Linux CI exercise it with the mock
implementation (no package identity required).

## Release packaging

`packaging/windows/package-sparse.ps1`:

1. `cargo build --release`
2. `MakeAppx pack /p tpt-focus.appx /f packaging/windows/AppxManifest.xml /d <staging>`
3. `SignTool sign /fd SHA256 /a /f <cert> tpt-focus.appx`
4. `Add-AppxPackage -Path tpt-focus.appx -ExternalLocation <install dir>`

Uninstall: `Get-AppxPackage TPT.Focus | Remove-AppxPackage` (binaries are left
untouched — they were never inside the package).

## Open questions

- Whether Store submission is wanted for v1 (would replace the CA cert with
  Store signing, keeping the same manifest)
- Minimum supported Windows build: currently `10.0.17763.0` (1809), the floor
  for `UserNotificationListener`
