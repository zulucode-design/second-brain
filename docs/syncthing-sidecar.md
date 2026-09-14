# Syncthing sidecar operations and verification

This document is the operational record for issue #84. The product contract remains
`SPEC.md` §8 and ADR-0009.

## Supply and packaging

The app pins Syncthing `v2.1.5`. `pnpm sidecar:prepare` downloads the official archive,
checks its SHA-256 digest, and writes the target-suffixed executable expected by Tauri:

| Target | Official archive SHA-256 |
| --- | --- |
| Linux x86-64 | `3d222b609f7ab2944e02748cb10488b4160d446b49e0eafc107ef2a525ab3486` |
| Windows x86-64 | `39571e4d0900c2a2cab14c0b170f49751340a869e49734ccc8079d9b98a7974b` |

`pnpm tauri:dev` and `pnpm tauri:build` run that preparation and merge
`src-tauri/tauri.sidecar.conf.json`, whose `externalBin` entry bundles the executable. The
downloaded executable and version stamp are build artifacts and are not committed. Updating
Syncthing is an app release: update the pinned version and both official digests together,
exercise this protocol, then ship a new app. Syncthing self-upgrade is disabled.

## Ownership and lifecycle

The app creates a private Syncthing home outside the vault and controls its loopback REST API
with a random machine-local API key. After generation and before every first `serve`, it
durably rewrites and validates the pinned configuration schema; a missing safety field stops
startup rather than falling back to a Syncthing default. It then starts the process paused.
The key is supplied through the process environment rather than the command line.
Syncthing logs, certificates, its database, API key, pairing state, and process control state
are machine-local. They never enter the synced vault.
Per-vault navigation, window, and list state is also machine-local. Older
`.helixnotes/state.json` files are migrated out on open, and legacy conflict copies
of that device-only file are removed so absolute paths from one OS cannot reach the other.

Enable restores the sidecar for the active vault. Disable and vault switching request an
authenticated shutdown and kill a process that does not exit promptly. Unexpected exits are
restarted at most three times. A minimal watchdog process receives the API key through its
environment and requests authenticated sidecar shutdown if the app process disappears; this
also bounds cleanup after a forced app termination that cannot run normal Tauri teardown. A
five-minute scheduler requests guarded sync runs on shared UTC boundaries
so independently launched machines overlap; it does not leave the folder or paired device
continuously resumed. An immediate manual transfer requires pressing Sync now on both
machines within the 30-second reachability window.

The REST GUI listens only on `127.0.0.1`. Data transfer listens only on the local Tailscale
IPv4 address. Global discovery, local discovery, relays, NAT traversal, usage reporting, and
crash reporting are disabled. The app does not install, configure, or supervise Tailscale.

## Pairing and vault bootstrap

Pairing is explicit and bilateral. Each person copies the other machine's Syncthing device
ID, the shared vault ID, a human name, and its `100.64.0.0/10` Tailscale IPv4 address into
Settings. Tailnet membership never authorizes a device and folder auto-accept is disabled.

Both machines must start from a copy of the same vault, including `.helixnotes/vault_id`.
Pairing fails closed when the vault IDs differ. This prevents two unrelated vaults from
being silently combined. Only the active vault folder is configured in the app-owned
Syncthing instance.

## Guarded batch contract

Each manual or scheduled run uses the shared bulk-mutation lease from issue #83. The order
is fixed:

1. exclude other bulk and note mutations and suppress watcher delivery;
2. create an identifiable full-vault pre-sync backup outside the vault, always including
   attachments; abort without resuming Syncthing if this fails;
3. resume the configured folder and paired device, require Tailscale reachability, request a
   scan, and wait for two consecutive bilateral completion observations: the local database
   must contain the peer's remote sequence and need nothing, while peer completion must report
   a valid shared folder and need no items, deletions, or bytes;
4. latch bilateral completion and keep the connection resumed for a short handoff grace so the
   peer can confirm the same state before either machine pauses;
5. re-pause both folder and device on success or error (with a drop guard as a second cleanup
   path);
6. reconcile the shared settings projection, keyword index, and semantic index;
7. release watcher suppression and emit exactly one `success`, `changed-incomplete`, or
   `failure` terminal outcome.

The app owns the guard, backup, terminal event, projections, and conflict UI. Syncthing owns
file reconciliation, delete/move propagation, delayed arrival, and conflict-copy creation.

## Conflicts

Syncthing conflict copies use
`<stem>.sync-conflict-YYYYMMDD-HHMMSS-<device>.<extension>`. Markdown copies are paired with
their original in Settings. Until resolved they are excluded from note enumeration, PARA
and notebook counts, keyword and semantic search, graph data, and Notion publication.
Choosing either version archives the other under `.helixnotes/trash/`; no version is silently
discarded. Keyword and semantic watcher batching re-checks exclusions when a path settles,
not only when its filesystem event first arrives; Syncthing can rename a queued path into a
conflict copy before that later read.

## Real-machine verification protocol

Run this on the packaged Windows and Fedora builds over Tailscale, using disposable copies
of one vault. Record app version, Syncthing version/device IDs, OS versions, and timestamps.

1. Enable both sidecars and pair both directions. Confirm an unpaired Tailnet device is not
   added and both GUIs remain loopback-only.
2. Create a note and attachment on A. Run sync on A and B; open and search for both on B.
3. With B offline, move a note between PARA categories and delete another on A. Reconnect;
   confirm the move has one destination and the deletion does not resurrect.
4. Reference an attachment before B receives its bytes. Confirm the UI treats it as delayed,
   then sync and open it normally.
5. Edit the same note differently while disconnected. Reconnect; confirm Settings shows the
   paired versions while ordinary notes, counts, search, graph, and Notion omit the copy.
   Exercise both resolution choices.
6. During an incoming batch, terminate Syncthing and separately terminate the app. Confirm a
   pre-sync backup exists, restart supervision is bounded, the next run converges, watcher
   delivery resumes, and the outcome does not claim clean success after partial change.
7. Disable sync and switch vaults. Confirm the old process/folder stops and no data from one
   vault appears in the other.

The Linux build/test and local process smoke checks can establish packaging and control-flow
correctness, but they do not replace this two-real-machine gate.
