# Log privacy and diagnostics

Package H (#88) requires a written redaction policy and a user-controlled diagnostic export
that excludes note contents and secrets by default. This document is the policy. The export
is not implemented yet; see "Diagnostic export" below.

## Where logs go

The backend uses `tauri-plugin-log` at `Info` level, configured in `src-tauri/src/lib.rs`: up to
5 MB per file, three rotated files kept, written to standard output and to the app log
directory (`%LOCALAPPDATA%\io.github.zulucodedesign.SecondBrain\logs` on Windows,
`~/.local/share/io.github.zulucodedesign.SecondBrain/logs` on Fedora). Logs are
machine-local and never enter the synced vault (SPEC §8). Frontend `console` output is not
forwarded to the log file.

## Rules

1. **Never log secrets.** Provider API keys, the Notion token, the Syncthing API key, and
   pairing material stay in the OS keyring or process environment. No log call may format a
   request header, credential, or configuration struct that contains one.
2. **Never log note bodies.** No log call may include Markdown content, a clipped page, a
   search query, an AI prompt, or an AI response.
3. **Never log remote response bodies.** AI provider error bodies can echo the prompt, so
   they may be shown to the user but must not be logged. Notion API `message` strings are
   short validation text but can quote property values, so they count as note metadata.
4. **Note metadata is allowed in the local log but is sensitive.** Vault-relative paths,
   note ids, and Notion page ids are what make a log useful for repair, and a path usually
   contains a note title. They may be logged locally. They must be removed or replaced before
   anything leaves the machine (see the export rules).
5. **Third-party output is untrusted.** Syncthing standard error is forwarded verbatim and can
   name files. Treat it as note metadata.
6. **Prefer counts and outcomes.** Where a log line exists to show that something happened,
   log how many and whether it worked rather than which notes.

## Current state (audited 2026-09-14 at `8155213`)

- No log call in `src-tauri/src` formats a secret. Keys are held by `secret_store.rs`, and the
  watchdog receives the Syncthing API key through its environment, not its arguments.
- AI provider error bodies (`ai.rs`) are returned to the UI and are not logged. This complies
  with rule 3, but the UI message is not a log and is out of scope here.
- Paths and note ids are logged in `commands.rs` (semantic indexing), `notion/publish.rs`,
  `notion/map.rs`, `notion/commands.rs`, and `vault/relocation.rs`. This is allowed by rule 4.
- `notion/publish.rs` logs Notion errors that may include the API `message`. That is note
  metadata under rule 3.
- `sync_sidecar.rs` forwards Syncthing standard error at `warn`. That is note metadata under
  rule 5.
- Development-only `println!` and `eprintln!` calls exist in tests and in the portal probe.
  They do not reach the log file.

## Diagnostic export (not yet implemented)

The export must be started by the user, never automatic or uploaded by the app. By default
it contains:

- app version, build commit, OS and version, installer type;
- the rotated log files with vault-relative paths, note ids, Notion page ids, Tailscale
  addresses, and Syncthing device ids replaced by stable per-export placeholders;
- sync status and the last terminal outcome, with the same replacements;
- configuration with every secret field removed, not masked.

It never contains note files, attachments, history, trash, backups, the search or semantic
indexes, or the keyring. Including unredacted paths is a separate, explicit choice that the
export screen explains.

Implementation is deferred until #86 merges, because it touches the same startup and command
modules. Tests must prove that a planted secret, a planted note body, and a planted note path
never appear in a default export.
