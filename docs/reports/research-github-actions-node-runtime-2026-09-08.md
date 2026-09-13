# GitHub Actions Node-runtime verification — 2026-09-08

Status: **Implemented in issue #88.** The workflow action wrappers were upgraded to
their Node 24 major versions on 2026-09-13 while the application's CI runtime remained
on Node 22. This report is retained as the evidence and rationale for that maintenance.

## Scope

Determine whether the Node 20 annotation on GitHub Actions run `34241116273`
means that the application is configured to use an obsolete Node.js version, and
whether a workflow-action upgrade is required.

## Findings

| Finding | Evidence | Confidence |
| --- | --- | --- |
| The application CI runtime is Node 22. | `.github/workflows/verify.yml` sets `actions/setup-node` input `node-version: 22`. | High |
| The annotation is about the implementation runtime of three GitHub Actions, not the Node version used to run the application commands. | The workflow references `actions/checkout@v4`, `pnpm/action-setup@v4`, and `actions/setup-node@v4`. Their `action.yml` files declare `node20`. | High |
| Each affected action has a compatible Node 24 major version. | `actions/checkout@v5`, `actions/setup-node@v5`, and `pnpm/action-setup@v5` each declare `node24` in `action.yml`. | High |
| This is time-sensitive maintenance, rather than a passing but indefinitely harmless warning. | GitHub changed the default JavaScript-action runtime to Node 24 on 2026-06-16 and plans to remove the Node 20 fallback from runners on 2026-09-23. | High |

## Interpretation

The project is **not** running its tests or build under Node 20: it uses Node
22. The warning instead identifies old action *wrappers* whose declared runtime
is Node 20. GitHub already forced them to run under Node 24 in the successful
CI run. Once Node 20 is removed from GitHub-hosted runners, Node 24 is simply
the only available runtime; the passed run is evidence that this workflow is
currently compatible, not a guarantee that old action versions remain
supported indefinitely.

The prior warning and this one are therefore the same class of compatibility
issue: a deprecated JavaScript runtime inside the workflow actions themselves.
It is not an application dependency failure and it did not invalidate PR #47's
successful verification.

## Implemented follow-up

Issue #88's initial verification-matrix work changed these workflow references before
2026-09-23:

```yaml
actions/checkout@v4  -> actions/checkout@v5
pnpm/action-setup@v4 -> pnpm/action-setup@v5
actions/setup-node@v4 -> actions/setup-node@v5
```

Keep `node-version: 22` unchanged. Run the normal `pnpm verify` gate and one
GitHub Actions run after the upgrade. The workflow uses GitHub-hosted
`ubuntu-22.04` and `windows-2022` runners, so the minimum self-hosted-runner
compatibility caveat for the new action majors does not apply. The new Windows job
also runs the repository's required `pnpm test:rust` entrypoint, preserving the test
binary manifest setup documented in `AGENTS.md`.

Final hosted-run evidence is recorded on the implementing pull request.

## Sources

- [Current project workflow](../../.github/workflows/verify.yml), inspected 2026-09-08.
- [GitHub: Deprecation of Node 20 on GitHub Actions runners](https://github.blog/changelog/2025-09-19-deprecation-of-node-20-on-github-actions-runners/), including its 2026-08-25 date update.
- [actions/checkout action definition](https://github.com/actions/checkout/blob/main/action.yml), which currently declares `node24`.
- [actions/setup-node v5 release notes](https://github.com/actions/setup-node/releases/tag/v5.0.0), which records the Node 24 runtime upgrade.
- [pnpm/action-setup action definition](https://github.com/pnpm/action-setup/blob/v5/action.yml), which declares `node24`.
