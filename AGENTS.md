# AGENTS.md

Second Brain — a Building a Second Brain (BASB) knowledge app.
Forked from HelixNotes (AGPL-3.0-or-later). Tauri 2 + SvelteKit + Rust.

The authoritative design lives in `docs/SPEC.md`. Read it before proposing
work; it records settled decisions, rejected options, and open questions.

## Running tests

Rust tests go through `pnpm test:rust`, never `cargo test` directly.

On Windows, bare `cargo test` **cannot load its own test binary**. It dies with
`STATUS_ENTRYPOINT_NOT_FOUND` (`0xc0000139`) before running a single test, naming
no symbol, no DLL and no test — it looks like the whole suite is broken. It is not:
`pnpm test:rust` passes.

`rfd`, via `tauri-plugin-dialog`, imports `TaskDialogIndirect` from `comctl32.dll`.
That symbol exists only in Common Controls v6, a side-by-side assembly, so a binary
with no manifest naming it binds `System32\comctl32.dll` — v5.82, which does not
export it. The app binary is fine because `tauri_build` embeds a manifest; the test
binary gets one only from `scripts/test-rust.mjs`, which sets
`HELIX_WINDOWS_TEST_MANIFEST=1`. That gate cannot be removed: applied unscoped it
also hits the app binary, where a second manifest is a hard linker error (`CVT1100`).

## Agent skills

### Issue tracker

Issues live in this repo's GitHub Issues (`zulucode-design/second-brain`),
managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context: one `CONTEXT.md` at the repo root plus `docs/adr/`.
See `docs/agents/domain.md`.

### Work reports

Review, remediation-plan, and ticket-recap HTML reports are committed project records.
Store them under `docs/reports/` and update the relevant report in the same pull request
when its findings, verification status, or remaining work changes. See
`docs/reports/README.md`.
