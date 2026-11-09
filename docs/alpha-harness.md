# Installed-app alpha harness

Issue #137 tracks the unattended installed-package harness required by #88. Gates are built
and run in matrix order. Gates 1, 2, and 3 are implemented.

## Installing the Fedora candidate

Agents install and remove the candidate RPM without Nicolas, through a root-owned helper that
sudo runs without a password. [linux-test-package.md](linux-test-package.md) covers its use,
setup, and reach.

The walkthrough and acceptance gates install the `--fedora-rpm` candidate themselves. The
walkthrough removes it after a passing run; the acceptance gate removes it after every run whose
Fedora app it could stop. Gates 1 and 2 use whatever is installed, so install the candidate with
the helper before running them.

## Gate 1: interrupted sync recovery

Run from the Fedora 44 desktop session while `sb-windows` is reachable:

```sh
node scripts/alpha-harness.mjs sync --candidate <sha>
```

`--candidate` names the commit both installed packages were built from (default `HEAD`). The
binaries must embed that commit, and the harness branch must not change app sources (`src`,
`src-tauri`, `static`, lockfile, Vite and Svelte config) relative to it, so harness-only commits
reuse the same packages.

The controller uses only Node standard-library process and filesystem APIs. It copies its
Windows worker and the existing fixture/interruption scripts to
`D:\SecondBrainTest\sb88\tools`, then:

1. refuses to start if either installed app is already running or either test disk has less
   than 5 GiB free, if the targets are not Fedora 44 x86-64 and Windows 11 x86-64, or if
   either installed binary does not embed the controller repository's exact commit;
2. creates one fresh vault identity and disposable replicas under `~/sb88/runs/` and
   `D:\SecondBrainTest\sb88\runs\`;
3. starts both installed apps, pairs their private Syncthing instances over their existing
   loopback REST APIs, and relies on the product's hourly scheduler for the batch trigger, so
   each of its two batches can take up to an hour to start;
4. proves the Windows receiver is partially transferred, stops the exact packaged sidecar
   with `scripts/windows/sync-interrupt.ps1`, and records its replacement PID;
5. closes and relaunches both installed apps, then polls the deterministic fixture until both
   replicas contain exactly 5,000 notes with no conflicts, duplicate IDs, wrong content, or
   strays;
6. requires a pre-sync backup on the receiving Windows machine, records any on Fedora (a machine
   backs up only before a batch that brings it changes), and requires no app, watchdog, sidecar,
   or orphan after exit.

Every fixture poll and lifecycle observation is appended immediately to
`docs/reports/evidence/alpha-harness-sync-<timestamp>.jsonl`. A failed run keeps its trace and
disposable machine state for diagnosis. Public traces use per-run device placeholders and do
not record Syncthing device IDs, API keys, or Tailscale addresses.

## Gate 2: interrupted restore recovery

Run from the Fedora 44 desktop session while `sb-windows` is reachable:

```sh
node scripts/alpha-harness.mjs restore --candidate <sha>
```

Restore is started from the app's own Settings > Backup UI through `tauri-driver`, so the app gets
no control channel: the webview allows automation only when `tauri-driver` sets
`TAURI_WEBVIEW_AUTOMATION=true`. Fedora uses `WebKitWebDriver`. Windows uses `tauri-driver.exe`
and a `msedgedriver.exe` matching the installed WebView2, both under
`D:\SecondBrainTest\sb88\driver`. The driver starts through the `SecondBrainAlphaHarnessDriver`
scheduled task so the app lands on the interactive desktop, and the controller reaches it over an
`ssh -L` tunnel.

Each machine runs in turn, with sync off:

1. generate the 5,000-note fixture, make a backup through the UI, then edit, delete, and add notes
   so the vault differs from the backup;
2. settle and hash the pre-restore state: every directory and file, `.helixnotes` included. A
   launch can still write (it normalizes new notes and the next open sweeps the empty
   `.helixnotes/staging` that leaves), so a state counts only once another launch leaves its hash
   unchanged;
3. restore once without interruption, settle, and hash the restored state;
4. for each kill point (`journal`: the restore journal appears; `stage-half` and `stage-late`: 50%
   and 95% of the backup's files are staged), reset the vault to the pre-restore snapshot, start
   the restore, and kill the exact app PID from a watcher running next to the app. The journal
   still being on disk after the kill proves the kill landed inside the restore;
5. relaunch, which must show the "Restore interrupted" notice (screenshotted to `~/sb88/evidence`,
   then dismissed), leave no `.second-brain-restore-*` entry beside the vault, and settle to exactly
   the pre-restore or the restored hash.

An external kill lands during unpacking; the millisecond commit window is covered by the per-step
tests in `backup.rs` (#142). Evidence is written to
`docs/reports/evidence/alpha-harness-restore-<timestamp>.jsonl`.

Never run two controllers at once. The Windows `recover` step cannot tell a stale lock from a live
run's, so a second controller would restore the real Windows config under the first.

## Gate 3: installed-package walkthrough

Run from the Fedora 44 desktop session while `sb-windows` is reachable, with Ollama serving
`embeddinggemma` on the Windows desktop and a disposable Notion page shared with an integration:

```sh
export SECOND_BRAIN_NOTION_TOKEN=ntn_... SECOND_BRAIN_NOTION_PAGE='<page title>'
node scripts/alpha-harness.mjs walkthrough --candidate <sha> \
  --windows-installer 'D:\SecondBrainTest\src\src-tauri\target\release\bundle\nsis\Second Brain_0.1.0-alpha.1_x64-setup.exe' \
  --fedora-rpm ~/second-brain-candidate/src-tauri/target/release/bundle/rpm/<rpm>
```

The walkthrough uses the existing Windows Ollama service on port 11434 by default. For a
gate-owned temporary server, add `--ollama-port 11436`; the Windows app, Windows preflight,
and Fedora SSH tunnel then use that port.

`--machine fedora` or `--machine windows` runs one machine while a step is being fixed. Only a run
of both machines counts as the gate; `run-start` and `run-complete` record the machines and
`gate: true` only for a run of both.

The controller installs the RPM through the test package helper, then checks it
with `rpm -V`, `scripts/verify-linux-package.sh`, and the installed desktop entry's `Exec`. It
installs the NSIS package silently and checks that the Start-menu entry targets the installed
executable. Each machine first launches that installed entry and checks the app process, then
stops it by PID. The matrix v2 walkthrough runs through `tauri-driver`, which launches the same
installed binary, with one screenshot per step
under `~/sb88/evidence/walkthrough-<run>/`:

1. open a fresh vault showing the four PARA roots and no repair issues. Sync is on, unpaired, so the
   Syncthing sidecar and its watchdog must be running;
2. create a note, edit it, navigate away at once, reopen it, edit it again, and close the window
   without waiting; both edits must be on disk after the app exits;
3. keyword search, a semantic search once the index is complete, graph, tasks, trash restore,
   note history, backup, and restore;
4. clip `https://en.wikipedia.org/wiki/Zettelkasten` and attach a local file. The route from the
   Windows test machine to the page stalled past the app's 20 s limit in one of 40 plain fetches,
   so on either machine a clip the app reports as timed out is retried once, and the result
   records it; any other clip error fails the step;
5. publish to the disposable Notion page with no note failing, then disconnect, which removes the
   token from the keyring. The controller reads the databases the run created from the vault's
   `.helixnotes/notion/databases.json` and confirms, through the Notion API, that the capture, the
   clip, and the attachment note arrived. The clip and the attachment note carry anchor and
   relative links (#152). It also looks the run's token up in the OS keyring: `secret-tool` on
   Fedora, and on Windows Credential Manager from the desktop session, because an SSH logon has
   none. The lookup must find the token while the vault is connected, and not after Disconnect.
   Whether or not the walkthrough passed, the controller archives the run's databases and deletes
   a token that a failed step left;
6. show the Export diagnostics button in Settings > Maintenance, export diagnostics, and search the
   archive for a planted credential, a note body marker, a note title and path, and the vault path.
   The app moves the planted key into the OS keyring, so the run refuses to start when the keyring
   already holds that entry and deletes it afterwards (see Machine-state safety);
7. with the embedding backend pointed at a closed port, capture a note, edit an existing one (the
   edit must be on disk after exit), move a note, and keyword search;
8. start with a malformed `config.json`, which must show the startup error and leave exactly one
   damaged copy beside the config, then restore the run's configuration;
9. on Windows only (#49): with the vault folder renamed, press Ctrl+Alt+N twice from the desktop
   session. The app's log must record a toast shown for each press, and after each press its
   notification history must hold exactly one "Quick capture" toast saying this run's vault isn't
   available: the second replaced the first (#153);
10. launch once more with the sidecar and watchdog running, exit through the window's close button,
    and check that no app, sidecar, or watchdog is left.

After the last step, the controller records each vault's tree hash. It uninstalls the Windows
package, checks that the executable and Start-menu entry are gone and the vault hash is unchanged,
and reinstalls the candidate, also when the uninstall fails. It removes the Fedora RPM through the
test package helper and runs the same checks: package, executable, and desktop entry gone, and
the vault unchanged, so a passing run of the Fedora walkthrough leaves no test build installed.
The candidate stays installed after a failed run and a `--machine windows` run; an agent removes
it with the helper when it is no longer needed.

WebDriver cannot answer native dialogs. The diagnostics export calls the button's own
`export_diagnostics` command with the path the save dialog would return, and the trace says so.
The file attachment goes to the editor's hidden file input, as the picker would deliver it: on
Fedora through a `DataTransfer`, and on Windows, whose WebView2 ignores a synthetic file list, as
the path of a file staged in the run folder, sent the way WebDriver uploads a file.
WebKitWebDriver rejects key input, so Fedora types through `document.execCommand('insertText')`;
Windows uses real WebDriver key actions.

Before every step the Windows desktop must be signed in and unlocked: explorer.exe names the desktop
session, and LogonUI.exe running in it means it is locked. Fedora must also be unlocked: its
WebDriver screenshot timed out under the GNOME lock screen in run 20260926T144833Z. A GNOME idle
inhibitor keeps that screen from locking during the run. The Windows keep-awake request
comes from the SSH session, and Windows ignores a display request from there, so the per-step check
is what proves the desktop stayed unlocked.

The evidence trace is `docs/reports/evidence/alpha-harness-walkthrough-<timestamp>.jsonl`.

## Running a gate from GitHub Actions

`.github/workflows/alpha-harness.yml` runs one gate by `workflow_dispatch` from `main`. Its
first job refuses a candidate unless the hosted `verify` and `windows-rust` checks passed on that
commit. The second job runs on a self-hosted runner labelled `second-brain-alpha` and uses the
`alpha-harness` environment, which holds the Notion secrets `SECOND_BRAIN_NOTION_TOKEN` and
`SECOND_BRAIN_NOTION_PAGE`. Set the dispatch `ollama_port` input to 11436 when using a
gate-owned server; the default 11434 uses the existing user service.

The controller copies the Fedora app's `config.json` as each run's template. It reads
`$XDG_CONFIG_HOME/io.github.zulucodedesign.SecondBrain/config.json`, falling back to
`~/.config/io.github.zulucodedesign.SecondBrain/config.json` when the variable is unset, and stops
if the file is missing. The real profile gains that file only when an installed #89 build first
launches against it. Until then, set the `fedora_config_home` input to the absolute path of an
isolated XDG config directory whose migrated `config.json` exists, such as the one left by an
installed-package migration check. `~` is not expanded, so give an absolute path.

The repository is public, so no runner stays registered. Before a dispatch, register one from a
terminal in the Fedora desktop session, with a registration token from the repository's
Actions > Runners page:

```sh
./config.sh --url https://github.com/zulucode-design/second-brain --token <token> \
  --labels second-brain-alpha --ephemeral --unattended
./run.sh
```

`--ephemeral` makes it take exactly one job and deregister. Starting it from the desktop terminal
gives tauri-driver the session's display and D-Bus, which a system service would not have.
The runner runs as Nicolas's account, so a walkthrough dispatch installs and removes the
candidate RPM through the test package helper with no one present. The runner gets no other
sudo access.

## Keeping both machines awake

Run `20260921T023725Z` failed when Windows slept seven minutes in (Kernel-Power 42 at
02:43:06Z). Every run now holds both machines awake before it touches either one:

- Fedora: `systemd-inhibit --what=sleep:idle` and `gnome-session-inhibit --inhibit=idle` for the controller's lifetime.
- Windows: a `PowerSetRequest(PowerRequestSystemRequired)` held by a PowerShell started over
  SSH. It appears in `powercfg /requests` as "Second Brain alpha harness run". The holder
  reports its PID and the controller stops exactly that process when the run ends. Windows
  OpenSSH does not end a session's processes when the client disconnects, so if the controller
  dies the holder still expires by itself after three run timeouts plus 15 minutes (225 minutes
  by default).

The run refuses to start if either hold is not in place within 60 seconds, and fails if either
hold is lost before it finishes.

## Machine-state safety

### Temporary Ollama servers on Windows

Before a gate uses Ollama, decide who owns the server. An existing user service stays
user-owned: leave its process and configuration alone. If a gate needs a temporary server,
start `scripts/windows/ollama-gate.ps1` before the gate in a dedicated PowerShell process.
The harness selects its port but does not start or stop that process. Keep its companion
`owned-process.cs` beside it when copying it to Windows:

```powershell
powershell.exe -NoProfile -File scripts/windows/ollama-gate.ps1 -Port 11436 -LifetimeSeconds 3600
```

From Fedora, run that command through `ssh sb-windows` as a background process that stays
connected until the gate ends. Do not launch it with `Start-Process` from an SSH command that
then returns: on 2026-10-03 a supervisor started that way vanished within a minute of answering,
with nothing on its stderr, and the walkthrough failed its Ollama preflight. The same script
held open in one SSH session served the whole gate. Before starting the controller, check that
`/api/tags` still answers with the same server PID a minute or more after the first answer.

Run the walkthrough with `--ollama-port 11436` and probe `/api/tags` before sending requests.
The output records the supervisor PID and server PID; record them with the gate evidence.
The default endpoint is `http://127.0.0.1:11436`, separate from the usual user service on
11434. The harness forwards the selected port to Fedora over SSH and checks both endpoints
again before recording a passed walkthrough. The one-hour default lease
must exceed the whole gate run; increase it for longer runs, up to 7200 seconds.
The wrapper uses the installed Ollama and its existing model store; it does not download models.

Whoever starts the temporary server must stop the exact supervisor PID after the gate and
wait for it to exit. The Windows Job Object then terminates the server and all descendants.
The same cleanup happens if the supervisor is killed or the server exits early. If SSH or
the controller disappears without stopping the supervisor, its finite lease expires and
closes the job. An expiry before the final endpoint check fails the gate; treat any expiry
while gate work is still running as a failed gate. Capture process
creation times with the PID if cleanup is deferred to a later session; refuse a reused PID.

Do not start a bare background `ollama serve` for a gate or stop only its parent process.
Windows does not terminate children when a parent exits. Recent Ollama releases load models
in `llama-server.exe`, which can survive as an orphan holding GPU memory. An absent
`ollama.exe` process is therefore insufficient cleanup evidence. While a model is loaded,
record worker PIDs and creation times from the recorded server's process tree (for direct
children, query `Get-CimInstance Win32_Process -Filter "ParentProcessId = $serverPid"` with
`$serverPid` set to the emitted server PID). After cleanup,
check the exact server and observed worker PIDs. The Job Object owns descendants even when no
worker was observed; the regression below tests that guarantee with synthetic child processes.
Do not sweep processes by name or adopt existing orphans.

Run the lifecycle regression on Windows without loading any model:

```powershell
powershell.exe -NoProfile -File tests/windows/ollama-gate.test.ps1
```

### App state

Fedora gets isolated `XDG_CONFIG_HOME` and `XDG_DATA_HOME` directories. Windows does not honor
those variables for known folders, so the worker journals the original
`%APPDATA%\io.github.zulucodedesign.SecondBrain\config.json`, swaps only the active vault and backup location, and restores
the original bytes after all exact-path process checks are empty. A global lock prevents a
second run from replacing that journal. Every launch also saves window geometry to
`.window-state.json` in the same folder, so the worker journals that file with its SHA-256,
restores it (or removes it if the run created it) wherever it restores `config.json`, and fails
unless the restored file has the pre-run hash, or is absent when it was absent before. Fresh vault IDs prevent any existing machine-local
vault state from being reused. A junction directs the new Windows machine state into the run
directory; retained state is evidence, not production state. Each launch also writes the
profile's `logs` and `EBWebView` (WebView2) folders, so the worker renames both aside, to
`%LOCALAPPDATA%\io.github.zulucodedesign.SecondBrain.alpha-harness-<run>`, and junctions each to
the run directory (#174). Recover and finalize remove only junctions that target the run and
rename the folders back. Finalizing a run also removes the vault junction (after checking it
targets the run) and the scheduled task; the run directory stays.
Forced cleanup stops only packaged processes that started after the run's journal was written,
and refuses if an older one is running. Manual checks that launch the installed app outside the
harness get neither the journal nor the junctions: they change the real `.window-state.json`,
`logs` and `EBWebView`, and write per-vault state into the real
`%LOCALAPPDATA%\io.github.zulucodedesign.SecondBrain\vaults`, and whoever runs them must record and
restore or remove all of it.

Every gate ends each machine's part with the same teardown, and none of its steps can be skipped.
On Windows that is: stop the driver, wait for the app's processes to exit, force-stop them if they
do not, then restore the config, window state and profile folders. On Fedora it is: stop the
app's and driver's exact PIDs, then wait for them to exit. Each step runs even when an earlier
one failed or a trace line could not be written (a full evidence disk, say). The Windows restore
is attempted even when the processes would not stop; the worker refuses it while any of them
remains, and that refusal fails the run. The teardown reports its first error, adding that the
trace could not be written when that happened, so a teardown whose trace is incomplete never
passes.

The same holds inside the Windows worker, down to each item. Stopping the driver stops each
`msedgedriver` under the run's `tauri-driver`, then `tauri-driver` itself, each by exact PID and
each even when one before it failed; a PID that has exited already counts as stopped. The forced
stop does the same for each of the run's app, watchdog and sidecar processes. Finalize restores
`config.json`, then the window state, moves each of the run's damaged config copies out of the
profile, renames each profile folder back, and removes the run's junction and each of its tasks.
Each of these runs even when one before it failed: a `config.json` a file lock keeps from being
replaced, a `logs` folder that cannot be put back, or a full D: drive that stops a damaged copy.
A replace that fails leaves no temporary file beside the config. Finalize removes the lock only
when every step succeeded. Otherwise the lock and journal stay, and the next run's recovery
restores the config, window state, damaged copies and profile folders before that run starts; a
later finalize of the run removes its junction and tasks.

Each gate's own teardown follows the same rule: Gate 1's stops of both apps and its Windows
finalize, and the acceptance gate's network and isolation restores, its machine teardown and its
removal of the Fedora candidate. A failure report or trace line that cannot be written never skips
a later step, and the gate ends with its first real error. A teardown that failed only to write
its trace never takes the place of a real error from another step: the gate reads every
teardown's result, reports the first real error, and adds that the trace could not be written,
whichever teardown it failed in.

Traces are public. The controller redacts the home directory, Windows profile paths, and host
names at the single point where it writes them.

Windows launch uses the harness-owned `SecondBrainAlphaHarness` scheduled task with interactive
logon and the exact installed executable under `D:\SecondBrainTest`. The task supplies desktop
session placement only. Key presses and full-desktop screenshots come from
`scripts/windows/alpha-desktop.ps1`, which runs in the same session through a short-lived
`SecondBrainAlphaHarnessDesktop` task. The worker waits up to five minutes for that task, while it
is `Queued` or `Running`; only `Ready` or `Disabled` count as finished, and any other state, or one
that cannot be read, does not. On every way out (a finish, an overrun, or a failure while waiting or
stopping) it removes the task only once the task is confirmed finished. A task not known to be
finished is stopped first, even when its state cannot be read. Each timeout around the wait
(the worker's PowerShell call, then the controller's SSH call) is longer than the one inside it, so
a script never writes a result after its caller has stopped waiting for it. If the task cannot be
confirmed stopped, it stays registered, the call fails naming it, and a credential check keeps its
result file; the file's name is that call's own, so it answers no later call. Stop the task by hand
before the next run: that run's first desktop call refuses while it still runs.

Gate 1 passed unattended on candidate `4ea6b9e` (run `20260921T114623Z`), so Gate 2 may start.

### AI keys in the OS keyring

The Gate 3 walkthrough plants a synthetic OpenAI key (`sk-alpha-harness-<uuid>`) in the app's
`config.json` to prove that diagnostics redact it. The app moves that key into the OS keyring, under
account `ai:openai` and service `io.github.zulucodedesign.SecondBrain` (Secret Service on Fedora;
Credential Manager on Windows, where the credential's target is
`ai:openai.io.github.zulucodedesign.SecondBrain`). The run's isolated config folders do not reach
the keyring, so it is the user's own store, and the key the app files there would replace a real
one. The rule is therefore part of the machine-state restore, and the run keeps it as follows:

- **It refuses when an AI key the run would plant is already in the keyring.** The controller asks
  each machine in the run whether the entry exists before it changes either machine: before the
  candidate RPM is installed and before the Windows stale-run recovery. Only reading the candidate
  and harness commits and deploying the harness's own tools to `D:\SecondBrainTest` come first,
  because the Windows check runs from those tools. Today the run plants only `openai_api_key`, so
  it asks about `ai:openai`. If an entry is there, the run stops and names the machine, account and
  service. Move the key aside by hand, then run again. The trace keeps the check and the refusal.
- **It asks again just before the plant.** Launches between the first check and the plant can file
  a key from the copied config. Immediately before a machine's `config.json` gets the key, the
  controller looks again and arms that machine's removal only when no entry is there. If one is,
  that machine's run stops with nothing planted and nothing deleted.
- **A locked entry is present.** On Fedora, existence comes from the Secret Service's own
  `SearchItems` method (`busctl --user call org.freedesktop.secrets /org/freedesktop/secrets
  org.freedesktop.Secret.Service SearchItems`), which lists the matching unlocked and locked items
  by path and never returns a secret. An entry exists when either list is non-empty. `secret-tool
  lookup` is not used: it exits 1 with no message both for no entry and for an entry it could not
  unlock. On Windows, `alpha-desktop.ps1` reads the credential before and after its work, and an
  entry either read saw exists, so `{ found: false, left: true }` counts as present.
- **A lookup that fails is never read as "absent".** On Fedora, a `SearchItems` call that fails (no
  session bus, no Secret Service) or answers in any other shape is a failure. On Windows, only
  `ERROR_NOT_FOUND` (1168) from `CredReadW` means absent, and only success or `ERROR_NOT_FOUND` from
  `CredDeleteW` is a completed delete. Any other code stops `alpha-desktop.ps1` before it answers;
  it writes the call and the code, nothing else, and exits 1. A failed check before the run or
  before the plant stops the run with nothing planted. A failed check during cleanup records the
  entry as `left: null` (unknown) and fails the run.
- **It learns only whether the entry exists.** On Fedora, `SearchItems` answers with item paths
  only, and removal uses `secret-tool clear`, which prints nothing and still runs with its standard
  output sent to the operating system's null device. `clear` deletes unlocked items only, so an
  entry locked by then stays, and the search after it reports the entry as left. On Windows,
  `scripts/windows/alpha-desktop.ps1` runs in the desktop session, because an SSH logon has no
  Credential Manager, and writes `found` and `left` to a result file named for that one call, which
  the worker deletes once read. It accepts exactly the app's four AI targets (`ai:anthropic`,
  `ai:openai`, `ai:ollama` and `ai:openai-compatible`, each followed by the service) and the Notion
  token, and refuses every other target. No value, part of one, length or hash reaches the
  controller, a log, a file or a trace.
- **It removes what it planted, after a pass, a failure or Ctrl+C.** When a machine's part of the
  run ends and its app and driver are stopped, the controller deletes the entry on every machine
  whose plant was armed, and checks that none is left. An entry that is left fails the run. A run
  that refused, or failed or stopped before the plant, deletes nothing. Each cleanup step (Notion,
  then the machine's teardown described under App state, then the key) runs even when an earlier
  one, a failure report or a trace write threw, for example because the evidence disk filled up.
- **Ctrl+C and `SIGTERM` stop the run and clean up.** The first signal stops the walkthrough at its
  next step. If the gate is still running, the controller ends its WebDriver sessions, stops its
  machine's app and driver by exact PID (on Windows the app's sidecar and watchdog too), waits until
  none of them runs, and waits for the gate to unwind. Only then does it run the same cleanup as a
  failed run, so a request the app was still serving, such as a Notion Connect, cannot store a
  credential after the cleanup has looked. A failure while stopping them is recorded and the
  cleanup goes on: the machine's full teardown still runs, stops them again and restores what the
  run changed. A signal during the final checks still fails the run: a run that received one
  never records `run-complete` and always exits non-zero. Press Ctrl+C once: the terminal also
  sends it to the controller's own child processes, so a second press can cut short the commands
  the cleanup is running.
- **Only a hard kill leaves the entry.** A controller killed any other way (`SIGKILL`, a closed
  terminal, a power loss) cannot clean up. Cancelling the GitHub Actions run counts as one: the
  runner signals the step's shell rather than the controller, and does not wait for the controller's
  cleanup ([workflow cancellation](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-cancellation)).
  The entry left behind makes the next run refuse, naming it, and someone deletes it by hand after
  confirming it holds the synthetic value.
- **The trace records existence only.** Each machine gets an `ai-keys-checked` event before the run
  (`existed` for each account), an `ai-keys-armed` event before its plant (`armed`, and `existed`
  for each account), and an `ai-keys-removed` event after its cleanup (`armed`, and when armed
  `removed` and `left` for each account). A machine whose gate a signal stopped also gets a
  `stopped-gate-settled` event once its app is stopped and its gate has unwound.

Any one-off verification script that plants a key in the app config must do the same: check the
keyring first, counting a locked entry as present, and refuse if the entry is there; delete what it
planted whether it passed, failed or was interrupted; check that nothing is left; and keep every
value out of its output. The installed checks for #237 planted a key without doing so and left a
fake `ai:openai` entry on both machines, deleted by hand on 2026-11-02.

`tests/alpha-harness-keyring.test.mjs` runs the walkthrough controller's own sequence against fake
machines, fake signals and fakes of both keyrings: an entry present before the run, locked or not,
one that appears before the plant or between the Windows script's two reads, failures before and
after the plant, `SIGINT` and `SIGTERM` (including during a Notion Connect and during the final
checks), trace writes that fail after the plant, lookup and delete errors on each platform, and the
Windows result files and timeouts. It uses neither the real keyring nor a real machine.

## Harness upkeep

The sections above describe how to run the harness. This section describes how to keep it true to
the app as the app changes. It governs every run, not only a maintenance pass, and it is the one
home for these rules: a later upkeep skill points here instead of restating them.
[alpha-harness-features.md](alpha-harness-features.md) is the companion coverage record, one entry
per user-facing feature with the gate that drives it and the gaps no gate drives.

Whoever runs a gate owns the drift that run exposes. A step whose observed behavior no longer matches
this file or the coverage record is that run's finding, and it is carried to one of the three
outcomes below before the run is called done. Product defects go to Nicolas as issues; nobody else
inherits the drift.

**Check an instance before driving it.** Run the health check before the first drive of a session,
again on each fresh session where sessions are the unit, and again after any failed drive. Where
the check cannot see the failure, such as a wedged view on a healthy process, reset to a known
state or relaunch rather than drive on. Two runs have already failed this way: the Ollama
supervisor that vanished within a minute of answering on 2026-10-03, and run `20260926T144833Z`,
whose WebDriver screenshot timed out under the GNOME lock screen.

**Evidence outlives cleanup, and gets checked where it lives.** Teardown removes instances and
scratch state. It never removes a trace, a screenshot, or a hash record. After cleanup, confirm
the evidence is still at the path the gate names. A cleanup that eats its own proof fails the run
even when every step passed.

**Nothing a drive started outlives that drive.** This already holds for the Ollama supervisor and
the state junctions. It holds for failed attempts too: an iteration that errored cleans its own
residue before the next one starts, and a shared instance has its residue removed rather than the
instance.

**A gate is green only on one candidate.** Every row the gate depends on reaches a real run, and a row
that has never been run keeps the gate amber however well the reasoning reads. A row may carry evidence
from an earlier candidate only when that run happened and nothing since affected it, under the matrix's
candidate invalidation rule: a later change to runtime code, dependencies, packaging, capabilities, or
configuration invalidates the affected evidence and that row is rerun. Carried rows name the commit they
ran on and why the change since does not reach them. Spreading green rows across candidates and reading
them together is how a regression survives a passing gate.

**Say how the run ended, in these words.** `clean` means full coverage with nothing worth shipping.
`changed` means one pull request of proven corrections. `blocked` means coverage could not finish,
and names what blocked it. Inconclusive is not a pass. Report a negative result instead of
retrying until it reads better.

**A feature nobody can reach is `verified-unreachable`, with its reason.** Record the concrete
prerequisite that blocks it, such as an authorization, an entitlement, an operating system, or
external state, and record the route that was attempted. A prerequisite this file omits is a
defect in this file. #42 is the current example, and both halves of it have moved since it was
filed. The vault-unavailable notification needed a packaged install, which #49 delivered, and Gate 3
step 9 now proves it displays. Its error path is no longer unreportable either: `f14b0ac` dropped
`tauri-plugin-notification`, whose Windows backend swallowed the result, and
`hotkey::windows::notify_vault_unavailable` logs a warning when the WinRT `Show` call returns an
error. So that path is reachable and reported, and unverified only because nothing the harness can
do makes `Show` fail on a signed-in desktop session. Record that missing route as the prerequisite,
and restate #42 around it.

**Triage a mismatch before changing anything.** A description the app no longer matches is
documentation drift, so fix the documentation. Working behavior the harness cannot drive is a
harness gap, so fix the harness. Behavior that is actually broken is a product defect, so record
it for Nicolas and keep it out of the documentation change. Never make a regression disappear by
editing this file to match it.

**Upkeep touches the harness, this file, the coverage record, and the evidence it proves, never
product code.** That includes the report updates `docs/reports/README.md` requires when findings or
verification status change. A maintenance pass that edits the app has stopped being a maintenance
pass: record the defect instead.

**Re-drive a harness fix before it ships.** A change to the controller, a worker, or a gate script
is proved by a live run of the gate it touches, not by review alone. Commit `6b501eb` is the
shape: a lease-timing fix in the acceptance save path earns its diff once the acceptance gate has
passed with it.
