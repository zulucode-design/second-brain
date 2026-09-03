#!/usr/bin/env bash
# Prove that a finished package contains the desktop entry the portal resolves.
#
# Both formats, because both ship it and the two are configured separately in
# tauri.conf.json: a change that fixes one and misses the other looks fine until an install
# on the other distro reports only "App info not found". RPM is the shipping path for the
# project's own Linux machine (Fedora), so checking only the deb leaves the primary target
# unguarded.

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 path/to/package.{deb,rpm}" >&2
  exit 2
fi

# Absolute, because extraction runs from inside a temporary directory and a relative path
# would stop resolving the moment it does.
package="$(readlink -f "$1")"
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app_id="$(node -e "const c=require('$repo/src-tauri/tauri.conf.json');process.stdout.write(c.identifier)")"
source_entry="$repo/src-tauri/linux/$app_id.desktop"
installed_entry="usr/share/applications/$app_id.desktop"

if [[ ! -f "$package" ]]; then
  echo "package not found: $package" >&2
  exit 1
fi
if [[ ! -f "$source_entry" ]]; then
  echo "desktop entry does not match configured app id: $source_entry" >&2
  exit 1
fi

extract_dir="$(mktemp -d)"
trap 'rm -rf "$extract_dir"' EXIT

case "$package" in
  *.deb)
    data_member="$(ar t "$package" | sed -n '/^data\.tar\./{p;q;}')"
    case "$data_member" in
      data.tar.gz) ar p "$package" "$data_member" | tar -xzf - -C "$extract_dir" ;;
      data.tar.xz) ar p "$package" "$data_member" | tar -xJf - -C "$extract_dir" ;;
      data.tar.zst) ar p "$package" "$data_member" | tar --zstd -xf - -C "$extract_dir" ;;
      *)
        echo "unsupported or missing Debian data archive: ${data_member:-none}" >&2
        exit 1
        ;;
    esac
    ;;
  *.rpm)
    # rpm2cpio and cpio rather than `rpm -i --root`: no root, no rpmdb, no install scripts,
    # and the payload is all this needs to see.
    ( cd "$extract_dir" && rpm2cpio "$package" | cpio -idm --quiet )
    ;;
  *)
    echo "unsupported package type: $package (expected .deb or .rpm)" >&2
    exit 2
    ;;
esac

packaged_entry="$extract_dir/$installed_entry"
if [[ ! -f "$packaged_entry" ]]; then
  echo "package is missing $installed_entry" >&2
  exit 1
fi
if ! cmp --silent "$source_entry" "$packaged_entry"; then
  echo "packaged desktop entry differs from $source_entry" >&2
  diff --unified "$source_entry" "$packaged_entry" || true
  exit 1
fi

echo "verified $installed_entry in $(basename "$package")"
