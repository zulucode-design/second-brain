# Issue #158, attempt 1: indexing baseline verification

Run ended: 2026-10-17. Owner: Sol. Outcome: **blocked**. **VERIFY: FAIL**.

The continuation confirmed that Windows JavaScript verification has exactly the three known failures from main tracked in [#221](https://github.com/zulucode-design/second-brain/issues/221). Windows Rust tests passed fresh. A new Fedora package setup failure then stopped the run before any installed-app baseline. No indexing performance measurements were collected.

The first run, recorded in `152c04b`, stopped on the three #221 failures and cancelled its concurrent Fedora package build. Nicolas permitted those three failures for this continuation and required an immediate stop on any new failure. The original evidence remains unchanged; [continuation evidence](evidence/158-attempt-1-2026-10-17/continuation.json) records the new results.

Issue #158 stays open. This attempt adds measurement instrumentation; it does not speed indexing up. Its baseline acceptance criterion remains unverified.

## Candidate, merge, and reviews

- Candidate and verification head: `6f30ae6d4a882e1059a395ccf5ae84b8d0658a10`.
- Measurement head: none; zero measurement runs started. Both planned measurements still target `6f30ae6`.
- Delivery branch: `fix/158-indexing-pass-timing`, clean at report commit `152c04b` when the continuation began. That commit changed only the report and its evidence; product sources match the candidate.
- Carried merge result: the first run's `git fetch origin` found main and merge base at `9eb5d20cd066465490467588228febc4a290c2df`: **no merge needed**. This continuation made no merge or bundle transfer.
- Round 1 Spec: **CLEAN**, on `6f30ae6`.
- Round 1 Standards: **CLEAN**, on `6f30ae6`.
- These are earlier Sol reviews. No new final review was performed. The new report commit still needs the exact-head final review required by `AGENTS.md` before publication.
- No push or pull request was made. No commit was amended, reset, or rebased.

Imported Kiro commits:

| Commit | Change |
| --- | --- |
| `2219fd0` | Time each semantic indexing pass by stage, in memory only. |
| `7742105` | Type the last pass and format its times. |
| `3722201` | Show the last pass in Settings. |

Claude's fix after import: `6f30ae6`, `fix(semantic): log only the first empty pass of an Ollama outage`.

## Verification results

All results below concern `6f30ae6`. Fedora full verification and Windows typecheck carry from the first run on that unchanged candidate. Windows JavaScript and Rust results are fresh continuation runs. The three Kiro skill copies matched. Sol usage probes returned `OK` before the test and package runs; a further probe for Windows lint completed, but lint never started after the Fedora failure.

| Machine | Command or stage | Result and counts |
| --- | --- | --- |
| Fedora | `pnpm verify` | **PASS, carried**, exit 0, 48.00 seconds. |
| Fedora | `pnpm check`, inside verify | **PASS, carried**, 0 errors, 0 warnings. |
| Fedora | `pnpm test`, inside verify | **PASS, carried**, 253 passed, 0 failed, 0 skipped. |
| Fedora | `pnpm test:rust`, inside verify | **PASS, carried**, 687 passed, 0 failed, 10 ignored. |
| Fedora | `pnpm lint:rust`, inside verify | **PASS, carried**, formatting and Clippy with warnings denied. |
| Fedora | `pnpm build`, inside verify | **PASS, carried**. |
| Fedora | `pnpm tauri:build` | **FAIL**, exit 1 after 1.11 seconds; `ERR_PNPM_UNSAFE_MODULES_DIR`, before build scripts or product compilation. |
| Fedora | `scripts/verify-linux-package.sh <rpm>` | **NOT RUN**; no RPM available. |
| Windows | Checkout and disk preflight | **PASS**, clean at candidate. C: 49.88 GiB free; D: 25.01 GiB free, above the 15 GiB threshold. No Windows package build started. |
| Windows | `pnpm check`, first verify | **PASS, carried**, 0 errors, 0 warnings. |
| Windows | `pnpm test` | **FAIL, permitted #221 failures only**, exit 1, 63.61 seconds: 247 passed, 3 failed, 3 skipped, 253 total. No cancelled tests or additional failures. |
| Windows | `pnpm test:rust` | **PASS, fresh**, exit 0, 22.98 seconds: 662 passed, 0 failed, 9 ignored. |
| Windows | `pnpm lint:rust` | **NOT RUN**; stopped after Fedora package failure. |
| Windows | `pnpm build` | **NOT RUN**. |
| Windows | `pnpm tauri:build` | **NOT RUN**. |

Windows `pnpm verify` was not repeated: its first-run exit 1 at JavaScript tests is historical evidence. The continuation ran the requested stages separately until the new Fedora failure. Neither package was installed. The first-run Windows install inventory found zero matching registry entries, known installed executables, or shortcuts; this continuation did not change installs.

### Known Windows failures from main: #221

| Test and location | Fresh continuation result |
| --- | --- |
| `pnpm install fails when the prepare svelte-kit sync fails`, `tests/install-failure.test.mjs:12` | Assertion expected `/INTENTIONAL_SYNC_FAILURE/`; received `undefined\nundefined`. |
| `the test package helper exits 0 after an install and removes its staging copy`, `tests/linux-test-package.test.mjs:87` | Expected exit status 0; received `null`. |
| `the test package helper refuses to run without sudo`, `tests/linux-test-package.test.mjs:105` | Expected exit status 1; received `null`. |

Sol confirmed that `git diff --quiet 9eb5d20 HEAD -- tests/install-failure.test.mjs tests/linux-test-package.test.mjs scripts/linux-test-package.sh package.json` exits 0. These files are unchanged from main. Sol also reran Claude's Windows spawn probe: both `spawnSync('pnpm', ...)` and `spawnSync('bash', ...)` returned `status: null`, `error: ENOENT`. Windows provides pnpm through a `.cmd` shim; bash is not on PATH. The `windows-rust` job in `.github/workflows/verify.yml` runs `pnpm test:rust`, not JavaScript tests, so it does not exercise these failures.

The fresh JavaScript log names exactly the three tests above. All 247 other executed tests passed; three platform skips remain. These #221 failures are permitted for this attempt and did not cause the continuation to stop.

### New blocker: Fedora package setup

The build embeds its checkout's HEAD through `src-tauri/build.rs`. To build `6f30ae6` while keeping `152c04b` on the delivery branch, Sol created a detached candidate worktree and linked its `node_modules` to the delivery checkout's existing dependencies. The existing Rust target cache was selected through `CARGO_TARGET_DIR`.

`pnpm tauri:build` automatically entered dependency installation, then refused the shared modules directory because its resolved target lies outside the temporary project's root: `ERR_PNPM_UNSAFE_MODULES_DIR`. This was Sol's build setup failure. The refusal occurred before the package scripts ran; it provides no product compile or packaging verdict. The shared dependency directory remains present, and both working trees stayed clean.

Sol stopped without fixing the setup or retrying the command, as instructed. The Windows Rust run had already completed successfully. No later verification or baseline stage ran.

### Named-test reconciliation

All Rust names have prefix `semantic_search::tests::`. None of these five tests is ignored. Windows results now come from the fresh Rust run, rather than the earlier build gate.

| Test | Fedora full verify, carried | Windows continuation, fresh |
| --- | --- | --- |
| `a_pass_records_its_counts_and_stage_times_in_memory_only` | PASS | PASS |
| `a_pass_that_cannot_use_the_backend_records_that_it_stopped` | PASS | PASS |
| `only_the_first_empty_pass_of_an_outage_is_logged` | PASS | PASS |
| `a_pass_that_sends_nothing_to_the_backend_keeps_the_earlier_record` | PASS | PASS |
| `the_status_carries_the_last_pass_in_the_shape_settings_reads` | PASS | PASS |
| `tests/indexing-pass.test.mjs` | PASS, 5/5 | PASS, 5/5 |

The five JavaScript cases cover sub-minute seconds, minute formatting, rounding across a minute, the pass summary, and stage labels in order. Exact names and source-log SHA-256 digests appear in the continuation evidence. The carried Fedora log was checked against the same names.

## Baseline protocol and execution

The original eight-step protocol is retained below. It follows `scripts/perf-fixture.mjs`, `perf-interactive.mjs`, `perf-startup.mjs`, `perf-summary.mjs`, the Windows helpers, and the #88 performance matrix. Step 1 stopped at Fedora packaging. No installation, configuration swap, fixture generation, model selection, app launch, or measurement began; steps 2 through 8 were not exercised. Process checks for the verification commands appear after the results.

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

No baseline vault, configuration backup, new bundle, installed package, app, driver, model server, or tunnel was created. No app configuration was changed, so no configuration restoration was needed. The detached candidate worktree, its dependency symlink, scripts, logs, and process records remain as private artifacts. The failed Fedora systemd unit has an empty control group, and its recorded build PID is absent. Windows' final check found no run processes and a clean checkout.

The first run's scratch directories and build outputs, and the earlier build-gate evidence and bundles, remain intact. Fresh Windows tests also left two temporary directories: machine state (149 files, 78,652 bytes) and an asynchronous semantic-test fixture (one file, 36 bytes). Both are retained. No manual deletion was performed; the requested tests performed their own internal fixture cleanup. Retained artifact paths and logical byte sizes are provided in the private final handoff, with symlink targets excluded from directory totals.

Committed evidence contains only counts, test names, outcomes, commit IDs, sanitized failure descriptions, and source-log digests. Private raw logs, hostnames, usernames, and machine paths are excluded.

- [Continuation: fresh tests, known #221 failures, package blocker, and cleanup](evidence/158-attempt-1-2026-10-17/continuation.json)
- [First run: verification, named tests, and source-log hashes](evidence/158-attempt-1-2026-10-17/verification.json)
- [First run: Windows assertions, now classified under #221](evidence/158-attempt-1-2026-10-17/windows-failures.json)
- [First run: process cleanup and untouched-state evidence](evidence/158-attempt-1-2026-10-17/cleanup.json)

Remaining work: a Fedora package build with dependencies belonging to its candidate checkout; Windows Rust lint, web build, and NSIS packaging; RPM verification; both installed-app baselines; and an exact-head final review before publication. Nothing in this run establishes indexing throughput or responsiveness.
