# Batch Syncthing restart (#224): verification

Dates: 2026-10-19 to 2026-10-21 (UTC). Candidate: commit
`31d01e5cadc9036958732018f4442c6458754dff` on `fix/224-batch-syncthing-restart`.
Base: `1e99ab122dcdb873c17b7055e1cb8e148b592de2`. A fetch before writing this report
confirmed that `origin/main` still equals the base, so workflow section 7, step 4 needed no merge.
The branch was unpushed. This report's commit adds only this report and the three existing traces.

This is Kiro attempt 1, workflow section 7, step 6. The acceptance contract is the immutable
`docs/handoffs/224-attempt-1-brief.md` at `c0f92676e37eeee0d48eaa83a2a1fc5a9330760b`.
Sol drafted this report from the local status, review and verification records, then hit her usage
limit before the privacy check of the traces and the commit. A Claude Sonnet 5.5 agent at high
effort, standing in for her as AGENTS.md (Roles) provides, checked the draft against those records,
corrected it, ran the privacy check and committed it. No test, build, review or live gate was rerun
to write it. Kiro's sandbox runs are not gate evidence.

## What changed

A batch whose pause went unacknowledged could kill the paused Syncthing replacement the supervisor
had just started. A failed next restart then left sync stopped without a log explaining why.
The batch now records an immutable launch identity while holding the lifecycle lock through folder
and device resume. Its fallback stops only that launch and waits for processes already exiting.
A later replacement stays running, including when its PID reuses the old one's. Without a running
child to record, the batch does not resume. Disable and readiness-failure stop paths keep their
existing behavior.

The supervisor logs a failed restart and reaching `MAX_RESTARTS`, one warning at each site, with
counts and outcomes only. Error text stays in `last_error`. The hourly schedule, note-write lease,
peer handoff, pre-sync backup, restart limit, one-second delay and stable-run reset are preserved.
No dependency was added.

| Commit | Agent and change |
| --- | --- |
| `402cfed` | Kiro: select the batch's recorded process for fallback stopping, spare replacements, retain exit waits and add decision tests; cherry-picked from mirror commit `c60f6e8`. |
| `cd5ec2b` | Kiro: warn on failed supervisor restart and restart-budget give-up; cherry-picked from mirror commit `99ae0f5`. |
| `630d5ba` | Claude intake: record the process after successful resume as well as before, covering a replacement that answered resume; adapt the live-test callers and documentation. |
| `058c47c` | Claude, round 1: test successful and skipped resume-hook calls; remove the warning's inaccurate promise that the supervisor will not retry. |
| `31d01e5` | Claude, round 2: replace PID snapshots with launch identity, serialize identity capture and resume against start/stop, refuse resume without a child, and adapt tests and documentation. |

The final mechanism differs from the brief's literal PID prescription: the launch identity also
distinguishes PID reuse, and the shared lifecycle lock closes the launch/publication gap. It keeps
the brief's requirement to stop only the process the batch resumed. The intermediate intake's
"one of the recorded processes" deviation was superseded by this single-launch rule.

## Review rounds

Round 1 ran on `630d5ba`. A Claude Sonnet 5.5 stand-in at extra-high (`xhigh`) effort ran it
because Sol was at her usage limit, as AGENTS.md (Roles) provides. Spec plus Adversarial returned
**CHANGES REQUIRED**. Standards plus Adversarial was cut off by a Sonnet session limit (HTTP 429)
and had no completed verdict. The repository owner had asked to stop if the round was not clean;
work paused there and resumed later.

| Finding | Disposition |
| --- | --- |
| A1, P2: no non-ignored test proved the `resumed` hook ran, or that `run_sync` recorded and passed the second PID. | `058c47c` added two direct `sync_batch` hook tests; `31d01e5` adapted them to guard lifetime and refusal without a child. Actual Tauri glue coverage remains limited below. |
| A2, P3: "the supervisor will not retry it" could be false when a queued start followed the failed start. | `058c47c` removed the clause. |
| S1, P3: the PR body needed to explain the criterion 1 deviation to "one of the recorded processes". | Record that intermediate deviation and the final launch-identity mechanism in the publication description; round 3 accepted the final mechanism. |

Round 2 ran on `058c47c`. Sol (Codex `gpt-6.1-sol`, `xhigh`) ran Spec and Standards separately,
each with Adversarial. Spec plus Adversarial returned **CHANGES REQUIRED**. Standards itself had
**NO FINDINGS**, but its Adversarial pass returned **CHANGES REQUIRED** for the same race.

| Finding | Disposition |
| --- | --- |
| Spec P2: a post-resume snapshot could record a replacement that never resumed; PID reuse could select a later launch. | `31d01e5` records immutable launch identity under the lifecycle lock. |
| Spec Adversarial P1: a child could answer resume before `start` published it, evade both snapshots, then be spared while still able to write after lease release, violating #189. | `31d01e5` serializes resume with launch/publication and refuses it without a known child. |
| Standards Adversarial P2: the same launch/publication gap could let an unrecorded resumed replacement survive fallback. | Covered by the same `31d01e5` fix. |

Round 3 ran on exact candidate `31d01e5`. Sol (`gpt-6.1-sol`, `xhigh`) ran Spec and Standards
separately, each with Adversarial: both verdicts **CLEAN**, with no findings on either axis or
Adversarial pass. The review accepted the lifecycle/launch-identity correction and the frozen
#229/#230 deferrals. Live Gate 1 was still pending at that review; its later results follow.

Each round also had a Fedora build gate (`pnpm check`, `pnpm test`, `pnpm lint:rust`,
`pnpm test:rust`, and the three ignored `sync_sidecar::live_tests` run by name with `--ignored`):

| Round | Head | Fedora gate |
| --- | --- | --- |
| 1 | `630d5ba` | Passed: 692 Rust passed, 0 failed, 10 ignored; Node 253/253; live tests 3/3. The Windows half was not run: free space on D: was 12.68 GB, under the 15 GB floor, so the gate reported FAIL for disk space, not code. Run by the stand-in. |
| 2 | `058c47c` | Passed: 694 Rust passed, 0 failed, 10 ignored; Node 253 passed; live tests 3/3. |
| 3 | `31d01e5` | Passed: 694 Rust passed, 0 failed, 10 ignored; Node 253 passed; live tests 3/3. |

The Windows half of the build gate was run once, on `31d01e5`, in the full verification below.

## Checks on the candidate

Sol ran the full verification on `31d01e5` on 2026-10-20. The Fedora and Windows reports record
successful usage probes before the runs; neither needed the stand-in. All counts below are the
actual runner summary lines, not Kiro's handoff counts.

### Fedora

Fedora 44, x86_64; Node 24.22.0, pnpm 11.22.0, Rust 1.98.0. `pnpm verify` passed its complete
configured chain in 49.323 seconds:

| Command | Result and population |
| --- | --- |
| `pnpm check` | svelte-check: 0 errors, 0 warnings. |
| `pnpm test` | Node suite: 253 passed, 0 failed, 0 skipped, 0 cancelled. |
| `pnpm test:rust` | Rust library suite: 694 passed, 0 failed, 10 ignored, 0 filtered; main and doc suites each ran 0 tests. |
| `pnpm lint:rust` | rustfmt and all-target Clippy with warnings denied passed. |
| `pnpm build` | Production frontend build and static adapter passed. |
| `pnpm tauri:build --bundles rpm` | Candidate RPM built successfully in 491.749 seconds; source tree clean before and after. |

The ten ignored Rust tests are existing integration/measurement checks. None of the seven tests
added by this branch is ignored. The full-verification run did not count ignored tests as passed
acceptance. The earlier round 3 Fedora gate also reports the three Syncthing live tests passing
3/3 on this same candidate; those are separate from the full-suite count above.

The RPM's metadata, payload allowlist, desktop entry and embedded full commit were checked.
An extra verifier assertion initially failed because it expected the packaged executable to be
byte-identical to the unbundled release executable. Read-only diagnosis proved the only difference
was Tauri's three-byte `UNK` to `RPM` bundle marker; a corrected comparison passed for every other
byte. This was a verifier assumption, not a package defect. Vite's chunk-size advisory was nonfatal.

### Windows

Windows 11 Pro, x64; Node 24.19.0, pnpm 11.22.0, Rust 1.98.1. Sol used a fresh detached checkout
of `31d01e5` carried in a local Git bundle. The runs used an isolated build target, disabled
incremental compilation and debug symbols, and kept debug assertions. Rust tests went through
`pnpm test:rust`, which scopes the Windows test manifest and selects the library suite.

| Command | Result and population |
| --- | --- |
| `pnpm install --frozen-lockfile` | Passed; 477 reused packages, 0 downloads. |
| `pnpm test:rust` | 678 library tests: 669 passed, 0 failed, 9 ignored, 0 filtered; 121.683 seconds including compilation. |
| `pnpm lint:rust` | rustfmt passed; Clippy exited 101 for the existing #221 error only. |
| `pnpm tauri:build --bundles nsis` | Passed, one NSIS installer; 251.228 seconds. |
| Post-build cleanliness assertion | Failed on ` M src-tauri/Cargo.toml`; Claude accepted the byte-identical line-ending status on 2026-10-20. |

Clippy's #221 error is `clippy::bind_instead_of_map` at `src-tauri/src/commands.rs:4295`:
`std::fs::copy(...).and_then(...)` has a Windows closure returning only `Ok(())`. That file is
unchanged from the base. The brief explicitly accepts known #221 failures; no unrelated fix was made.

The Windows run reported ` M src-tauri/Cargo.toml` after the build and therefore wrote
**VERIFY: FAIL**. With `core.autocrlf=true`, checkout used CRLF and the build rewrote the manifest
as LF. The resulting file is byte-identical to the HEAD blob: filtered, unfiltered and HEAD blob
hashes are all `2195ed7bfdbb622667dd0bf5cdd303153b8c23de`; `git diff --raw` is empty.
Claude accepted this status artifact. The report does not erase the failed assertion or claim
Windows lint was wholly green. The Windows checkout was left untouched; subsequent live runs
carried this accepted result.

The brief requested the Windows Rust tests, lint and Windows build. No separate Windows
`pnpm verify` was run. Platform-conditional compilation accounts for the different Rust counts.

### Named regression tests

The Fedora report matched all seven added tests against their actual successful Rust output.
The Windows report matched those seven plus the two existing safety tests below, all successful.
Names share the prefix `sync_sidecar::tests::`; the command on each machine was `pnpm test:rust`.

| Test | Fedora | Windows |
| --- | --- | --- |
| `a_batch_whose_syncthing_was_replaced_leaves_the_new_one_running` | Passed | Passed |
| `a_batch_whose_syncthing_was_not_replaced_stops_it` | Passed | Passed |
| `a_replacement_that_reuses_the_process_id_is_left_running` | Passed | Passed |
| `a_batch_that_recorded_no_syncthing_stops_none` | Passed | Passed |
| `a_batch_whose_syncthing_is_gone_still_waits_for_those_exiting` | Passed | Passed |
| `a_batch_holds_its_lock_just_while_it_resumes` | Passed | Passed |
| `a_batch_with_no_syncthing_to_record_does_not_resume` | Passed | Passed |
| `an_unacknowledged_pause_stops_syncthing_whether_or_not_the_app_is_closing` | Not separately matched in the Fedora report | Passed |
| `a_close_also_waits_for_a_syncthing_a_disable_already_stopped` | Not separately matched in the Fedora report | Passed |

### Packages

Both packages are version `0.1.0-alpha.1`, built from the candidate. Their checksums were matched
before installation. The live runs independently confirmed both installed executables contain the
full candidate commit `31d01e5cadc9036958732018f4442c6458754dff`.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| Fedora `Second Brain-0.1.0-alpha.1-1.x86_64.rpm` | 30,156,156 | `6808c380ca99aa6c543793f202096c8bdb326133c9e4280ade509d74811e6ab7` |
| Fedora packaged executable | 52,167,952 | `38eca14460c51bb52eb564f46752d433139b1974dc6956e60227a708d0571a15` |
| Windows `Second Brain_0.1.0-alpha.1_x64-setup.exe` | 20,315,858 | `c0a062bd88fe12f67e36b3b1fdf36b6fe1ebee077719bd3009995173bc06779d` |
| Windows built release executable | 35,592,704 | `a482f3ca81e0675f9b8eca148abac8bc7693de6396f345b2f5621a80558a73ce` |
| Windows installed executable | 35,592,704 | `f717669c987288ecef2ef355d4159305a57e54407bb35fe4c624a266b6c47487` |

The Windows build report checked the uninstalled release executable's embedded commit and the
fresh NSIS output. Try 1 installed that installer successfully and checked the installed executable;
later runs retained it. The installed hash is reported separately from the build-output hash.
Try 4 reinstalled the Fedora RPM and reconfirmed both installed commit markers.

## Live Gate 1

Sol ran all four tries on installed packages of `31d01e5`; no application code changed between
them. The brief's live criterion is: "Its sidecar-restart step must see a new Syncthing appear
after the gate stops the old one, and the app must log no `stop_writer` kill of it." The issue
also requires a passing live Gate 1.

The command was `node scripts/alpha-harness.mjs sync --candidate
31d01e5cadc9036958732018f4442c6458754dff`. After try 1, the controller received an isolated
`XDG_CONFIG_HOME` template containing `{"theme":"system"}`. That override applied only to the
controller. Each run used the harness's generated 5,000-note fixture, paired Fedora and Windows,
and the app's hourly scheduled batches.

| Try | Run | Result |
| --- | --- | --- |
| 1 | `20261020T225459Z` | Stopped before `run-start`: Fedora template config missing. No app launch, batch, interruption or trace. |
| 2 | `20261020T230234Z` | Claude stopped the run at the repository owner's request because of usage, before its first scheduled batch. No local report was completed; the stopped trace is retained. |
| 3 | `20261021T164504Z` | Restart and spared-replacement evidence passed; gate failed on one SSH check timeout before the second scheduled batch. |
| 4 | `20261021T174531Z` | Passed: controller exit 0, `run-complete`, full convergence and backups. |

### Try 3: replacement spared, later transport failure

At `17:00:18.1586656Z`, the harness stopped Windows Syncthing PID **37472** during partial
transfer. Replacement PID **32900** was created at `17:00:21.9689310Z` and recorded by the gate
at `17:00:23.689Z`. The corrected Windows log audit found exactly this line:

```text
[2026-10-21][17:00:22][app_lib::sync_sidecar][WARN] Sync folder did not acknowledge its pause; leaving Syncthing 32900 running, since it started after this run resumed the folder
```

The independent process observer's last sample containing the transfer app also contained
replacement 32900 at `17:00:38.3948686Z`; both were absent in the next sample at
`17:00:39.5290768Z`. That proves sampled liveness immediately before the intentional Windows
close, not continuous observation between samples. Fedora had no requested matching log lines;
Windows had only the spared-case line. Neither machine logged a `stop_writer` kill of the replacement.
An earlier Windows audit used the wrong log directory; the corrected audit supersedes its zero matches.

At `17:23:28.953Z`, one read-only Windows fixture check hit its 45-second SSH bound and reported
`spawnSync ssh ETIMEDOUT`. The controller exited 1 before the 18:00 second batch. Its last fixture
observation had 2,008 matching and 2,992 missing notes, with no wrong content, duplicates, conflicts
or strays. Final convergence and the final backup assertion were not reached. The timeout's cause
was not established; it was not a sidecar-restart assertion failure.

### Try 4: full gate pass

Run `20261021T174531Z` started at `17:45:27.534628Z` and exited 0 at `19:00:49.654025Z`.
The trace records `run-complete` at `19:00:48.361Z`, with no `run-failed` or `cleanup-error`.
There was no independent Windows observer during this try.

- Partial transfer was proven at `18:00:16.771Z`: 1,036 matching notes, 3,964 missing.
- Windows Syncthing PID **20944** was stopped at `18:00:18.1779863Z`; different replacement
  PID **35716** was observed running at `18:00:21.698Z`.
- Scheduled batches ran at 18:00 and 19:00 UTC. Windows completed at `19:00:24.476Z`, Fedora
  at `19:00:25.593Z`: exactly 5,000 notes each, zero missing, wrong content, duplicates,
  conflicts or strays. The pre-sync backup assertion passed: two Windows backups, one Fedora.
- All retained app-log locations on both machines had zero occurrences of `stopping Syncthing`,
  `leaving Syncthing`, `its Syncthing had already stopped`, `Syncthing restart attempt`, or
  `Could not stop Syncthing`. This try produced no spared-case warning either.

Try 4's timing limit matters: replacement 35716 was alive at the last harness observation before
the intentional closes, `18:00:21.698Z`. Whether it remained alive at the instant immediately
before Windows closed is not directly observed. There is no later pre-close snapshot. The trace
does not emit exact close or recovery-launch timestamps; retained logs place recovery startup in
the 18:00:39 second on Fedora and 18:00:44 second on Windows.

Together, tries 3 and 4 meet the brief's live criterion: try 3 exercises the fallback and explicitly
logs that the replacement was spared, with sampled pre-close liveness; try 4 sees a different
replacement without a kill log and completes Gate 1 on the same candidate and packages. Try 3's
overall gate failure and try 4's observation gap remain part of that evidence.

After try 4, the harness's final process checks and independent exact-path checks were empty on
both machines. Windows config and window-state hashes were restored, profile directories restored,
and lock, vault junction and harness task absent. The authorized Fedora test-package removal passed;
the Windows candidate installation was retained. A separate aggregate post-run SSH audit exceeded
60 seconds; narrower reads recovered the required file, log and process checks. That timeout was
after the passing gate and did not invalidate its result.

## Retained evidence and privacy

The three traces are committed unchanged. Their outcomes are intentionally different:

| Trace | Outcome | SHA-256 |
| --- | --- | --- |
| [20261020T230234Z](evidence/alpha-harness-sync-20261020T230234Z.jsonl) | Try 2, stopped on request; no terminal gate result | `26dab7d5b037ce5ec4e3d2d1dafd8be84c82426d35439f49bf1a3becf3afacd8` |
| [20261021T164504Z](evidence/alpha-harness-sync-20261021T164504Z.jsonl) | Try 3, SSH timeout after successful restart | `bf5822d43364b18021d84497f1fa45206da2bdd119bd0e011c5292e87fe6d783` |
| [20261021T174531Z](evidence/alpha-harness-sync-20261021T174531Z.jsonl) | Try 4, pass | `99aed09ecfc584dacac617ab8685af2510c3b83669c0fa2a823b929ec3b7c810` |

Sol's usage limit hit before this check, so the stand-in ran it on the three traces and on this
report. It searched all four files for the home directory path, the Windows profile path, the
machine's hostname, the ssh host alias, the local and Windows user names, IPv4 addresses (including
Tailscale ones), email addresses and URLs, API-key and token words, and anything shaped like a
Syncthing device ID. Nothing matched. It also parsed every trace line as JSON and listed the event
and field names: they carry counts, timestamps, hashes, PIDs and test paths, no note content. What
remains is safe: device labels are `device-1` and `device-2`; the Windows profile paths are the
placeholder `%USERPROFILE%`; the per-run vault ID is the harness's random fixture ID; the only
`~/` path is the Fedora fixture vault under `~/sb88/runs/` in the try 4 trace, as in earlier
committed traces; and the other paths are the `D:\SecondBrainTest\...` test layout documented in
`docs/alpha-harness.md`. The harness redacts these when it writes traces, and the check confirmed
it. The report names no person, only the agents.

The local source records are `status.md`, `round1-pending.md`, the round 2 and round 3 Spec and
Standards review files, `verify-fedora-report.md`, `verify-windows-report.md`,
`gate1-live-report.md`, `gate1-live3-report.md` and `gate1-live4-report.md`. Those local reports
and their private logs are not committed here. There is no completed try 2 report.

## Not proven and follow-ups

- Actual Tauri `run_sync` hook wiring and `start` publication interleavings lack direct automated
  glue tests. Round 3 traced the shared lifecycle lock and accepted the guard-lifetime and identity
  tests as sufficient; this remains a coverage limit, not a clean-review finding left open.
- No live run forced a failed supervisor restart or exhausted `MAX_RESTARTS`. The two warning
  sites and privacy were checked in source review; the live log audits observed neither event.
- Existing ignored tests were not all run in full verification. No unrelated live, network,
  calibration or performance acceptance is claimed.
- The final review of the report head is still to come, and so are publication and PR CI. Round 3
  covers the code candidate, not this report's commit. Nothing was pushed by this step.

Kiro identified two existing problems at handoff; the frozen brief leaves both out of scope.
**#229** tracks the stable-run restart-budget reset. **#230** tracks the scheduler ending after
restart give-up or readiness failure. Neither was changed by #224; the PR body must name both.
