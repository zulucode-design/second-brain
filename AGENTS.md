# AGENTS.md

Second Brain — a Building a Second Brain (BASB) knowledge app.
Forked from HelixNotes (AGPL-3.0-or-later). Tauri 2 + SvelteKit + Rust.

The authoritative design lives in `docs/SPEC.md`. Read it before proposing
work; it records settled decisions, rejected options, and open questions.

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
