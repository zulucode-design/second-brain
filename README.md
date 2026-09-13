# Second Brain

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](./LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%2011%20%7C%20Fedora%2044-lightgrey)]()

A personal knowledge management desktop app implementing Tiago Forte's
*Building a Second Brain* (BASB) methodology — PARA organization, frictionless capture,
and a local-AI-assisted knowledge graph.

Built with Tauri 2, SvelteKit, and Rust. Notes are plain Markdown files on your
filesystem. No lock-in.

> **Status: early development.** The design is settled and documented in
> [`docs/SPEC.md`](./docs/SPEC.md); implementation is in progress. The external alpha
> targets the safe Capture-and-Organize core. Q&A, capture similarity, semantic graph
> suggestions, and voice transcription remain planned for the v1 feature-complete beta.
> Second Brain has its own release line beginning at `0.1.0-alpha.1`; the inherited in-app
> updater is disabled until this project has a signed release channel.

## What it does

**PARA organization** — every note lives in exactly one of Projects, Areas, Resources, or
Archives. You choose the category at capture time; the AI never files anything for you.

**Frictionless capture** — a global hotkey opens a capture overlay from anywhere. External
alpha covers Markdown notes, web clippings (paste a URL), files, and PDFs. Voice recording
and transcription arrive in the v1 feature-complete beta.

**Knowledge graph** — a scrollable, zoomable map of the vault. External alpha shows the
links you made. AI-detected similarity edges and promotion to real links arrive in the v1
feature-complete beta.

**Local AI, no cloud** — external alpha uses Ollama for semantic search. Grounded note Q&A,
similarity workflows, and whisper.cpp transcription arrive in the v1 feature-complete beta.
A weaker second machine reaches the stronger one over a private Tailscale network. Ollama
has no authentication of its own, so the endpoint must never be bound to a public interface
or forwarded on a router. Tailscale is the security boundary. When AI is unreachable,
capture, editing, organization, and keyword search keep working.

**Sync between your machines** — Second Brain manages a bundled Syncthing sidecar that
syncs paired devices over your private Tailscale network, carrying notes and attachments
both ways. Tailscale must already be installed; no sync account or cloud storage is needed.
When the same note changes in two places, both versions are surfaced for you to choose
rather than silently overwritten.

Second Brain supports one logical vault replicated across Windows 11 x86-64 and Fedora 44
Workstation x86-64. Other desktop and mobile operating systems are not supported. A
10,000-note vault is the external-alpha tested baseline, not a hard limit.

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

## Logs

Every build writes a log, release included. If something silently does nothing — a global
hotkey that never fires is the usual one — the reason is here rather than on screen.

| Platform | Location |
| --- | --- |
| Windows | `%LOCALAPPDATA%\io.github.zulucodedesign.SecondBrain\logs` |
| Fedora | `~/.local/share/io.github.zulucodedesign.SecondBrain/logs` |

Files roll at 5 MB and the three most recent are kept.

## Credits and license

This project is a fork of **[HelixNotes](https://gitlab.com/ArkHost/HelixNotes)** by Yuri
Karamian, a local-first Markdown note-taking app. The upstream project provides the Tauri
shell, editor, vault handling, Tantivy search, graph renderer, and sync foundations that
this app builds on. Enormous credit to that work.

Licensed under **AGPL-3.0-or-later**, inherited from upstream. See [`LICENSE`](./LICENSE).
Modified versions that you distribute (including over a network) must also be released
under the AGPL.
