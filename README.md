# HelixNotes

A local-first markdown note-taking app built with Tauri, SvelteKit, and Rust.

Your notes are stored as standard Markdown files on your local filesystem. No cloud, no lock-in.

## Download

| Platform | Download | Notes |
|----------|----------|-------|
| Linux (Arch/rolling) | [HelixNotes_1.0.6_amd64.AppImage](https://codeberg.org/ArkHost/HelixNotes/releases/download/v1.0.6/HelixNotes_1.0.6_amd64.AppImage) | Best for Arch, Fedora, openSUSE |
| Linux (Debian/Ubuntu/Mint) | [HelixNotes_1.0.6_amd64.deb](https://codeberg.org/ArkHost/HelixNotes/releases/download/v1.0.6/HelixNotes_1.0.6_amd64.deb) | Ubuntu 22.04+ |
| Windows | [HelixNotes_1.0.6_x64-setup.exe](https://codeberg.org/ArkHost/HelixNotes/releases/download/v1.0.6/HelixNotes_1.0.6_x64-setup.exe) | Windows 10/11 |
| macOS | Coming soon | |

See all releases: [codeberg.org/ArkHost/HelixNotes/releases](https://codeberg.org/ArkHost/HelixNotes/releases)

## Features

- **Markdown editor** with rich formatting toolbar, slash commands, and source mode
- **Wiki-links** — link notes with `[[Note Title]]` syntax
- **Graph view** — visualize connections between your notes
- **Full-text search** powered by Tantivy
- **AI writing tools** — improve, summarize, translate, and more (Anthropic / OpenAI)
- **Version history** — per-note snapshots with diff view
- **Backups** — automatic zip-based vault backups
- **PDF preview** — inline rendering of embedded PDFs
- **Obsidian import** — convert Obsidian wiki-links to standard markdown
- **Themes** — light/dark mode with customizable accent colors and fonts
- **Focus mode** — distraction-free writing
- **Local-first** — everything stays on your machine

## Tech Stack

- **Frontend**: SvelteKit (Svelte 5) + TailwindCSS v4 + TipTap v3
- **Backend**: Rust (Tauri 2.0) + Tantivy (search) + Notify (file watcher)
- **Platforms**: Linux (AppImage), Windows, macOS

## Building from Source

### Prerequisites

- [Rust](https://rustup.rs/) (1.77+)
- [Node.js](https://nodejs.org/) (18+)
- [pnpm](https://pnpm.io/)
- System dependencies for Tauri: see [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

### Development

```bash
pnpm install
pnpm tauri dev
```

### Production Build

```bash
pnpm tauri build
```

## License

[AGPL-3.0](LICENSE)
