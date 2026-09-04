# Second Brain

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](./LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-lightgrey)]()

A personal knowledge management desktop app implementing Tiago Forte's
*Building a Second Brain* (BASB) methodology — PARA organization, frictionless capture,
and a local-AI-assisted knowledge graph.

Built with Tauri 2, SvelteKit, and Rust. Notes are plain Markdown files on your
filesystem. No lock-in.

> **Status: early development.** The design is settled and documented in
> [`docs/SPEC.md`](./docs/SPEC.md); implementation is in progress.

## What it does

**PARA organization** — every note lives in exactly one of Projects, Areas, Resources, or
Archives. You choose the category at capture time; the AI never files anything for you.

**Frictionless capture** — a global hotkey opens a capture overlay from anywhere. Markdown
notes, web clippings (paste a URL), files and PDFs, and voice memos with local
transcription.

**Knowledge graph** — a scrollable, zoomable map of the vault. Solid edges are links you
made; dashed edges are AI-detected semantic similarity, which you can promote to real links.

**Local AI, no cloud** — Ollama and whisper.cpp run on your own hardware for semantic
search, note Q&A, transcription, and similarity detection. A weaker second machine reaches
the stronger one over a private Tailscale network. Ollama has no authentication of its own,
so the endpoint must never be bound to a public interface or forwarded on a router —
Tailscale is the security boundary. When AI is unreachable, everything else keeps working.

**Sync between your machines** — your devices sync directly to each other over the same
private Tailscale network, carrying notes and attachments both ways. No account, no cloud
service, and no internet required. When the same note changes in two places, both versions
are surfaced for you to choose rather than silently overwritten.

**Notion for reading anywhere** — optionally publish your notes to Notion so they are
readable from a phone or browser when neither machine is to hand. This is a read-only view:
notes flow out, nothing flows back.

## Development

Requires Rust (MSRV 1.88), Node, and pnpm.

```bash
pnpm install
pnpm tauri:dev     # run the app
pnpm verify        # typecheck, tests, clippy, build
```

Linux builds also need the usual Tauri system dependencies (WebKitGTK and friends) — see
the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

## Credits and license

This project is a fork of **[HelixNotes](https://gitlab.com/ArkHost/HelixNotes)** by Yuri
Karamian, a local-first Markdown note-taking app. The upstream project provides the Tauri
shell, editor, vault handling, Tantivy search, graph renderer, and sync foundations that
this app builds on. Enormous credit to that work.

Licensed under **AGPL-3.0-or-later**, inherited from upstream. See [`LICENSE`](./LICENSE).
Modified versions that you distribute (including over a network) must also be released
under the AGPL.
