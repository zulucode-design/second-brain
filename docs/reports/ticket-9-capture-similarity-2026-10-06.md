# Capture-time similarity (#9): verification

Dates: 2026-10-05 and 2026-10-06. Candidate: commit fc4d283 on `feat/9-capture-similarity`.
Runs by Sol (until her usage limit on 2026-10-05) and Claude; hand checks by Nicolas. The
design is #9 comment 5998786646.

## Checks on the candidate

`pnpm verify` at fc4d283 on the Fedora 44 laptop: 235 Node tests, 670 Rust tests (10 ignored,
the live and measurement tests), svelte-check with no errors or warnings, rustfmt, clippy with
warnings denied, and the frontend build all passed.

## The similarity bar

`similarity_calibration` (an ignored Rust test) indexes the 13 notes of
`scripts/similarity-fixture.json` with live `embeddinggemma` and scores its 17 one-line captures
against every note, embedding each capture as the overlay files it. Run by Sol at b1be541 through
an `ssh -L` tunnel to the desktop's Ollama.
[Output](evidence/9-similarity-calibration-2026-10-05-b1be541.txt). `semantic_search.rs` has not
changed between b1be541 and fc4d283, so the scores still apply.

| Captures | Count | Score against the note |
| --- | --- | --- |
| Reworded duplicates, against the note they repeat | 8 | 0.769 to 0.850 |
| Every other capture–note pair, same-topic notes included | — | 0.533 at most (rye sourdough against "Sourdough hydration notes") |

The bar is 0.62, between the two. An earlier run at 693603c reported 0.714 for the Atomic Habits
duplicate: the test wrote titles containing `: ` unquoted, so two book notes were indexed under
their file names. Sol caught it; e982b6f quotes titles, and the rerun above gave 0.777 for that
pair.

## End-to-end run

`node scripts/similar-run.mjs --app src-tauri/target/debug/second-brain`, by Claude at fc4d283 on
the Fedora laptop: a debug build under `tauri-driver`, a fresh profile and vault holding the
fixture's notes, the desktop's Ollama tunnelled in. Each capture goes through the command the
overlay uses (WebDriver cannot press a global hotkey), and the card it raises in the main window
is read back. [Trace](evidence/9-similar-run-2026-10-06-fc4d283.json).

| Check | Result |
| --- | --- |
| 8 duplicate captures | Each card named the note it repeats, with its matching passage |
| 6 same-topic and 3 unrelated captures | No card |
| Append | The note kept its text and gained the marker and the capture; the capture moved to trash |
| Dismiss | Both files byte-for-byte unchanged |
| Append after the note was edited on disk | Refused: "One of the notes changed since the check, so nothing was added." Both files unchanged |
| Capture with the tunnel closed | Saved; no card |

The same run passed at 4d0844a (Sol). No fixture capture passes the bar for more than 3 notes, so
the "Showing 3 of N" line is covered only by `tests/similar.test.mjs`. Notes created in the app,
checked when the user first leaves them, are covered by the same unit tests and were not run end
to end.

## Desktop notification, by hand

A script can neither press the hotkey nor click a desktop notification, so Nicolas checked both on
installed builds: capture a duplicate while another app has focus, then click the notification.

| Machine | Build | Notification | Click | Card |
| --- | --- | --- | --- | --- |
| Windows 11, `D:\SecondBrainTest\app` (NSIS, per user) | 4d0844a | Shown | Raised the main window | Shown |
| Fedora 44 GNOME 50 (Wayland), RPM through the test package helper | fc4d283 | Shown | Raised the main window | Shown |

The Windows result carries to fc4d283: between the two commits the Windows notification code
changed only in passing no activation token to the shared window-raising function. CI's
`windows-rust` job compiles fc4d283; the Windows build was not reinstalled.

Fedora took four builds to pass. Each failure, and its fix:

1. **Portal click without a token (4d0844a).** Notifications went through the notification
   portal. A click raised GNOME's "… is ready" notice instead of the window: on Wayland, raising a
   window needs an activation token, and GNOME 50's portal delivered the click with an empty
   parameter list (recorded with `dbus-monitor`).
2. **Token passed, still none (6e3f62e).** The click handler handed the portal's token to GTK, but
   there was none to hand over.
3. **Notifications closed within milliseconds (11126ec).** Notifications moved to
   `org.freedesktop.Notifications`, which sends `ActivationToken` before `ActionInvoked`. Each was
   closed by GNOME 13 to 25 ms after it was sent. GNOME Shell's `notificationDaemon.js`
   (`_onNameVanished`) withdraws an app's notifications when the D-Bus connection that sent them
   closes, and the code opened a connection per notification. ec1d8a0 guessed at focus instead
   and was reverted (5a7dc50).
4. **Pass (fc4d283).** One connection for the app's lifetime, shared with the click listener.

The #9 design comment names `notify-rust` and `tauri-winrt-notification`. The build uses neither:
Windows reuses the `windows` crate toast code already in `hotkey/windows.rs`, and Linux uses the
`zbus` that `ashpd` already brings in. The behaviour is the one decided, with no new crate.

Two things about the test machines, not the app: the Fedora laptop had GNOME's Do Not Disturb on
for the first checks, which sends notifications to the list without a banner. The desktop's
Ollama alternated between Nicolas's usual server on port 11434 and the TransMiVoz scheduled-task
server on 11435, so several checks skipped silently with the backend unreachable, as designed.

## Seen, not part of #9

- **One-line quick captures become all title** (#213): the first line of a capture is its title,
  so a one-line thought leaves the body empty. Nicolas wants it resolved before #10.
- GNOME labels the main window "Tauri App" (its title bar and the "… is ready" notice).
