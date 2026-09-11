# Ticket #12 live verification — 2026-09-10

## Scope

Verify the Notion publisher against a real workspace rather than a scripted server. The
unit tests prove the code does what it intends; they cannot prove Notion behaves as the
code assumes, and several decisions rest on exactly that.

Driven by `src-tauri/src/notion/live_tests.rs`, which exercises the same functions the app
calls — `setup_databases`, `enumerate`, `publish::run` — against the live API. Assertions
about what Notion holds are made through a separate raw HTTP probe, so the verification
does not trust the client it is verifying.

## Environment

| | |
| --- | --- |
| Machine | Linux laptop (Fedora 44) — not the Windows desktop that will publish in practice |
| Workspace | `sb-notion-spike`, disposable, free plan |
| API version | `2026-03-11` |
| Branch | `feat/12-notion-push` at `03091c1` |
| Vault | 120 generated notes across all four categories; every 25th note is 250 paragraphs, needing three block batches |

## Results

All passed on the second attempt (the first is covered below).

| Check | Result | Evidence |
| --- | --- | --- |
| Setup creates four databases | Pass | 4 requests, registry complete |
| Large first sync completes without failures | Pass | 120 created, 0 failed, 134 requests in 122.3 s |
| Requests stay within the rate limit | Pass | 1.10 req/s against a ceiling of 3; no 429 observed |
| An unchanged vault costs nothing | Pass | 120 up to date, **0 requests** |
| An edit replaces content via `erase_content` | Pass | Page reads only the new text; the old "Original paragraph" is gone, not appended beside |
| A category change moves the page, keeping identity | Pass | Same page id, now parented to the Archives data source |
| The move's dropped tags are restored | Pass | Tags `["live", "batch-2"]` present after the move |
| A deletion trashes the page | Pass | `in_trash = true` |
| An interrupted create is resolved, not duplicated | Pass | Exactly one page carries `live-0004`, with its original id |
| The whole map lost is rebuilt without duplicates | Pass | 0 created, 119 updated, 0 failed; spot-checked notes each have exactly one page |
| Cleanup | Pass | All four databases trashed |

## Findings

### The first attempt: a database creation hung

Three of the four `POST /databases` calls returned promptly; the fourth, Archives, hung
until the 30-second client timeout. Notion did not create it server-side — checked
afterwards, no late Archives appeared — so the timeout did not strand a duplicate. The
second attempt created all four without delay, so this reads as transient.

The product handled it correctly: setup saves each database as it succeeds, and a retry
creates only what is missing. **The test harness did not**: setup ran outside the cleanup
guard, so the panic left three real databases in the workspace. They were trashed by hand
and the harness fixed so setup runs inside the guard.

The same failure exposed that network errors reported only "error sending request for url",
with the actual cause buried in reqwest's source chain. That line is what the Settings panel
would have shown a user. Errors now carry the full chain.

### Throughput is bounded by latency, not by the pacer

The pacer allows three requests a second; the first sync achieved **1.10**. Requests are
sequential, so each waits for Notion's response before the next is sent, and a page create
carrying its content takes roughly 0.9 s to return. The pacer is almost never the thing
waiting.

What that means in practice, extrapolated from 134 requests in 122 s:

| Vault | First sync, approximately |
| --- | --- |
| 120 notes | 2 minutes |
| 1,000 notes | 17 minutes |
| 3,000 notes | 50 minutes |

The acceptance criterion — a large initial sync completes without failures — is met: it is
slow, not failing, it runs in the background, and it reports progress. Sending requests
concurrently up to the three-a-second ceiling could cut this by roughly two and a half
times. That is a performance change with its own risks (ordering against a single page,
retry interaction) and is **not** made here.

### Rebuilding a lost map is expensive but correct

The map-lost run cost 371 requests for 119 notes — one listing sweep, then an erase, an
append, and a property write per note, since an adopted page's content is refreshed rather
than trusted. At the observed rate that is about five and a half minutes. It is a recovery
path for a rare event, and it produced no duplicates, which is the property that matters.

## Faults this verification found before it ran

Planning the scenarios, rather than running them, surfaced one serious fault and three
smaller ones, fixed in `0591622`:

- **A lost map duplicated every page.** A note with no map entry was treated as never
  published. It now checks what Notion holds first. The run above is the evidence it works.
- Modification times in whole seconds could leave the last edit of an autosave burst
  unpublished. Now nanoseconds.
- Network errors hid their cause.
- "An unchanged vault costs nothing" was inferred, not measured. The client now counts.

## Windows

The publisher is meant to run on the Windows desktop (#57), so the same live test was run
there, over `ssh sb-windows`, on the branch after merging `main` (`710d5ba`).

| Check | Linux | Windows |
| --- | --- | --- |
| Setup | Pass, 4 requests | Pass, 4 requests |
| 120-note first sync | 134 requests, 122.3 s, 1.10 req/s | 134 requests, 106.9 s, 1.25 req/s |
| Unchanged rerun | **0 requests** | **0 requests** |
| Edit, move with tags, delete, interrupted create | Pass | Pass |
| Entire map lost | 0 created, 119 updated | 0 created, 119 updated |

The unchanged rerun is the check that mattered. Enumeration rewrites `\` separators to `/`
before matching a note's path against its map entry; if that were wrong on Windows, every
note would look new and the whole vault would republish every five minutes. Zero requests
means every one of the 120 Windows paths matched.

All 132 Notion unit tests also pass on Windows, including the enumeration tests that walk
real files.

The full Windows suite was **not** clean, in three modules this branch doesn't touch. On
the branch's original base, `concurrent_opens_agree_on_one_identity` failed every time with
`os error 33`. #60 fixed that, and merging `main` picked the fix up. After the merge it
still failed once under full-suite load, though it passed 5/5 alone. Separately, `main`
itself failed two `search::external` tests with `Access is denied (os error 5)` in one of
two runs, and a debounce test is timing-sensitive. CI runs only on Ubuntu, so none of this
was visible. All three are recorded as **#62**.

## Not covered

- **The guided setup clicked through in the app.** The backend path was driven directly;
  the Settings tab has been type-checked but not operated by hand.
- **The packaged app.** Everything here ran from a test binary.

## Incidental

One pre-existing test, `vault::relocation::tests::killing_the_app_mid_notebook_move_still_recovers`,
failed once in eight full-suite runs during this work.

**Update, same day:** a loop set up to catch it failed on run 24 of 25, with output. It is not
flaky and not a coverage shortfall. The *correctness* assertion failed: recovery could not
parse an **empty** `directory-move.json`. The manifest is written with `create_new` followed
by `write_all`, so a kill between the two leaves a zero-byte file under the final name.
Recovery handles a missing manifest but not an incomplete one.

No note is lost, because the manifest precedes the rename. But the vault is stuck: every open
fails recovery, shows a repair warning the user can't clear, and skips category reconciliation,
until the file is deleted by hand. Filed as **#61**. Nothing on this branch touches relocation.

## Reproduction

```text
NOTION_TEST_PARENT_PAGE=<page id> cargo test --lib notion::live_tests -- --ignored --nocapture
```

The token is read from `~/.config/second-brain-notion-test-token`. Use a disposable
workspace, and share the parent page with the connection first — a new connection can see
nothing until a page is explicitly shared with it.
