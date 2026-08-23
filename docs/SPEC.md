# Second Brain — Specification

Status: **v1 design agreed, pre-implementation**
Last updated: 2026-08-23

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
- **Index**: a derived, rebuildable index for full-text search (Tantivy, inherited) and
  vector embeddings for semantic search (to be added).
- **Metadata**: YAML frontmatter (`vault/frontmatter.rs` already supports this) — the PARA
  category lives here.

> **Open engineering decision**: HelixNotes uses Tantivy, not SQLite. We must decide
> whether to keep Tantivy for full-text and add a separate embeddings store, or consolidate
> onto SQLite (FTS5 + vector extension). Do not treat "markdown + SQLite" from early
> planning as settled — the fork changed this premise.

### Version history / backup

HelixNotes ships per-note version history (`history.rs`) and backup (`backup.rs`).

> **Open engineering decision**: evaluate whether the inherited version history already
> satisfies the local-backup requirement, or whether to add git auto-commit-on-save on top.
> Do not build git auto-commit before confirming the existing feature is insufficient.

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
- **whisper.cpp**, bundled, provides audio transcription. (Ollama has no native
  speech-to-text — verified; a separate engine is mandatory.)
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
| Notion sync | Transcription; AI-similarity edges |

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

### Topology

Notion is the sync hub: **Desktop ⟷ Notion ⟷ Laptop**. Not peer-to-peer. This deliberately
offloads most of the distributed-sync problem onto Notion rather than building CRDT-based
multi-writer merging.

### Notion mapping

**Four separate Notion databases**, one per PARA category. A note moving between categories
means moving between databases — the sync layer must handle this.

### Transport

- **Polling every ~10 minutes** for v1. Notion webhooks require a public HTTPS endpoint,
  which is unavailable (no hosting).
- Notion API rate limit is ~3 requests/second — sync must be queued and metered.
- Two-way: a note started locally can be continued in Notion and vice versa.

> **Required future milestone (not optional polish)**: real-time sync via Notion webhooks
> plus a tunnel/relay (e.g. Cloudflare Tunnel). Explicitly logged as a must-have for a
> later version.

### Offline-first

The app is fully usable with zero internet. Local storage is the working copy; sync is
opportunistic background reconciliation when connectivity returns.

### Conflicts

When a note is edited both locally and in Notion before either side observed the other's
change, the app **surfaces both versions and lets the user choose**. No silent
last-write-wins overwrite.

### Attachments

Binary attachments (PDFs, images, audio) are **local-only** and never uploaded to Notion.
Notion receives text content, metadata, and transcripts only.

Each attachment carries a **device tag** indicating which machine physically holds the file
("stored on: Desktop" / "stored on: Laptop"), so a reference viewed from the other machine
or from Notion is unambiguous.

> Current Notion plan is **free tier** (5MB/file upload cap). The local-only attachment rule
> holds regardless of plan. Revisit if upgrading to Notion Plus.

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
| Peer-to-peer CRDT sync | Far larger than the rest of v1 combined |
| External sync tool (Syncthing) | Superseded by the Notion-hub decision |
| AI auto-filing / auto-tagging | User wants manual control of organization |
| Passive related-notes sidebar | Superseded by prompt window + capture-time check |
| Browser extension for clipping | Doubles packaging/maintenance surface |
| Uploading attachments to Notion | 5MB free-tier cap; local-only is plan-independent |

---

## 11. Open engineering decisions

Carried forward into ticket breakdown — these are unresolved, not settled:

1. **Search index**: keep Tantivy + separate vector store, or consolidate onto SQLite?
2. **Version history**: is inherited `history.rs` sufficient, or add git auto-commit?
3. **Embedding model**: which model, and how are embeddings invalidated on note edit?
4. **Global hotkey**: Linux support varies across X11/Wayland — needs verification.
5. **Notion schema**: exact property mapping between frontmatter and Notion database
   properties, including the identity/mapping table that links a local file to a Notion
   page ID.
6. **Tailscale**: bundled guidance vs. assumed pre-installed by the user.
