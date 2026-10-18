# Deferred capture-time similarity (#216): verification

Date: 2026-10-17 (local date). Run by Sol for Kiro attempt 1, workflow section 7, steps 4–6.

**VERIFY: FAIL.** The command gates completed, with only the allowed Windows #221 lint failure.
The first installed-package attempt stopped at an empty backend reply; its RPM was removed.
On the follow-up attempt, the temporary Windows server answered with `embeddinggemma`, but Sol
scheduled the second stability probe only 40.112935 seconds after the first, below the required
60 seconds. The timing assertion failed. Following the instruction to stop on any failure, Sol
stopped before installing the RPM or launching the app. All four recovery steps remain unverified.
This follow-up failed because of the controller's probe timing; it establishes no product failure.

## Candidate and merge

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
[verification.json](evidence/ticket-216-2026-10-17/verification.json).

## Command results

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
| Required stability interval | **FAIL:** 40.112935 seconds, below the 60-second minimum. The controller's interval assertion failed; no later probe was attempted. |

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
app's behavior failed. No screen-only obstacle was reached. A future run must first obtain two
successful `/api/tags` replies at least 60 seconds apart from the same server PID, then forward
11436 and perform all four steps above on the installed candidate. Unit tests do not replace
those observations.

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

## Review rounds and ownership

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
   report commit; its status is qualified because the installed check did not run.

The judged P3s left in the code were a card lost if the main window is destroyed (ordinary close
is prevented), two file races requiring a window of a few milliseconds, and one full vault scan
per deleted note's deferred check. Earlier notes about the Tauri delivery wrapper and removal
of value-equal records were judged without a code change. The wrapper remains unverified in an
installed recovery run because this attempt stopped before app launch.

Kiro's imported commits:

- `43d24a1427ef76621430ee86378cbd8aeaf140c9`: record and rerun failed similarity checks.
- `509ab6e150f114f6e3f8bd61aca82449771f2483`: deferred-check tests.
- `5de61cbaea6006943c912531a92e6d88a6d2fc82`: SPEC rule.

Claude's three fix commits:

- `c33a9be8286dee837dc3ccd3b310b9887a45913f`: intake documentation of machine-local state.
- `7e8ff9f496f7014436c4220f90af33c95650942c`: round-1 fixes.
- `19727aab6a2dcc0e1c5f1b7fc9db53b1110d6c37`: accepted round-2 fixes.

## Retained paths

[retained-paths.json](evidence/ticket-216-2026-10-17/retained-paths.json) lists retained artifact
roots from the first run, their byte sizes, and the individual gate-evidence files, with private
profile backups grouped. The follow-up's additional private scripts, logs, profile backup, and two
Windows supervisor source files are listed separately in
[installed-retry-retained-paths.json](evidence/ticket-216-2026-10-17/installed-retry-retained-paths.json).
Paths use
logical machine/run/temp roots so the evidence contains no private machine paths. Build-cache
totals include pre-existing contents and must not be read as bytes created by this run. Nested
installer/RPM entries are included in their parent totals.

A shell-quoting error in the first Windows disk query and a transfer-directory typo were confined
to throwaway setup commands. They were corrected before Windows verification, without a product
edit. The typo left one empty directory, included in the inventory. The existing Windows install
was left in place.

Disposable run directories, copied bundles, retained package extraction, and the two machines'
test-created temporary state are cleanup candidates only after item-specific approval. Build
caches are retained for reuse. No deletion is requested or performed by this report.

The remaining acceptance proof is the installed recovery sequence. **VERIFY: FAIL.**
