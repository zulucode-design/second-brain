# Capture-time similarity (#9): verification

Dates: 2026-10-05 to 2026-10-08 (UTC−5); evidence files are named by their UTC date. Candidate:
commit e5ae7af on `feat/9-capture-similarity`, after nine rounds of review. This report's own
commit changes only the report and its evidence. Earlier evidence on b1be541, 4d0844a, fc4d283,
413af12, 18d1bb2 and a38b4e6 is named where it is used, with the reason it still applies. The
design is #9 comment 5998786646.

Sol ran the calibration and the first end-to-end run, at 4d0844a (run root
`~/sb-similar-run/2026-10-05T21-44-20.781Z`, kept on the laptop, not committed). Sol then hit a
usage limit on 2026-10-05, so Nicolas asked Claude to run the builds and checks too. Claude ran
`pnpm verify` and the end-to-end runs from fc4d283 to a38b4e6. Sol's usage was back on 2026-10-07,
and Sol ran every check on the candidate. Nicolas did the hand checks.

## Checks on the candidate

Sol ran `pnpm verify` at e5ae7af on the Fedora 44 laptop (kernel 7.2.8-200.fc44.x86_64). It
passed: svelte-check with no errors or warnings, the Node tests, the Rust tests, rustfmt, clippy
with warnings denied, and the frontend build. `pnpm tauri build --debug --no-bundle` also passed.
The logs are not committed. The counts are their summary lines:

- `ℹ pass 236`, from `node --test tests/*.test.mjs`, the 34 `.test.mjs` files directly under
  `tests/` (`ls tests/*.test.mjs | wc -l`).
- `test result: ok. 675 passed; 0 failed; 10 ignored`, from `pnpm test:rust`, the crate's unit
  tests. The 10 ignored are the live and measurement tests (`grep -rc '#\[ignore' src-tauri/src`
  sums to 10), `similarity_calibration` among them.

These run on Linux only. CI's `windows-rust` job compiles the Windows-only code on the pull
request; it had not run when this report was written.

## The similarity bar

`similarity_calibration` (an ignored Rust test) indexes the 13 notes of
`scripts/similarity-fixture.json` with live `embeddinggemma`. It scores the fixture's 17 one-line
captures (8 duplicates, 6 same-topic, 3 unrelated) against every note, embedding each capture as
the overlay files it. The counts come from the fixture:
`python3 -c "import json;f=json.load(open('scripts/similarity-fixture.json'));c=f['captures'];print(len(f['notes']),len(c),sum(1 for x in c if x.get('duplicateOf')),sum(1 for x in c if not x.get('duplicateOf') and x.get('sameTopic')))"`
prints `13 17 8 6`.
[Output](evidence/9-similarity-calibration-2026-10-05-b1be541.txt), with its conditions in the
header:

- Run by Sol at b1be541 on 2026-10-05, on the Fedora laptop.
- The desktop's usual Ollama on port 11434, reached over an `ssh -L` tunnel.
- `embeddinggemma:latest`, digest `85462619ee72`. The Ollama version was not recorded; it may
  have been 0.35.0 rather than the 0.35.1 of the later runs.

Scoring has not changed since b1be541 (`git diff b1be541 e5ae7af --
src-tauri/src/semantic_search.rs`). The file gained two tests and a `query_input` helper for the
search prompt; search embeds the same text as before. `similar` now returns every note that
passes, each with the revision the index holds for it, and the check drops a match whose note
changed on disk since it was indexed. That decides which notes a card shows, not how a note
scores. `similar_notes.rs` changed its bar, its notification code and that filter.

| Captures | Count | Score against the note |
| --- | --- | --- |
| Reworded duplicates, against the note they repeat | 8 | 0.769 to 0.850 |
| Every other capture–note pair, same-topic notes included | 81 listed | 0.533 at most (rye sourdough against "Sourdough hydration notes") |

The output lists only pairs at or above the index's search floor, `MIN_SEMANTIC_SCORE` (0.22). The
other 132 of the 221 pairs (17 × 13) scored below it. `grep -cE '^  0\.[0-9]{3} '` on the evidence
file counts the 89 listed pairs, 8 of them duplicates.

The bar is 0.65, midway between the two groups. The first calibrated bar, 0.62 (e982b6f), was the
midpoint against an earlier run at 693603c that reported 0.714 for the Atomic Habits duplicate.
That run's output survives on the laptop at `~/sb-sol/i9/calibration.txt` but is not committed.
It was wrong: the test wrote titles containing `: ` unquoted, so two book notes were indexed under
their file names. Sol caught it, e982b6f quotes titles, and the rerun above gave 0.777 for that
pair. The SPEC and the code kept 0.714 until the final review found it, and 413af12 moves the bar
to the new midpoint.

## End-to-end run

`node scripts/similar-run.mjs --app src-tauri/target/debug/second-brain`, by Sol at e5ae7af on
the Fedora laptop, at 17:47 UTC−5 on 2026-10-07.

- **Build and setup.** A debug build of e5ae7af under `tauri-driver`, with a fresh profile and a
  vault holding the fixture's notes. Run root, kept on the laptop:
  `~/sb-similar-run/2026-10-07T22-47-25.128Z`.
- **Backend.** The desktop's usual Ollama on port 11434, tunnelled in. It answered 0.35.1 just
  before the run (the trace does not record it). `embeddinggemma:latest`, digest `85462619ee72`.
- **Method.** Each capture goes through the command the overlay uses (WebDriver cannot press a
  global hotkey), and the card it raises in the main window is read back. Where no card is due,
  the run waits 5 seconds and confirms none appeared.

[Trace](evidence/9-similar-run-2026-10-07-e5ae7af.json). The trace records which notes each card
showed, not their scores. So the run shows that every duplicate still passes 0.65 under these
conditions, not how far above it each one scored.

| Check | Result |
| --- | --- |
| 8 duplicate captures | Each card named the note it repeats, with its matching passage |
| 6 same-topic and 3 unrelated captures | No card |
| Capture time | The capture command returned in 35 to 94 ms (median 43) |
| Append | The note kept its text and gained the marker and the capture. The capture moved to trash. The note's history held its text from before the append, although the run first wrote a history snapshot a moment old, the case where a save's own snapshot is skipped |
| Dismiss | Both files byte-for-byte unchanged |
| Append after the note was edited on disk | Refused: "One of the notes changed since the check, so nothing was added." Both files unchanged |
| Append after the note was moved on disk | Refused: "That note is no longer there, so nothing was added." Both files unchanged |
| Open | The similar note ("Book notes: Atomic Habits") opened in the editor, and the card went |
| A capture with a body under its title line | Its card named the note it repeats and also the earlier one-line capture of the same text, which was still in the vault: both are near-duplicates of it |
| Notification, main window focused | None sent for the 17 captures above |
| Notification, main window minimized | Two duplicate captures sent two `Notify` calls, read off the session bus with `dbus-monitor` with the service's replies. The first, "Similar note found for your capture", got id 24. The second, "2 captures have similar notes", named 24 as the notification it replaces. Both bodies read "Open Second Brain to review." and name no note |
| Capture with the tunnel closed | Saved, with no card |

**Capture time.** It is an upper bound. Each figure is a WebDriver round trip around the capture
command, with `dbus-monitor` running. What came before a capture changes it:

- The 8 captures made right after a card appeared took 38 to 94 ms (median 61.5).
- The 8 made after a 5-second wait for no card took 35 to 47 ms (median 42).
- The first capture, after the index finished, took 41 ms.

The overlay closes when the command returns, so it never waits for the card. That does not show
capture is as fast as before #9, because there is no baseline run without the check. That part
rests on the code: the capture command starts the check on a background thread after the note is
saved, and returns.

The figures come from the trace, where each result follows the one before it:
`python3 -c "import json,statistics as s; r=json.load(open('<trace>'))['results']; print([(r[i]['captureMs'], bool(r[i-1]['shown']) if i else None) for i in range(len(r))])"`
lists each capture time with whether a card preceded it.

**Notification.** The replacement is shown on the bus: the second call carries the first call's
id. Nobody looked at the screen during the run, so how GNOME displayed the pair is not recorded.

**Coverage line.** No fixture capture passes the bar for more than one note in the main loop. The
"Showing 3 of N" line is covered only by `tests/similar.test.mjs` and the Rust test of the cap.
Notes created in the app, checked when the user first leaves them, are covered by the same unit
tests and were not run end to end.

**Earlier runs.** These versions of this run passed too:

- fc4d283, before the review fixes, with the 0.62 bar and no capture-time, undo, or notification
  checks. [Trace](evidence/9-similar-run-2026-10-06-fc4d283.json). On 2026-10-06 its `app` field
  was cut to the file name and its `path` fields to vault-relative paths, so it names no home
  directory. Nothing else in it changed.
- 413af12, after the first round. [Trace](evidence/9-similar-run-2026-10-06-413af12.json). Its
  undo check could not fail: the target had no earlier snapshot, so a save's own snapshot already
  kept the text. Its notification check did not read the service's replies.
- 18d1bb2, after the second round. [Trace](evidence/9-similar-run-2026-10-07-18d1bb2.json). It
  had no Open or title-and-body checks.
- a38b4e6, after the third round, run by Claude.
  [Trace](evidence/9-similar-run-2026-10-07-a38b4e6.json).
- 0cb920c and f83a489, after the sixth and seventh rounds, run by Sol. Their run roots,
  `~/sb-similar-run/2026-10-07T18-32-11.708Z` and `~/sb-similar-run/2026-10-07T22-27-43.418Z`,
  are kept on the laptop and not committed.

## Ask's run script after the refactor

`scripts/similar-run.mjs` reuses `scripts/ask-run.mjs`'s setup, so this branch moved that setup
into exported helpers. It also renamed the trace's `error` field to `failure` (4d0844a), because
WebdriverIO treats a returned object's `error` field as a failed command. `src-tauri/src/ask.rs`
is unchanged on the branch (`git diff 47bb85e e5ae7af -- src-tauri/src/ask.rs` is empty).

To check that #8's script still works, Sol ran
`node scripts/ask-run.mjs --app src-tauri/target/debug/second-brain` on the e5ae7af build, against
the desktop's `gpt-oss:20b-cloud` (digest `9a01793d9ef8`). All six requests completed, and Sol
judged all six answers acceptable against #8's fixture: expected facts present, expected notes
cited, the cold-brew answer free of the injected link and image. The run root,
`~/sb-ask-run/2026-10-07T22-48-57.003Z`, is kept on the laptop and not committed.

The same script on earlier builds of this branch completed every request too, but the model's
answers varied:

- a38b4e6: the sourdough answer used plain-text source references, the shed answer omitted
  citations to two expected notes, and cold brew returned a refusal.
- 0cb920c: the shed answer left one expected note uncited.
- f83a489: the shed answer called the shed "80 % complete", which no note says, and the sourdough
  answer was a refusal.

These are model outcomes, not script failures. Sol assessed the Ask answers at 0cb920c and
f83a489 and recorded failures; those #8 findings remain separate from #9. Overstated and uncited
shed answers are #210.

## Desktop notification, by hand

A script cannot click a desktop notification, so Nicolas checked both on installed builds:
capture a duplicate while another app had focus, then click the notification.

| Machine | Build | Notification | Click | Card |
| --- | --- | --- | --- | --- |
| Windows 11, `D:\SecondBrainTest\app` (NSIS, per user) | 4d0844a | Shown | Raised the main window | Shown |
| Fedora 44, GNOME 50 (Wayland), RPM through the test package helper | fc4d283 (installed 07:10 −05:00, 4 minutes after the commit) | Shown | Raised the main window | Shown |

The passes rest on what Nicolas saw; no recording or log of them was kept. Each build is tied to
its commit as follows:

- **Windows.** The install came from the checkout `D:\SecondBrainTest\src9`, which is at 4d0844a
  with only a local `Cargo.toml` edit (`git -C D:\SecondBrainTest\src9 rev-parse --short HEAD`).
  Its `second-brain.exe` is dated 16:53 on 2026-10-05, after 4d0844a (16:43) and before the next
  commit (19:41).
- **Fedora.** The RPM was installed at 07:10:02 on 2026-10-06 (`journalctl -t sudo`, the
  helper's `install-candidate` line). That is after fc4d283 (07:05:55) and before any later
  commit. The installed `/usr/bin/second-brain` holds the strings `org.freedesktop.Notifications`
  and `ActivationToken`, which 11126ec introduced, and lacks the message "Could not keep a copy of
  the note", which 413af12 introduced (`grep -a -c -F '<string>' /usr/bin/second-brain`).

Neither hand check was repeated at e5ae7af. No installed package of e5ae7af exists, so everything
at e5ae7af stops at the debug build.

- **Windows.** Since 4d0844a, these changes reach the Windows notification:
  - The click handler passes no activation token to the shared window-raising function, which
    ignores it off Linux, and the wording moved into helpers.
  - Toasts go out one at a time. When its turn comes, a toast is skipped if the main window has
    focus or the vault the check ran in is no longer open.
  - Shared by both systems, a capture no longer notifies when the main window had focus as the
    capture overlay opened.

  The hand check had another app in focus and one vault, the case that still sends. So the toast
  and its click at e5ae7af are argued from that diff, not run. The Windows build has not been
  compiled locally since 4d0844a, and the card's frontend has changed since then, so its look on
  Windows (WebView2) at e5ae7af is not checked either.
- **Linux.** Since fc4d283, the app sends one notification at a time. It connects to the
  notification service first, then skips the send if the main window has focus or the vault
  changed. The click listener starts after the first notification is out, not before, and a
  listener that fails lets the next notification start it again. The focus guard above applies
  here too. The listener's handling of a click is unchanged from fc4d283: it matches the
  notification's id, takes the `ActivationToken`, and raises the window. The end-to-end run shows
  the sending and replacement at e5ae7af; the click is argued, not run.

Reinstalling e5ae7af (or the merged commit) on both machines and clicking a notification would
settle both.

Fedora took five installed builds to pass (`journalctl -t sudo -S 2026-10-05 -U '2026-10-06 08:00' |
grep -c install-candidate` prints 5). Each failure, and its fix:

1. **Portal click without a token (4d0844a).** Notifications went through the notification
   portal. A click raised GNOME's "… is ready" notice instead of the window: on Wayland, raising a
   window needs an activation token, and GNOME 50's portal delivered the click with an empty
   parameter list (seen with `dbus-monitor`; the capture was not kept).
2. **Token passed, still none (6e3f62e).** The click handler handed the portal's token to GTK, but
   there was none to hand over.
3. **Notifications closed within milliseconds (11126ec).** Notifications moved to
   `org.freedesktop.Notifications`, which sends `ActivationToken` before `ActionInvoked`. GNOME
   closed each one 13 to 25 ms after it was sent (read off `dbus-monitor` timestamps; the capture
   was not kept). GNOME Shell's `notificationDaemon.js` (`_onNameVanished`) withdraws an app's
   notifications when the D-Bus connection that sent them closes, and the code opened a connection
   per notification.
4. **Same failure (ec1d8a0).** This build guessed the capture overlay was taking focus and waited
   for it to close before notifying. Notifications still closed at once, and 5a7dc50 reverted it.
5. **Pass (fc4d283).** One connection for the app's lifetime, shared with the click listener.

The #9 design comment names `notify-rust` and `tauri-winrt-notification`. The build uses neither:

- Windows reuses the `windows` crate toast code already in `hotkey/windows.rs`.
- Linux uses the `zbus` that `ashpd` already brings in, plus `gtk` to hand the activation token to
  the window. `gtk` was already in the tree through tauri and is now a direct dependency.

The behaviour is the one decided, with one exception: under Do Not Disturb the app's send
succeeds, so it cannot tell and logs nothing. The SPEC records this. The decisions also say the
capture is "embedded directly as a query". The build embeds it as a document, the way notes are
indexed, so a near-copy of a note scores close to 1. The SPEC records this, and the calibration
used the same path.

Two things about the test machines, not the app:

- The Fedora laptop had GNOME's Do Not Disturb on for the first checks, which sends
  notifications to the list without a banner.
- The desktop's Ollama alternated between Nicolas's usual server on port 11434 and a second
  Ollama server on 11435, kept for another project. Several hand checks skipped silently with
  the backend unreachable, as designed.

## What the reviews changed

Review rounds 5 to 8 found races and edge cases in code the end-to-end run cannot reach. The
fixes, each held by a unit test or argued from the code:

- **An open note during an append.** The open capture or similar note is saved and closed first,
  then reopened inside the same queued run, unless it went to trash. A note the user picked
  meanwhile opens after it (`tests/similar.test.mjs`).
- **Changes during an append.** One lock covers the revision checks, the history snapshot, the
  save and the move to trash. A capture changed on disk while the append ran stays where it is,
  with a message (`a_note_changed_since_it_was_read_is_not_moved_to_trash`). So does a capture
  that cannot move to trash
  (`an_append_whose_capture_cannot_move_to_trash_keeps_both_and_says_why`).
- **The undo point.** A forced history snapshot takes a name after the newest one, so it never
  overwrites an earlier snapshot and pruning never removes it
  (`forced_snapshots_in_one_second_keep_the_newest_text`).
- **A stale index.** A match whose note changed on disk since it was indexed is dropped before
  the card counts and caps its matches
  (`the_similarity_check_skips_short_notes_drops_gone_or_changed_matches_and_caps_the_card`).
- **A vault switch.** A switch clears the cards from the vault left, which
  `tests/similar.test.mjs` covers. A check that finishes after the switch shows no card and sends
  no notification. That part is argued from the code, with no test.

## After the merge (2026-10-08)

#9 merged as b46e718. Nicolas asked for the gaps listed under Not proven to be tested before the
next ticket. Branch `test/9-post-merge-checks` adds checks to `scripts/similar-run.mjs` and changes
no app code: `git diff --stat b46e718 b61a6fa -- src src-tauri` prints nothing.

**End-to-end run.** Sol built `pnpm tauri build --debug --no-bundle` at b61a6fa and ran
`node scripts/similar-run.mjs --app src-tauri/target/debug/second-brain` on the Fedora laptop, at
06:58 UTC−5 on 2026-10-08. Just before it, Sol read the desktop's Ollama as 0.35.1 and
`embeddinggemma:latest` as digest `85462619ee72` (`/api/version`, `/api/tags`); the trace does not
record them. Run root, kept on the laptop: `~/sb-similar-run/2026-10-08T11-58-10.393Z`.
[Trace](evidence/9-similar-run-2026-10-08-b61a6fa.json). The original checks passed as at e5ae7af:
8 of 8 duplicates, no wrong cards, every card action, and the replaced notification pair (ids 63
and 63). Capture took 36 to 120 ms (median 39); 39 to 120 ms after a card (median 81.5), 36 to
43 ms after a wait (median 37), 41 ms for the first, by the command under End-to-end run. The new
checks:

| Check | Result |
| --- | --- |
| Append with the capture open in the editor | No failure. The capture moved to trash, the similar note gained it, and the editor was left empty rather than on the trashed capture |
| Append with the similar note open | No failure. The capture moved to trash, and the note reopened in the editor showing the appended capture |
| A capture just before a vault switch | A relay held every request to the backend until released. Control: a capture whose embedding was held for 3 s showed no card while held, then its card, and sent 1 notification with the main window minimized. Then a second capture: once a request carrying its text reached the relay and was held, the vault switched, finishing in 0.029 s with the request still held. The request was then released and the backend answered. The app logged one `Skipped the similarity check for a new note`, a failed check, in the same second, and over the next 10 s no card and no notification appeared |

The run switches vaults through `open_vault`, the command the vault picker calls, and leaves the
page on the old vault. The picker's native folder dialog cannot be driven, and a page reload
waited on startup that had already happened. With the page on the old vault, its own vault check
would pass a late card, so a card or a notification there could only come from the backend.

What this shows: a check for a capture made just before a switch failed, and a failed check is
dropped quietly. It does not show when the check failed. The check and the indexer both embed the
capture's text, so the held request may have been the indexer's, and the check may have started
after the switch and failed reading the capture under the new vault. The log's one-second
resolution cannot order the two. Nor does it reach the vault check that drops a check finishing
normally after a switch (`similar_notes.rs`, `vault_still_open`); that stays argued from the
code. The picker path, which clears cards from the vault left, has a unit test only.

An earlier version of this check (0af164f) delayed only new connections. The app reused an open
one, so its late check was not held, and the review caught it before this report was merged.

**Installed builds.** Sol built both packages at daa43f9, the same app code as b46e718:

- Fedora 44, GNOME 50 (Wayland): the RPM, installed through the test package helper at 20:09
  UTC−5 on 2026-10-07. Embeddings from the desktop's Ollama over an `ssh -L` tunnel on port 11437.
- Windows 11: the NSIS installer, built on the desktop in `D:\SecondBrainTest\src9` with no
  compile errors, the first local Windows compile since 4d0844a, and installed per user to
  `D:\SecondBrainTest\app`. Embeddings from the desktop's own Ollama.

Each machine got a fresh vault of the 13 fixture notes, with `close_to_tray` on.

**Hand checks.** On 2026-10-08 Nicolas captured a duplicate with Ctrl+Alt+N three times on each
machine:

| Case | Fedora | Windows |
| --- | --- | --- |
| Another app focused | Notification shown; its click raised the main window with the card | Same |
| Main window focused | Card shown, no notification | Same |
| Main window closed to the tray | Notification shown; its click restored the window with the card | Same |

The passes rest on what Nicolas saw and on screenshots shared in the conversation, not committed.
The first Fedora attempt showed nothing: the tunnel had stopped forwarding overnight while its
process lived on, so the app logged `Skipped the similarity check for a new note` for each
capture, as designed for an unreachable backend. After the tunnel was restarted, all three cases
passed. A check skipped that way is never run later; #216 covers that.

## Not proven

The list as written for e5ae7af. Items settled after the merge are marked; the rest still hold.

- The notification and its click at e5ae7af on Windows, and the click on Fedora (argued above).
  The Windows code at e5ae7af is compiled only by CI, and the card was not seen on Windows.
  **Settled 2026-10-08:** compiled on Windows and clicked on both machines at daa43f9.
- Any installed package of e5ae7af: every result at e5ae7af comes from a debug build. **Settled
  2026-10-08** for the hand checks above, on packages of daa43f9.
- The focus guard: no notification when the main window had focus as the overlay opened. It needs
  the hotkey, which WebDriver cannot press. **Settled 2026-10-08** by hand on both machines.
- A hotkey capture while the main window is hidden to the tray. The run minimized it instead.
  **Settled 2026-10-08** by hand on both machines.
- Appending while the capture or the similar note is open in the editor, which closes and reopens
  it. The run appended with neither open; the order of steps has a unit test. **Settled
  2026-10-08** by the b61a6fa run, for one append each way. A note picked during an append still
  has the unit test only.
- Restoring a note from history after an append. The run checks only that the snapshot exists.
- Open on a note that cannot be opened: handled in code, not run. The capture that cannot move to
  trash has a Rust test, not an end-to-end run.
- Capture speed compared with a build without the check: there is no baseline run.
- The in-app trigger (a note created in the app, checked on first leave), the "Showing 3 of N"
  line, and clearing cards on a vault switch through the picker, end to end. Each has unit tests
  only.
- A check that finishes after a vault switch, dropped before its card and its notification:
  argued from the code, with no test. **Partly settled 2026-10-08** by the b61a6fa run: a check
  for a capture made just before a switch through `open_vault` failed and left no card and no
  notification. Whether the check itself was waiting on the backend at the switch is not shown,
  and a check that finishes normally after a switch, the case the vault check drops, is still
  argued from the code.
- Calibration on notes with a separate title and body. Every fixture capture is one line, so its
  body is empty (#213). The run's one capture with a body matched as expected, which is a single
  case, not a calibration.
- The Do Not Disturb path: the card waits as designed, and nothing is logged. This was seen by
  hand, not tested.

## Seen, not part of #9

- **One-line quick captures become all title** (#213): the first line of a capture is its title,
  so a one-line thought leaves the body empty. Nicolas wants it resolved before #10.
- GNOME labels the main window "Tauri App" (its title bar and the "… is ready" notice).
