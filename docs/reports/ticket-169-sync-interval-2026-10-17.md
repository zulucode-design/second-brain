# Ticket #169: hourly sync verification and measurement

Completed 2026-10-17, America/Bogota; the last observations fall on 2026-10-18 UTC.
All times below are UTC. Sol completed this report and the installed-package manual check.

## Candidate and scope

The candidate and measured build are `9fd70814fd43a57cbaad8a0b3d190e1a4bed7d9f` on
`fix/169-hourly-sync`. A fresh `git fetch origin` still returned
`70859f6bc09811d90305ec69cfec5d808b77cb5c` for `origin/main`, so no merge was needed.
HEAD matched the candidate before the manual check. Only this report and its evidence were
added; no product or harness source was changed, and nothing was pushed.

The chosen interval is **one hour**, aligned to the top of each UTC hour. The immediate
`sync_now` path remains unchanged. This completes the measurement/report portion of
kiro-workflow section 7, steps 5 and 6. The report commit needs its own exact-head final
review under step 7 before any push or pull request.

## Verification already completed on this candidate

These are Sol's prior runs, checked against their retained logs and progress record, not
new executions during this continuation. The [verification summary](evidence/ticket-169-verification-summary-20261017.json)
records command results, source log sizes and SHA-256 hashes, and the named test's output.

| Machine | Command / population | Result |
| --- | --- | --- |
| Fedora 44 x86-64 | `pnpm verify` | PASS: Svelte check 0 errors and 0 warnings; 253 JavaScript tests passed; 687 Rust tests passed, 10 ignored, 0 failed; formatting, Clippy and frontend build passed |
| Fedora | `pnpm tauri:build` | PASS: one RPM bundle |
| Fedora | `scripts/verify-linux-package.sh <candidate.rpm>` | PASS: one installed desktop-entry payload checked |
| Windows 11 x86-64 | `pnpm test:rust` | PASS: 662 passed, 9 ignored, 0 failed |
| Windows | `pnpm lint:rust` | FAIL, allowed baseline exception #221 only: `clippy::bind_instead_of_map`, `src-tauri/src/commands.rs:4295`; formatting passed |
| Windows | `pnpm build` | PASS |
| Windows | `pnpm tauri:build` | PASS: one NSIS bundle |

The Rust counts cover the library suite; the other binary/doc-test targets contained zero
tests. Both machines ran and passed
`sync_sidecar::tests::scheduled_windows_are_aligned_and_never_more_than_one_interval_away`.
Rust tests used `pnpm test:rust`. Windows JavaScript tests were not rerun in this full
verification; their three known failures remain tracked in #221 and are not claimed green.

The installed binaries used by both live Gate 1 runs and the manual check have these hashes:

| Package | Installed app SHA-256 |
| --- | --- |
| RPM | `ffc672cdd986887db9399cc373bc2883ce3013530b89d3b294e3d25a00d96359` |
| NSIS | `fd713fa642516ed90e63cd38ced5f3151bb1a86e618a7acd2e4020e78220b2e2` |

## Both live Gate 1 runs

Both runs used candidate `9fd70814fd43a57cbaad8a0b3d190e1a4bed7d9f`, the installed
packages, disposable replicas of the 5,000-note deterministic sync fixture, and paired
Syncthing v2.1.5 instances over Tailscale. The command was
`node scripts/alpha-harness.mjs sync --candidate 9fd70814fd43a57cbaad8a0b3d190e1a4bed7d9f`,
with the default 70-minute per-wait timeout. Sleep/idle inhibition covered the run.

| Run | Executor | Result and evidence |
| --- | --- | --- |
| `20261017T215139Z` | Sol | **FAIL**: no replacement Windows sidecar within 30 seconds of the deliberate stop. [Trace](evidence/alpha-harness-sync-20261017T215139Z.jsonl) |
| `20261017T230950Z` | Sonnet 5.5, standing in for Sol under the usage-limit rule | **PASS; passing Gate 1 of record.** [Trace](evidence/alpha-harness-sync-20261017T230950Z.jsonl) |

In the first run, the app logged “Sync folder did not acknowledge its pause for the close;
stopping Syncthing”. In the rerun, the gate stopped sidecar PID 28600 at 00:00:17.9315;
replacement PID 3724 appeared at 00:00:20.881, **2.95 seconds later**. Final replicas both
held 5,000 matching notes, no wrong content, conflicts, duplicate IDs or strays, and the
sentinel. The final process reports were empty on both machines.

The sidecar relaunch race is treated as intermittent and pre-existing, for a separate
follow-up issue. Neither result is hidden or averaged away. A comparison with `a5c3670`,
where Gate 1 passed, shows only the interval and its comment changed in
`src-tauri/src/sync_sidecar.rs`; the restart and pause code did not change. This supports
that treatment but does not identify the race's cause or establish a failure rate. No fix
or investigation was attempted here.

The rerun's observable actions align with both hourly boundaries:

| Scheduled batch | Fedora | Windows |
| --- | --- | --- |
| 00:00 | Backup filename timestamp 00:00:11; archive finished 00:00:12.276 | Backup created 00:00:11.785; deliberate interruption at 00:00:17.932 |
| 01:00 | Backup filename timestamp 01:00:11; archive finished 01:00:12.259 | Backup created 01:00:10.907; first complete fixture poll 01:00:21.812 |

Batch starts are inferred from the scheduler and these observations, not timestamped
start log messages. Between the interrupted batch and 01:00, Windows remained at 2,146
matching notes until 01:00:11; the next advancing poll was at 01:00:13.9. The Windows app
stayed alive through both waits. Its second launch PID 62948 appears in the interruption
process report. Its third launch PID was not captured, but that app created the 01:00 backup
and received the remaining notes. The two-hour task limit covered the run, which finished
at 01:00:43.717, about nine minutes before the original task's lifetime would expire.

## Measurement and its limits

Sol independently checked the rerun's JSONL polls, retained application logs, backup file
metadata and Fedora archive entries. The [measurement evidence](evidence/ticket-169-measurements-20261017.json)
contains those observations. Conditions: the candidate packages above, the Gate 1 fixture,
Fedora initially holding 5,000 notes and Windows initially empty. Batch 2 began with Windows
holding 2,146 notes. App indexing and the normal sync/watchdog processes were active;
the logs report an unreachable embedding backend. This is a fixture measurement, not a
long-term measurement of a personal vault or CPU usage.

| Batch / machine | Duration observation | Bulk-mutation lease hold | Pre-sync backup |
| --- | --- | --- | --- |
| 1, Windows | Interrupted deliberately at 00:00:17.932; no full duration | Unavailable | 1,315 bytes; empty receiver |
| 1, Fedora | Interrupted by the gate's close/relaunch; no full duration | Unavailable | 1,483,774 bytes; 5,000 notes |
| 2, Windows | Transfer first observed complete at 01:00:21.812, about 21.8 seconds after the inferred boundary start; last semantic database write 01:00:28.672 | Not logged. Rough timing proxy about 11–39 seconds; see qualification below | 634,404 bytes; 2,146 notes before receiving the remainder |
| 2, Fedora | Sender for the same transfer; local semantic database last written 01:00:21.148. No separate receiving duration or exact batch end | Not logged; exit gives an upper bound of about 38 seconds from the inferred boundary start, with no independently measured lower bound | 1,483,774 bytes; 5,000 notes |

The Windows estimate spans backup creation at 01:00:10.907 through the first complete poll
at 01:00:21.812 (about 11 seconds), bounded above by the empty process report at
01:00:39.610 relative to the inferred 01:00 start. Polling makes that lower estimate
approximate, not a rigorous bound: the last incomplete poll was 01:00:19.693. Fedora's
empty process report was at 01:00:37.859. Database modification times are observations,
not lease-release instrumentation.

**Unavailable:** exact batch start, natural batch end, and lease acquire/release times on
either machine. Neither application logs them. Batch 1 was interrupted intentionally;
batch 2 completed its file transfer, after which Gate 1 closed the apps without waiting
for a natural terminal sync outcome. Therefore 21.8 seconds is a transfer-completion
observation, not a measured full batch duration. There is no dedicated scheduled batch
in this continuation: the existing paired run gives measured work and backup costs on
both machines, provided these limits remain explicit.

**Reason for one hour:** the measured transfer occupied tens of seconds, and each observed
scheduled round created a full archive: roughly 1.48 MB on Fedora, and up to 0.63 MB on
the partly populated Windows receiver. Those are real backup writes and sync work on
both personal machines. An hourly schedule reduces scheduled opportunities from twelve
per hour to one, reducing exposure to repeated full backups and lease pauses. This is
an arithmetic frequency reduction, not a measured twelvefold CPU or retained-space saving.
The lease duration still needs instrumentation before finer tuning. The immediate manual
path below covers transfers that cannot wait an hour.

### Open question: backups on the sender

Fedora wrote a pre-sync backup before both batches despite already holding all 5,000 notes
and acting as sender. Both archives contain 5,000 notes and have the same size. Gate 1's
documentation says backup occurs only before a batch bringing changes. Earlier traces
[`20260929T031635Z`](evidence/alpha-harness-sync-20260929T031635Z.jsonl) and
[`20260930T170722Z`](evidence/alpha-harness-sync-20260930T170722Z.jsonl) show the same
pattern, predating this candidate. This remains an open question; no cause is claimed and
it was not investigated or changed.

## “Sync now” on both installed packages

Sol's usage check returned `OK` before this run. The candidate RPM was installed through
the test-package helper; Windows used the existing candidate NSIS install. Both unlocked
desktop sessions were driven through `tauri-driver`, using the Settings button's actual
click handler. Pairing was explicit on both sides. A fresh shared vault identity isolated
all machine state; Fedora began with one synthetic fixture note and Windows empty. This
small manual fixture proves immediate execution, separately from the scheduled measurement.

The [manual trace](evidence/ticket-169-sync-now-20261018.jsonl) records both folders paused
before the clicks, both showing “Syncing…” afterwards, and the next scheduled boundary
at **02:00:00**:

| Machine | Click began / returned | Batch observed running | Result |
| --- | --- | --- | --- |
| Fedora | 01:45:23.475 / 01:45:23.683 | Folder resumed send-only by 01:45:24.354 | PASS; terminal `success`, “Both devices are up to date.” |
| Windows | 01:45:23.476 / 01:45:23.758 | Folder resumed send-only by 01:45:24.355 | PASS; terminal `success`, “Both devices are up to date.” |

Both clicks resumed a batch within one second of the observation window, roughly fifteen
minutes before the scheduled boundary. Windows created a 1,315-byte pre-sync backup at
01:45:44.162 before receiving. Both terminal outcomes were collected by 01:47:00.104.
That collection time is not an exact per-machine completion time. A post-restoration check
confirmed the received note's SHA-256 matched Fedora's and no Windows test process remained.

## State restoration, privacy and retained artifacts

Fedora used isolated XDG config/data directories; its real app config and data directories
were absent before and remained absent afterwards. The isolated launch config was saved.
On Windows, the complete original roaming and local app directories were preserved by
same-volume rename before launch. Generated test profiles were renamed aside and originals
restored after exit; both original tree hashes matched exactly. This covers config, window
state, logs, WebView state and per-vault state without deleting the test profiles. No app,
watchdog or sidecar remained after exit. The keep-awake holders and drivers were stopped.

The RPM was removed through `remove-test-install`; `rpm -q second-brain` reported it not
installed, and the app executable and both possible desktop-entry paths were absent.
The Windows candidate install remains in place as requested. The manual check's scheduled
driver task remains registered but disabled, because deletion was forbidden.

No cleanup deletions were performed apart from the explicitly requested RPM removal.
Retained artifacts include both live Gate 1 machine-state trees, the rerun's empty initial
start directory, prior verification/build logs and caches, and the manual run's disposable
vaults, test profiles, screenshots, scripts, journals and disabled-task export. These are
listed with byte sizes in a private artifact inventory delivered with the completion reply.
Disposable run trees and the disabled task are cleanup candidates only after approval;
the Windows app and shared build cache are retained, not proposed for deletion.

Public evidence uses generic test-root/profile placeholders. Device identities were already
per-run placeholders in the Gate traces. Hostnames, usernames, private paths, network
addresses, credentials, personal note content and screenshots are excluded. Raw logs,
original traces, profiles and the detailed path inventory remain outside the repository.

## Import and review record

Kiro's imported commits were `1866e33`, `084be02` and `48766e5`. Claude's fixes were
`fe6a937` (Settings hint and Gate 1 timeout) and `9fd7081` (Windows task lifetime).

Round 1 on `fe6a937` found one P2 on the Spec/adversarial axis: the one-hour Windows task
lifetime was shorter than Gate 1's new 70-minute wait. Standards was clean. Claude fixed
it in `9fd7081`. Round 2 on that exact candidate was CLEAN on both Spec and Standards.
The passing live rerun supplies the required execution evidence for the harness changes.

Verification for this requested continuation: **PASS**, with the existing #221 exception
and the measurement limitations stated above. Harness upkeep outcome: `changed` (this
report and evidence); publication and the report commit's final review remain outside
this task.
