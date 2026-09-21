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

## Pull requests

An issue is complete only once its branch has passed a **final review** of its exact head commit:
review `git diff origin/main...HEAD`, after `git fetch`, on two separate axes. **Spec** checks it
against what it was asked to do: the issue, or Nicolas's request when there is no issue.
**Standards** checks it against this repo's rules. Claude runs this as the `code-review` skill; any
agent without that skill runs both axes by hand.

Fix every finding. A finding left unfixed is named in the PR body with its reason, for Nicolas to
accept. The PR body carries a `Final review: <sha>` line naming the commit the review covered.

Any new head commit (a fix, a review fix, or a merge from main) needs a fresh final review of the
whole branch, and the `Final review:` line moves to it.

Hard rule, no exceptions: no pull request is opened, by any means and including drafts, and nothing
is pushed to a pull request branch or to main, until the final review of that exact commit is done.

Why: PR #140 was opened on 2026-09-21 without one; the review that followed found a
silent-failure path and an unverified Windows build.

## PowerShell on the Windows machine

`ssh sb-windows` is Nicolas's real desktop, not a sandbox. Every PowerShell script run there
follows three rules:

1. **Name your own variables** (`$tempDir`, `$stHome`, `$launchedPid`). PowerShell's automatic
   variables — `$HOME`, `$PID`, `$HOST`, `$INPUT`, `$ARGS`, `$PROFILE`, `$PWD`, `$ERROR`,
   `$MATCHES`, `$_` and the rest — are read-only or reserved: assigning one, looping over it, or
   declaring it as a parameter fails silently and the old value stays.
2. **Guard every recursive delete.** Before any `Remove-Item -Recurse`, call a guard that resolves
   the path and throws unless it is under `$env:TEMP`:
   `function Assert-UnderTemp($p) { $full = [IO.Path]::GetFullPath($p); if (-not $full.StartsWith([IO.Path]::GetFullPath($env:TEMP) + '\')) { throw "refusing to delete $full" } }`
3. **Stop processes by the exact PID you launched**, never by name or command-line match.

Why: on 2026-09-15 a script stored a temp path in `$home`; the assignment was ignored, so it
started Syncthing in `C:\Users\Nicolas`, killed a process matched by that path, and ran
`Remove-Item -Recurse -Force` on the user profile. It deleted nothing only because files were
locked. Claude sessions on this machine also run a blocking hook
(`~/.claude/hooks/powershell-guard.py`), but these rules bind every agent, hook or not.

## Agent skills

### Issue tracker

Issues live in this repo's GitHub Issues (`zulucode-design/second-brain`),
managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Issues use the configured GitHub triage-state labels. See
`docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` at the repo root plus `docs/adr/`.
See `docs/agents/domain.md`.

### Work reports

Review, remediation-plan, and ticket-recap HTML reports are committed project records.
Store them under `docs/reports/` and update the relevant report in the same pull request
when its findings, verification status, or remaining work changes. See
`docs/reports/README.md`.

## Chat style

<!-- caveman-begin -->
Respond terse like smart caveman. All technical substance stay. Only fluff die.

Rules:
- Drop: articles (a/an/the), filler (just/really/basically), pleasantries, hedging
- Fragments OK. Short synonyms. Technical terms exact. Code unchanged.
- Pattern: [thing] [action] [reason]. [next step].
- Not: "Sure! I'd be happy to help you with that."
- Yes: "Bug in auth middleware. Fix:"

Switch level: /caveman lite|full|ultra|wenyan-lite|wenyan-full|wenyan-ultra
Stop: "stop caveman" or "normal mode"

Auto-Clarity: drop caveman for security warnings, irreversible actions, user confused. Resume after.

Boundaries: code, comments, commits, PRs, issues, docs, reports, and memory files are written in normal prose. Caveman style is for chat replies only.
<!-- caveman-end -->
