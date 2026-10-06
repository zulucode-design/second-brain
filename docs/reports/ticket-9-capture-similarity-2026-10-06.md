# Capture-time similarity (#9): verification

Dates: 2026-10-05 and 2026-10-06. Candidate: commit 413af12 on `feat/9-capture-similarity`, the
fixes from the branch's final review. Earlier evidence on b1be541, 4d0844a, and fc4d283 is named
where it is used, with the reason it still applies. The design is #9 comment 5998786646.

Sol ran the calibration and the first end-to-end runs. Sol reached a usage limit on 2026-10-05
that lasts until 2026-10-09, and Nicolas then had Claude run the builds and checks as well, so
Claude ran `pnpm verify` and the end-to-end runs on fc4d283 and 413af12. Nicolas did the hand
checks.

## Checks on the candidate

`pnpm verify` at 413af12 on the Fedora 44 laptop (kernel 7.2.8-200.fc44.x86_64) passed:
svelte-check with no errors or warnings, the Node tests, the Rust tests, rustfmt, clippy with
warnings denied, and the frontend build. The counts come from its output: `ℹ pass 235` from
`node --test tests/*.test.mjs` (every file under `tests/`), and `test result: ok. 671 passed; 0
failed; 10 ignored` from `pnpm test:rust` (the crate's unit tests; the 10 ignored are the live
and measurement tests, `similarity_calibration` among them).

## The similarity bar

`similarity_calibration` (an ignored Rust test) indexes the 13 notes of
`scripts/similarity-fixture.json` with live `embeddinggemma` and scores its 17 one-line captures
against every note, embedding each capture as the overlay files it.
[Output](evidence/9-similarity-calibration-2026-10-05-b1be541.txt), with its conditions in the
header:

- Run by Sol at b1be541 on 2026-10-05, on the Fedora laptop.
- The desktop's usual Ollama on port 11434, reached over an `ssh -L` tunnel.
- `embeddinggemma:latest`, digest `85462619ee72`. The Ollama version was not recorded.

Scoring has not changed since b1be541. `semantic_search.rs` gained only a test between b1be541
and 413af12 (`git diff b1be541 413af12 -- src-tauri/src/semantic_search.rs` touches only
`mod tests`), and `similar_notes.rs` changed its bar and its notification code, not how a note is
scored.

| Captures | Count | Score against the note |
| --- | --- | --- |
| Reworded duplicates, against the note they repeat | 8 | 0.769 to 0.850 |
| Every other capture–note pair, same-topic notes included | 81 listed | 0.533 at most (rye sourdough against "Sourdough hydration notes") |

The output lists only pairs at or above the index's search floor, `MIN_SEMANTIC_SCORE` (0.22); the
other 132 of the 221 pairs scored below it. The listed counts come from the evidence file: 89
lines of the form `  0.nnn <title>`, less the 8 duplicates.

The bar is 0.65, midway between the two groups. The first version of the branch set 0.62, the
midpoint against an earlier run at 693603c that reported 0.714 for the Atomic Habits duplicate.
That run was wrong: the test wrote titles containing `: ` unquoted, so two book notes were
indexed under their file names. Sol caught it, e982b6f quotes titles, and the rerun above gave
0.777 for that pair. The SPEC and the code kept 0.714 until the final review found it, and
413af12 moves the bar to the new midpoint.

## End-to-end run

`node scripts/similar-run.mjs --app src-tauri/target/debug/second-brain`, by Claude at 413af12 on
the Fedora laptop.

- **Build and setup.** A debug build under `tauri-driver`, with a fresh profile and a vault
  holding the fixture's notes.
- **Backend.** The desktop's usual Ollama, tunnelled in from port 11434. Ollama 0.35.1,
  `embeddinggemma:latest` digest `85462619ee72`.
- **Method.** Each capture goes through the command the overlay uses (WebDriver cannot press a
  global hotkey), and the card it raises in the main window is read back. Where no card is due,
  the run waits 5 seconds and confirms none appeared.

[Trace](evidence/9-similar-run-2026-10-06-413af12.json).

| Check | Result |
| --- | --- |
| 8 duplicate captures | Each card named the note it repeats, with its matching passage, within a second of the capture |
| 6 same-topic and 3 unrelated captures | No card |
| Capture time | The capture command returned in 33 to 85 ms (median 42), which is all the overlay waits for |
| Append | The note kept its text and gained the marker and the capture. The capture moved to trash, and the note's history held its text from before the append |
| Dismiss | Both files byte-for-byte unchanged |
| Append after the note was edited on disk | Refused: "One of the notes changed since the check, so nothing was added." Both files unchanged |
| Notification, main window focused | None sent for the 17 captures above |
| Notification, main window minimized | Two duplicate captures sent two `Notify` calls, read off the session bus with `dbus-monitor`. The first was "Similar note found for your capture". The second replaced it as "2 captures have similar notes". Both read "Open Second Brain to review." and name no note |
| Capture with the tunnel closed | Saved, with no card |

The capture time shows the overlay does not wait for a card. It does not show that capture is as
fast as before #9, because there is no baseline run without the check. That rests on the code:
the capture command starts the check on a background thread after the note is saved, and
returns.

No fixture capture passes the bar for more than one note. The "Showing 3 of N" line is covered
only by `tests/similar.test.mjs` and the Rust test of the cap. Notes created in the app, checked
when the user first leaves them, are covered by the same unit tests and were not run end to end.

The same run passed at fc4d283 before the review fixes.
[Trace](evidence/9-similar-run-2026-10-06-fc4d283.json). That run used the 0.62 bar and had no
capture-time, undo, or notification checks.

## Ask's run script after the refactor

`scripts/similar-run.mjs` reuses `scripts/ask-run.mjs`'s setup, so this branch moved that setup
into exported helpers. It also renamed the trace's `error` field to `failure` (4d0844a), because
WebdriverIO treats a returned object's `error` field as a failed command. To check that #8's
script still works, Claude ran `node scripts/ask-run.mjs --app src-tauri/target/debug/second-brain`
on the same 413af12 build, against the desktop's `gpt-oss:20b-cloud`. All six questions were
answered with no failure, and every note each question expects was cited. The run checked the
script, so its answers were not judged again for #8; they are in the run root, not committed.

## Desktop notification, by hand

A script cannot click a desktop notification, so Nicolas checked both on installed builds. He
captured a duplicate while another app had focus, then clicked the notification.

| Machine | Build | Notification | Click | Card |
| --- | --- | --- | --- | --- |
| Windows 11, `D:\SecondBrainTest\app` (NSIS, per user) | 4d0844a | Shown | Raised the main window | Shown |
| Fedora 44, GNOME 50 (Wayland), RPM through the test package helper | fc4d283 (built 07:10 −05:00, 5 minutes after the commit) | Shown | Raised the main window | Shown |

Neither hand check was repeated at 413af12:

- **Windows.** Between 4d0844a and 413af12 the Windows notification code changed in two ways. The
  click handler now passes no activation token to the shared window-raising function, and the
  wording function was renamed. The Windows build was not reinstalled, so the click at 413af12 is
  argued from that diff, not run. CI's `windows-rust` job compiles the pull request's head.
- **Linux.** 413af12 sends one notification at a time, and starts the click listener after the
  first notification is out rather than before. The listener's handling of a click is the code
  that passed at fc4d283: it matches the notification's id, takes the `ActivationToken`, and
  raises the window. The end-to-end run above shows the sending and replacement at 413af12. The
  click itself at 413af12 is argued, not run.

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

The #9 design comment names `notify-rust` and `tauri-winrt-notification`. The build uses neither.
Windows reuses the `windows` crate toast code already in `hotkey/windows.rs`. Linux uses the
`zbus` that `ashpd` already brings in, plus `gtk` to hand the activation token to the window;
`gtk` was already in the tree through tauri and is now a direct dependency. The behaviour is the
one decided.

Two things about the test machines, not the app:

- The Fedora laptop had GNOME's Do Not Disturb on for the first checks, which sends
  notifications to the list without a banner.
- The desktop's Ollama alternated between Nicolas's usual server on port 11434 and a second
  Ollama server he runs on 11435 for another project, so several hand checks skipped silently
  with the backend unreachable, as designed.

## Not proven

- The notification click at 413af12, on either machine (argued above).
- Capture speed compared with a build without the check: there is no baseline run.
- The in-app trigger (a note created in the app, checked on first leave), the "Showing 3 of N"
  line, and clearing cards on a vault switch, end to end. Each has unit tests only.
- Calibration on notes with a separate title and body. Every fixture capture is one line, so its
  body is empty (#213).
- The Do Not Disturb path: the app's send succeeds, so it logs nothing, and the card waits as
  designed. This was seen by hand, not tested.

## Seen, not part of #9

- **One-line quick captures become all title** (#213): the first line of a capture is its title,
  so a one-line thought leaves the body empty. Nicolas wants it resolved before #10.
- GNOME labels the main window "Tauri App" (its title bar and the "… is ready" notice).
