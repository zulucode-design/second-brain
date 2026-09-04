# ADR-0002: Machines sync to each other; Notion becomes a read-only view

- **Status**: Accepted
- **Date**: 2026-09-03
- **Context**: Tickets #6 (attachment device tags), #12, #13 (Notion sync)
- **Supersedes**: SPEC.md §8 "Notion is the sync hub", and two rows of §10

## Context

Spec §8 made Notion the sync hub — **Desktop ⟷ Notion ⟷ Laptop**, deliberately not
peer-to-peer, to offload multi-writer merging onto Notion rather than build CRDTs. §10
rejected peer-to-peer CRDT sync ("far larger than the rest of v1 combined") and external
sync tools ("superseded by the Notion-hub decision").

That topology has one consequence §8 states plainly: **attachments never upload to Notion**
(free tier caps files at 5MB). Binaries stay on whichever machine created them.

Ticket #6 exists to explain that consequence to the user. Because Notion carries text and
the machines never talk directly, a note captured on the Desktop can reach the Laptop, but
the PDF it references cannot. #6 proposed tagging every attachment with the machine holding
it, so the Laptop renders "stored on: Desktop" instead of a broken image.

Designing #6 is what invalidated it. Working through how a device tag survives a text-only
channel produced a scheme — the device name baked into the attachment's path, carried for
free inside the markdown link that already travels with the note — and then the question
that undid it: *if the two machines could simply sync their files, what is the tag for?*

Nothing. It answers "which machine holds this file" in an architecture where the answer is
only ever one machine because nothing was built to make it two. The tag is a label for a
limitation, not a feature.

The limitation was never a requirement. It followed from choosing a hub whose free tier
caps uploads at 5MB and whose data model is text. Both machines are already on the same
private Tailscale network, already reach each other, and already run this app.

### What Notion was actually chosen for

Not conflict resolution — reachability. The README's promise is that notes are "reachable
from anywhere", meaning a phone or a browser, when neither machine is awake or to hand.
Direct sync between two machines cannot do that: if the Desktop sleeps and the Laptop is in
a bag, there is nothing to read.

That is worth keeping, and it does not require Notion to be the hub. It requires Notion to
be a **view**.

### What direct sync costs

The transport is the easy half — Tailscale already provides an authenticated private network
between exactly these two machines. The parts that consume projects are:

- **Deletion versus absence.** Without tombstones and version vectors, "deleted on the
  Desktop" and "not yet arrived on the Laptop" are the same observation. Get it wrong and
  deleted notes resurrect indefinitely.
- **Move detection**, which this vault is unusually exposed to. PARA reconciliation
  relocates notes between category folders as an ordinary operation, and PR #23 hardened
  exactly that path. A syncer that models a move as delete-then-create collides head-on with
  the deletion problem, on the operation this app performs most.

Writing that engine is the same category of mistake §10 avoided when it rejected CRDTs. The
conclusion is not to avoid direct sync; it is to avoid *authoring* it.

## Decision

**The two machines sync directly to each other. Notion becomes a read-only published view.**

Sync is provided by a **bundled sync engine run as an app-managed sidecar**, not written
here and not installed by the user.

### Notion is read-only, and write-back is deferred

Notes flow to Notion for reading. Nothing flows back — not new pages, not edits.

This is narrower than the obvious design, and deliberately so. A note created in Notion has
no PARA category, and this vault refuses uncategorised notes by construction: every creation
path requires an explicit, confirmed category, enforced since #1. Importing such a note means
either guessing a category — which §4 of the spec forbids — or inventing a filing prompt for
content that arrived while the user was elsewhere.

Deferred rather than rejected. The holding area (`.helixnotes/unfiled`) already exists for
uncategorised notes and is the likely home when this is revisited; that is a design note for
a later ticket, not a commitment here.

Making Notion a third writer would also reintroduce three-way conflicts between two machines
and a hub — precisely the problem that moving off the Notion hub eliminates.

### The engine is bundled, not written and not installed

Tauri ships sidecar binaries (`externalBin`) as a first-class feature. The app owns the
engine's lifecycle, configuration, and folder scoping. To the user, sync is a toggle in
Settings: nothing to install, no second application, no configuration file to get wrong.

Syncthing is the intended engine. It is MPL-2.0, which is clean alongside this project's
AGPL-3.0-or-later as a separately-bundled process.

The trade accepted here is **conflict semantics we inherit rather than choose**. The engine
resolves a conflicting edit by writing a conflict copy next to the original. That is not the
UX this project would have designed, and it is the reason the next subsection exists.

If those semantics prove wrong in practice, replacing the sidecar is a contained swap,
because the app-side boundary — files appearing and changing on disk — is identical whatever
produces them.

### Conflict copies are detected, never surfaced as notes

A conflicting edit produces a file like
`note.sync-conflict-20260903-141500-ABCDEF.md`, in the notes tree, ending in `.md`. Left
alone the note scanner indexes it as an ordinary note with a machine-generated title,
polluting search, the graph, and PARA counts with a duplicate.

The app recognises the pattern, pairs the copy with its original, and presents the choice
SPEC §8 already promises: both versions surfaced, the user chooses, never a silent
last-write-wins.

### The synced boundary is physical, not a configured ignore list

`.helixnotes/` mixes user data with machine-local state, and some of it is actively unsafe
to share:

| Path | What it is | Synced |
| --- | --- | --- |
| notes tree | the notes | yes |
| `.helixnotes/attachments/` | binaries — the reason for this ADR | yes |
| `.helixnotes/trash/` | deletions | yes — a deletion that does not propagate is a resurrected note |
| `.helixnotes/history/` | version snapshots | yes |
| `search_index/` | Tantivy index | no — machine-local, each machine builds its own |
| `relocation/` | in-flight transaction and recovery manifests | no — sharing these is a data-loss bug |
| `sync_state`, `repair_issues` | per-machine bookkeeping | no |

`relocation/` is the sharp one. It holds crash-recovery state for interrupted vault
transactions, from the hardening in PR #23. Replaying another machine's in-flight manifest is
not a rough edge; it is the failure that work exists to prevent.

**Machine-local state therefore moves out of the vault entirely**, into the per-machine
config directory. The boundary becomes a fact of where files live rather than a rule someone
configures correctly. "Sync the vault folder" is then correct by construction, with no ignore
list to mistype.

### Settings are machine-local by default, shared only where correctness demands

Sync forces every setting to be classified, and sharing them wholesale is wrong. The
decisive example: **the AI endpoint must stay machine-local.** The Desktop reaches Ollama at
`localhost:11434`; the Laptop reaches the same models at the Desktop's Tailscale address.
Sync that value and the Laptop silently loses AI. Backup directories differ per OS for the
same reason.

The rule is therefore: **machine-local by default; shared only when both machines must agree
for correctness.** Shared settings live in a new in-vault settings file; machine-local ones
stay in `AppConfig`, which is already per-machine.

The first shared setting is snapshot retention. History is
`.helixnotes/history/<note-id>/<timestamp>.md` — keyed by note ID, so it already survives
PARA moves, and append-only, so two machines writing into it converge without merge logic.
The obstacle is not structure but pruning: `max_versions_per_note` currently lives in
per-machine `AppConfig`, and two machines pruning a shared folder to different limits delete
each other's snapshots in a loop. Moving it to vault scope makes both machines compute the
same target set from the same rule.

### Pairing is explicit

Two devices must exchange identities once, and this is the one place the invisible-sidecar
promise cannot hold: something has to authorise the pairing. The app shows an identifier on
one machine and accepts it on the other.

Auto-pairing anything on the Tailnet was rejected: it makes network membership the only thing
protecting the vault, and a Tailnet can carry devices belonging to other people.

### Externally-arrived notes must reach the search index

The Tantivy index is machine-local and is currently maintained by the app's own write paths.
`VaultWatcher` already observes the vault via `notify`, but only emits a `file-changed` event
to the frontend — it does not index. A note arriving from the other machine would appear in
the tree and be **invisible to search**.

External change events therefore drive index updates, through the same path the app's own
writes use. This is strictly more correct than a sync-specific hook: it also fixes editing a
note in another editor, or restoring one by hand, both broken today for the same reason. It
means the sync engine needs no privileged integration at all — it is simply another external
writer.

This carries a debounce problem — a sync burst fires many events — and is its own ticket
rather than a clause of the restructure.

## Consequences

- **#6 is closed as superseded.** Device tagging answers a question that stops existing once
  attachments sync. Its one surviving requirement — an attachment not yet arrived reads as
  "not synced yet", never as broken or missing — moves to the sync ticket.
- **#13 is closed as superseded.** With Notion read-only it has nothing left to describe;
  the conflicts it existed to resolve now happen between machines. **#12 is rescoped** to
  the whole Notion story: one-way push for remote reading. Notion write-back becomes a new,
  explicitly deferred ticket.
- Locality is determined by **the file being present**, never by a recorded tag. A file on
  both machines opens on both; a tag would have been consulted only in its absence.
- The restructure moves directories the crash-recovery path depends on. It requires the same
  treatment #4 received: interrupt a move mid-flight and confirm recovery still works from
  the new location. A green suite is not evidence here.
- Sync introduces a state the app has never had — a file that is *correctly* absent and will
  arrive later. Every path that treats a missing file as an error has to distinguish it from
  one that is merely in transit.
- Version history stops being per-machine, which was never a deliberate choice but is a real
  behavioural change: restore now offers the same versions wherever the user is sitting.
- Notion is no longer load-bearing. Sync between machines works with no Notion account, no
  API token, and no internet — only the Tailnet.

## Rejected

| Option | Why rejected |
| --- | --- |
| Keeping Notion as the sync hub | Text-only and 5MB-capped, which is what forced attachments to be single-machine and #6 to exist |
| Device tags on attachments (#6 as written) | Explains a limitation instead of removing it; vestigial the day attachments sync |
| Writing the sync engine here | Tombstones, version vectors, and move detection against a vault that relocates files as routine — the same "larger than the rest of v1" reasoning that killed CRDTs |
| Requiring the user to install Syncthing | Rejected as UX; sync should be a toggle, not a second application to configure |
| Auto-pairing every device on the Tailnet | Makes network membership the only protection on the vault |
| Syncing settings wholesale | The Laptop would inherit `localhost` as its AI endpoint and silently lose AI |
| A documented ignore list for `.helixnotes/` | One typo replays another machine's relocation manifest; the boundary must be physical |
| Notion as a third writer | Reintroduces three-way conflicts, and Notion-created notes have no PARA category |
| Peer-to-peer CRDT sync | Still rejected, unchanged from §10 — file-level sync is not CRDT sync |

## References

- SPEC.md §8 (sync topology, superseded here), §10 (rejected options, two rows superseded)
- ADR-0001 — precedent for platform-divergent design recorded rather than assumed
- PR #23 — vault transaction hardening; the source of `relocation/`
- Tauri sidecars (`externalBin`): https://v2.tauri.app/develop/sidecar/
- Syncthing conflict handling: https://docs.syncthing.net/users/syncing.html#conflicting-changes
- Syncthing licence (MPL-2.0): https://github.com/syncthing/syncthing/blob/main/LICENSE
