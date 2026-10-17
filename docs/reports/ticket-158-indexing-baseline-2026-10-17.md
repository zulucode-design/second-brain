# Issue #158, attempt 1: indexing baseline verification

Finished: 2026-10-17. Owner: Sol. Outcome: **blocked**. **VERIFY: FAIL**.

Windows full verification failed in three JavaScript tests. Verification stopped at that failure, without fixes or retries. The concurrent Fedora package build was cancelled. Neither installed-app baseline ran, so this report contains no indexing performance measurements.

Issue #158 stays open. This attempt adds measurement instrumentation; it does not speed indexing up. Its baseline acceptance criterion remains unverified.

## Candidate, merge, and reviews

- Candidate and verification head: `6f30ae6d4a882e1059a395ccf5ae84b8d0658a10`.
- Measurement head: none; zero measurement runs started.
- Initial branch: `fix/158-indexing-pass-timing`, clean at the candidate.
- `git fetch origin` passed. Fetched main and merge base both remained `9eb5d20cd066465490467588228febc4a290c2df`: **no merge needed**. No bundle transfer was needed.
- Round 1 Spec: **CLEAN**, on `6f30ae6`.
- Round 1 Standards: **CLEAN**, on `6f30ae6`.
- Those reviews are earlier Sol evidence supplied for this task. No new final review was performed. The report commit needs the exact-head final review required by `AGENTS.md` before any publication.
- No push or pull request was made.

Imported Kiro commits:

| Commit | Change |
| --- | --- |
| `2219fd0` | Time each semantic indexing pass by stage, in memory only. |
| `7742105` | Type the last pass and format its times. |
| `3722201` | Show the last pass in Settings. |

Claude's fix after import: `6f30ae6`, `fix(semantic): log only the first empty pass of an Ollama outage`.

## Verification results

Every result below names the candidate above. Four Sol usage probes returned `OK`: before Fedora verification, Fedora packaging, Windows verification, and the report checks. The three Kiro skill copies matched.

| Machine | Command or stage | Result and counts |
| --- | --- | --- |
| Fedora | `pnpm verify` | **PASS**, exit 0, 48.00 seconds. |
| Fedora | `pnpm check`, inside verify | **PASS**, 0 errors, 0 warnings. |
| Fedora | `pnpm test`, inside verify | **PASS**, 253 passed, 0 failed, 0 skipped. |
| Fedora | `pnpm test:rust`, inside verify | **PASS**, 687 passed, 0 failed, 10 ignored. |
| Fedora | `pnpm lint:rust`, inside verify | **PASS**, formatting and Clippy with warnings denied. |
| Fedora | `pnpm build`, inside verify | **PASS**. |
| Fedora | `pnpm tauri:build` | **CANCELLED** after Windows failed; controller exit 143. The build had reached Rust release compilation. No completed RPM or compile verdict. |
| Fedora | `scripts/verify-linux-package.sh <rpm>` | **NOT RUN**; no RPM available. |
| Windows | Disk and checkout preflight | **PASS**, clean at the candidate. C: 49.89 GiB free; D: 26.70 GiB free, above the 15 GiB stop threshold. |
| Windows | `pnpm verify` | **FAIL**, exit 1, 86.88 seconds. |
| Windows | `pnpm check`, inside verify | **PASS**, 0 errors, 0 warnings. |
| Windows | `pnpm test`, inside verify | **FAIL**, 247 passed, 3 failed, 3 skipped, 253 total. |
| Windows | Rust tests, Rust lint, web build inside verify | **NOT RUN**; the command stopped at JavaScript tests. |
| Windows | `pnpm tauri:build` | **NOT RUN**. |
| Windows | `pnpm test:rust`, earlier build gate | **PASS, carried evidence**: 662 passed, 0 failed, 9 ignored on the same unchanged candidate. This does not make the failed full verification pass. |

Windows installation inventory returned zero matching registry entries, zero known installed executables, and zero Start-menu or desktop shortcuts. No installer was run. No candidate package was installed on either machine.

### Windows failures

| Test and location | Observed failure |
| --- | --- |
| `pnpm install fails when the prepare svelte-kit sync fails`, `tests/install-failure.test.mjs:12` | Assertion at line 37 expected output matching `/INTENTIONAL_SYNC_FAILURE/`; received `undefined\nundefined`. |
| `the test package helper exits 0 after an install and removes its staging copy`, `tests/linux-test-package.test.mjs:87` | Assertion at line 96 expected exit status 0; received `null`. |
| `the test package helper refuses to run without sudo`, `tests/linux-test-package.test.mjs:105` | Assertion at line 107 expected exit status 1; received `null`. |

The first test calls `spawnSync('pnpm', ...)`; the other two call `spawnSync('bash', ...)`. Their recorded outputs do not identify the process-launch error. No further diagnosis, environment change, code fix, or rerun was attempted. Claude owns any fixes.

A private console-log reader also encountered Windows output that was not valid UTF-8. The existing logs were copied and inspected with replacement decoding; no test was rerun. The exit-1 result and the three assertion failures come from the preserved verification log and result file.

### Named-test reconciliation

All Rust names have prefix `semantic_search::tests::`. None of these five tests is ignored.

| Test | Fedora full verify | Windows |
| --- | --- | --- |
| `a_pass_records_its_counts_and_stage_times_in_memory_only` | PASS | PASS, earlier build gate only |
| `a_pass_that_cannot_use_the_backend_records_that_it_stopped` | PASS | PASS, earlier build gate only |
| `only_the_first_empty_pass_of_an_outage_is_logged` | PASS | PASS, earlier build gate only |
| `a_pass_that_sends_nothing_to_the_backend_keeps_the_earlier_record` | PASS | PASS, earlier build gate only |
| `the_status_carries_the_last_pass_in_the_shape_settings_reads` | PASS | PASS, earlier build gate only |
| `tests/indexing-pass.test.mjs` | PASS, all 5 tests | PASS, all 5 tests in the failed full verify |

The five JavaScript cases cover sub-minute seconds, minute formatting, rounding across a minute, the pass summary, and stage labels in order. Their exact names and both Rust evidence sources appear in [verification evidence](evidence/158-attempt-1-2026-10-17/verification.json). The earlier Windows Rust results apply because the checkout and candidate did not change. They were not run again after the full-verification failure.

## Baseline protocol: planned, not executed

This protocol follows `scripts/perf-fixture.mjs`, `perf-interactive.mjs`, `perf-startup.mjs`, `perf-summary.mjs`, the Windows helpers, and the #88 performance matrix. These are conditions for a future run, not claims about this stopped attempt.

1. **Packages and identity.** Build both packages on one head. Check the RPM with the package-verification script, install it through the privileged test-package helper, and remove it through that helper afterward. Before a Windows install, inventory existing installs and stop for any install outside the test area. Install the NSIS package silently into the test app directory. Hash both installed binaries and confirm they contain the full candidate commit.
2. **Isolation and recovery.** Back up existing configuration bytes and hashes before changing app state. Use isolated Fedora XDG config/data roots. Journal Windows config, window geometry, logs, WebView data, and machine-local vault state as the alpha harness requires; restore original state on success or failure. Retain scratch artifacts rather than deleting them without approval. Keep a process ledger with PID and creation identity, and hold both desktops awake and unlocked.
3. **Fixture and cold state.** Create a fresh vault identity and run `node scripts/perf-fixture.mjs generate <vault>` on each machine. Verify 10,000 notes, 19,998 explicit links, and the 5,000-word editor note; compare a canonical content digest between machines. Confirm that machine-local keyword and semantic indexes are absent before opening. Cold means an empty index; do not claim cold OS page caches or unload a user-owned model.
4. **Embedding topology.** Use `embeddinggemma` on the Windows desktop for both runs, sequentially. Prefer an already-running user-owned Ollama service after checking its endpoint and model; leave it unchanged. Otherwise use the bounded job supervisor on port 11436, held open through a dedicated SSH session, with the delayed same-PID liveness check required by the harness. Windows uses loopback directly; Fedora uses an explicitly recorded loopback SSH forward to the same server. Record the actual ports, server ownership, model identity, and `/api/ps` loaded state before each run. No backend was selected or contacted in this attempt.
5. **Cold rebuild interval.** Run once per machine in the installed app. Record a monotonic start immediately before the configured fresh vault opens and stop at the first Settings observation of exactly 10,000 indexed and zero waiting. Preserve timestamped progress observations and state the polling interval. Divide 10,000 by wall seconds for unique-note throughput, and completed chunk count by wall seconds for chunk throughput. Record model-loaded state without a warm-up request. Do not repeat a failed run to improve its number.
6. **Pass accounting and Settings.** Collect every `Semantic indexing pass:` line in the run interval. Parse the complete allowed numeric/boolean field set; reject extra payload containing a path, title, body, or query. Sum notes, chunks, `total_ms`, `read_ms`, `chunk_ms`, `embed_ms`, and `database_ms`; each stage share is its sum divided by summed `total_ms`. Keep wall-clock time separate from summed pass time. Check the sum reaches the 10,000-note fixture; account separately for repeat embedding work caused by probe edits. Capture the rendered Settings summary and four stage labels, and compare formatted values with the final log record. Capture the null row before the first pass if observable.
7. **Memory and foreground work.** During the same rebuild, sample app RSS and the Ollama server/worker tree separately at a stated interval, retaining process identities and sample timestamps. On Windows record the working-set equivalent and distinguish observed peaks from lifetime peak counters. Reuse the existing editor probe and native Fedora/WebDriver Windows input for 200 keys in the 5,000-word note; require all keys retained. Run a keyword query using the existing `keyword-search` probe in the same app session. Bracket probes with status observations proving indexing was active, and label every sample **during indexing**. Preserve raw timing-only samples and report count, median, nearest-rank p95, and maximum using the summary tooling's statistics. #88's editor limits are p95 ≤ 50 ms and maximum ≤ 100 ms. #88 defines no keyword-specific latency budget; report that measurement without inventing a threshold or applying the semantic-search budget to it. Record any probe-induced note changes and repeat indexing work.
8. **Teardown and proof.** Close the app through its save-aware path, then stop the exact launched driver, tunnel, awake holders, and any owned Ollama supervisor. Verify all recorded process identities, including model workers, are gone. Leave user-owned Ollama alone. Restore original config/profile hashes even on failure. Confirm evidence survives cleanup. The Fedora helper removal is the requested package teardown; list all other retained artifacts and sizes for approval instead of deleting them.

### Baseline results

| Measure | Fedora | Windows |
| --- | --- | --- |
| Installed candidate / measurement head | Not installed / none | Not installed / none |
| Fresh 10,000-note fixture / rebuild runs | Not generated / 0 | Not generated / 0 |
| Backend endpoint, ownership, model-loaded state | Not observed | Not observed |
| Cold rebuild wall time | Not measured | Not measured |
| Notes/s and chunks/s | Not measured | Not measured |
| Summed pass notes, chunks, total time | Not measured | Not measured |
| File reads, Chunking, Ollama, Database totals and shares | Not measured | Not measured |
| Settings final-log match / null state | Not observed | Not observed |
| Pass-log privacy check | Not run; no installed app log | Not run; no installed app log |
| Peak app / Ollama memory | Not measured | Not measured |
| Editor latency during indexing | Not measured | Not measured |
| Keyword latency during indexing | Not measured | Not measured |

## State left behind and evidence

No baseline vault, configuration backup, new bundle, installed package, app, driver, model server, or tunnel was created. Configuration restoration was unnecessary because configuration was never changed. Fedora retained build outputs and two test scratch directories; Windows retained verification logs and generated typecheck files. The earlier build-gate evidence and its bundle were left intact.

The Fedora build was cancelled by its recorded process group. All 17 original process identities were absent afterward. The Windows final observation found no remaining gate processes and a clean checkout. No manual file deletion was performed; the requested test commands perform their own internal fixture cleanup.

Committed evidence contains only counts, test names, outcomes, commit IDs, and raw-log digests. Private raw logs, paths, and the artifact inventory remain outside the repository. The final handoff lists retained locations and sizes. These scratch artifacts may be removed only after approval; no removal is required to preserve this failure record.

- [Verification, named tests, and source-log hashes](evidence/158-attempt-1-2026-10-17/verification.json)
- [Three Windows assertion failures](evidence/158-attempt-1-2026-10-17/windows-failures.json)
- [Process cleanup and untouched-state evidence](evidence/158-attempt-1-2026-10-17/cleanup.json)

Next work belongs to Claude: address the Windows verification failures. Then Sol must rerun the required gates on the resulting head, complete the two installed-app baselines, update this record, and obtain the exact-head final review before publication. No indexing-speed conclusion can be drawn from this attempt.
