# Issue #158, attempt 1: indexing baseline verification

Run 3 ended: 2026-10-17. Owner: Sol. Outcome: **stopped on Windows Rust lint**. **VERIFY: FAIL**.

`pnpm lint:rust` failed on Windows at `src-tauri/src/commands.rs:4295`: Clippy's `bind_instead_of_map` is denied by `-D warnings`. The file is unchanged from main. The exemption for the three JavaScript failures tracked in [#221](https://github.com/zulucode-design/second-brain/issues/221) does not cover this failure. The run stopped as instructed; the concurrent Fedora package build was cancelled. No package was installed and no baseline was measured.

Issue #158 stays open. This attempt adds measurement instrumentation; it does not speed indexing up. Its baseline acceptance criterion remains unverified. No product code was changed during verification.

## Candidate, merge, and reviews

- Reviewed product candidate: `6f30ae6d4a882e1059a395ccf5ae84b8d0658a10`.
- Fresh-build and selected measurement head on both machines: `8c2a375136d81f91a8694095fb9be6e6447c7921`. Neither measurement started.
- `git diff --quiet 6f30ae6 HEAD -- . ':!docs'` exited 0 before builds. Both intervening commits changed only documentation, so the specified verification results on `6f30ae6` carry. Fresh build results below belong to `8c2a375`.
- Delivery branch: `fix/158-indexing-pass-timing`, clean at `8c2a375` when run 3 began. Fedora built directly from that worktree. Windows received a bundle and checked out the same head detached. No new checkout, worktree, or dependency symlink was created.
- Fresh `git fetch origin` found main still at `9eb5d20cd066465490467588228febc4a290c2df`: **no merge needed**.
- Round 1 Spec and round 1 Standards: **CLEAN**, both on `6f30ae6`. No new final review was performed. This report commit needs an exact-head final review before publication.
- No push or pull request was made. No commit was amended, reset, or rebased.

Imported Kiro commits:

| Commit | Change |
| --- | --- |
| `2219fd0` | Time each semantic indexing pass by stage, in memory only. |
| `7742105` | Type the last pass and format its times. |
| `3722201` | Show the last pass in Settings. |

Claude's fix after import: `6f30ae6`, `fix(semantic): log only the first empty pass of an Ollama outage`.

## Verification results

Carried results name their original candidate. The Kiro skill copies matched. Sol's usage probe returned `OK` before the build/test work; another probe for Windows web build completed, but that build never started after lint failed.

| Machine | Command or stage | Candidate | Result and counts |
| --- | --- | --- | --- |
| Fedora | `pnpm verify` | `6f30ae6` | **PASS, carried**, exit 0, 48.00 seconds. |
| Fedora | `pnpm check`, inside verify | `6f30ae6` | **PASS, carried**, 0 errors, 0 warnings. |
| Fedora | `pnpm test`, inside verify | `6f30ae6` | **PASS, carried**, 253 passed, 0 failed, 0 skipped. |
| Fedora | `pnpm test:rust`, inside verify | `6f30ae6` | **PASS, carried**, 687 passed, 0 failed, 10 ignored. |
| Fedora | `pnpm lint:rust`, inside verify | `6f30ae6` | **PASS, carried**, formatting and Clippy with warnings denied. |
| Fedora | `pnpm build`, inside verify | `6f30ae6` | **PASS, carried**. Run 3's package build also completed its web substage before cancellation. |
| Fedora | `pnpm tauri:build` | `8c2a375` | **CANCELLED** after Windows lint failed. Normal dependency handling, sidecar preparation, web build, and Rust release compilation were reached; no finished RPM. |
| Fedora | `scripts/verify-linux-package.sh <rpm>` | `8c2a375` | **NOT RUN**; no RPM. |
| Windows | Checkout, install inventory, disk preflight | `8c2a375` | **PASS**. Clean detached checkout; code parity confirmed; no existing test or non-test install found. C: 49.81 GiB free; D: 23.15 GiB free, above 15 GiB. |
| Windows | `pnpm check`, first run | `6f30ae6` | **PASS, carried**, 0 errors, 0 warnings. |
| Windows | `pnpm test`, run 2 | `6f30ae6` | **FAIL, permitted #221 failures only, carried**, exit 1: 247 passed, 3 failed, 3 skipped, 253 total; no cancelled tests. |
| Windows | `pnpm test:rust`, run 2 | `6f30ae6` | **PASS, carried**, exit 0: 662 passed, 0 failed, 9 ignored. |
| Windows | `pnpm lint:rust`, run 3 | `8c2a375` | **FAIL**, exit 101 after 117.58 seconds. Formatting passed; Clippy reported one error at `src-tauri/src/commands.rs:4295`. |
| Windows | `pnpm build` | `8c2a375` | **NOT RUN**. |
| Windows | `pnpm tauri:build` | `8c2a375` | **NOT RUN**. |

Windows `pnpm verify` was not repeated. Its first-run exit at the JavaScript stage remains historical evidence; run 2 ran JavaScript and Rust tests separately. Neither installed-binary identity check could run because neither package was installed.

### Windows Rust lint blocker

Clippy reports: `using Result.and_then(|x| Ok(y)), which is more succinctly expressed as map(|x| y)`. The diagnostic is `clippy::bind_instead_of_map`, denied through `-D warnings`; both library targets consequently fail with exit 101.

The call is in `damaged_config`. Its closure applies owner-only file permissions under `#[cfg(unix)]`, then returns `Ok(())`. On Windows, that conditional block is absent. This explains why the carried Fedora lint result can pass while Windows fails; no fix or retry was attempted. `git diff --quiet 9eb5d20 HEAD -- src-tauri/src/commands.rs` exited 0, so the failing file is unchanged from main. This is outside the explicit #221 exemption and blocks the run.

### Known Windows JavaScript failures: #221

The run-2 evidence carries unchanged:

| Test | Result |
| --- | --- |
| `pnpm install fails when the prepare svelte-kit sync fails` | Expected `/INTENTIONAL_SYNC_FAILURE/`; received `undefined\nundefined`. |
| `the test package helper exits 0 after an install and removes its staging copy` | Expected exit status 0; received `null`. |
| `the test package helper refuses to run without sudo` | Expected exit status 1; received `null`. |

Run 2 confirmed exactly those names and no additional JavaScript failures. Its spawn probe returned `ENOENT` for both `pnpm` and `bash`: pnpm is provided by a Windows `.cmd` shim and bash is absent from PATH. The Windows CI job runs `pnpm test:rust`, not JavaScript tests. Run 3 reconfirmed that the involved tests, helper script, and `package.json` are unchanged from main. All 247 other executed JavaScript tests passed; three platform skips remain.

### Named-test reconciliation

All Rust names have prefix `semantic_search::tests::`; none is ignored. Both columns carry results on `6f30ae6`: Fedora from first-run full verification, Windows from run 2.

| Test | Fedora | Windows |
| --- | --- | --- |
| `a_pass_records_its_counts_and_stage_times_in_memory_only` | PASS | PASS |
| `a_pass_that_cannot_use_the_backend_records_that_it_stopped` | PASS | PASS |
| `only_the_first_empty_pass_of_an_outage_is_logged` | PASS | PASS |
| `a_pass_that_sends_nothing_to_the_backend_keeps_the_earlier_record` | PASS | PASS |
| `the_status_carries_the_last_pass_in_the_shape_settings_reads` | PASS | PASS |
| `tests/indexing-pass.test.mjs` | PASS, 5/5 | PASS, 5/5 |

The JavaScript cases cover sub-minute seconds, minute formatting, rounding across a minute, the pass summary, and stage labels in order. Their exact names and source-log hashes remain in the evidence.

## Baseline protocol and execution

The eight-step protocol below remains the measurement method, derived from the existing #88 tooling. Run 3 stopped in step 1 at Windows Rust lint. Steps 2 through 8 were not exercised. No fixture, configuration swap, app launch, or measurement began. The process-stop proof after the results covers build processes only.

1. **Packages and identity.** Build both packages on one head. Check the RPM with the package-verification script, install it through the privileged test-package helper, and remove it through that helper afterward. Before a Windows install, inventory existing installs and stop for any install outside the test area. Install the NSIS package silently into the test app directory. Hash both installed binaries and confirm they contain the full candidate commit.
2. **Isolation and recovery.** Back up existing configuration bytes and hashes before changing app state. Use isolated Fedora XDG config/data roots. Journal Windows config, window geometry, logs, WebView data, and machine-local vault state as the alpha harness requires; restore original state on success or failure. Retain scratch artifacts rather than deleting them without approval. Keep a process ledger with PID and creation identity, and hold both desktops awake and unlocked.
3. **Fixture and cold state.** Create a fresh vault identity and run `node scripts/perf-fixture.mjs generate <vault>` on each machine. Verify 10,000 notes, 19,998 explicit links, and the 5,000-word editor note; compare a canonical content digest between machines. Confirm that machine-local keyword and semantic indexes are absent before opening. Cold means an empty index; do not claim cold OS page caches or unload a user-owned model.
4. **Embedding topology.** Use `embeddinggemma` on the Windows desktop for both runs, sequentially. Prefer an already-running user-owned Ollama service after checking its endpoint and model; leave it unchanged. Otherwise use the bounded job supervisor on port 11436, held open through a dedicated SSH session, with the delayed same-PID liveness check required by the harness. Windows uses loopback directly; Fedora uses an explicitly recorded loopback SSH forward to the same server. Record the actual ports, server ownership, model identity, and `/api/ps` loaded state before each run. No backend was selected for a measurement. Run 3 only checked the default Windows service endpoint; it was unavailable, and no server was started.
5. **Cold rebuild interval.** Run once per machine in the installed app. Record a monotonic start immediately before the configured fresh vault opens and stop at the first Settings observation of exactly 10,000 indexed and zero waiting. Preserve timestamped progress observations and state the polling interval. Divide 10,000 by wall seconds for unique-note throughput, and completed chunk count by wall seconds for chunk throughput. Record model-loaded state without a warm-up request. Do not repeat a failed run to improve its number.
6. **Pass accounting and Settings.** Collect every `Semantic indexing pass:` line in the run interval. Parse the complete allowed numeric/boolean field set; reject extra payload containing a path, title, body, or query. Sum notes, chunks, `total_ms`, `read_ms`, `chunk_ms`, `embed_ms`, and `database_ms`; each stage share is its sum divided by summed `total_ms`. Keep wall-clock time separate from summed pass time. Check the sum reaches the 10,000-note fixture; account separately for repeat embedding work caused by probe edits. Capture the rendered Settings summary and four stage labels, and compare formatted values with the final log record. Capture the null row before the first pass if observable.
7. **Memory and foreground work.** During the same rebuild, sample app RSS and the Ollama server/worker tree separately at a stated interval, retaining process identities and sample timestamps. On Windows record the working-set equivalent and distinguish observed peaks from lifetime peak counters. Reuse the existing editor probe and native Fedora/WebDriver Windows input for 200 keys in the 5,000-word note; require all keys retained. Run a keyword query using the existing `keyword-search` probe in the same app session. Bracket probes with status observations proving indexing was active, and label every sample **during indexing**. Preserve raw timing-only samples and report count, median, nearest-rank p95, and maximum using the summary tooling's statistics. #88's editor limits are p95 ≤ 50 ms and maximum ≤ 100 ms. #88 defines no keyword-specific latency budget; report that measurement without inventing a threshold or applying the semantic-search budget to it. Record any probe-induced note changes and repeat indexing work.
8. **Teardown and proof.** Close the app through its save-aware path, then stop the exact launched driver, tunnel, awake holders, and any owned Ollama supervisor. Verify all recorded process identities, including model workers, are gone. Leave user-owned Ollama alone. Restore original config/profile hashes even on failure. Confirm evidence survives cleanup. The Fedora helper removal is the requested package teardown; list all other retained artifacts and sizes for approval instead of deleting them.


### Baseline results

| Measure | Fedora | Windows |
| --- | --- | --- |
| Selected measurement head | `8c2a375` | `8c2a375` |
| Installed binary embeds that head | Not checked; not installed | Not checked; not installed |
| Fresh 10,000-note fixture / rebuild runs | Not generated / 0 | Not generated / 0 |
| Backend endpoint, ownership, model-loaded state at rebuild | Not selected or observed | Not selected or observed |
| Cold rebuild wall time | Not measured | Not measured |
| Notes/s and chunks/s | Not measured | Not measured |
| Summed pass notes, chunks, total time | Not measured | Not measured |
| File reads, Chunking, Ollama, Database totals and shares | Not measured | Not measured |
| Settings final-log match / null state | Not observed | Not observed |
| Pass-log privacy check | Not run; no installed app log | Not run; no installed app log |
| Peak app / Ollama memory | Not measured | Not measured |
| Editor latency during indexing | Not measured | Not measured |
| Keyword latency during indexing | Not measured | Not measured |

The Windows preflight found no existing Ollama or model-worker process, and the default service endpoint did not answer. No temporary server, model request, or tunnel followed. This availability check supplies no measurement-topology or model-loaded result.

## Earlier stops

- Run 1, report commit `152c04b`: stopped on the three Windows JavaScript failures subsequently exempted under #221; the concurrent Fedora package build was cancelled.
- Run 2, report commit `8c2a375`: a temporary candidate checkout linked `node_modules` to the delivery worktree. pnpm refused it with `ERR_PNPM_UNSAFE_MODULES_DIR` before product compilation. Run 3 built from the delivery worktree as instructed and passed that setup point. The run-2 checkout, dependency symlink, and worktree registration remain untouched for deletion approval.

## State left behind and evidence

The Fedora build ran in a user systemd unit. After the new Windows failure, the unit was stopped and all five captured process identities were absent. Its final state was inactive/dead, with MainPID 0 and an empty control group. A later Windows check found zero build, app, driver, Ollama, model-worker, or stage-script processes. Both checkouts were clean at `8c2a375` before report editing.

No app configuration was changed. The existing Windows configuration hash matched its preflight hash; Fedora used no app configuration. No configuration restoration was needed. No baseline vault, configuration backup, installed package, app, driver, model server, or tunnel was created. A new bundle, build logs, helper scripts, and intermediate build outputs remain. Earlier logs, scratch directories, bundles, and the run-2 checkout remain intact. No manual deletion was performed. Private artifact paths and logical byte sizes, excluding symlink targets, are listed in the final handoff.

Committed evidence contains only candidate IDs, counts, test names, outcomes, repository-relative diagnostic locations, cleanup assertions, and raw-log hashes. Private logs, machine paths, hostnames, and profile contents stay outside the repository.

- [Run 3: code parity, carried gates, Windows lint blocker, cancellation, and cleanup](evidence/158-attempt-1-2026-10-17/run-3.json)
- [Run 2: tests, #221 failures, package setup refusal, and cleanup](evidence/158-attempt-1-2026-10-17/continuation.json)
- [Run 1: verification, named tests, and source-log hashes](evidence/158-attempt-1-2026-10-17/verification.json)
- [Run 1: Windows assertions, classified under #221](evidence/158-attempt-1-2026-10-17/windows-failures.json)
- [Run 1: cleanup and untouched-state evidence](evidence/158-attempt-1-2026-10-17/cleanup.json)

Remaining work: Claude fixes the Windows Rust lint finding; Sol reruns the affected verification, finishes both packages and installed-app baselines on the resulting head, and performs the required exact-head final review before publication. No indexing-throughput or responsiveness claim is established by this run.
