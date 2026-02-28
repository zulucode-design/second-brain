# HelixNotes

A local markdown note-taking app built with Tauri, SvelteKit, and Rust.

Your notes are stored as standard Markdown files on your local filesystem.
No cloud, no lock-in.

## Download (v1.2.1)

### Linux

#### Arch / Manjaro (AUR)

```bash
yay -S helixnotes-appimage-bin
```

#### Debian / Ubuntu / Mint (APT)

```bash
curl -fsSL https://repo.arkhost.com/gpg.key | sudo gpg --dearmor -o /usr/share/keyrings/arkhost.gpg && echo "deb [signed-by=/usr/share/keyrings/arkhost.gpg arch=amd64] https://repo.arkhost.com stable main" | sudo tee /etc/apt/sources.list.d/helixnotes.list && sudo apt update && sudo apt install helix-notes
```

#### AppImage (Arch, Fedora 43+, openSUSE Tumbleweed)

[Download AppImage](https://download.helixnotes.com/releases/v1.2.1/HelixNotes_1.2.1_amd64.AppImage)

#### .deb (manual)

[Download .deb](https://codeberg.org/ArkHost/HelixNotes/releases/download/v1.2.1/HelixNotes_1.2.1_amd64.deb) — Ubuntu 22.04+

### Windows

[Download Installer](https://codeberg.org/ArkHost/HelixNotes/releases/download/v1.2.1/HelixNotes_1.2.1_x64-setup.exe) — Windows 10/11

### macOS

[Download .dmg](https://codeberg.org/ArkHost/HelixNotes/releases/download/v1.2.1/HelixNotes_1.2.1_x64.dmg) — macOS (Intel, runs on Apple Silicon via Rosetta)

### Android

[Download APK](https://download.helixnotes.com/releases/v1.2.1/HelixNotes_1.2.1_android.apk) — Tested on Android 16

---

All releases: [codeberg.org/ArkHost/HelixNotes/releases](https://codeberg.org/ArkHost/HelixNotes/releases)

## Features

- **Markdown editor** with rich formatting toolbar, slash commands, source mode toggle, and code syntax highlighting
- **Wiki-links** — link notes with `[[Note Title]]` syntax
- **Graph view** — visualize connections between your notes
- **Outline panel** — heading navigation for long notes
- **Full-text search** powered by Tantivy
- **Math support** — KaTeX rendering for inline and block equations
- **Daily notes** — one-click button to create or open today's note
- **AI writing tools** — improve, summarize, translate, and more (Ollama / Anthropic / OpenAI)
- **Version history** — per-note snapshots with diff view
- **Backups** — automatic zip-based vault backups
- **PDF preview** — inline rendering of embedded PDFs
- **Tag management** — organize notes with tags, bulk edit from context menu
- **Drag-and-drop** — move notes between notebooks, reorganize notebooks by dragging
- **Obsidian import** — convert Obsidian wiki-links to standard markdown
- **Multi-window** — open notes in separate windows (right-click → "Open in New Window")
- **File associations** — open .md files from your file manager directly in HelixNotes
- **Themes** — light/dark mode with customizable accent colors, fonts, and line height
- **Focus mode** — distraction-free writing
- **View mode** — read-only toggle for distraction-free reading
- **Local** — everything stays on your machine

Full documentation: [helixnotes.com/docs](https://helixnotes.com/docs.html)

## Tech Stack

- **Frontend**: SvelteKit (Svelte 5) + TailwindCSS v4 + TipTap v3
- **Backend**: Rust (Tauri 2.0) + Tantivy (search) + Notify (file watcher)
- **Platforms**: Linux (AppImage), Windows, macOS, Android

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

## Screenshots

![Editor](https://cdn.helixnotes.com/assets/screenshots/screenshot-1.png)
![Settings](https://cdn.helixnotes.com/assets/screenshots/screenshot-2.png)
![Graph View](https://cdn.helixnotes.com/assets/screenshots/screenshot-3.png)
![AI Actions](https://cdn.helixnotes.com/assets/screenshots/screenshot-4.png)

## License

[AGPL-3.0](https://codeberg.org/ArkHost/HelixNotes/src/branch/main/LICENSE)
