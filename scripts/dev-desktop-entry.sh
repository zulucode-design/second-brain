#!/usr/bin/env bash
# Install a user-level desktop entry so the GlobalShortcuts portal will accept a dev build.
#
# Since xdg-desktop-portal 1.20 a non-sandboxed app must call Registry.Register(app_id)
# before any other portal call, and the portal only accepts an app id it can find an
# installed desktop entry for. Without this, a dev build cannot register the quick-capture
# hotkey at all. A user-level entry is enough; no system install is needed.
#
# Run once, and again whenever the repo moves or the binary is rebuilt somewhere new.

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# /usr/bin/python3 explicitly, not whatever python3 is on PATH: the check below needs GLib's
# gi bindings, which live in the system interpreter. A conda or venv python3 fails on
# "import gi" and the entry then goes unverified.
app_id="$(/usr/bin/python3 -c "import json;print(json.load(open('$repo/src-tauri/tauri.conf.json'))['identifier'])")"

# The binary name is the Cargo package name, and the WM class follows it. Both are read
# rather than repeated: hardcoding them here means a rename breaks the entry silently, and
# the portal's only report of that is "App info not found".
binary_name="$(sed -n 's/^name = "\(.*\)"/\1/p' "$repo/src-tauri/Cargo.toml" | head -1)"
binary="$repo/src-tauri/target/debug/$binary_name"
dest="${XDG_DATA_HOME:-$HOME/.local/share}/applications/$app_id.desktop"

# GLib parses Exec with shell rules and then requires argv[0] to name a program that
# exists. Fail either test and it refuses to load the entry at all, after which the portal
# reports only "App info not found" — which points nowhere near the real cause. Measured:
#
#   existing binary, no spaces .................. loads
#   non-existent binary ......................... refused
#   existing binary, quoted path with spaces .... loads
#   existing binary, unquoted path with spaces .. refused   (argv[0] truncated at the space)
#
# desktop-file-validate does NOT catch either case; it calls both files valid.
# Hence the quoting below, and this check.
if [[ ! -x "$binary" ]]; then
  echo "no binary at $binary" >&2
  echo "build first (cargo build --manifest-path src-tauri/Cargo.toml), then re-run" >&2
  exit 1
fi

mkdir -p "$(dirname "$dest")"
cat > "$dest" <<EOF
[Desktop Entry]
Type=Application
Name=Second Brain (dev)
Comment=Development build
Exec="$binary"
Icon=$repo/src-tauri/icons/128x128.png
Terminal=false
Categories=Office;
StartupWMClass=$binary_name
EOF

chmod 644 "$dest"
command -v update-desktop-database >/dev/null && update-desktop-database "$(dirname "$dest")" || true

echo "wrote $dest"
echo "app id: $app_id"

# Prove the entry resolves. A written file is not necessarily a found file.
if /usr/bin/python3 -c "
import gi, sys
gi.require_version('Gio', '2.0')
from gi.repository import Gio
sys.exit(0 if Gio.DesktopAppInfo.new('$app_id.desktop') else 1)
" 2>/dev/null; then
  echo "resolves: yes"
else
  echo "resolves: NO — the portal will reject this app id" >&2
  exit 1
fi
