# Capture-time similarity (#9): verification

Dates: 2026-10-05 to 2026-10-07. Candidate: commit 18d1bb2 on `feat/9-capture-similarity`, after
two rounds of final review. Its app code is that of dbde52d; 18d1bb2 changes only
`scripts/similar-run.mjs`. Earlier evidence on b1be541, 4d0844a, fc4d283, and 413af12 is named
where it is used, with the reason it still applies. The design is #9 comment 5998786646.

Sol ran the calibration and the first end-to-end runs. Sol reached a usage limit on 2026-10-05
that lasts until 2026-10-09. Nicolas then asked Claude to run the builds and checks as well, so
Claude ran `pnpm verify` and the end-to-end runs on fc4d283, 413af12, and 18d1bb2. Nicolas did the
hand checks.

## Checks on the candidate

`pnpm verify` at 18d1bb2 on the Fedora 44 laptop (kernel 7.2.8-200.fc44.x86_64) passed:
svelte-check with no errors or warnings, the Node tests, the Rust tests, rustfmt, clippy with
warnings denied, and the frontend build. The log is not committed. The counts are its summary
lines:

- `ℹ pass 235`, from `node --test tests/*.test.mjs`, the 34 `.test.mjs` files directly under
  `tests/`.
- `test result: ok. 671 passed; 0 failed; 10 ignored`, from `pnpm test:rust`, the crate's unit
  tests. The 10 ignored are the live and measurement tests (`grep -rc '#\[ignore' src-tauri/src`
  sums to 10), `similarity_calibration` among them.

These run on Linux only. The Windows-only code is compiled by CI's `windows-rust` job on the pull
request, which had not run when this report was written.

## The similarity bar

`similarity_calibration` (an ignored Rust test) indexes the 13 notes of
`scripts/similarity-fixture.json` with live `embeddinggemma`. It scores the fixture's 17 one-line
captures against every note, embedding each capture as the overlay files it.
[Output](evidence/9-similarity-calibration-2026-10-05-b1be541.txt), with its conditions in the
header:

- Run by Sol at b1be541 on 2026-10-05, on the Fedora laptop.
- The desktop's usual Ollama on port 11434, reached over an `ssh -L` tunnel.
- `embeddinggemma:latest`, digest `85462619ee72`. The Ollama version was not recorded; it may
  have been 0.35.0 rather than the 0.35.1 of the later runs.

Scoring has not changed since b1be541. Between b1be541 and 18d1bb2, `semantic_search.rs` gained
two tests and moved the search prompt into a `query_input` helper, so search embeds the same text
as before (`git diff b1be541 18d1bb2 -- src-tauri/src/semantic_search.rs`). `similar_notes.rs`
changed its bar and its notification code, not how a note is scored.

| Captures | Count | Score against the note |
| --- | --- | --- |
| Reworded duplicates, against the note they repeat | 8 | 0.769 to 0.850 |
| Every other capture–note pair, same-topic notes included | 81 listed | 0.533 at most (rye sourdough against "Sourdough hydration notes") |

The output lists only pairs at or above the index's search floor, `MIN_SEMANTIC_SCORE` (0.22). The
other 132 of the 221 pairs (17 × 13) scored below it. `grep -cE '^  0\.[0-9]{3} '` on the evidence
file counts the 89 listed pairs, 8 of them duplicates.

The bar is 0.65, midway between the two groups. The first calibrated bar, 0.62 (e982b6f), was the
midpoint against an earlier run at 693603c that reported 0.714 for the Atomic Habits duplicate.
That run's output was not kept, and it was wrong: the test wrote titles containing `: ` unquoted,
so two book notes were indexed under their file names. Sol caught it, e982b6f quotes titles, and
the rerun above gave 0.777 for that pair. The SPEC and the code kept 0.714 until the final review
found it, and 413af12 moves the bar to the new midpoint.

## End-to-end run

`node scripts/similar-run.mjs --app src-tauri/target/debug/second-brain`, by Claude at 18d1bb2 on
the Fedora laptop, on 2026-10-07 at 02:35 UTC.

- **Build and setup.** A debug build of 18d1bb2 under `tauri-driver`, with a fresh profile and a
  vault holding the fixture's notes. Run root, kept on the laptop:
  `~/sb-similar-run/2026-10-07T02-35-37.093Z`.
- **Backend.** The desktop's usual Ollama 0.35.1 on port 11434, tunnelled in.
  `embeddinggemma:latest`, digest `85462619ee72`.
- **Method.** Each capture goes through the command the overlay uses (WebDriver cannot press a
  global hotkey), and the card it raises in the main window is read back. Where no card is due,
  the run waits 5 seconds and confirms none appeared.

[Trace](evidence/9-similar-run-2026-10-07-18d1bb2.json). The trace records which notes each card
showed, not their scores. So the run shows that every duplicate still passes 0.65 under these
conditions, not how far above it each one scored.

| Check | Result |
| --- | --- |
| 8 duplicate captures | Each card named the note it repeats, with its matching passage, within a second of the capture |
| 6 same-topic and 3 unrelated captures | No card |
| Capture time | The capture command returned in 36 to 89 ms (median 42) |
| Append | The note kept its text and gained the marker and the capture. The capture moved to trash. The note's history held its text from before the append, although the run first wrote a history snapshot a moment old, the case where a save's own snapshot is skipped |
| Dismiss | Both files byte-for-byte unchanged |
| Append after the note was edited on disk | Refused: "One of the notes changed since the check, so nothing was added." Both files unchanged |
| Append after the note was moved on disk | Refused: "That note is no longer there, so nothing was added." Both files unchanged |
| Notification, main window focused | None sent for the 17 captures above |
| Notification, main window minimized | Two duplicate captures sent two `Notify` calls, read off the session bus with `dbus-monitor` with the service's replies. The first, "Similar note found for your capture", got id 85. The second, "2 captures have similar notes", named 85 as the notification it replaces. Both bodies read "Open Second Brain to review." and name no note |
| Capture with the tunnel closed | Saved, with no card |

**Capture time.** It is an upper bound. Each figure is a WebDriver round trip around the capture
command, with `dbus-monitor` running. The 8 duplicates, captured right after the previous card
appeared, took 38 to 89 ms (median 52). The other 9, captured after a 5-second wait, took 36 to
77 ms (median 41). The overlay closes when the command returns, so it never waits for the card.
That does not show capture is as fast as before #9, because there is no baseline run without the
check. That part rests on the code: the capture command starts the check on a background thread
after the note is saved, and returns.

The figures come from the trace:
`python3 -c "import json,statistics as s; r=json.load(open('<trace>'))['results']; c=[x['captureMs'] for x in r]; print(min(c), max(c), s.median(c))"`.
Add `if x['kind']=='duplicate'` to the list comprehension, or negate it, for the two groups.

**Notification.** The replacement is shown on the bus: the second call carries the first call's
id. Nobody looked at the screen during the run, so how GNOME displayed the pair is not recorded.

**Coverage line.** No fixture capture passes the bar for more than one note. The "Showing 3 of N"
line is covered only by `tests/similar.test.mjs` and the Rust test of the cap. Notes created in
the app, checked when the user first leaves them, are covered by the same unit tests and were not
run end to end.

**Earlier runs.** Two earlier versions of this run passed:

- fc4d283, before the review fixes, with the 0.62 bar and no capture-time, undo, or notification
  checks. [Trace](evidence/9-similar-run-2026-10-06-fc4d283.json). On 2026-10-06 its `app` field
  was cut to the file name and its `path` fields to vault-relative paths, so it names no home
  directory. Nothing else in it changed.
- 413af12, after the first round. [Trace](evidence/9-similar-run-2026-10-06-413af12.json). Its
  undo check could not fail: the target had no earlier snapshot, so a save's own snapshot already
  kept the text. Its notification check did not read the service's replies.

## Ask's run script after the refactor

`scripts/similar-run.mjs` reuses `scripts/ask-run.mjs`'s setup, so this branch moved that setup
into exported helpers. It also renamed the trace's `error` field to `failure` (4d0844a), because
WebdriverIO treats a returned object's `error` field as a failed command.

To check that #8's script still works, Claude ran
`node scripts/ask-run.mjs --app src-tauri/target/debug/second-brain` on the 413af12 build, against
the desktop's `gpt-oss:20b-cloud`. `scripts/ask-run.mjs` has not changed since 413af12. All six
questions were answered with no failure, and every note each question expects was cited. The run
checked the script, so its answers were not judged again for #8. Its run root,
`~/sb-ask-run/2026-10-06T22-06-13.297Z`, is kept on the laptop and not committed.

## Desktop notification, by hand

A script cannot click a desktop notification, so Nicolas checked both on installed builds:
capture a duplicate while another app had focus, then click the notification.

| Machine | Build | Notification | Click | Card |
| --- | --- | --- | --- | --- |
| Windows 11, `D:\SecondBrainTest\app` (NSIS, per user) | 4d0844a | Shown | Raised the main window | Shown |
| Fedora 44, GNOME 50 (Wayland), RPM through the test package helper | fc4d283 (installed 07:10 −05:00, 4 minutes after the commit) | Shown | Raised the main window | Shown |

Each build is tied to its commit by time only:

- The Windows `second-brain.exe` is dated 16:53 on 2026-10-05, after 4d0844a (16:43) and before
  the next commit (19:41).
- The Fedora RPM was installed at 07:10:02 on 2026-10-06 (`journalctl -t sudo`, the helper's
  `install-candidate` line). That is after fc4d283 (07:05:55) and before any later commit.

Neither hand check was repeated at 18d1bb2. No installed package of 18d1bb2 exists, so everything
at 18d1bb2 stops at the debug build.

- **Windows.** Two changes since 4d0844a reach the Windows notification:
  - In the notification code, the click handler now passes no activation token to the shared
    window-raising function, which ignores it off Linux, and the wording function was renamed.
  - Shared by both systems, a capture no longer notifies when the main window had focus as the
    capture overlay opened. The hand check had another app in focus, which is the case that
    still notifies.

  So the toast and its click at 18d1bb2 are argued from that diff, not run, and the Windows build
  has not been compiled locally since 4d0844a.
- **Linux.** Since fc4d283 the app sends one notification at a time, starts the click listener
  after the first notification is out rather than before, and has the same focus guard. The
  listener's handling of a click is the code that passed at fc4d283: it matches the
  notification's id, takes the `ActivationToken`, and raises the window. The end-to-end run shows
  the sending and replacement at 18d1bb2; the click is argued, not run.

Reinstalling 18d1bb2 (or the merged commit) on both machines and clicking a notification would
settle both.

Fedora took five installed builds to pass. Each failure, and its fix:

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
succeeds, so it cannot tell and logs nothing. The decisions also say the capture is "embedded
directly as a query". The build embeds it as a document, the way notes are indexed, so a
near-copy of a note scores close to 1. The SPEC records this, and the calibration used the same
path.

Two things about the test machines, not the app:

- The Fedora laptop had GNOME's Do Not Disturb on for the first checks, which sends
  notifications to the list without a banner.
- The desktop's Ollama alternated between Nicolas's usual server on port 11434 and a second
  Ollama server on 11435, kept for another project. Several hand checks skipped silently with
  the backend unreachable, as designed.

## Not proven

- The notification and its click at 18d1bb2 on Windows, and the click on Fedora (argued above).
  The Windows code at 18d1bb2 is compiled only by CI.
- Any installed package of 18d1bb2: every result at 18d1bb2 comes from a debug build.
- The focus guard: no notification when the main window had focus as the overlay opened. It needs
  the hotkey, which WebDriver cannot press.
- Restoring a note from history after an append. The run checks only that the snapshot exists.
- An append whose capture cannot then be moved to trash, and Open on a note that cannot be opened.
  Both are handled in code and not run.
- Capture speed compared with a build without the check: there is no baseline run.
- The in-app trigger (a note created in the app, checked on first leave), the "Showing 3 of N"
  line, and clearing cards on a vault switch, end to end. Each has unit tests only.
- Calibration on notes with a separate title and body. Every fixture capture is one line, so its
  body is empty (#213).
- The Do Not Disturb path: the card waits as designed, and nothing is logged. This was seen by
  hand, not tested.

## Seen, not part of #9

- **One-line quick captures become all title** (#213): the first line of a capture is its title,
  so a one-line thought leaves the body empty. Nicolas wants it resolved before #10.
- GNOME labels the main window "Tauri App" (its title bar and the "… is ready" notice).
