# Quick capture title and body fields (#213): verification

Dates: 2026-10-08 to 2026-10-09 (UTC−5). Candidate: commit 1da54a7 on `fix/213-capture-fields`.
This report's own commit changes only the reports and their evidence. The design is in two #213
comments: 6068855989 (the grilling decisions) and 6089263819 (Ctrl+Z and the message size, added
by Nicolas after the first hand checks).

Sol's usage had run out until 2026-10-09 19:28, so a Claude Sonnet 5.5 agent at extra-high effort
ran the tests and checks in her place, as AGENTS.md (Roles) provides. Nicolas did the hand checks.

## What changed

- The capture overlay has a Title input and a Body textarea, and both are required. Ctrl+Enter
  opens the category picker, or names the empty field ("Add a title." / "Add a body.") and puts the
  caret there. Tab and Shift+Tab switch fields and wrap. Enter in Title moves to Body. The discard
  prompt's S follows the same rule. Esc asks before discarding when either field has text.
- `quick_capture_note` takes `title` and `body` and refuses either one empty.
  `hotkey::capture::validate` replaces `split`.
- Note file names are cut to 100 characters and 200 UTF-8 bytes (`note_file_stem` in
  `vault/operations.rs`) for create, web clipping, rename and duplicate. The frontmatter title keeps
  its full length. Attachments keep plain `sanitize_filename`, so their extensions are never cut.
  After a rename, the note list takes the new file name from the path the backend returns, not
  from the title.
- Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y undo and redo in plain inputs and textareas in every window.
- The similarity fixture's 17 captures each gained a short title. `scripts/similar-run.mjs` and
  `similarity_calibration` send both fields. The run's "capture with a body under its title" check
  is gone, because every capture now has one.

## Checks on the candidate

The agent ran `pnpm verify` at 1da54a7 on the Fedora 44 laptop (kernel 7.2.8-200.fc44.x86_64). It
passed: svelte-check with no errors or warnings, the Node tests, the Rust tests, rustfmt, clippy
with warnings denied, and the frontend build. The logs are not committed. The counts are their
summary lines:

- 247 passed, 0 failed, from `node --test tests/*.test.mjs`, the 36 `.test.mjs` files directly
  under `tests/` (`ls tests/*.test.mjs | wc -l`).
- `680 passed; 0 failed; 10 ignored`, from `pnpm test:rust`. The 10 ignored are the live and
  measurement tests (`grep -rc '#\[ignore' src-tauri/src` sums to 10), `similarity_calibration`
  among them.

The first `pnpm verify`, at 6cc023f, failed one new test: the expectation for duplicating a
long-titled note was wrong, not the code. The cut name of "<title> copy" is the source's own
name, so the first copy correctly takes number 2. 5435dba fixed the test.

These run on Linux only. CI's `windows-rust` job compiles the Windows-only code on the pull
request; it had not run when this report was written.

## Evidence from b672554

The calibration and the end-to-end run were done at b672554. They still apply to 1da54a7, which
changes no backend code and no script: `git diff --stat b672554 1da54a7 -- src-tauri scripts`
prints nothing. Its four changed files are the text-undo helper and its test, the root layout, and
the overlay page.

### The similarity bar

The 0.65 bar (`SIMILAR_SCORE` in `similar_notes.rs`) was calibrated in #9 on captures that were all
title and no body. Captures now carry both, so their embeddings change. `similarity_calibration`
ran at b672554 against the desktop's Ollama 0.35.1 with `embeddinggemma:latest` (digest
85462619ee72, the same weights as #9).
[Output](evidence/213-similarity-calibration-2026-10-08-b672554.txt).

- The 8 duplicates scored 0.778 to 0.917 against their notes (#9: 0.769 to 0.850).
- No other pair, same-topic notes included, passed 0.571 (#9: 0.533). The highest is Cold brew
  ratio against Espresso dialing in.

The bar stays at 0.65, with 0.079 of room above the highest other pair and 0.128 below the lowest
duplicate. b672554 updated the bar's comment to these numbers.

### End-to-end run

The agent built `pnpm tauri build --debug --no-bundle` at b672554 and ran
`node scripts/similar-run.mjs --app src-tauri/target/debug/second-brain` on the Fedora laptop at
16:09 UTC−5 on 2026-10-08, against the same Ollama. The run root is kept on the laptop:
`~/sb-similar-run/2026-10-08T21-09-33.670Z`.
[Trace](evidence/213-similar-run-2026-10-08-b672554.json).

- The 8 duplicates each got exactly their expected card, and the 9 other captures got none.
- Every saved note's title is its capture's title.
- Each check passed: append, dismiss, refused appends on a changed note and a moved note, open,
  append with the capture open, and append with the target open. So did the replaced notification
  pair, offline capture, and the held check across a vault switch (no card, no notification).
- Capture took 74 to 133 ms (median 83).

## Hand checks

Nicolas ran these on installed builds. On Fedora he used the RPM, installed through the test
package helper. On Windows he used the NSIS build, installed to `D:\SecondBrainTest\app`.

| Build | Machine | Checks | Result |
|---|---|---|---|
| b672554 | Fedora | The 8 overlay steps below | Pass |
| 1da54a7 | Fedora | Undo and redo in Title, Body and a main-window field; the message's place and size | Pass |
| 1da54a7 | Windows | The 8 overlay steps, undo and redo, and the message | Pass |

The 8 overlay steps are:
1. The caret starts in Title.
2. Enter in Title moves to Body, and Enter in Body adds a newline.
3. Tab and Shift+Tab switch fields and wrap.
4. Ctrl+Enter with Title or Body empty shows "Add a title." or "Add a body." and focuses that field.
5. Pasting several lines into Title.
6. The discard prompt's S with a field empty.
7. A full capture is filed with its title and body.
8. A title over 100 characters saves under a shorter file name and keeps its full title.

Step 5 passed, but how the Title field showed the pasted line breaks was not recorded.

The 1da54a7 checks on Fedora did not rerun the 8 overlay steps. 1da54a7 changes the overlay page
only to move the message into the footer row, and the Windows checks on 1da54a7 cover all 8.

## Ctrl+Z

Nicolas found that Ctrl+Z did nothing in the overlay at b672554. A WebDriver probe on the b672554
debug build typed "abc" into fields and sent Ctrl+Z. The probe is not committed; it ran from
`/tmp/undo-probe`. The fields tried were a textarea added to the main window and both overlay
fields:

- Ctrl+Z left the text unchanged in all three.
- `document.execCommand('undo')` then cleared it.

So the Linux WebView keeps an undo history for plain fields but binds no key to it. This was not
specific to the overlay. 1da54a7 maps the keys in the root layout. The same probe on a 1da54a7 debug
build showed Ctrl+Z clearing the text in all three fields.

The probe's keys are synthesized by WebDriver, so on its own it is weaker proof than a real key
press. Nicolas's hand checks on both machines are the real key presses.

## Final review

Sol (Codex `gpt-6.1-sol`, effort high) reviewed `git diff origin/main...35e1d9a` in three phases:
spec, standards, adversarial. Standards had no findings. Three P2 findings followed:

| Finding | Fix |
|---|---|
| Trailing dots stay on a name that is not cut (`Buy paint etc..md`), against the decision's wording | The decision's stated reason was wrong, so it is corrected on #213 (comment 6089869944) and the code stays: `.md` follows every name, and trimming every title would name `...` as the hidden file `.md` |
| 100 characters of Chinese or emoji are 300 to 400 bytes, over Linux's 255 | Cap at 200 UTF-8 bytes as well; tested with 3- and 4-byte characters |
| A rename's list entry built its `relative_path` from the title, so a cut name left a path that does not exist (a Quick Access pin made before the next refresh would point at it) | `withRenamedFile` in `utils/paths.ts` takes the name from the returned path, in `NoteList.svelte` and `Editor.svelte` |

The rename fix also covers titles with characters the backend replaces (`a/b` is filed as
`a-b.md`), which built the same wrong path before #213.

## Not proven

- CI's `windows-rust` job on the pull request.
- A rename to a long title whose first 100 characters match another note's in the same folder is
  refused with "A note with that name already exists". This was argued from the code, not tested.
  It is rare, and the message is accurate about the file name.
