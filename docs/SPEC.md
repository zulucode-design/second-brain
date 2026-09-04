# Second Brain — Specification

Status: **v1 implementation in progress**
Last updated: 2026-09-03

A personal knowledge management desktop app implementing Tiago Forte's *Building a Second
Brain* (BASB) methodology. Forked from [HelixNotes](https://gitlab.com/ArkHost/HelixNotes)
(AGPL-3.0-or-later).

---

## 1. Scope

### In scope for v1

The BASB method has four phases: **C**apture, **O**rganize, **D**istill, **E**xpress. v1
implements **Capture and Organize only**.

Distill (highlighting, progressive summarization) and Express (compiling notes into
outputs) are explicitly deferred to a later milestone. They are not v1 features.

### Content types

| Type | v1 handling |
| --- | --- |
| Markdown notes | Primary primitive. Full editor support. |
| Web clippings | Paste a URL; app fetches and stores readable content. No browser extension. |
| Files / PDFs / images | Stored locally, referenced from notes. |
| Audio / voice memos | Recorded/imported, transcribed locally via whisper.cpp. |

### Out of scope

- Browser extension for clipping (URL-paste only)
- Mobile apps
- Multi-user / collaboration features
- Hosted web version
- Distill and Express phases

---

## 2. Platforms and hardware

| Machine | Role | Hardware |
| --- | --- | --- |
| Windows desktop | Primary + AI backend host | Ryzen 5700X, RTX 3080, 2TB NVMe |
| Linux laptop | Secondary client | GTX 1050 (2018-era) — too weak for local inference |

Both run the full app natively. The laptop does **not** run its own inference; it calls
the desktop's AI backend over the network (see §6).

---

## 3. Architecture

### Stack (inherited from HelixNotes)

- **Shell**: Tauri 2 (Rust backend, system WebView — not Electron)
- **Frontend**: SvelteKit + Svelte 5, Tailwind 4
- **Editor**: TipTap 3
- **Full-text search**: Tantivy
- **Rust MSRV**: 1.88

Tauri was chosen over Electron specifically for its small memory footprint, leaving
headroom for local AI models on the desktop.

### Storage

- **Source of truth**: plain markdown files on disk, one file per note, in PARA folders.
  Human-readable, greppable, portable, no lock-in.
- **Index**: two derived, rebuildable indices —
  - **Tantivy** (inherited, unchanged) for full-text search
  - **SQLite** (new, via `rusqlite`) holding embeddings as blobs plus semantic metadata
- **Metadata**: YAML frontmatter (`vault/frontmatter.rs` already supports this) — the PARA
  category lives here.

**Decided 2026-08-23: keep Tantivy, add SQLite alongside it.** Consolidating everything
onto SQLite (FTS5 + a vector extension) would mean removing working, tested search code and
permanently diverging from upstream, making every future upstream search change a manual
merge conflict. The fork exists to inherit working machinery; search is the most
load-bearing piece we inherited.

Start with **brute-force cosine similarity** over embeddings stored as SQLite blobs. At
personal-vault scale (~1k–10k notes) this is fast enough, and it avoids depending on
`sqlite-vec`, which is still pre-1.0. Add an ANN index (usearch/HNSW, LanceDB) only if
measurement shows it is needed.

Rejected: MySQL / SQL Server / Oracle (available via the Uniandes software catalog). They
are client-server databases requiring a running DB server on every machine, which is wrong
for an embedded desktop app and impossible to ship to other users. Their academic licenses
are also tied to enrollment and incompatible with a public AGPL repo.

### Version history / backup

**Decided 2026-08-23: no git auto-commit.** Verified redundant against three inherited
protections:

1. **Trash on delete** — `vault/operations.rs::delete_note` moves files to
   `.helixnotes/trash/<timestamp>_<name>`; it never hard-deletes.
2. **Per-note version history** — `history.rs` snapshots at ≥5 min intervals into
   `.helixnotes/history/<note-id>/`, pruned to `max_versions`. Survives note deletion,
   since `delete_note` does not remove the history directory.
3. **Automatic whole-vault backups** — `backup.rs` zips the vault (attachments optional)
   on a configurable interval, triggered from `AppLayout.svelte`, with restore and
   max-count pruning.

**Remaining gap to handle.** All three are bounded/pruned, and backup restore is
all-or-nothing — it rolls back good changes along with bad. Vanilla HelixNotes never writes
to many notes at once, but this app adds sync and AI operations that can modify dozens of
notes in a single run.

Mitigation (no new dependencies): **force a vault backup immediately before any bulk AI
operation, and before a machine-to-machine sync applies a batch of incoming changes**,
reusing the existing `create_backup` path.

> Revised by [ADR-0002](adr/0002-machine-to-machine-sync-notion-becomes-read-only.md): this
> previously named the Notion sync run. Notion is now a read-only push and cannot modify
> local notes — the incoming writes come from the other machine instead. Note that
> `history/` is itself synced (§8), so it is no longer an independent local record of what a
> note used to say; the pre-batch backup carries more weight than it did.

---

## 4. PARA model

Four categories, **hardcoded** to the book's terms, not user-renameable:

- **Projects** — short-term efforts with a goal and deadline
- **Areas** — long-term responsibilities to maintain
- **Resources** — topics of ongoing interest
- **Archives** — inactive items from the other three

**Strict single-location filing**: every note lives in exactly one PARA category. No
multi-category tagging. This mirrors the book's original design and keeps the Notion
mapping 1:1.

**Category is chosen at capture time** — there is no unfiled Inbox stage. The user always
files deliberately; the AI never auto-files.

### The note's category is the only source of truth

**Decided 2026-08-23.** A note's own `category` field decides what it is. Folders are
only where an already-categorised note is stored, so a folder never determines, overrides,
or implies a note's category.

When the two disagree — an external program, a file sync, or a file manager put a file
somewhere that contradicts it — **the file moves to match the note**. The note is never
rewritten to match the folder. Reconciliation runs when a vault opens, so no other program
can leave a note stored somewhere its category does not allow.

Moving a note between categories *inside the app* is the opposite case: that is the user
saying what the note now is, so the note's category is updated and the file follows.

A note carrying no category cannot be stored anywhere, since every location implies a
category it does not have. Those notes go to a holding area under the app's metadata
folder and are surfaced to the user, who must give each one a category. This covers notes
from a vault predating PARA and any note arriving without one. A missing category is never
guessed.

The holding area is a narrow compatibility boundary, not a fifth category. Its queue can
list and preview regular Markdown files directly inside that directory, but previews are
read-only and holding notes cannot be renamed, dragged, linked from Quick Access, opened in
a second window, edited, or trashed. The only mutating action is filing a note into an
explicitly chosen PARA category.

The four PARA roots are fixed: they cannot be created, renamed, moved, reordered, or
deleted through the app. Folders below them remain ordinary user organisation. Moving a
populated folder within or across categories moves the whole subtree; when the destination
category changes, every descendant note is updated to that category and its identity,
links, Quick Access entry, and search visibility follow the move.

> An earlier draft had this the other way round, with the folder as the source of truth and
> frontmatter as a mirror. That was rejected: it let any program that moved a file silently
> recategorise a note.

### No time-based organisation

Notes are stored by category, never by when they were made. The upstream daily-notes
feature — a `Daily` folder, a calendar view, and a "new daily note" action — is **removed
entirely**, because it filed notes by date and produced the one kind of note with no
category.

---

## 5. Capture UX

A **global system-wide hotkey** opens a quick-capture overlay from anywhere, without
switching focus to the app. This directly serves BASB's emphasis on removing friction at
the moment of capture.

Flow:

1. Hotkey pressed → capture overlay appears
2. User types/pastes content and picks a PARA category
3. Note is saved to the correct PARA folder
4. **AI similarity check runs**: if a semantically similar note already exists, the app
   surfaces it and offers to merge into / edit that note instead of leaving a duplicate

Requires OS-level global hotkey registration on both Windows and Linux.

---

## 6. AI

### Runtime and topology

- **Ollama** on the Windows desktop provides LLM + embedding inference.
- **whisper.cpp on the Windows desktop** provides audio transcription. (Ollama has no
  native speech-to-text — verified; a separate engine is mandatory.) It runs beside Ollama
  as a second desktop-side service, reached the same way.

  > **Corrected 2026-08-23.** An earlier draft said whisper.cpp was bundled and ran
  > locally on whichever machine recorded. That contradicted this section: it would have
  > run transcription on the GTX 1050 laptop, the exact hardware established as too weak.
  > Transcription now runs on the desktop like every other AI feature. Voice memos
  > recorded while the desktop is unreachable are stored as audio and **queued**, then
  > transcribed automatically on reconnect.
- The Linux laptop reaches the desktop's Ollama over **Tailscale**, a private WireGuard
  network joining only the two machines. Works from home or campus, requires no port
  forwarding, and exposes nothing to the public internet.

> Ollama has **no built-in authentication or rate limiting**. It must never be bound to a
> public interface. Tailscale is the security boundary — this is a hard constraint, not a
> preference.

`src-tauri/src/ai.rs` already supports Ollama alongside OpenAI/Anthropic, including custom
host configuration. Pointing it at a Tailscale address is close to a configuration change.

### Graceful degradation

When the desktop is unreachable (off, asleep, no Tailscale), the laptop must remain fully
usable:

| Works without AI | Requires AI backend |
| --- | --- |
| Capture, organize, browse | Semantic search |
| Keyword/full-text search | AI prompt window / Q&A |
| Editing, linking, graph view (explicit edges) | Related-note suggestions |
| Sync (between machines, and the Notion push) | AI-similarity edges |
| Recording voice memos (queued for later transcription) | Transcribing them |

Failures must degrade quietly, never block the core loop.

### AI scope — deliberately narrow

The AI **never files, tags, or categorizes automatically**. The user keeps full manual
control of organization. AI does exactly three things:

1. **Semantic search + Q&A** via a dedicated prompt/chat window ("what did I write
   about X")
2. **Similarity detection at capture time** (§5)
3. **Suggested links** — semantic similarity surfaces candidate connections which the user
   can promote to explicit links (§7)

---

## 7. Graph / mind-map view

A scrollable, zoomable node graph of the whole vault. `GraphView.svelte` (~1050 lines,
custom canvas renderer, no external graph library) is inherited and extended — we do
**not** swap in react-force-graph or Cytoscape.

Two visually distinct edge types:

| Edge | Meaning | Rendering |
| --- | --- | --- |
| Explicit link | User-created connection (wiki-link etc.) | Solid |
| AI similarity | Embedding-derived semantic relatedness | Dashed / lighter |

AI-similarity edges can be **promoted to explicit links** on user confirmation. Promotion
is always user-driven; the AI proposes, the user disposes.

The graph includes search, and shows connections between the currently-viewed note and
related notes.

---

## 8. Sync

> **Revised 2026-09-03 by [ADR-0002](adr/0002-machine-to-machine-sync-notion-becomes-read-only.md).**
> Notion was previously the sync hub (**Desktop ⟷ Notion ⟷ Laptop**). It no longer is.
> That topology made attachments single-machine — Notion is text-only and free-tier caps
> uploads at 5MB — which is the limitation ticket #6 existed to explain to the user.
> Removing the limitation was cheaper than explaining it.

### Topology

**The two machines sync directly to each other: Desktop ⟷ Laptop.** This is the real sync —
full fidelity, including attachments — and it runs over the private Tailscale network the
machines already share. It needs no Notion account, no API token, and no internet.

**Notion is a read-only published view**, kept for the one thing it was actually chosen for:
notes reachable from anywhere, on a phone or in a browser, when neither machine is awake or
to hand.

### The sync engine is bundled, not written here

Sync is provided by a **bundled engine run as an app-managed sidecar** (Tauri `externalBin`).
The app owns its lifecycle, configuration, and folder scoping. To the user, sync is a toggle
in Settings: nothing to install, no second application.

This project does not author the sync engine. Deletion-versus-absence and move detection are
where file syncers go wrong, and this vault relocates notes between category folders as an
ordinary operation (§4 reconciliation) — the case a naive syncer handles worst is the one
this app performs most.

The accepted trade is **conflict semantics inherited rather than chosen**: a conflicting edit
produces a conflict copy on disk. See Conflicts below.

### Pairing

Two devices exchange identities **once, explicitly** — one machine shows an identifier, the
other accepts it. Auto-pairing every device on the Tailnet is rejected: it would make network
membership the only protection on the vault.

### What syncs, and what must not

The boundary is **physical, not a configured ignore list**. Machine-local state lives outside
the vault entirely, so "sync the vault folder" is correct by construction.

| Path | Synced |
| --- | --- |
| Notes tree | yes |
| `.helixnotes/attachments/` | yes — the reason for this revision |
| `.helixnotes/trash/` | yes — a deletion that does not propagate is a resurrected note |
| `.helixnotes/history/` | yes |
| Search index (Tantivy) | **no** — machine-local, each machine builds its own |
| Relocation/recovery manifests | **no** — sharing in-flight transaction state is a data-loss bug |
| Per-machine bookkeeping (sync state, repair issues) | **no** |

### Settings: machine-local by default

Settings are **machine-local unless both machines must agree for correctness.** The decisive
case: the AI endpoint differs per machine — the Desktop reaches Ollama at `localhost`, the
Laptop at the Desktop's Tailscale address (§6). Syncing that value would silently cost the
Laptop its AI. Backup paths differ per OS for the same reason.

Shared settings live in an in-vault settings file. The first is snapshot retention, which
must be shared or two machines pruning `history/` to different limits delete each other's
snapshots.

### Notion mapping

**Four separate Notion databases**, one per PARA category. A note moving between categories
means moving between databases — the push layer must handle this.

- **Read-only.** Notes are pushed for reading. Nothing flows back: not edits, not new pages.
- **Polling** for push; Notion's API rate limit is ~3 requests/second, so pushes are queued
  and metered.

**Notion write-back is deferred, not rejected.** A note created in Notion has no PARA
category, and this vault refuses uncategorised notes by construction (§4). Importing one
would mean guessing a category — which §4 forbids — or prompting for content that arrived
while the user was elsewhere. The holding area (§4) is the likely home when this is
revisited.

> **Future milestone**: Notion write-back, and real-time push via webhooks plus a
> tunnel/relay. Neither is v1.

### Offline-first

The app is fully usable with zero internet. Local storage is the working copy; sync between
machines is opportunistic reconciliation when both are reachable, and the Notion push is
opportunistic when online.

### Conflicts

When a note is edited on both machines before either observed the other's change, the app
**surfaces both versions and lets the user choose**. No silent last-write-wins overwrite.

Concretely, the bundled engine writes a conflict copy beside the original
(`note.sync-conflict-<timestamp>-<device>.md`). The app **detects that pattern and pairs the
copy with its original**, presenting the choice above. A conflict copy is never surfaced as
an ordinary note — left alone it would be indexed under a machine-generated title and pollute
search, the graph, and PARA counts.

### Externally-arrived notes

A note arriving from the other machine is, to this app, an external filesystem change. The
vault watcher drives search-index updates for such changes, through the same path the app's
own writes use — otherwise a synced-in note would appear in the tree but be invisible to
search. This also covers editing a note in another editor or restoring one by hand.

Sync introduces a state the app has not had before: a file that is **correctly absent and
will arrive later**. This is never presented as broken or missing.

### Attachments

Binary attachments (PDFs, images, audio) sync between the machines like any other file. They
are still **never uploaded to Notion** — Notion receives text content, metadata, and
transcripts only, so a note read in Notion references attachments it cannot display.

> Current Notion plan is **free tier** (5MB/file upload cap). The local-only rule for Notion
> holds regardless of plan.

**Superseded:** attachments previously carried a **device tag** ("stored on: Desktop") because
they could only ever exist on one machine. With direct sync they exist on both, and the tag
answers a question that no longer arises. Ticket #6 is closed as superseded; what survives is
that an attachment which has not yet arrived reads as *not synced yet*, never as broken.

---

## 9. Project conventions

- **Repo**: public on GitHub from day one
- **License**: AGPL-3.0-or-later, inherited from HelixNotes. Any distributed modified
  version must remain open source. Upstream attribution must be preserved.
- **Upstream**: `upstream` remote points at `https://gitlab.com/ArkHost/HelixNotes.git`;
  upstream improvements can be merged in.
- **Verification**: the inherited `pnpm verify` runs typecheck, JS tests, Rust tests,
  clippy, and build. Keep it green.

---

## 10. Decisions considered and rejected

| Option | Why rejected |
| --- | --- |
| Electron | Heavier footprint; RAM matters with local AI |
| Native per-OS UIs | Two UIs to maintain solo |
| Green-field build | HelixNotes matches the target architecture closely enough to fork |
| Noteriv as fork base | MIT and on GitHub, but user preferred HelixNotes |
| Logseq / Joplin / AFFiNE | Electron-based, undermining the Tauri rationale |
| SiYuan | Block-database model, not plain-markdown-per-note |
| Foam | VS Code extension, not a standalone app |
| Peer-to-peer CRDT sync | Far larger than the rest of v1 combined. **Still rejected** — file-level sync is not CRDT sync (ADR-0002) |
| ~~External sync tool (Syncthing)~~ | ~~Superseded by the Notion-hub decision~~ — **reversed by ADR-0002.** The rejection was downstream of the Notion-hub choice, not independent of it. A sync engine is now bundled as an app-managed sidecar; the user still installs nothing |
| Writing the sync engine in this project | Tombstones, version vectors, and move detection, against a vault that relocates notes as routine — the reasoning that killed CRDTs (ADR-0002) |
| Notion as a third writer | Reintroduces three-way conflicts; Notion-created notes have no PARA category (ADR-0002) |
| AI auto-filing / auto-tagging | User wants manual control of organization |
| Passive related-notes sidebar | Superseded by prompt window + capture-time check |
| Browser extension for clipping | Doubles packaging/maintenance surface |
| Uploading attachments to Notion | 5MB free-tier cap; local-only is plan-independent |

---

## 11. Open engineering decisions

Carried forward into ticket breakdown — these are unresolved, not settled:

1. **Embedding model**: which model, and how are embeddings invalidated on note edit?
2. ~~**Global hotkey**: Linux support varies across X11/Wayland~~ — **resolved** by
   [ADR-0001](adr/0001-linux-global-shortcuts-via-xdg-portal.md) and ticket #4: Linux
   registers through the XDG GlobalShortcuts portal only, never an X11 grab. X11 is
   untested, not promised.
3. **Notion schema**: exact property mapping between frontmatter and Notion database
   properties, including the identity/mapping table that links a local file to a Notion
   page ID. Narrower since ADR-0002 — the mapping is needed to *update* pushed pages, not
   to reconcile edits made in Notion.
4. **Tailscale**: bundled guidance vs. assumed pre-installed by the user. **Load-bearing
   since ADR-0002** — it is now the transport for sync between machines, not only the route
   to the AI backend. Sync between machines is unavailable without it.
5. **Multi-vault support**: inherited, kept, and unused. See §12.
6. **Conflict-copy UX**: the bundled engine's conflict semantics are inherited (§8). How the
   paired versions are presented, and whether a merge is ever offered rather than a choice,
   is unresolved.

---

## 12. Vaults, and why they are not categories

A **vault** is the whole second brain: the root folder holding the entire note collection,
plus a `.helixnotes/` folder carrying that collection's trash, attachments, version history,
holding area, and shared settings. The four categories live *inside* one vault. The
hierarchy is **one vault → four categories → notes**.

Since [ADR-0002](adr/0002-machine-to-machine-sync-notion-becomes-read-only.md), `.helixnotes/`
holds **only what is safe to sync**. Machine-local state — the search index, relocation and
recovery manifests, sync bookkeeping, and machine-local settings — lives outside the vault, in
the per-machine config directory. That boundary is what lets the whole vault folder sync
without an ignore list (§8).

Only one vault is open at a time: `active_vault` is a single value, and opening a vault
replaces the active search index.

**Decided 2026-08-23: keep one vault, leave multi-vault support unused.** There will only
ever be one second brain, so the extra vaults are dead weight rather than a feature. They
are also harmless, so removing them is not urgent.

**Rejected: making each category its own vault.** It reads naturally — four categories,
four containers — but because only one vault is open at a time it would break features
this project has already committed to:

| Committed feature | What one-vault-per-category would do to it |
| --- | --- |
| Knowledge graph (§7) | Could only ever draw one category, so a Project linked to a Resource becomes unrepresentable — which is the point of the graph |
| Semantic search (§6) | Four separate indices; a question only searches the open category |
| Capture-time similarity | Could only compare against one category |
| Moving between categories | Becomes a cross-vault migration, losing index entry and version history |
| Holding area for uncategorised notes | Has no home, belonging to no category |

**If this is revisited**, the requirement to preserve is that search, the graph, and
similarity all see *every* note at once, regardless of category. Any change that scopes
them to one category at a time is the failure this rejection is about.
