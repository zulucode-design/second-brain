# Harness feature map

One entry per user-facing feature, naming how a user reaches it, how the harness drives it, and
what observable state proves it. [alpha-harness.md](alpha-harness.md) owns the gates, the commands,
and the upkeep rules; this file owns coverage. A proof that drives one convenient entry point is
incomplete when this map lists others.

This map is one file with one section per feature rather than a directory of files. Split it when a
feature grows sub-features of its own, or when the file passes roughly 400 lines.

## Proof level

Every entry records how its strongest proof was established. The scale is this file's own rubric for
gate coverage. The verification matrix does not use it: that file carries Area, Evidence, and Status
columns, names the candidate inside the evidence it cites, and tracks ownership elsewhere. This scale
is a shorter way to say how far a feature's gate coverage reaches.

1. Asserted.
2. A line of code was cited.
3. The failing case was argued unreachable.
4. A script or test ran.
5. The feature was driven in the installed app.

A level here means gate coverage only. A feature can carry evidence from somewhere else and still sit
low, so an entry names that evidence rather than hiding behind the number. Compact layout is the case
to keep in mind: no gate drives it, and it has already passed twice outside the gates, in a
release-build drive recorded in
[issue-89-review-fixes-verification-2026-09-28.html](reports/issue-89-review-fixes-verification-2026-09-28.html)
and on both installed packages in the matrix's migration and compact-layout row.

Current evidence candidate is `a5c3670`, the commit external alpha 0.1.0-alpha.1 is published from.
The editor, graph, semantic retrieval, and exact-scan performance budgets keep their `8d290b7`
evidence for the reasons the matrix records.

## Index

| Feature | Driven by | Level |
| --- | --- | --- |
| [Vault open and PARA roots](#vault-open-and-para-roots) | Gate 3 step 1 | 5 |
| [Capture and edit durability](#capture-and-edit-durability) | Gate 3 steps 2 and 7 | 5 |
| [Keyword search](#keyword-search) | Gate 3 steps 3 and 7, acceptance criterion 3 | 5 |
| [Semantic search](#semantic-search) | Gate 3 step 3 | 5 |
| [Graph](#graph) | Gate 3 step 3 | 5 |
| [Tasks](#tasks) | Gate 3 step 3 | 5 |
| [Trash and restore from trash](#trash-and-restore-from-trash) | Gate 3 step 3 | 5 |
| [Note history](#note-history) | Gate 3 step 3 | 5 |
| [Backup and whole-vault restore](#backup-and-whole-vault-restore) | Gate 2, Gate 3 step 3 | 5 |
| [Web clipping](#web-clipping) | Gate 3 step 4 | 5 |
| [File attachments](#file-attachments) | Gate 3 step 4, acceptance criteria 4 and 7 | 5 |
| [Notion publish and disconnect](#notion-publish-and-disconnect) | Gate 3 step 5 | 5 |
| [Diagnostics export](#diagnostics-export) | Gate 3 step 6 | 5 |
| [Operation without the AI backend](#operation-without-the-ai-backend) | Gate 3 step 7 | 5 |
| [Damaged configuration at startup](#damaged-configuration-at-startup) | Gate 3 step 8 | 5 |
| [Vault-unavailable toast](#vault-unavailable-toast) | Gate 3 step 9, Windows only | 5 |
| [Quick capture](#quick-capture) | nothing | 2 |
| [Clean shutdown](#clean-shutdown) | Gate 3 steps 2 and 10, Gate 1 step 6 | 5 |
| [Sync between machines](#sync-between-machines) | Gate 1, acceptance criteria 1 to 11 | 5 |
| [Conflicting edits](#conflicting-edits) | Acceptance criterion 8 | 5 |
| [External note viewer](#external-note-viewer) | nothing | 2 |
| [Obsidian import](#obsidian-import) | nothing | 2 |
| [Holding area](#holding-area) | nothing | 2 |
| [Command palette](#command-palette) | nothing | 2 |
| [Compact layout](#compact-layout) | nothing | 2 |
| [Second note window](#second-note-window) | nothing | 2 |

The seven entries at level 2 are the coverage gaps. Each names what driving it would need.

## Vault open and PARA roots

A fresh vault opens showing the four PARA roots with no repair issues, and sync is on but unpaired.
The user reaches it from the vault picker on first launch. Gate 3 step 1 drives it and also requires
the Syncthing sidecar and its watchdog to be running. Proof is the four roots present, no repair
issue, and both processes alive.

## Capture and edit durability

Writing a note and editing it, including the two races this area keeps producing defects in: editing
then navigating away at once, and editing then closing the window without waiting. Reached from the note
list or quick capture. Gate 3 step 2 drives both races and requires both edits on disk after the app
exits. Step 7 repeats capture and edit with the embedding backend unreachable. Gotcha: the window
save and close path has produced #149 (an edit during an in-flight rename), #191 (a note left stale
after a conflict choice or sync), #192 (unsaved edits on a note that changed on disk), and #196 (a
close rewriting the note with no edit). The mechanisms differ, so a change here needs the whole step
re-driven, not only the case that broke.

## Keyword search

Literal full-text search over the vault, independent of AI. Reached from the search panel. Gate 3
step 3 drives it; step 7 drives it with the backend down; acceptance criterion 3 requires a note
created on one machine to be findable by search on the other. Proof is the created note in results.

## Semantic search

Retrieval by meaning, over the machine-local semantic index. Reached from the search panel's mode
switch. Gate 3 step 3 drives it once indexing reports complete. Gotcha: the index is machine-local
and must settle first, so a run that searches too early measures the queue, not retrieval.

## Graph

The whole-vault node graph. External alpha renders explicit links only; similarity edges and
promote-to-link are beta work in #10. Reached from the graph view. Gate 3 step 3 drives first draw.
Its performance budget keeps `8d290b7` evidence.

## Tasks

The tasks view over checkbox items in notes. Reached from the sidebar. Gate 3 step 3 drives it.

## Trash and restore from trash

Deleting a note moves it to `.helixnotes/trash/` rather than removing it, and it can be restored.
Reached from the note list and the trash view. Gate 3 step 3 drives a restore. Acceptance criterion 5
separately requires a deletion to propagate between machines without resurrecting.

## Note history

Per-note snapshots under `.helixnotes/history/`, pruned to the retention limit. Reached from the
note's history panel. Gate 3 step 3 drives it. Gotcha: retention is a shared vault setting, so two
machines pruning to different limits delete each other's snapshots.

## Backup and whole-vault restore

A whole-vault ZIP, taken on a schedule, before a bulk mutation, and on demand, with a transactional
restore. Reached from Settings, Backup. Gate 3 step 3 drives a backup and a restore. Gate 2 drives
restore interruption at three kill points per machine and requires recovery to the exact pre-restore
or restored tree hash, with the "Restore interrupted" notice and no leftovers. Acceptance criterion 9
requires a forced pre-sync backup on the receiver.

## Web clipping

Pasting a URL fetches the page and stores its readable content as a note. Reached from the Clip web
page action, which calls `clip_web_page`. Quick capture is not a route to it: that path saves the text
it was given through `quick_capture_note` and fetches nothing. Gate 3 step 4 drives a clip of a known
article. Gotcha: one plain fetch in forty stalled past the app's
20 second limit from the Windows test machine, so a reported timeout is retried once and recorded;
any other clip error fails the step.

## File attachments

A local file stored under `.helixnotes/attachments/` and referenced from a note. Reached from the
editor's attach action. Gate 3 step 4 drives it. Acceptance criterion 4 requires an attachment to
arrive on the other machine and open, and criterion 7 requires one that has not arrived yet to read
as not-synced-yet rather than broken. Gotcha: WebDriver cannot drive a native picker, so Fedora
delivers the file through a `DataTransfer` and Windows sends a staged path, because WebView2 ignores
a synthetic file list.

## Notion publish and disconnect

A read-only push of notes to four Notion databases, one per PARA category, and a disconnect that
removes the token. Reached from Settings, Notion. Gate 3 step 5 drives publish with no note failing,
confirms the capture, the clip, and the attachment note arrived through the Notion API, then
disconnects and checks the OS keyring holds the token while connected and not after. Gotcha: the
keyring lookup needs the desktop session on Windows, because an SSH logon has no credential store.

## Diagnostics export

A redacted support archive the user exports from Settings, Maintenance. Gate 3 step 6 drives the
button and then searches the archive for a planted credential, a note body marker, a note title and
path, and the vault path. Gotcha: WebDriver cannot answer the native save dialog, so the step calls
`export_diagnostics` with the path the dialog would return, and the trace records that.

## Operation without the AI backend

Capture, edit, move, and keyword search stay available when the embedding backend is unreachable.
Gate 3 step 7 drives all four against a closed port and requires the edit on disk after exit.

## Damaged configuration at startup

A malformed `config.json` shows the startup error and leaves exactly one damaged copy beside the
config. Gate 3 step 8 drives it and then restores the run's configuration.

## Vault-unavailable toast

When the vault is gone, each hotkey press shows a toast that replaces the previous one rather than
stacking (#153). Gate 3 step 9 drives two presses of Ctrl+Alt+N from the Windows desktop session with
the vault folder renamed away, and requires exactly one "Quick capture" toast in notification history
after each press. The toast goes straight through WinRT under a fixed tag and group, because
`tauri-plugin-notification`'s Windows backend dropped the id and set no tag. Gotcha: Windows only, and
the step renames the vault away first, so it proves the failure path and nothing else. #42 covers
what is left: the display path is proven here, and the error path is reachable and logged but
unverified, because nothing the harness can do makes the WinRT `Show` call fail on a signed-in
session.

## Quick capture

The global hotkey opens the capture overlay from anywhere, the user types and picks a PARA category,
and the note lands in that category. This is the capture path SPEC section 5 is about, and the one
the product exists to make frictionless. **No gate drives it**: Gate 3 step 9 is the only hotkey
drive and it renames the vault away first, so the overlay never opens. Every note a gate creates
interactively comes from the main window's New Note button instead, through the walkthrough's
`createNote`. The clip has its own route, and the fixtures write their notes to disk directly. Driving it needs the hotkey pressed from the
desktop session with the vault present, the overlay reaching focus without the app taking it, a
category chosen, and the note on disk in that category's folder. Linux needs its own route, because
the XDG GlobalShortcuts portal registers the chord differently from Windows.

## Clean shutdown

Exiting through the window's close button leaves no app, sidecar, watchdog, or orphan. Gate 3 closes
the app first at the end of step 2, where it doubles as the durability proof, and again at step 10
after relaunching with the sidecar and watchdog running. Gate 1 step 6 requires the same after a sync
run. The #189 close check covers closing during a sync run.

## Sync between machines

Direct machine-to-machine sync through the bundled Syncthing sidecar, as a Settings toggle. Gate 1
drives an interrupted paired sync: it pairs both installed apps over their loopback REST APIs, lets
the product's five-minute scheduler trigger the batch, stops the Windows sidecar partway, relaunches
both apps, and requires both replicas to converge to exactly 5,000 notes with no conflicts, duplicate
IDs, wrong content, or strays. The acceptance gate drives all eleven #28 criteria, including pairing
that survives relaunch, deletion propagation, a category move that neither duplicates nor loses the
note, and operation with no internet and no Notion account.

## Conflicting edits

An edit on both machines produces a conflict copy, and the app pairs it with its original and offers
the choice. The conflict copy never appears as an ordinary note in search, the graph, or PARA counts.
Acceptance criterion 8 drives it. Gotcha: how the pair is presented, and whether a merge is ever
offered, is still open in SPEC section 11.6.

## External note viewer

A Markdown file outside the vault that the user opened explicitly, shown read-only and never saved,
moved, or deleted by the app. Named in the external-alpha contract and defined in `CONTEXT.md`.
**No gate drives it**: neither gate script mentions it. Driving it needs a file staged outside the
run's vault, the open path WebDriver can reach, and a check that the app never writes to it. The
read-only guarantee is the part worth proving, because breaking it writes outside the vault.

## Obsidian import

A bulk mutation that converts the active vault from Obsidian's conventions to this app's, in place.
The user opens the Obsidian directory as their Second Brain vault, then runs the conversion from
Settings, Import. `import_obsidian` in `commands.rs` reads `active_vault` and
`src-tauri/src/vault/import.rs` rewrites the files it finds there, so nothing is copied in from
outside. The Settings copy says so and tells the user to back up first, because the conversion
changes files in place.
**No gate drives it**: neither gate script calls the import command, and the `.import-result`
selectors in the scripts belong to backup, restore, Notion, and sync. Driving it needs an Obsidian
fixture vault opened as the active vault, the bulk-mutation lease check, a terminal outcome of
success, changed-incomplete, or failure, and a kill point, since an interrupted in-place conversion
leaves a half-converted vault, which is the class of defect Gate 2 exists for.

## Holding area

Notes carrying no category live under the app's metadata folder and are surfaced for the user to
file. Previews are read-only and the only mutating action is filing into a chosen PARA category.
Defined in SPEC section 4. **No gate drives it.** Driving it needs a note placed with no category,
the queue listing it, a refusal of every mutating action except filing, and the file landing in the
chosen category.

## Command palette

The keyboard-driven command surface in `CommandPalette.svelte`, whose scope SPEC now settles.
**No gate drives it.** Driving it needs the open shortcut, one command executed through it, and the
same observable end state the direct route produces.

## Compact layout

The single-panel layout shown while the window is 768 px wide or narrower, on any supported platform,
showing one panel at a time. Defined in `CONTEXT.md`. **No gate drives it, and the harness actively
avoids it**: `alpha-harness.mjs` sets every driven window to 1280 by 860 before waiting, because
WebDriver's default window or a persisted compact one hides the wide-layout ready selector and folds
the editor toolbar away. A one-off release-build drive has already passed it, narrowing to 600 px
inside the 500 ms save delay and widening back to 1200 px with both edits on disk, recorded in
[issue-89-review-fixes-verification-2026-09-28.html](reports/issue-89-review-fixes-verification-2026-09-28.html).
Driving it in a gate needs a deliberate resize below the threshold, a check that each panel is
reachable one at a time, and the wide-layout selector replaced by one the compact layout also shows.

## Second note window

Opening a note in its own window, through `NoteWindow.svelte` and `NoteSwitcher.svelte`. **No gate
drives it**, though #192's live verification covered one case outside the gates: a secondary note
window closed after the keep-or-discard choice, with the lock released and no alert
([ticket-192-verification-2026-10-02.html](reports/ticket-192-verification-2026-10-02.html)).
Driving it needs a second window opened on a note, an edit in it, and the same durability proof Gate 3
step 2 applies to the main window. The recent defects here share rename, save, and reload timing
rather than one cause, so the claim that a second window is the riskiest of the gaps is an inference
from that area's defect rate, not something the evidence settles.
