# Installed-app alpha harness

Issue #137 tracks the unattended installed-package harness required by #88. Gates are built
and run in matrix order. Gates 1, 2, and 3 are implemented.

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
   loopback REST APIs, and relies on the product's five-minute scheduler for the batch trigger;
4. proves the Windows receiver is partially transferred, stops the exact packaged sidecar
   with `scripts/windows/sync-interrupt.ps1`, and records its replacement PID;
5. closes and relaunches both installed apps, then polls the deterministic fixture until both
   replicas contain exactly 5,000 notes with no conflicts, duplicate IDs, wrong content, or
   strays;
6. requires pre-sync backups and no app, watchdog, sidecar, or orphan after exit.

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

`--machine fedora` or `--machine windows` runs one machine while a step is being fixed. Only a run
of both machines counts as the gate; `run-start` and `run-complete` record the machines and
`gate: true` only for a run of both.

The RPM must already be installed (`sudo rpm -Uvh --replacepkgs <rpm>`). The controller checks it
with `rpm -V`, `scripts/verify-linux-package.sh`, and the installed desktop entry's `Exec`. It
installs the NSIS package silently and checks that the Start-menu entry targets the installed
executable. Each machine then runs the matrix v2 walkthrough through `tauri-driver`, with one
screenshot per step under `~/sb88/evidence/walkthrough-<run>/`:

1. open a fresh vault showing the four PARA roots and no repair issues;
2. create a note, edit it, navigate away at once, reopen it, edit it again, and close the window
   without waiting; both edits must be on disk after the app exits;
3. keyword search, a semantic search once the index is complete, graph, tasks, trash restore,
   note history, backup, and restore;
4. clip `https://en.wikipedia.org/wiki/Zettelkasten` and attach a local file;
5. publish to the disposable Notion page with no note failing, then disconnect, which removes the
   token from the keyring. The controller reads the databases the run created from the vault's
   `.helixnotes/notion/databases.json` and confirms, through the Notion API, that the capture, the
   clip, and the attachment note arrived. The clip and the attachment note carry anchor and
   relative links (#152). It also looks the run's token up in the OS keyring: `secret-tool` on
   Fedora, and on Windows Credential Manager from the desktop session, because an SSH logon has
   none. The lookup must find the token while the vault is connected, and not after Disconnect. Whether or not the walkthrough passed,
   the controller archives the run's databases and deletes a token that a failed step left;
6. export diagnostics and search the archive for a planted credential, a note body marker, a
   note title and path, and the vault path;
7. with the embedding backend pointed at a closed port, capture, edit, move, and keyword search;
8. start with a malformed `config.json`, which must show the startup error, then restore the
   run's configuration;
9. on Windows only (#49): with the vault folder renamed, press Ctrl+Alt+N from the desktop
   session; the app's notification history must gain a new "Quick capture" toast saying the vault
   isn't available;
10. exit through the window's close button and check that no app, sidecar, or watchdog is left.

After the last step, the controller records each vault's tree hash. It uninstalls the Windows
package, checks that the executable and Start-menu entry are gone and the vault hash is unchanged,
and reinstalls the candidate, also when the uninstall fails. The RPM needs root, so after the run
Nicolas runs `sudo rpm -e second-brain`, then:

```sh
node scripts/alpha-harness.mjs walkthrough-uninstalled --run <runId>
```

This appends the Fedora uninstall result to the same trace. It refuses a run with no completed
Fedora walkthrough, and compares the vault with the hash the run recorded.

WebDriver cannot answer native dialogs. The diagnostics export calls the button's own
`export_diagnostics` command with the path the save dialog would return, and the trace says so.
The file attachment goes to the editor's hidden file input, as the picker would deliver it: on
Fedora through a `DataTransfer`, and on Windows, whose WebView2 ignores a synthetic file list, as
the path of a file staged in the run folder, sent the way WebDriver uploads a file.
WebKitWebDriver rejects key input, so Fedora types through `document.execCommand('insertText')`;
Windows uses real WebDriver key actions.

Before every step the Windows desktop must be unlocked; LogonUI.exe running in the desktop session
means it is locked. Fedora's GNOME lock state is recorded, not required, because WebKitGTK keeps
running under the lock screen. The Windows keep-awake request comes from the SSH session, and
Windows ignores a display request from there, so the per-step check is what proves the desktop
stayed unlocked.

The evidence trace is `docs/reports/evidence/alpha-harness-walkthrough-<timestamp>.jsonl`.

## Running a gate from GitHub Actions

`.github/workflows/alpha-harness.yml` runs one gate by `workflow_dispatch` from `main`. Its
first job refuses a candidate unless the hosted `verify` and `windows-rust` checks passed on that
commit. The second job runs on a self-hosted runner labelled `second-brain-alpha` and uses the
`alpha-harness` environment, which holds the Notion secrets `SECOND_BRAIN_NOTION_TOKEN` and
`SECOND_BRAIN_NOTION_PAGE`.

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

## Keeping both machines awake

Run `20260921T023725Z` failed when Windows slept seven minutes in (Kernel-Power 42 at
02:43:06Z). Every run now holds both machines awake before it touches either one:

- Fedora: `systemd-inhibit --what=sleep:idle` for the controller's lifetime.
- Windows: a `PowerSetRequest(PowerRequestSystemRequired)` held by a PowerShell started over
  SSH. It appears in `powercfg /requests` as "Second Brain alpha harness run". The holder
  reports its PID and the controller stops exactly that process when the run ends. Windows
  OpenSSH does not end a session's processes when the client disconnects, so if the controller
  dies the holder still expires by itself after three run timeouts plus 15 minutes (75 minutes
  by default).

The run refuses to start if either hold is not in place within 60 seconds, and fails if either
hold is lost before it finishes.

## Machine-state safety

Fedora gets isolated `XDG_CONFIG_HOME` and `XDG_DATA_HOME` directories. Windows does not honor
those variables for known folders, so the worker journals the original
`%APPDATA%\helixnotes\config.json`, swaps only the active vault and backup location, and restores
the original bytes after all exact-path process checks are empty. A global lock prevents a
second run from replacing that journal. Fresh vault IDs prevent any existing machine-local
vault state from being reused. A junction directs the new Windows machine state into the run
directory; retained state is evidence, not production state. Finalizing a run removes that
junction (after checking it targets the run) and the scheduled task; the run directory stays.
Forced cleanup stops only packaged processes that started after the run's journal was written,
and refuses if an older one is running.

Traces are public. The controller redacts the home directory, Windows profile paths, and host
names at the single point where it writes them.

Windows launch uses the harness-owned `SecondBrainAlphaHarness` scheduled task with interactive
logon and the exact installed executable under `D:\SecondBrainTest`. The task supplies desktop
session placement only. Key presses and full-desktop screenshots come from
`scripts/windows/alpha-desktop.ps1`, which runs in the same session through a short-lived
`SecondBrainAlphaHarnessDesktop` task.

Gate 1 passed unattended on candidate `4ea6b9e` (run `20260921T114623Z`), so Gate 2 may start.
