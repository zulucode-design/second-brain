# Issue #158, attempt 1: indexing baseline verification

Finished 2026-10-17. **Outcome: verified, and the baseline is recorded on both machines.** Every
check passed except four Windows failures that already exist on main. They are tracked in
[#221](https://github.com/zulucode-design/second-brain/issues/221) and are not caused by this branch.

Issue #158 stays open. This attempt adds stage timing to semantic indexing, so a baseline can be
measured before anyone speeds the worker up. It does not speed anything up.

**Who ran what.**
- Sol ran the build gate, the round 1 reviews and verification runs 1 to 3.
- Sol then reached her usage limit. The repository owner asked Claude and Claude Sonnet agents
  to finish the pilot.
- Sonnet agents at xhigh effort, standing in for Sol as `AGENTS.md` "Roles" allows, built both
  packages and ran both baselines.
- Claude wrote this record from their results.

## Candidate, merge and reviews

- Reviewed candidate: `6f30ae6d4a882e1059a395ccf5ae84b8d0658a10`, on branch
  `fix/158-indexing-pass-timing`.
- Packages and baselines: `32c7fbf75413eb0cfcd294afece2aa3e9600ca8f`. The commits after `6f30ae6`
  change only `docs/` (`git diff --quiet 6f30ae6 32c7fbf -- . ':!docs'` exits 0). So the app code
  measured is the reviewed code, and results on `6f30ae6` carry to it.
- Merge: main was still `9eb5d20`, the merge base, so no merge was needed.
- Review round 1, both on `6f30ae6`: Spec **CLEAN**, Standards **CLEAN**. The final review runs on
  the commit that adds this record.

Kiro wrote the first pass. Claude made one fix after import.

| Commit | Author | Change |
| --- | --- | --- |
| `2219fd0` | Kiro | Time each semantic indexing pass by stage, in memory only. |
| `7742105` | Kiro | Type the last pass and format its times. |
| `3722201` | Kiro | Show the last pass in Settings. |
| `6f30ae6` | Claude | Log only the first empty pass of an Ollama outage. While Ollama is down the worker retries every 20 s, and logging every empty retry would write about 4,300 lines a day. |

## Verification

| Machine | Check | Commit | Result |
| --- | --- | --- | --- |
| Fedora | `pnpm verify` | `6f30ae6` | **PASS**: check 0 errors, 253 JS tests, 687 Rust tests (10 ignored), lint, build |
| Fedora | `pnpm tauri:build` | `32c7fbf` | **PASS** (stand-in) |
| Fedora | `scripts/verify-linux-package.sh` | `32c7fbf` | **PASS** (stand-in); the installed binary embeds `32c7fbf` |
| Windows | `pnpm check` | `6f30ae6` | **PASS**, 0 errors |
| Windows | `pnpm test` | `6f30ae6` | 247 passed, **3 failed (#221)**, 3 skipped |
| Windows | `pnpm test:rust` | `6f30ae6` | **PASS**, 662 passed, 9 ignored |
| Windows | `pnpm lint:rust` | `8c2a375` | Formatting passed; **1 Clippy error (#221)** |
| Windows | `pnpm build` | `32c7fbf` | **PASS** (stand-in) |
| Windows | `pnpm tauri:build` | `32c7fbf` | **PASS** (stand-in); the installed `second-brain.exe` embeds `32c7fbf` |

### Known failures from main: #221

All four fail on Windows only, in files this branch does not change. CI's Windows job runs only
`pnpm test:rust`, so it never sees them.

| Failure | Cause |
| --- | --- |
| `pnpm install fails when the prepare svelte-kit sync fails` | `spawnSync('pnpm')` gets `ENOENT`, because pnpm is a `.cmd` shim on Windows. |
| `the test package helper exits 0 after an install and removes its staging copy` | `spawnSync('bash')` gets `ENOENT`, because bash is not on PATH. |
| `the test package helper refuses to run without sudo` | Same as above. |
| Clippy `bind_instead_of_map` at `src-tauri/src/commands.rs:4295` | The `#[cfg(unix)]` block in `damaged_config` is compiled out on Windows, leaving `.and_then(\|_\| Ok(()))`. |

### The new tests, by name

All five Rust tests are in `semantic_search::tests`, and none is ignored.

| Test | Fedora | Windows |
| --- | --- | --- |
| `a_pass_records_its_counts_and_stage_times_in_memory_only` | PASS | PASS |
| `a_pass_that_cannot_use_the_backend_records_that_it_stopped` | PASS | PASS |
| `only_the_first_empty_pass_of_an_outage_is_logged` | PASS | PASS |
| `a_pass_that_sends_nothing_to_the_backend_keeps_the_earlier_record` | PASS | PASS |
| `the_status_carries_the_last_pass_in_the_shape_settings_reads` | PASS | PASS |
| `tests/indexing-pass.test.mjs` | PASS, 5/5 | PASS, 5/5 |

## Baseline

### Method

- **One cold rebuild per machine**, in the installed package. The vault is the fresh 10,000-note
  fixture from `scripts/perf-fixture.mjs`, with an empty index. Both machines had the same content
  digest.
- **Embeddings:** `embeddinggemma` on the Windows desktop's GPU, run in sequence for the two
  machines.
  - Each run started its own server with `scripts/windows/ollama-gate.ps1` on port 11436, so the
    model was not loaded at the start.
  - Windows used it over loopback. Fedora reached it through an SSH port forward, so Fedora's
    Ollama time includes network round trips.
- **Timing:** from the app launch until the first poll showing 10,000 indexed and 0 waiting.
  - The poll read the counts Settings shows (`notes` and `pending_notes` in `semantic.sqlite3`,
    read-only) every 5 s.
  - Stage times are the sum of every `Semantic indexing pass:` log line in the run.
- **Isolation and cleanup:**
  - Fedora used isolated XDG config and data roots.
  - Windows journaled the real `config.json` and restored it byte for byte afterwards.
  - Both apps closed through their window close button.
  - Every process a run started was stopped by its exact PID and shown to be gone.
  - The RPM was removed with the test package helper. The Windows test install stays in place.

### Results

| Measure | Fedora | Windows |
| --- | --- | --- |
| Cold rebuild, launch to 10,000 indexed / 0 waiting | **454.0 s** | **844.7 s** |
| Throughput | 22.03 notes/s, 22.08 chunks/s | 11.84 notes/s, 11.87 chunks/s |
| Chunks at the end | 10,024 | 10,024 |
| Passes; summed notes; summed `total_ms` | 3; 10,000; 445,475 | 6; 10,000; 800,043 |
| Ollama share of summed pass time | **78.1 %** (34.8 ms a note) | 26.7 % (21.3 ms a note) |
| Database share | 21.2 % (9.5 ms a note) | **72.6 %** (58.1 ms a note) |
| File reads share | 0.05 % | 0.21 % |
| Chunking share | 0.14 % | 0.10 % |
| Unattributed (total minus the four stages) | 0.53 % | 0.44 % |
| App peak memory during the rebuild | 692.0 MiB RSS, app plus WebKit children | 577.5 MiB working set, app plus WebView2 children (349.7 MiB private) |
| Ollama peak memory (server plus model worker) | 1,440.8 MiB working set | 1,398.4 MiB working set |
| Editor typing during indexing | not valid, see below | not measured |
| Keyword search during indexing | 189 ms from input to painted results (38 ms in the app, plus the 150 ms input debounce); #88 sets no budget | not measured |
| Settings "Last indexing pass" | read through WebDriver: "None since the app started" at +2.1 s; after convergence, all values matched the last log line | not captured |

The summed pass notes are exactly 10,000 on both machines, so no note was embedded twice. The
summed chunks equal the database's chunk count.

### Pass lines

| Machine | Pass | notes | chunks | total_ms | read_ms | chunk_ms | embed_ms | database_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Fedora | 1 | 1 | 1 | 3,501 | 0 | 0 | 3,491 | 9 |
| Fedora | 2 | 1,268 | 1,268 | 64,572 | 25 | 76 | 49,878 | 14,534 |
| Fedora | 3 | 8,731 | 8,755 | 377,402 | 189 | 527 | 294,416 | 79,966 |
| Windows | 1 | 1 | 1 | 2,707 | 0 | 0 | 2,614 | 92 |
| Windows | 2 | 115 | 115 | 13,542 | 19 | 8 | 2,296 | 11,144 |
| Windows | 3 | 315 | 315 | 33,925 | 54 | 25 | 6,936 | 26,884 |
| Windows | 4 | 913 | 913 | 95,700 | 154 | 72 | 19,791 | 75,570 |
| Windows | 5 | 2,663 | 2,663 | 240,553 | 444 | 206 | 57,233 | 182,123 |
| Windows | 6 | 5,993 | 6,017 | 413,616 | 1,013 | 466 | 124,578 | 284,774 |

No pass stopped. The first pass on each machine is the model load.

### What the baseline shows

- **On Windows the database stage dominates.** It averages 58.1 ms a note against Fedora's 9.5 ms,
  and it is still 69 % of pass 6, which ran after the queue was full. The stage includes waits for
  the database lock. Its cause is not diagnosed.
  - The database uses the `delete` journal mode, not WAL.
  - Defender real-time scanning is on.
  - Neither was tested as a cause.
- **On Fedora, Ollama dominates.** That time includes the round trips through the SSH forward.
  Windows reaches the same server over loopback and spends 21.3 ms a note there, against Fedora's
  34.8 ms.
- **File reads and chunking are negligible** on both machines.
- **Windows started slowly.** The first vault activity in its log came 33 s after launch. This was
  the app's first launch on that machine, and the cause was not traced. Its queue also filled
  slowly: it reached 10,000 queued or indexed notes at +384 s, against +44 s on Fedora.

So batching Ollama requests, the candidate the issue names, would help Fedora's topology most. On
Windows, the per-note database commit is the first thing to measure next.

### Gaps

- **Fedora typing latency is not valid.** The 200-key probe kept every key, but its samples were
  mostly negative (median -3 ms, p95 0 ms, max 20 ms), so the key-event clock ran ahead of the paint
  clock. No budget conclusion is drawn.
- **Fedora probe timing:** both probes ran in the first 35 s of the run, while the queue was still
  filling.
- **Windows:** typing latency, keyword latency and the Settings read were not measured. The Windows
  WebDriver tooling was removed on 2026-10-01 and was not reinstalled for this run.
- **Log privacy:** every pass line holds only the allowed counts, times and flag. Elsewhere in the
  log, a `scan_notes` line and the search index's tantivy lines name the vault path, which
  `docs/log-privacy.md` rule 4 allows locally. No line carries a title, note text or query.
- **One run per machine**, with no repeat. Wall time and the stage sums are the stable numbers. How
  a run splits into passes depends on when the vault scan queued the notes.

## Earlier stops

- **Run 1** (report commit `152c04b`): stopped on the three Windows `pnpm test` failures, which
  were then filed as #221.
- **Run 2** (`8c2a375`): stopped because pnpm refused a temporary checkout whose `node_modules`
  was a symlink (`ERR_PNPM_UNSAFE_MODULES_DIR`). Later runs built from the worktree itself.
- **Run 3** (`32c7fbf`): stopped on the Windows Clippy error, which was added to #221.

## Evidence

Committed evidence holds only commit ids, counts, test names, outcomes and timings. Raw logs,
samples, screenshots and machine paths stay outside the repository.

- [Fedora baseline](evidence/158-attempt-1-2026-10-17/baseline-fedora.json)
- [Windows baseline](evidence/158-attempt-1-2026-10-17/baseline-windows.json)
- [Run 3: code parity, Windows lint failure, cleanup](evidence/158-attempt-1-2026-10-17/run-3.json)
- [Run 2: tests, #221 failures, package setup refusal](evidence/158-attempt-1-2026-10-17/continuation.json)
- [Run 1: verification, named tests, source-log hashes](evidence/158-attempt-1-2026-10-17/verification.json)
- [Run 1: Windows assertions, now under #221](evidence/158-attempt-1-2026-10-17/windows-failures.json)
- [Run 1: cleanup and untouched state](evidence/158-attempt-1-2026-10-17/cleanup.json)
