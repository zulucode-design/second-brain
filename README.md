# HelixNotes

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](https://codeberg.org/ArkHost/HelixNotes/src/branch/main/LICENSE)
[![Latest Release](https://img.shields.io/badge/release-v1.2.5-green)](https://codeberg.org/ArkHost/HelixNotes/releases)
[![Website](https://img.shields.io/badge/web-helixnotes.com-purple)](https://helixnotes.com)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Windows%20%7C%20macOS%20%7C%20Android-lightgrey)]()

A local markdown note-taking app built with Tauri, SvelteKit, and Rust.

Your notes are stored as standard Markdown files on your local filesystem.
No cloud, no lock-in.

## Download (v1.2.5)

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

[Download AppImage](https://download.helixnotes.com/releases/v1.2.5/HelixNotes_1.2.5_amd64.AppImage)

#### .deb (manual)

[Download .deb](https://codeberg.org/ArkHost/HelixNotes/releases/download/v1.2.5/HelixNotes_1.2.5_amd64.deb) (Ubuntu 22.04+)

### Windows

[Download Installer](https://codeberg.org/ArkHost/HelixNotes/releases/download/v1.2.5/HelixNotes_1.2.5_x64-setup.exe) (Windows 10/11)

### macOS

[Download .dmg](https://codeberg.org/ArkHost/HelixNotes/releases/download/v1.2.5/HelixNotes_1.2.5_x64.dmg) (Intel, runs on Apple Silicon via Rosetta)

### Android

[Download APK](https://codeberg.org/ArkHost/HelixNotes/releases/download/v1.2.5/HelixNotes_1.2.5_android.apk)

---

All releases: [codeberg.org/ArkHost/HelixNotes/releases](https://codeberg.org/ArkHost/HelixNotes/releases)

## Features

- Markdown editor with toolbar, slash commands, source mode, code highlighting
- `[[Wiki-links]]` and graph view
- Full-text search (Tantivy)
- Outline panel, daily notes with calendar view, tags, drag-and-drop
- Math (KaTeX), PDF preview, Obsidian import
- AI writing tools (Ollama / Anthropic / OpenAI)
- Version history with diffs, automatic backups
- Multi-window, file associations, focus mode, view mode
- Themes, accent colors, fonts
- Local files, no cloud

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
