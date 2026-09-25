# Archon support — design only (v1)

Archon is TPT Solutions' capability-gated kernel IPC platform
([tpt-archon](https://github.com/tpt-solutions/tpt-archon)). When an Archon
sink is present, notifications become *kernel-mediated messages*: apps that
want to notify must hold a `notify` capability, and tpt-focus can enforce
rules at the IPC boundary — a muted app's message is dropped before it ever
reaches a notification service.

For v1 this is design-only. The core engine is deliberately structured so
Archon becomes two small implementations of existing traits, not a fork of
the pipeline.

## Where Archon plugs in

```
                 ┌────────────────────────────┐
  OS toasts ───▶ │ NotificationSource (WinRT) │──┐
  D-Bus Notify ─▶│ NotificationSource (dbus)  │  │   ┌──────────────┐
                 └────────────────────────────┘  ├─▶│   Pipeline   │──▶ decision
                 ┌────────────────────────────┐  │   └──────────────┘
  Archon IPC ───▶│ ArchonNotificationSource   │──┘          │
                 └────────────────────────────┘             ▼
                 ┌────────────────────────────┐      allow / mute / batch
  Archon IPC ───▶│ ArchonContextProvider      │      (history + digest)
                 └────────────────────────────┘
```

* `ArchonNotificationSource: NotificationSource` — receives
  `notification.send` messages over the `tpt-archon-bridge` and feeds the
  pipeline. `dismiss()` maps to bridge `notification.revoke`.
* `ArchonContextProvider: ContextProvider` — snapshots the foreground app /
  fullscreen state from Archon's window-manager channel instead of X11/Wayland
  polling; on Archon the kernel already knows the sender of every message, so
  rule conditions like `App` can match on the *authoritative* capability id.

## Bridge IPC message shapes (draft)

Carrier: Archon's existing bridge transport (native, capability-gated
channels). Payloads are CBOR-encoded envelopes:

```cbor
NotificationSend {
    seq: u64,                 // monotonic per sender
    sender: CapabilityId,     // e.g. "archon:cap:notify:com.slack.Slack"
    urgency: u8,              // 0 low, 1 normal, 2 critical
    title: string,
    body: string,
    actions: [ { key: string, label: string } ],
    replaces: Option<u64>,    // idempotent update
}

NotificationRevoke { sender: CapabilityId, seq: u64 }

NotificationAck { status: "shown" | "suppressed", reason: string }

ContextEvent {                 // pushed by Archon, not polled
    foreground: Option<CapabilityId>,
    fullscreen: bool,
}
```

Mapping into the core model is mechanical: `CapabilityId` → `AppIdentity`,
`seq` → `NotificationId` (`archon:{seq}`), `urgency` → `Urgency`.

## Capability gating model

- Granting an app a `notify` capability is what makes it *able* to notify at
  all; tpt-focus never sees unauthorised traffic.
- Revocation is enforced by the kernel, so a "mute Slack" rule can optionally
  be promoted to "revoke Slack's notify capability" — a stronger action than
  anything the OS notification stack offers.
- tpt-focus itself runs under a `sink` capability: apps' notifications are
  routed to it because Archon's policy says so, not because of listener APIs.

## Open questions (for the tpt-archon team)

1. **Bridge API shape** — is the envelope above acceptable, and does the
   bridge support the `NotificationAck` round-trip so senders can learn their
   message was suppressed?
2. **Grant/revocation model** — can tpt-focus request a *scoped* grant
   ("revoke notify for app X") or does every revocation need an explicit user
   policy action in Archon?
3. **Performance targets** — what is the budget for sink processing per
   message? tpt-focus's per-notification cost is a rule evaluation plus an
   SQLite insert (typically < 1 ms); Archon may require harder bounds.

## Post-v1 plan

Behind an `archon` Cargo feature flag:

```
core/src/platform/archon/mod.rs      # bridge transport + capability discovery
core/src/platform/archon/source.rs   # NotificationSource impl
core/src/platform/archon/context.rs  # ContextProvider impl
```

Selection order once it lands: Archon sink present → use it; else
WinRT/D-Bus; else degraded. No changes to the pipeline, config format or UI.
