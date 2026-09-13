# Reaffirm the bundled Syncthing sidecar over Tailscale

- **Status**: Accepted
- **Date**: 2026-09-13
- **Reaffirms**: ADR-0002

Second Brain will bundle Syncthing as an app-managed sidecar for machine-to-machine vault
sync. Tailscale is a user-installed prerequisite and supplies the private network path, but
Tailnet membership never grants vault access: the user must explicitly pair the two
Syncthing device identities. The app owns a private sidecar process and generated
configuration without adopting or changing any separately installed Syncthing instance.

The alternative was to retain the authored WebDAV engine already present in the codebase.
That would save near-term implementation work, but it would make this project permanently
responsible for remote-path validation, authentication transport, deletion and move
reconciliation, conflict semantics, concurrent runs, and interruption recovery. The
pre-alpha audit found path-traversal and plaintext-credential P1 defects in that engine.
We accept the sidecar's packaging, supervision, pairing, and cross-platform verification
cost instead of owning a bespoke file-sync protocol.

The synced boundary is the vault's user and shared state: notes, attachments, trash,
history, staging, vault identity, shared vault settings, and Notion identity records.
Search and semantic indexes, credentials, device configuration, transaction manifests,
repair state, backups, and logs remain machine-local. Syncthing conflict copies are kept
but excluded from ordinary notes, search, graphs, PARA counts, and Notion publication until
the user resolves them. Local work remains available offline; synchronization resumes when
both explicitly paired devices are reachable.

WebDAV is removed from production commands, scheduling, settings, and client construction.
Migration detects legacy configuration, explains the change, and removes stored WebDAV
credentials once they are no longer needed for recovery. Packages B (#82) and D (#84) own
that removal and the sidecar implementation; package C (#83) supplies bulk-mutation
coordination and checked pre-incoming-change backups.
