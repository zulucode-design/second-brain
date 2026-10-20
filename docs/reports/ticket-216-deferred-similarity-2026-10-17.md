# Deferred capture-time similarity (#216): verification

Date: 2026-10-20 (local date). Sol ran Kiro workflow section 7, steps 5 and 6, on candidate
`32fe3afdc3092e42e937fd2f6edbd09453680f7b`.

**VERIFY: PASS**, with only the allowed Windows #221 Clippy failure. Both machines passed their
requested tests and builds. All four installed-package recovery steps passed again; the debug
end-to-end similarity run also passed. The Fedora RPM was removed, the temporary Ollama job and
all observed descendants were stopped, and production profiles remained unchanged. No push or
pull request was made. The new report commit still needs its own final review before publication.

## Current candidate and method

- Branch: `fix/216-deferred-similarity`. Fedora HEAD was the exact candidate and the tree was clean
  before verification. `git fetch origin` confirmed `origin/main` remained
  `1e99ab122dcdb873c17b7055e1cb8e148b592de2`. Commit `7763c73` had already merged that main head
  (PR #225, #169 hourly sync); no further merge was needed. Its changes were disjoint from this work.
- The old Windows candidate checkout no longer existed. A fresh disposable checkout was cloned
  from a full-history candidate bundle; HEAD and a clean tree were confirmed before the gates.
  `CARGO_TARGET_DIR` reused the shared Rust cache. The existing source checkout and #169 install
  were left alone. `pnpm install --frozen-lockfile` reused 477 packages and downloaded none.
  No app was installed on Windows.
- The required Sol usage probe returned `OK` before each run. Sol ran every verification here;
  no stand-in was needed. No product code was edited or fixed.
- Windows disk preflight: C: 72,486,268,928 free bytes (67.51 GiB), D: 146,170,503,168 (136.13 GiB),
  above the 15 GB requirement and the script's conservative 15 GiB check. No space was freed.
- Post-build Windows porcelain reported `M src-tauri/Cargo.toml`, but `git diff --exit-code`
  returned 0 and its normalized working blob exactly matched HEAD:
  `2195ed7bfdbb622667dd0bf5cdd303153b8c23de`. The first inventory assertion required empty
  porcelain and stopped on that status. Its corrected read-only inventory records both facts
  and also requires unchanged index and tracked content. The file was left as it was; no
  restore or product fix was made. [Status and identity evidence](evidence/ticket-216-2026-10-17/windows-post-build-state-32fe3af.json).

[Current verification summary](evidence/ticket-216-2026-10-17/verification-32fe3af.json).

## Current command results

Every result in this section is from `32fe3af`, not carried from an earlier build.

| Machine | Command | Result and counts |
| --- | --- | --- |
| Fedora | `pnpm verify` | PASS, exit 0; all five constituent commands below passed. |
| Fedora | `pnpm check` (inside verify) | PASS; 0 errors, 0 warnings. |
| Fedora | `pnpm test` (inside verify) | PASS; 254 passed, 0 failed, 0 skipped. |
| Fedora | `pnpm test:rust` (inside verify) | PASS; 747 listed, 737 passed, 0 failed, 10 ignored. Main and doc tests each ran 0. |
| Fedora | `pnpm lint:rust` (inside verify) | PASS; formatting and Clippy clean. |
| Fedora | `pnpm build` (inside verify) | PASS; one static site. |
| Fedora | `pnpm tauri:build` | PASS, exit 0; one RPM, 30,175,462 bytes. |
| Fedora | `scripts/verify-linux-package.sh <RPM>` | PASS, exit 0; one packaged desktop entry matched its source. Only its cleanup command was intercepted, retaining the extraction. |
| Fedora | Package helper install/remove | PASS, exits 0; installed binary contained the full candidate SHA; removal confirmed package, binary and desktop entries absent. |
| Fedora | `pnpm tauri build --debug --no-bundle` | PASS, exit 0; one debug app. |
| Fedora | `node scripts/similar-run.mjs --app src-tauri/target/debug/second-brain` | PASS, exit 0; 17 captures, 8/8 duplicates, 0 wrong cards, 0 inconclusive. |
| Windows | `pnpm test:rust` | PASS, exit 0; 716 listed, 707 passed, 0 failed, 9 ignored. |
| Windows | `pnpm lint:rust` | FAIL, allowed #221 only, exit 101; formatting passed. One distinct `bind_instead_of_map` error, reported for lib and lib test, at `src-tauri/src/commands.rs:4420`. It is the unchanged expression at main's line 4295. |
| Windows | `pnpm build` | PASS, exit 0; one static site. |
| Windows | `pnpm tauri:build` | PASS, exit 0; one x64 NSIS installer. One informational linker warning reported creation of import-library files. Nothing installed. |

Full logs: [Fedora verify](evidence/ticket-216-2026-10-17/fedora-verify-32fe3af.txt),
[Fedora RPM build](evidence/ticket-216-2026-10-17/fedora-tauri-build-32fe3af.txt),
[package check](evidence/ticket-216-2026-10-17/fedora-package-32fe3af.txt),
[debug build](evidence/ticket-216-2026-10-17/fedora-debug-build-32fe3af.txt),
[Windows Rust tests](evidence/ticket-216-2026-10-17/windows-test-rust-32fe3af.txt),
[Windows lint](evidence/ticket-216-2026-10-17/windows-lint-rust-32fe3af.txt),
[Windows frontend build](evidence/ticket-216-2026-10-17/windows-build-32fe3af.txt), and
[Windows NSIS build](evidence/ticket-216-2026-10-17/windows-tauri-build-32fe3af.txt).
Windows `pnpm test` was outside the requested Windows list and was not run; #221's three known
JavaScript failures were not exercised. Ignored live/hardware/network tests are not claimed as passed.

## Current named tests

All 50 requested Rust tests ran and passed on Fedora; all 45 applicable tests ran and passed on
Windows. The five Unix-only tests did not compile on Windows. None of these results is ignored or
filtered. The population below also exactly matches the 50 Rust test functions added by this branch
relative to `origin/main`, including all five `commands::restore_outcome_tests` tests.

Regenerate the name-to-log check from the repository root:
`python3 docs/reports/evidence/ticket-216-2026-10-17/match-tests-32fe3af.py`.
The script reads the explicit [50-name manifest](evidence/ticket-216-2026-10-17/named-tests-32fe3af.json)
and matches each full `test <name> ... ok` line against the two complete logs.
[Added-function comparison](evidence/ticket-216-2026-10-17/added-tests-matched-32fe3af.json) records
test functions present at this candidate and absent from `origin/main`. Regenerate that comparison with
`python3 docs/reports/evidence/ticket-216-2026-10-17/match-added-tests-32fe3af.py`; it pins both commits.

| Test | Fedora | Windows |
| --- | --- | --- |
| `semantic_search::tests::a_failed_similarity_check_is_recorded_and_survives_reloading_its_file` | PASS | PASS |
| `semantic_search::tests::a_deferred_check_runs_after_the_backend_returns_and_its_card_is_delivered` | PASS | PASS |
| `semantic_search::tests::a_moved_note_still_gets_its_deferred_check_and_a_deleted_one_is_dropped` | PASS | PASS |
| `semantic_search::tests::a_deferred_check_that_fails_again_stays_recorded` | PASS | PASS |
| `semantic_search::tests::a_pass_that_stopped_or_found_the_queue_empty_reruns_nothing` | PASS | PASS |
| `semantic_search::tests::a_pass_that_leaves_notes_queued_reruns_nothing_until_the_queue_is_through` | PASS | PASS |
| `semantic_search::tests::a_check_is_kept_while_it_runs_and_leaves_the_list_when_it_succeeds` | PASS | PASS |
| `semantic_search::tests::a_check_waiting_behind_another_is_kept_while_it_waits` | PASS | PASS |
| `semantic_search::tests::a_started_index_reruns_nothing_before_its_first_reconcile` | PASS | PASS |
| `semantic_search::tests::a_rerun_waits_until_no_rebuild_or_reconcile_is_filling_the_queue` | PASS | PASS |
| `semantic_search::tests::the_end_of_a_rebuild_runs_the_caught_up_hook_it_held` | PASS | PASS |
| `semantic_search::tests::the_end_of_a_rebuild_leaves_the_notes_it_queued_to_the_next_pass` | PASS | PASS |
| `semantic_search::tests::a_rebuild_or_reconcile_started_since_the_index_caught_up_stops_the_rerun` | PASS | PASS |
| `semantic_search::tests::a_rerun_never_holds_the_index_slot_while_it_waits_for_the_config` | PASS | PASS |
| `semantic_search::tests::a_rebuild_during_a_pass_leaves_the_rerun_to_the_pass_that_embeds_what_it_queued` | PASS | PASS |
| `semantic_search::tests::a_deferred_note_moved_with_a_long_frontmatter_is_still_found` | PASS | PASS |
| `semantic_search::tests::a_note_without_an_id_never_matches_itself` | PASS | PASS |
| `semantic_search::tests::a_note_queued_in_place_of_an_unreadable_one_holds_the_rerun` | PASS | PASS |
| `semantic_search::tests::a_fill_that_stops_part_way_holds_the_rerun_until_a_reconcile_completes` | PASS | PASS |
| `semantic_search::tests::a_rerun_waits_while_a_bulk_change_holds_the_note_lock` | PASS | PASS |
| `semantic_search::tests::a_rerun_scans_the_vault_without_the_note_lock_and_a_bulk_change_meanwhile_keeps_the_check` | PASS | PASS |
| `semantic_search::tests::the_worker_retries_a_failed_fill_once_an_interval_until_it_completes` | PASS | PASS |
| `semantic_search::tests::a_bulk_change_that_comes_and_goes_during_a_check_stops_the_rerun` | PASS | PASS |
| `semantic_search::tests::a_bulk_change_reconciles_the_semantic_index_even_when_an_earlier_step_fails` | PASS | PASS |
| `semantic_search::tests::overlapping_fills_hold_the_rerun_until_one_completes_alone` | PASS | PASS |
| `semantic_search::tests::a_restore_that_rolls_back_reconciles_what_a_reconcile_meanwhile_dropped` | PASS | PASS |
| `semantic_search::tests::a_check_that_starts_after_a_vault_switch_is_kept_for_its_own_vault` | PASS | PASS |
| `semantic_search::tests::a_rerun_leaves_its_checks_to_the_pass_that_embeds_notes_saved_since_the_index_caught_up` | PASS | PASS |
| `semantic_search::tests::a_save_embedded_while_a_check_runs_stops_the_rerun` | PASS | PASS |
| `semantic_search::tests::an_index_replaced_after_a_check_scored_stops_the_rerun` | PASS | PASS |
| `semantic_search::tests::a_rerun_leaves_a_kept_check_to_its_first_attempt` | PASS | PASS |
| `semantic_search::tests::a_capture_renamed_before_its_check_starts_is_kept_by_its_id` | PASS | PASS |
| `semantic_search::tests::a_note_written_at_a_renamed_captures_old_path_never_takes_its_check` | PASS | PASS |
| `semantic_search::tests::a_note_written_at_a_captures_path_after_a_rerun_found_it_there_never_takes_its_check` | PASS | PASS |
| `similar_notes::tests::an_unreadable_deferred_file_gives_an_empty_list_and_the_vault_still_opens` | PASS | PASS |
| `similar_notes::tests::opening_a_vault_reads_its_checks_and_reopening_it_keeps_the_open_list` | PASS | PASS |
| `similar_notes::tests::a_check_kept_while_the_file_cannot_be_read_is_saved_when_another_vault_opens` | PASS | PASS |
| `similar_notes::tests::a_note_whose_folder_is_gone_is_placed_by_the_vault_unless_it_climbs_out` | PASS | PASS |
| `similar_notes::tests::a_check_that_ran_while_the_file_could_not_be_replaced_leaves_it_when_another_vault_opens` | PASS | PASS |
| `similar_notes::tests::a_check_that_ran_while_the_file_could_not_be_read_leaves_it_when_another_vault_opens` | PASS | PASS |
| `semantic_search::tests::a_deferred_note_moved_where_it_cannot_be_opened_is_not_taken_for_deleted` | PASS | Not compiled (Unix only) |
| `semantic_search::tests::a_deferred_note_without_an_id_behind_a_folder_that_cannot_be_read_is_not_taken_for_deleted` | PASS | Not compiled (Unix only) |
| `semantic_search::tests::a_note_without_an_id_never_matches_itself_through_a_vault_link` | PASS | Not compiled (Unix only) |
| `semantic_search::tests::a_check_kept_under_another_path_to_the_open_vault_joins_its_list` | PASS | Not compiled (Unix only) |
| `similar_notes::tests::a_note_is_in_a_vault_opened_through_a_link_and_never_in_a_folder_that_is_not_one` | PASS | Not compiled (Unix only) |
| `commands::restore_outcome_tests::a_failed_restore_whose_reconcile_also_fails_reports_both` | PASS | PASS |
| `commands::restore_outcome_tests::a_restore_and_its_reconcile_that_both_succeed_are_a_success` | PASS | PASS |
| `commands::restore_outcome_tests::a_restore_refused_the_lease_never_runs` | PASS | PASS |
| `commands::restore_outcome_tests::a_failed_restore_changed_the_vault_only_when_it_says_so` | PASS | PASS |
| `commands::restore_outcome_tests::a_restore_whose_reconcile_fails_changed_the_vault_and_says_so` | PASS | PASS |

Fedora `pnpm verify` also ran and passed `tests/similar.test.mjs`'s
"a new note left by a vault switch is checked in the vault it was opened in".

## Current installed-package recovery

The unchanged bounded Windows supervisor and its `owned-process.cs` companion ran on port 11436
in one held SSH session, with a 7,200-second lease. It used the installed Ollama and existing
`embeddinggemma` model store, downloading nothing. Supervisor PID `4128` and server PID `21604`
kept the same creation times across the first answer at 22:35:10.609009Z and the stability answer
at 22:36:23.3233857Z, **72.714376 seconds apart**. The final Windows endpoint answered again
after both behavior runs.

Before installation, eight production profile roots were inventoried and the two present roots
backed up. The population is the Cartesian product of four default XDG folders (`.config`,
`.local/share`, `.local/state`, `.cache`) and the two identifiers (`io.github.zulucodedesign.SecondBrain`,
`helixnotes`), as `profilePaths` in the reused controller generates it. Counts and source are
published in the current recovery evidence's `preinstall`; its `safety` step independently records
the same counts. Private manifests and backups remain outside the repository.

The helper installed this run's RPM (SHA-256
`a7e5831b4dde1f01d6c37d2fb521d2facbf766dd350f74a1bdb4a4d919eca0b2`).
The installed binary contained the exact candidate SHA; its SHA-256 was
`e87bdbb4d9e1b9daf2347ba8fa50ab290151c43194bbd26be42ddc0fb54a318d`.
The reused controller ran from the repository root through stdin, with only its candidate metadata
changed. It used the first three fixture notes, a fresh vault ID, and isolated XDG roots.

| Step | Result | Observation |
| --- | --- | --- |
| 1. Two offline captures | PASS | `Shed spending` and `Shed wall wood` saved; two skip lines, two durable records, no card. [Records](evidence/ticket-216-2026-10-17/installed-recovery-32fe3af-offline-deferred.json). |
| 2. Restart and delete second capture | PASS | Both records survived restart; the app's `delete_note` removed the second capture. [Records after deletion](evidence/ticket-216-2026-10-17/installed-recovery-32fe3af-after-delete-deferred.json). |
| 3. Restore app backend URL | PASS | App stopped; isolated config changed from closed port 11447 to forwarded 11436; third session confirmed it. |
| 4. Card and drain | PASS | After startup reconcile/indexing, four notes indexed and zero queued. `Shed spending` matched `Garden shed budget`; `ran=1 found=1 dropped=1 waiting=0`; zero records remained; zero observed app `Notify` calls. [Empty file](evidence/ticket-216-2026-10-17/installed-recovery-32fe3af-empty-deferred.json); [card](evidence/ticket-216-2026-10-17/installed-recovery-32fe3af-card.png). |

[Recovery observations, PIDs, timing, hashes and cleanup](evidence/ticket-216-2026-10-17/installed-recovery-32fe3af.json).
[Reused controller with current candidate metadata](evidence/ticket-216-2026-10-17/installed-recovery-controller-32fe3af.mjs).
The screenshot was inspected and contains only fixture text. No step needed a person at the screen.

Step 3 proves installed recovery after a restart with a reachable backend. A running installed app
recovering from an outage is covered only by the unit tests
`semantic_search::tests::a_deferred_check_runs_after_the_backend_returns_and_its_card_is_delivered`
and `semantic_search::tests::a_deferred_check_that_fails_again_stays_recorded`; these four installed
steps did not exercise that transition. The separate debug end-to-end run below also exercises
live outage recovery. The installed run's zero `Notify` count has no positive control in that run.
Source reading backs it: `rerun_deferred` calls `show_card` and does not call `notify`
(`src-tauri/src/similar_notes.rs`). The debug run's notification controls are separate evidence.

The helper removed the RPM (exit 0); the package query returned exit 1 and the binary and both
possible desktop entries were absent. The app, driver and forwarding ports were absent after the
run. Original profile existence and file hashes matched the preinstall baseline again after the
debug run; no restoration was needed.

## Current end-to-end similarity run

After the debug build, Sol ran the candidate's `scripts/similar-run.mjs` against
`src-tauri/target/debug/second-brain`, with a fresh disposable root. A private `ssh` PATH wrapper
changed only each `-L` destination from upstream port 11434 to the gate-owned Windows port 11436;
the initial tunnel and recovery relay reached that same server. Candidate scripts were unchanged.
[Routing method and wrapper source](evidence/ticket-216-2026-10-17/similar-run-routing-32fe3af.json).

[Complete scrubbed trace](evidence/ticket-216-2026-10-17/similar-run-32fe3af.json):

- Summary: `captures=17`, `duplicatesFound=8`, `duplicates=8`, `wrongCards=0`, `inconclusive=0`.
- `actions.offline`: `saved=true`, `card=null`, `cardOnceBack=true`.
- `actions.vaultSwitch`: switched while a request was held; `cardsAfterSwitch=0`,
  `notificationsAfterSwitch=0`, `activeVaultIsOther=true`. Its no-switch control raised one
  notification, so the late-notification check had a positive control.
- Append, dismiss, changed-note and moved-note refusals, open, append with either note open, and
  the replaced notification pair all produced the expected trace values. The two notification
  calls shared the returned ID; the second used it as `replacesId`.

After the final endpoint/process-tree observation, cleanup checked supervisor and server creation
times and stopped only supervisor PID `4128`. The Job Object removed server `21604`, model worker
`7404`, and console descendants `20684` and `10308`; all five unique PIDs and the listener were
confirmed absent. The deliberate stop ended the held SSH session. The user's port 11434 had no
listener at the recorded preflight or after cleanup, and no user-owned process was stopped or adopted.
No model was downloaded. The debug app was absent and all driver/forwarding ports were closed.

## Review history through the candidate

These are the supplied prior review results, checked against the local review records where
required, not new reviews performed during this verification. Every round identifies its reviewer.

| Round / reviewed head | Reviewer | Result | Following fix |
| --- | --- | --- | --- |
| 1 / `c33a9be` | Sol (Standards), Claude Sonnet stand-in (Spec) | One P2 and four P3s; Standards CLEAN | `7e8ff9f` |
| 2 / `7e8ff9f` | Claude Sonnet stand-ins, both axes | CLEAN, with P3s | Accepted P3s settled in `19727aa` |
| First final / `664f997` | Claude Sonnet stand-ins, both axes | Spec CLEAN; Standards CHANGES REQUIRED; four adversarial P3s | Claude: `f17ef4d`, `3c7f658`; report findings settled in this report commit |
| 3 / `7763c73` | Claude Sonnet stand-ins, both axes | Spec CHANGES REQUIRED (one P2 and P3s); Standards CLEAN with P3s | `32e9628` |
| 4 / `32e9628` | Sol | Spec CHANGES REQUIRED (one P1, two P2s); Standards CLEAN with one P3 | Claude wrote diagnosis for the owner; `0554c50`; main's lock-order pair filed as #227 |
| 5 / `0554c50` | Sol | Spec CHANGES REQUIRED (one P2, one P3); Standards CLEAN | `5ad1578` |
| 6 / `5ad1578` | Sol | Spec CHANGES REQUIRED (three P2s); Standards CLEAN | `7abb9e4` |
| 7 / `7abb9e4` | Sol | Spec CHANGES REQUIRED (two P2s); Standards CLEAN | `26baeae` |
| 8 / `26baeae` | Sol | One adversarial P2; Standards CLEAN | `60c9edd` |
| 9 / `60c9edd` | Sol | One Spec P2; Standards CLEAN | `04dd215` |
| 10 / `04dd215` | Sol | One Spec P2 and two adversarial P2s; Standards CLEAN | `d4c8d37` |
| 11 / `d4c8d37` | Claude Sonnet stand-ins, both axes | CLEAN with P3s | Claude took most P3s in `f805d5e` |
| 12 / `f805d5e` | Sol | One adversarial P2 and one P3; Standards CLEAN | `6fe7d67` |
| 13 / `6fe7d67` | Sol | One adversarial P2; Standards CLEAN | `a97d70b` |
| 14 / `a97d70b` | Sol | One Spec P2 and one adversarial P2; Standards CLEAN with one P3 | `71c2136`; adversarial P2 retained with #228 |
| 15 / `71c2136` | Sol | One Spec P2 and one adversarial P2; Standards CLEAN | `3a8ef6e` |
| 16 / `3a8ef6e` | Sol | One Spec P2 and one adversarial P2; Standards CLEAN | `2145382` |
| 17 / `2145382` | Sol | One adversarial P2; Standards CLEAN | `d4d3f2f` |
| 18 / `d4d3f2f` | Sol (Spec/Adversarial), Claude Sonnet stand-in (Standards) | Standards one P2 and seven P3s; one adversarial P2 | `3ac32fc` |
| 19 / `3ac32fc` | Sol | One adversarial P2; Spec and Standards CLEAN | `bade427` |
| 20 / `bade427` | Sol | One adversarial P2; Spec and Standards CLEAN | `19e9ae1` |
| 21 / `19e9ae1` | Sol | One adversarial P2; Spec and Standards CLEAN | `db2b5b9` |
| 22 / `db2b5b9` | Sol | One Spec P2 and one adversarial P2; Standards CLEAN | `cbf28c5` |
| 23 / `cbf28c5` | Sol | One Spec P2 and one adversarial P2; Standards CLEAN | Kiro attempt 2: `9e83632`, `5c7d847`, `dee93d6`, imported by Claude |
| 24 / `dee93d6` | Claude Sonnet stand-ins, both axes | CLEAN with four P3s | Kiro attempt 3: `4bf1bb2`, `c5dd3c3`, `bfb12a7`, `9f694aa`, imported by Claude |
| 25 / `9f694aa` | Sol | CLEAN on both axes, one adversarial P3 | Kiro attempt 4: `32fe3af`, imported by Claude |
| 26 / `32fe3af` | Sol | CLEAN on both axes, no findings | Full verification recorded here; report head still needs final review |

Main's lock-order issue #227 and semantic-index overlap issue #228 remain open follow-ups. This
verification does not claim to fix them. The report head still needs the final review required by
AGENTS.md; round 26 covered the product candidate, not the resulting report commit.

## Earlier evidence carried forward

The `19727aa` evidence below remains the historical record of the initial gates, two incomplete
preflights and the successful installed recovery. The controller and bounded-supervisor method
were reused because their behavior and harness ownership still fit this check. Their historical
results do not stand in for current-candidate tests, builds or recovery: all requested gates were
rerun on `32fe3af`. Old failure verdicts, timings, screenshots and inventories remain unchanged;
the report now explains their scope and the six report findings from the first final review.

## Current retained paths

[Complete retained-path inventory](evidence/ticket-216-2026-10-17/retained-paths-32fe3af.json) lists every run file, private backup group, build root and surviving test temporary root with its size. Paths use logical aliases. Counts are regenerated by summing regular file lengths recursively, without following directory links; the inventory records the population and commands. Cache totals include pre-existing files and nested packages are included in their parents.

| Machine | Retained path | Logical file bytes |
| --- | --- | ---: |
| Fedora | `<Fedora run>` | 9,338,097 |
| Fedora | `<private pointers>/verify-current.private.txt` | 61 |
| Fedora | `.svelte-kit` | 8,532,312 |
| Fedora | `build` | 6,787,344 |
| Fedora | `src-tauri/binaries` | 26,818,686 |
| Fedora | `src-tauri/target` | 20,606,459,090 |
| Windows | `<Windows run>` | 349,095,767 |
| Windows | `<Windows shared Rust target>` | 8,320,788,222 |

<details>
<summary>Every retained test temporary root and package extraction</summary>

| Machine | Path | Logical file bytes |
| --- | --- | ---: |
| Fedora | `<Fedora temp>/helixnotes-machine-state-87d56f1f-4838-4e94-b528-8e2ac974c2ec` | 78,798 |
| Fedora | `<Fedora temp>/machine-local-abandoned-claim-b0aca440-65de-4df5-830c-ea92f273ab7e` | 36 |
| Fedora | `<Fedora temp>/machine-local-concurrent-f231caa8-74a3-4f03-9419-4f6950bca400` | 36 |
| Fedora | `<Fedora temp>/machine-local-damaged-id-d0b21258-6748-4217-885b-9d811373755d` | 36 |
| Fedora | `<Fedora temp>/machine-local-existing-id-4ce2a3e4-da69-409e-9f5b-d7a42225ae93` | 37 |
| Fedora | `<Fedora temp>/machine-local-first-open-6393fcfa-e12d-4b4b-a327-42abbb7ede68` | 36 |
| Fedora | `<Fedora temp>/machine-local-read-only-id-b95c7b3c-e52a-47e2-93af-f54607a15db7` | 36 |
| Fedora | `<Fedora temp>/machine-local-truncation-9743d411-2292-471d-b56e-f66d9425c823` | 36 |
| Fedora | `<Fedora temp>/machine-local-unlocked-fallback-fa96bcaa-2973-42ae-a809-86fbd3312fe0` | 0 |
| Fedora | `<Fedora temp>/notion-config-140c6c5d-ea66-4daf-9533-66a5195650fc` | 139 |
| Fedora | `<Fedora temp>/notion-config-6bccc2ab-3410-4050-872d-e4c047a78fcb` | 0 |
| Fedora | `<Fedora temp>/notion-config-c9ce85f1-bf58-421e-968f-1832cdbd3c85` | 135 |
| Fedora | `<Fedora temp>/notion-config-f6943dda-5d2a-4dff-a858-5168bdcb5eac` | 18 |
| Fedora | `<Fedora temp>/notion-enumerate-06a26cb6-5c9c-4cc5-a9fd-f6c7dae4c416` | 234 |
| Fedora | `<Fedora temp>/notion-enumerate-11efe943-46e3-417a-9c86-646a464b5893` | 29 |
| Fedora | `<Fedora temp>/notion-enumerate-175e7e5c-d659-4e4f-bcac-7499be362e86` | 28 |
| Fedora | `<Fedora temp>/notion-enumerate-3ec25ef7-ffd8-4b08-8537-07cf94aa0aa9` | 109 |
| Fedora | `<Fedora temp>/notion-enumerate-4084d239-4b53-4b73-8c8f-828c4f198471` | 18 |
| Fedora | `<Fedora temp>/notion-enumerate-4453808e-e1c0-49b5-af06-52dc8259cb3c` | 56 |
| Fedora | `<Fedora temp>/notion-enumerate-48c693df-26c2-40bf-8e16-70563b59a2f0` | 432 |
| Fedora | `<Fedora temp>/notion-enumerate-4affe9c0-6058-4e83-a3b6-6042eb2028b9` | 218 |
| Fedora | `<Fedora temp>/notion-enumerate-506b0ca8-6279-4cfe-a600-41620b936ddd` | 216 |
| Fedora | `<Fedora temp>/notion-enumerate-51f91be7-0845-42b6-80a7-1728aeca2fe1` | 428 |
| Fedora | `<Fedora temp>/notion-enumerate-8cff271b-19e2-4a2e-90c0-3ad2b6cb2e59` | 214 |
| Fedora | `<Fedora temp>/notion-enumerate-d3209f14-8b6f-4b36-8a47-d35f3549605b` | 58 |
| Fedora | `<Fedora temp>/notion-enumerate-de4a2013-47fc-4f72-b911-857118687ec7` | 234 |
| Fedora | `<Fedora temp>/notion-map-1018ba04-c938-439b-884c-178f57824682` | 175 |
| Fedora | `<Fedora temp>/notion-map-225d1dae-aa28-46b0-8bc7-bdef89ae22a8` | 175 |
| Fedora | `<Fedora temp>/notion-map-4a9462b8-bedc-4a16-a3be-a45679925750` | 0 |
| Fedora | `<Fedora temp>/notion-map-580df563-fcce-4a1e-8476-76eeae59ce5d` | 208 |
| Fedora | `<Fedora temp>/notion-map-77f07e9b-8173-4175-8bb3-4e956bf087c2` | 0 |
| Fedora | `<Fedora temp>/notion-map-893f1489-0f82-4718-8583-2e4e5a4cdf92` | 0 |
| Fedora | `<Fedora temp>/notion-map-9b4b6dbf-1970-40c9-ac71-2b18d225b0df` | 173 |
| Fedora | `<Fedora temp>/notion-map-d6682342-672e-4003-b7a9-2b8dfe99af12` | 175 |
| Fedora | `<Fedora temp>/notion-map-e3ab0b14-a3c4-4a58-a0f4-21777a80de64` | 17 |
| Fedora | `<Fedora temp>/notion-map-e41b091b-1a94-427c-98fc-21ef1fb7210f` | 0 |
| Fedora | `<Fedora temp>/notion-publish-00bc0eb1-9ef1-4283-b0a3-96266d938ecd` | 237 |
| Fedora | `<Fedora temp>/notion-publish-02f08af9-3d8d-4ee3-99c6-ed7152864bc4` | 239 |
| Fedora | `<Fedora temp>/notion-publish-07ed03a5-c696-4bd3-8d53-7f4537fbe019` | 0 |
| Fedora | `<Fedora temp>/notion-publish-0d9e58fc-95a4-415f-bb6f-b9eaedae2eb1` | 232 |
| Fedora | `<Fedora temp>/notion-publish-12680f77-7cd4-4359-8d1f-bea95290a192` | 448 |
| Fedora | `<Fedora temp>/notion-publish-1e0fd8df-3280-42f2-a4b6-3e8429672a2a` | 329 |
| Fedora | `<Fedora temp>/notion-publish-2c148b12-38ca-4f53-bc8e-c849c756d518` | 0 |
| Fedora | `<Fedora temp>/notion-publish-37a29971-b702-4101-b648-c8af25beed96` | 232 |
| Fedora | `<Fedora temp>/notion-publish-3c0838bf-802c-435b-a3db-b5d498e99aab` | 239 |
| Fedora | `<Fedora temp>/notion-publish-492aadc4-6429-4757-8bcb-ee319bc3105b` | 26 |
| Fedora | `<Fedora temp>/notion-publish-4a68edda-5690-4ada-a54c-d96722fd2d1d` | 0 |
| Fedora | `<Fedora temp>/notion-publish-4afc1135-e02c-45b3-baa1-1f76c2082a75` | 234 |
| Fedora | `<Fedora temp>/notion-publish-5229a64c-a849-4605-8ce3-3b7c309bb1d8` | 0 |
| Fedora | `<Fedora temp>/notion-publish-65596c1c-d342-4f57-aecb-db74d487b3ed` | 234 |
| Fedora | `<Fedora temp>/notion-publish-8085a2ff-717d-488b-95bc-a557cf8afeb8` | 234 |
| Fedora | `<Fedora temp>/notion-publish-90336b22-a0b5-4ed9-8abf-c6490a7ba6ee` | 239 |
| Fedora | `<Fedora temp>/notion-publish-9306d9b0-82d3-4833-bd8a-2573ef0c9d85` | 239 |
| Fedora | `<Fedora temp>/notion-publish-956e94b7-188c-4cff-a484-d629f5bd0455` | 167 |
| Fedora | `<Fedora temp>/notion-publish-b872b1f5-f932-46e7-aab9-d28c48e2164c` | 228 |
| Fedora | `<Fedora temp>/notion-publish-bac7d397-d11d-4653-ad29-d3ec54a3649f` | 234 |
| Fedora | `<Fedora temp>/notion-publish-cef73ba6-13a2-4dfe-9a79-6ab1a89aede6` | 0 |
| Fedora | `<Fedora temp>/notion-publish-d7aca02f-9dd2-437f-be17-9c4825195163` | 236 |
| Fedora | `<Fedora temp>/notion-publish-e1c73292-2c8c-4130-9056-c119856ff065` | 0 |
| Fedora | `<Fedora temp>/notion-publish-e4e6d119-18d5-4a08-abde-b5f3862b3e39` | 226 |
| Fedora | `<Fedora temp>/notion-publish-f902973c-2e8c-45c8-abd8-d8d27935f215` | 178 |
| Fedora | `<Fedora temp>/notion-setup-76d24448-6168-4fa5-b96e-4f4ebc25dab7` | 439 |
| Fedora | `<Fedora temp>/notion-setup-820c9323-44c8-4429-ba53-9d018f90fd87` | 433 |
| Fedora | `<Fedora temp>/notion-setup-e65d49e1-7bda-4015-91c8-66850c16355a` | 439 |
| Fedora | `<Fedora temp>/semantic-async-2e208f27-25b3-42c3-89c5-c8c79d795d70` | 36 |
| Fedora | `<Fedora temp>/tmp.xt7xvywyBA` | 78,966,359 |
| Windows | `<Windows temp>/helixnotes-machine-state-98e03df2-3f2e-4493-aa62-cd4f6d0b7d1e` | 80,082 |
| Windows | `<Windows temp>/machine-local-abandoned-claim-b35d4a89-3948-4f76-a6e5-468d38ffe772` | 36 |
| Windows | `<Windows temp>/machine-local-concurrent-2871f045-f7ea-4e9d-875e-303c8dec3e26` | 36 |
| Windows | `<Windows temp>/machine-local-damaged-id-6cdd0761-373e-47ed-8014-62356fcc5eb8` | 36 |
| Windows | `<Windows temp>/machine-local-existing-id-87efc4bb-2728-4f54-81cb-4c6bc1157d7c` | 37 |
| Windows | `<Windows temp>/machine-local-first-open-92b22626-a51b-4e2a-b777-ffaa59adb260` | 36 |
| Windows | `<Windows temp>/machine-local-truncation-cacdfb4c-e3ed-4b51-8561-82e8d6b8ab33` | 36 |
| Windows | `<Windows temp>/machine-local-unlocked-fallback-cd324580-661d-4577-8374-c43028081f03` | 0 |
| Windows | `<Windows temp>/notion-config-3a3e0a61-5a1d-4619-a2fe-16d5ec4eabb9` | 139 |
| Windows | `<Windows temp>/notion-config-7096f7a3-185a-4e45-b363-f91837fca980` | 135 |
| Windows | `<Windows temp>/notion-config-8af3643b-52b3-4732-9195-de43fd69abb8` | 18 |
| Windows | `<Windows temp>/notion-config-dc67008a-f530-4142-a64c-4493b5b4cbcb` | 0 |
| Windows | `<Windows temp>/notion-enumerate-13e013ce-674b-4a26-8c07-61eaa5de0016` | 29 |
| Windows | `<Windows temp>/notion-enumerate-1851f2f3-0cb4-45cb-991e-b87517be901e` | 58 |
| Windows | `<Windows temp>/notion-enumerate-18b638a2-2129-461a-a369-20632b314d59` | 218 |
| Windows | `<Windows temp>/notion-enumerate-1aec42ca-be6b-4caf-b702-c9b57e11e3c5` | 234 |
| Windows | `<Windows temp>/notion-enumerate-22aa7022-194f-4310-8b8e-ad67255e013a` | 428 |
| Windows | `<Windows temp>/notion-enumerate-63faedbb-e0f3-4020-96bc-dd4106ba324e` | 234 |
| Windows | `<Windows temp>/notion-enumerate-ace33965-a9f7-4652-bbb0-488daab9eb9f` | 432 |
| Windows | `<Windows temp>/notion-enumerate-b47754d4-4608-4056-a585-d1c5601c1e27` | 56 |
| Windows | `<Windows temp>/notion-enumerate-c646df24-b320-40ea-b5e3-5be53cdd020f` | 109 |
| Windows | `<Windows temp>/notion-enumerate-cd67e2a0-c363-4161-8102-3494efb8a88b` | 216 |
| Windows | `<Windows temp>/notion-enumerate-d4a70c40-a717-4a24-97e6-3f2d8bf63dce` | 214 |
| Windows | `<Windows temp>/notion-enumerate-e295cd1b-6bee-4421-a1e8-fe8acec27578` | 18 |
| Windows | `<Windows temp>/notion-enumerate-ed097c7d-454e-4945-baee-7f9f0a390365` | 28 |
| Windows | `<Windows temp>/notion-map-11ca5790-fb2a-4605-ada0-28a08a23e150` | 208 |
| Windows | `<Windows temp>/notion-map-3e23ab0f-7ac9-4c63-bb7b-ad48f3b78d86` | 175 |
| Windows | `<Windows temp>/notion-map-4507dcf1-8a2e-4b2e-9afa-c01f5cc5e856` | 0 |
| Windows | `<Windows temp>/notion-map-46a116a7-85a7-44a7-a816-eeea387821ee` | 173 |
| Windows | `<Windows temp>/notion-map-61013d06-38de-4b3d-8cc4-8962fdb33739` | 0 |
| Windows | `<Windows temp>/notion-map-848e29c7-7e20-48e5-8ad1-205ef5b6f17f` | 0 |
| Windows | `<Windows temp>/notion-map-b0ac7fe8-28b1-4f2d-9bbf-307d64d01b4e` | 17 |
| Windows | `<Windows temp>/notion-map-bbd85387-95ff-4cd0-b981-294ed0191ef1` | 175 |
| Windows | `<Windows temp>/notion-map-c067a582-6049-4d5e-8498-ed92ddf0312f` | 175 |
| Windows | `<Windows temp>/notion-map-d3c19e70-7631-406e-a70e-fccd93fa8d0f` | 0 |
| Windows | `<Windows temp>/notion-publish-02681500-bda2-42e6-922c-e3a5d1af85b8` | 0 |
| Windows | `<Windows temp>/notion-publish-107dd784-086a-4852-823c-adf611b081eb` | 234 |
| Windows | `<Windows temp>/notion-publish-247ae587-916d-4b7a-adff-43082c4ef323` | 239 |
| Windows | `<Windows temp>/notion-publish-3ca33673-993c-4343-9fd6-1cab93d5e426` | 234 |
| Windows | `<Windows temp>/notion-publish-4c622868-5a16-44ec-87aa-3ef50a849ead` | 236 |
| Windows | `<Windows temp>/notion-publish-526fcb69-6707-4827-a58e-af3e30df5c89` | 232 |
| Windows | `<Windows temp>/notion-publish-5f6d6633-2a49-4c99-96d3-c1375f3fabb8` | 448 |
| Windows | `<Windows temp>/notion-publish-7c46cb93-620b-424c-9774-6e8bf86f64a2` | 0 |
| Windows | `<Windows temp>/notion-publish-80a7746f-4fc6-4aca-8476-00eeb47ec7de` | 167 |
| Windows | `<Windows temp>/notion-publish-8cae99ac-cef7-4cf5-86ab-5ca268be83d9` | 228 |
| Windows | `<Windows temp>/notion-publish-9002f9ef-0f45-46dd-9b7f-ea404a3db809` | 26 |
| Windows | `<Windows temp>/notion-publish-a090dad0-bbf7-4f47-b5d2-26073d809baf` | 239 |
| Windows | `<Windows temp>/notion-publish-a8ee4ad1-2361-4b58-a2bd-d189d595c991` | 0 |
| Windows | `<Windows temp>/notion-publish-ae05a353-e546-48e1-bd53-72dc94d66ab4` | 239 |
| Windows | `<Windows temp>/notion-publish-b045482a-d2a6-4b06-95fc-8efd1c4079b1` | 0 |
| Windows | `<Windows temp>/notion-publish-c0a3e2e2-b297-48d8-884f-a84b69ded2bc` | 178 |
| Windows | `<Windows temp>/notion-publish-c31c64c2-ed47-4f72-83c6-b6cce223abee` | 239 |
| Windows | `<Windows temp>/notion-publish-c547b787-4282-4088-be92-e4b00c8ea73f` | 237 |
| Windows | `<Windows temp>/notion-publish-cb811390-8060-4916-a428-3f3cf43ac498` | 226 |
| Windows | `<Windows temp>/notion-publish-d9263378-01c0-44e4-924b-91ec0a32e24b` | 0 |
| Windows | `<Windows temp>/notion-publish-dc273e0f-abaa-4f18-8ab1-282997e283f8` | 232 |
| Windows | `<Windows temp>/notion-publish-e48e4b1c-1eda-4d24-bfc0-1796658322ef` | 234 |
| Windows | `<Windows temp>/notion-publish-e88b6e00-f95d-4c98-bec1-f4f06a10a013` | 329 |
| Windows | `<Windows temp>/notion-publish-e8c71333-100e-4948-a115-17742a481237` | 0 |
| Windows | `<Windows temp>/notion-publish-f4fbafc1-b813-4c70-99d1-538aa847f44b` | 234 |
| Windows | `<Windows temp>/notion-setup-5f027f49-19cc-43ef-bc20-475d62ec0fbd` | 439 |
| Windows | `<Windows temp>/notion-setup-9a804771-91e8-4c31-971e-ef97110a6c67` | 433 |
| Windows | `<Windows temp>/notion-setup-c9b2685a-a1e9-484e-9ddd-98e8a0e86e25` | 439 |
| Windows | `<Windows temp>/semantic-async-9b3fb959-65a4-405c-9774-0afe526ddd50` | 36 |

</details>

RPM: 30,175,462 bytes. NSIS installer: 20,333,836 bytes; both included in target totals. The full test-temp population is 66 Fedora roots (including retained package extraction) and 64 Windows roots, exactly the entries expanded above. Individual run files are in the linked JSON; production backup contents are grouped for privacy.

Cleanup candidates: the two disposable run roots, private pointer, retained package extraction and each test temporary root listed above. Shared build caches are retained for reuse. No item was deleted; no cleanup is requested by this report.

## Historical candidate and merge (19727aa)

- Candidate: `19727aab6a2dcc0e1c5f1b7fc9db53b1110d6c37`.
- Branch: `fix/216-deferred-similarity`. HEAD matched the candidate and the tree was clean before
  verification; it remained clean through the builds and installed-check attempt.
- After `git fetch origin`, main was still
  `70859f6bc09811d90305ec69cfec5d808b77cb5c`: **no merge needed**.
- The #169 release conditions were checked before starting: its user unit was inactive and
  `rpm -q second-brain` reported no installed package.
- Windows reused the earlier build-gate candidate checkout by fetching a full-history bundle and
  checking out the candidate detached. Its tree was clean before and after verification.
  `CARGO_TARGET_DIR` pointed at the existing shared Rust target cache. The other checkout's
  tracked files and the #169 Windows test install were left alone. Nothing was installed on Windows.
- The required Sol usage probe returned `OK` before each verification run. No fallback agent ran
  any verification recorded here.
- No push, pull request, rebase, code edit, or artifact cleanup was performed. This report does not
  claim the final review required after the report commit.

Evidence for these conditions and the seven named tests:
[verification.json](evidence/ticket-216-2026-10-17/verification.json). This file records the first run's verdict, `VERIFY: FAIL`, because it stopped before the installed steps. The later installed recovery runs and the new candidate verification supersede that verdict; the historical evidence file is unchanged.

## Historical command results (19727aa)

Every result below concerns the candidate above. Earlier build-gate evidence on `c33a9be` was
read for context and was not substituted for a run here.

### Fedora

| Command | Result | Counts and observation |
| --- | --- | --- |
| `pnpm verify` | PASS, exit 0 | All five constituent commands below passed. |
| `pnpm check` (inside verify) | PASS | Svelte: 0 errors, 0 warnings. |
| `pnpm test` (inside verify) | PASS | 253 tests; 253 passed, 0 failed, 0 skipped. |
| `pnpm test:rust` (inside verify) | PASS | Library: 704 listed, 694 passed, 0 failed, 10 ignored. Main and doc-test binaries each ran 0 tests. |
| `pnpm lint:rust` (inside verify) | PASS | Formatting check passed; Clippy completed with no warnings or errors. |
| `pnpm build` (inside verify) | PASS | One static site; 263 server modules and 2,691 client modules transformed. |
| `pnpm tauri:build` | PASS, exit 0 | One RPM built. |
| `scripts/verify-linux-package.sh <candidate RPM>` | PASS, exit 0 | Packaged identifier desktop entry exists and matches its source byte for byte. Extraction was retained by intercepting the cleanup command, without changing the verifier's checks. |
| Test-package helper `install-candidate <candidate RPM>` | PASS, exit 0 | Installed binary contains the exact 40-character candidate SHA. |
| Test-package helper `remove-test-install` | PASS, exit 0 | Package, app binary, and both possible desktop entries absent afterwards. |

[Selected verify output](evidence/ticket-216-2026-10-17/fedora-verify.txt),
[package build output](evidence/ticket-216-2026-10-17/package-builds.txt), and
[installed-package facts and hashes](evidence/ticket-216-2026-10-17/installed-package.json).

### Windows

Free space was checked before candidate preparation: C: 51,715,661,824 bytes (48.16 GiB);
D: 16,380,592,128 bytes (15.26 GiB). D: exceeded both 15 GB and the more conservative 15 GiB floor.
A second check before the NSIS build also cleared 15 GiB. At the final inventory, D: had
16,417,280,000 bytes free. No space was freed by discretionary cleanup.

| Command | Result | Counts and observation |
| --- | --- | --- |
| `pnpm test:rust` | PASS, exit 0 | Library: 678 listed, 669 passed, 0 failed, 9 ignored. |
| `pnpm lint:rust` | FAIL, allowed #221, exit 101 | Formatting passed. One distinct Clippy error, `bind_instead_of_map`, reported for library and library-test builds. It is the unchanged `std::fs::copy(...).and_then(...)` expression at `src-tauri/src/commands.rs:4306`, line 4295 on main. No other lint error occurred. |
| `pnpm build` | PASS, exit 0 | One static site; 263 server modules and 2,691 client modules transformed. |
| `pnpm tauri:build` | PASS, exit 0 | One x64 NSIS installer built. One informational linker-output warning reported the creation of import-library files. Nothing was installed. |

Windows `pnpm test` was not part of the requested Windows command list and was not run; the three
known JavaScript failures from #221 were therefore not exercised here.

[Selected Rust output](evidence/ticket-216-2026-10-17/windows-test-rust.txt),
[complete lint failure](evidence/ticket-216-2026-10-17/windows-lint-rust.txt), and
[package build output](evidence/ticket-216-2026-10-17/package-builds.txt).

### Named Rust tests

All seven ran and passed on both machines; none of these results is an ignored or filtered test.
The population is the exact seven fully qualified names below, matched against each full Rust log.

| Test | Fedora | Windows |
| --- | --- | --- |
| `similar_notes::tests::an_unreadable_deferred_file_gives_an_empty_list_and_the_vault_still_opens` | PASS | PASS |
| `semantic_search::tests::a_failed_similarity_check_is_recorded_and_survives_reloading_its_file` | PASS | PASS |
| `semantic_search::tests::a_deferred_check_runs_after_the_backend_returns_and_its_card_is_delivered` | PASS | PASS |
| `semantic_search::tests::a_moved_note_still_gets_its_deferred_check_and_a_deleted_one_is_dropped` | PASS | PASS |
| `semantic_search::tests::a_deferred_check_that_fails_again_stays_recorded` | PASS | PASS |
| `semantic_search::tests::a_pass_that_stopped_or_found_the_queue_empty_reruns_nothing` | PASS | PASS |
| `semantic_search::tests::a_pass_that_leaves_notes_queued_reruns_nothing_until_the_queue_is_through` | PASS | PASS |

The 10 Fedora and 9 Windows ignored tests are outside this seven-test population. No ignored live
or hardware test is claimed as passed.

## First installed-package attempt: stopped before step 1

The candidate RPM was installed through the helper, and its installed binary contained the exact
candidate SHA. Before installation, configuration and state were backed up: eight current and
legacy profile roots were inventoried, with two present. The controller repeated that backup
before starting its isolated run. The intended app profile had separate XDG configuration, data,
state, and cache roots; no production vault would be opened.

The controller started its own loopback SSH forwarding process to the existing user-owned
embedding backend. The loopback listener existed, but the HTTP preflight did not complete.
An explicit `curl --silent --show-error --max-time 10` request to that listener's `/api/tags`
returned `curl: (52) Empty reply from server`, exit 52. The run stopped there. This establishes a
failed forwarding/backend preflight, not its underlying cause and not a product-code failure.

The exact owned tunnel PID was checked against its forwarding arguments and its controller's
run environment, then stopped. That intentional stop caused the controller's secondary
“tunnel exited” error. Its suggestion about an occupied port is generic helper text, not a
diagnosis. The existing desktop Ollama and existing tunnels were not stopped or reconfigured.

| Required step | Result | Observation source |
| --- | --- | --- |
| 1. Save a near-duplicate and a second capture offline; observe two skip logs and two durable records. | NOT RUN | Controller stopped at backend preflight; no fixture vault or app process was created. |
| 2. Quit/restart, then delete the second capture in the app. | NOT RUN | Step 1 never started. No fixture note was deleted. |
| 3. Restore the real app backend URL. | NOT RUN | No app configuration or backend URL was changed. |
| 4. Observe the duplicate card, no OS notification, `ran=1 found=1 dropped=1 waiting=0`, and an empty record list. | NOT RUN | No indexing pass, card, notification observation, or screenshot was produced. |

[Controller observations and the primary failure](evidence/ticket-216-2026-10-17/installed-check.json).

Cleanup completed: the controller found the original profile hashes and existence states unchanged,
with zero installed app processes left. The gate-owned tunnel was absent. The test-package helper
removed the RPM; `rpm -q second-brain` reported no package, and the app binary and both desktop
entries were absent. Backups and other evidence remain; no artifact cleanup was attempted.

### Follow-up with a temporary Windows server

The follow-up used the same candidate, with starting report HEAD
`8da341274cfee8ffbe0adf4610da20bcebe68777` on the requested branch and a clean tree. The required
usage probe returned `OK`. Earlier command-gate results above still apply: the report commit
changed no product code, and no build or test suite was repeated.

Sol followed the temporary-server launch procedure in `docs/alpha-harness.md`: copied
`scripts/windows/ollama-gate.ps1` and its unchanged `owned-process.cs` companion into a fresh
Windows run directory, then held the supervisor open in one SSH session from Fedora. The lease
was 7,200 seconds, on port 11436, using the installed Ollama and existing model store. Nothing
was downloaded. The user's service and its configuration were left alone.

| Observation | Evidence |
| --- | --- |
| Supervisor | PID `45132`, created `2026-10-18T02:17:11.9355940Z`. |
| Server | PID `49888`, created `2026-10-18T02:17:13.1832930Z`. |
| First `/api/tags` answer | `2026-10-18T02:17:27.1012416Z`; port owner `49888`; `embeddinggemma:latest` present. |
| Second `/api/tags` answer | `2026-10-18T02:18:07.2141760Z`; same server PID and creation time; model still present. |
| Required stability interval | **INCONCLUSIVE here; the controller recorded FAIL** in `installed-retry.json`: 40.112935 seconds, below the 60-second minimum. This report relabels that recorded FAIL because the probe was early, not because the server failed. The controller rejected it instead of waiting and probing again; the same server identity answered. |

The model digest was
`85462619ee721b466c5927d109d4cb765861907d5417b9109caebc4e614679f1`.
Eight production profile roots were inventoried before any possible installation, with two
present; both were backed up. No forwarding tunnel, fixture vault, installed app process, or
WebDriver session was started. The candidate RPM was not installed during this follow-up, so
its installed SHA was not checked again.

| Required step | Follow-up result | Evidence |
| --- | --- | --- |
| 1. Two offline captures, two skip log lines, and two durable records. | FAIL: not run | Stability assertion stopped the run before installation; no captures, skip logs, or record file were produced. |
| 2. Restart, then delete the second capture in the app. | FAIL: not run | No app session or fixture existed. Nothing was deleted. |
| 3. Restore the app's backend URL to forwarded port 11436. | FAIL: not run | No tunnel or isolated app configuration was created; no URL changed. |
| 4. Matching card after indexing, no notification, one ran/one dropped, and no records left. | FAIL: not run | No indexing, card, notification monitor, deferred summary, or screenshot was produced. |

The FAIL labels mean the requested acceptance proof was not completed; none claims that the
app's behavior failed. No screen-only obstacle was reached. The second follow-up below obtained
two successful replies more than 70 seconds apart, then completed all four steps on the installed
candidate. Unit tests were not substituted for these observations.

Cleanup stopped only the exact supervisor PID after checking its creation time. Closing its Job
Object terminated the server and observed descendant `conhost.exe` PID `1784`, created
`2026-10-18T02:17:14.6861840Z`. All three PIDs and the gate listener were confirmed absent;
the held SSH session ended after the deliberate supervisor stop. Only `/api/tags` requests had
been sent, so no embedding model was loaded and no model worker was observed. No process-name
sweep was used.

On Fedora, `rpm -q second-brain` returned exit 1 with no installed package; the app binary and
both possible desktop entries were absent. No removal was needed because this follow-up never
installed the package. Original production profile contents and existence states matched their
pre-run hashes, and no installed app process remained. No artifact was deleted.

[Follow-up observations, timing assertion, and cleanup](evidence/ticket-216-2026-10-17/installed-retry.json).

### Second follow-up: installed recovery passed

Starting report HEAD was `82193338e95255ecffbc211192e0121eb1b698f6` on the requested branch;
the tree was clean. It contains the same product code as candidate
`19727aab6a2dcc0e1c5f1b7fc9db53b1110d6c37`, so the earlier command-gate results still apply.
The required Sol usage probe returned `OK`. No build or test suite was repeated.

The unchanged bounded supervisor and its companion were copied into a fresh Windows test run.
One dedicated SSH session held it open on port 11436 with a 7,200-second lease, using the installed
Ollama and its existing model store. No download was requested; the user's service, processes,
and configuration were left alone.

| Observation | Evidence |
| --- | --- |
| Supervisor | PID `14528`, created `2026-10-18T03:09:21.6921760Z`. |
| Server | PID `42716`, created `2026-10-18T03:09:22.8510270Z`. |
| First `/api/tags` answer | `2026-10-18T03:09:32.6092032Z`; listener owner `42716`; `embeddinggemma:latest` present. |
| Stability answer | `2026-10-18T03:11:18.6225122Z`; **106.013309 seconds later**, with the same server PID and creation time; model still present. |
| Final Windows endpoint check | `2026-10-18T03:12:01.1204209Z`; same server identity and model, after the installed check. |

Before installation, production profile backups were retained. The committed observations record unchanged profiles, and the retained-path inventory groups the private backups; they do not publish an eight-root/two-present inventory for this historical run. The helper installed the existing 30,178,896-byte RPM; the installed
binary contained the exact candidate SHA. Its SHA-256 was
`90f4fba4a3f6a3f18e93806803f5515f7d6d205db649becac89ebc5016d5df7f`.
A fresh disposable vault held the first three fixture notes. Configuration, data, state, and cache
used isolated XDG roots. Only the app's isolated URL changed: closed port 11447 while offline,
then forwarded port 11436 when reachable. All three app sessions passed DOM, IPC, configuration,
and screenshot health checks before being driven.

The installed check ran from `2026-10-18T03:11:36.472Z` to `2026-10-18T03:11:52.321Z`
(2026-10-17 locally), through WebDriver and the app's actual capture/deletion commands.

| Required step | Result | Observation and evidence |
| --- | --- | --- |
| 1. Two offline captures, two skip lines, two durable records. | PASS | `Shed spending` and `Shed wall wood` were saved. Two `Skipped the similarity check for a new note` lines were captured at 03:11:39Z. Two generated note IDs were present in the machine-local file; no card appeared. [Offline records](evidence/ticket-216-2026-10-17/installed-recovery-offline-deferred.json). |
| 2. Restart, then delete the second capture in the app. | PASS | The first process exited and a new app session still read two records. `delete_note` removed `Shed wall wood` from its live path; both records remained pending for the rerun. [Records after deletion](evidence/ticket-216-2026-10-17/installed-recovery-after-delete-deferred.json). |
| 3. Restore the app's backend URL. | PASS | The app was stopped, the isolated configuration was set to `http://127.0.0.1:11436`, and the next session's `get_app_config` confirmed that URL. Both the forwarded and Windows endpoints answered. |
| 4. Card after indexing, no notification, one ran/one dropped, no records. | PASS | Four notes indexed, zero queued. One card said `“Shed spending” is similar to` and named `Garden shed budget`. The controller observed `Deferred similarity checks: ran=1 found=1 dropped=1 waiting=0` and an empty record list. The live session-bus monitor counted zero app `Notify` calls across all sessions. [Empty records](evidence/ticket-216-2026-10-17/installed-recovery-empty-deferred.json); [card screenshot](evidence/ticket-216-2026-10-17/installed-recovery-card.png). |

This installed run stopped the app, edited its isolated configuration, and relaunched it at step 3. It proves recovery after restarting with a reachable backend. A running app recovering from an outage was covered only by unit tests in this historical run: `semantic_search::tests::a_deferred_check_runs_after_the_backend_returns_and_its_card_is_delivered` and `semantic_search::tests::a_deferred_check_that_fails_again_stays_recorded`. Zero observed `Notify` calls has no positive control in this run. Source reading also backs that result: `rerun_deferred` calls `show_card` and never `notify` in `src-tauri/src/similar_notes.rs`.

The controller header should say: from the repository root, run `SB216_RUN_ROOT=<private root> SB216_SSH_TARGET=<alias> node --input-type=module < docs/reports/evidence/ticket-216-2026-10-17/installed-recovery-controller.mjs`. The committed controller remains unchanged.

[Complete scrubbed observations](evidence/ticket-216-2026-10-17/installed-recovery.json) include
session PIDs, health results, the actual skip lines, the indexing status, and cleanup facts.
The [controller](evidence/ticket-216-2026-10-17/installed-recovery-controller.mjs) records how the
observations were made; it asserts the deferred summary against the app log before recording it.
The screenshot was inspected and contains only fixture text. No step needed a person at the screen.
An optional post-run raw-log display encountered a binary database `.log` and stopped with a
UTF-8 decode error. No verification continued or reran after that display error; the completed
controller's saved evidence supplies the results above.

While the model was loaded, the server's process tree contained `llama-server.exe` PID `59648`
and console hosts `47992` and `28772`; their creation times are in the evidence. Cleanup checked
the supervisor's and server's creation times, then stopped only supervisor `14528`. The Job Object
terminated the server and descendants. All five exact PIDs and the gate listener were confirmed
absent. The held SSH session exited after that deliberate stop. The user's port 11434 had no
listener after the run; no user-owned process was stopped or adopted.

On Fedora, the app exited normally, and all recorded app, tunnel, and notification-monitor PIDs
were absent. Driver and forwarding ports had no listeners. The helper removed the RPM with exit 0;
`rpm -q second-brain` returned exit 1, and the app binary and both desktop entries were absent.
The original production profiles matched their pre-install contents and existence states, so no
restoration was needed. No artifact was deleted. The only fixture-note deletion was the second
capture through the app, as required by step 2.

## Original review record and ownership (rounds 1–2)

These are the prior review results supplied with the task and recorded in the local review
artifacts, not new reviews performed during this verification run.

1. Round 1, `c33a9be8286dee837dc3ccd3b310b9887a45913f`: Standards CLEAN by Sol; Spec by a Sonnet
   stand-in at Sol's usage limit. One P2 and four P3s were addressed in
   `7e8ff9f496f7014436c4220f90af33c95650942c`. The P2 was spending a deferred check against a
   partly filled index. The four changed P3s concerned the SPEC's removal wording, continuing
   through repeated backend failures, locating a note moved while the app was closed, and
   reading identity only after a slow failed check.
2. Round 2, `7e8ff9f496f7014436c4220f90af33c95650942c`: both axes CLEAN by Sonnet stand-ins,
   with P3 observations. Claude settled the accepted changes in
   `19727aab6a2dcc0e1c5f1b7fc9db53b1110d6c37`: quieter logs, the unreadable-note test, explicit
   SPEC limits, and corrected comments. Updating the historical #9 report belongs to this
   report commit; it is now labelled settled after the second follow-up passed.

At round 2, the judged P3s left in the code were a card lost if the main window is destroyed (ordinary close
is prevented), two file races requiring a window of a few milliseconds, and one full vault scan
per deleted note's deferred check. These were the decisions at round 2, not a current list of unresolved findings; the later review history above supersedes them. The repeated vault scan was removed in `3c7f658`. Earlier notes about the Tauri delivery wrapper and removal
of value-equal records were judged without a code change. The wrapper delivered the card in the
second follow-up's installed recovery run.

Kiro's imported commits:

- `43d24a1427ef76621430ee86378cbd8aeaf140c9`: record and rerun failed similarity checks.
- `509ab6e150f114f6e3f8bd61aca82449771f2483`: deferred-check tests.
- `5de61cbaea6006943c912531a92e6d88a6d2fc82`: SPEC rule.

Claude's first three fix commits:

- `c33a9be8286dee837dc3ccd3b310b9887a45913f`: intake documentation of machine-local state.
- `7e8ff9f496f7014436c4220f90af33c95650942c`: round-1 fixes.
- `19727aab6a2dcc0e1c5f1b7fc9db53b1110d6c37`: accepted round-2 fixes.

## Retained paths

[retained-paths.json](evidence/ticket-216-2026-10-17/retained-paths.json) lists retained artifact
roots from the first run, their byte sizes, and the individual gate-evidence files, with private
profile backups grouped. The follow-up's additional private scripts, logs, profile backup, and two
Windows supervisor source files are listed separately in
[installed-retry-retained-paths.json](evidence/ticket-216-2026-10-17/installed-retry-retained-paths.json).
The second follow-up's inventory is
[installed-recovery-retained-paths.json](evidence/ticket-216-2026-10-17/installed-recovery-retained-paths.json).

| Additional retained path | Logical file bytes |
| --- | ---: |
| Fedora private run `byhand2-19727aa-4ppeqwan` | 2,554,673 |
| Windows test root `sb216/byhand2-19727aa-4ppeqwan` | 6,016 |
| Private run pointer `byhand2-current.private.txt` | 62 |
| Existing candidate RPM (included in the earlier Rust-target total) | 30,178,896 |

Paths use logical machine/run/temp roots so the evidence contains no private machine paths. Build-cache
totals include pre-existing contents and must not be read as bytes created by this run. Nested
installer/RPM entries are included in their parent totals.

A shell-quoting error in the first Windows disk query and a transfer-directory typo were confined
to throwaway setup commands. They were corrected before Windows verification, without a product
edit. The typo left one empty directory, included in the inventory. The existing Windows install
was left in place.

Disposable run directories, copied bundles, retained package extraction, and the two machines'
test-created temporary state are cleanup candidates only after item-specific approval. Build
caches are retained for reuse. No deletion is requested or performed by this report.

All four installed recovery steps and cleanup passed on `19727aa`. Current-candidate results and the later review rounds appear above. No earlier result substitutes for a new-candidate command gate.
