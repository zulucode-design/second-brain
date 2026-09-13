# HelixNotes 1.3.5 and the Second Brain version lineage

Date: 2026-09-13

## Question

Should Second Brain adopt the inherited HelixNotes `1.3.5` version before establishing its
own release identity?

## Findings

1. **HelixNotes has an upstream `v1.3.5` tag.** The upstream Git remote advertises tag
   `v1.3.5` at `a15b267614bb4a5372e4c24e8beebaead2181e32`. The project remains a separate
   HelixNotes application and release line.
   - Publisher: Yuri Karamian / ArkHost
   - Source: https://gitlab.com/ArkHost/HelixNotes
   - Evidence checked: `git ls-remote --tags upstream`
   - Confidence: high

2. **The upstream 1.3.5 preparation commit entered this repository and was immediately
   reverted.** Commit `ade146f` (`chore(release): prepare v1.3.5`) changed much more than a
   version string: it replaced application code, dependency locks, build settings, and tests.
   Commit `43f1596` then reverted it in full thirteen minutes later. The current branch keeps
   version `1.3.4` in the three manifests.
   - Source: repository commits `ade146f` and `43f1596`
   - Evidence checked: `git show --stat`, manifest diffs, and current manifests
   - Confidence: high

3. **Adopting the number does not adopt useful upstream work.** A version bump to `1.3.5`
   would only claim membership in HelixNotes's release sequence. Actual upstream fixes must
   be reviewed and integrated as code changes against the diverged Second Brain branch.
   - This is an inference from the separate product decision, the full revert, and the scope
     of the upstream preparation commit.
   - Confidence: high

## Recommendation

Do not label Second Brain `1.3.5`. Start its independent public release line at
`0.1.0-alpha.1`. Review future HelixNotes changes by commit or release delta and merge only
the ones that benefit Second Brain; record their provenance without inheriting HelixNotes's
product version.

## Remaining uncertainty

The upstream tag confirms the release exists, but this focused check did not produce a
curated upstream changelog separating every 1.3.5 feature or fix. That does not affect the
version-lineage conclusion; it matters only when planning a future upstream-merge review.
