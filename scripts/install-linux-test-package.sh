#!/usr/bin/env bash
# One-time setup on the Fedora laptop, run as `sudo scripts/install-linux-test-package.sh`.
#
# Installs scripts/linux-test-package.sh as a root-owned helper and a sudoers rule that lets the
# calling account run its two actions without a password. The installed copy never follows the
# repository: rerun this after the helper changes. Undo with
#   sudo rm /etc/sudoers.d/second-brain-test-package /usr/local/libexec/second-brain-test-package

set -euo pipefail

helper=/usr/local/libexec/second-brain-test-package
rule=/etc/sudoers.d/second-brain-test-package

if [[ $EUID -ne 0 || -z ${SUDO_USER:-} || $SUDO_USER == root ]]; then
  echo "run from your own account as: sudo $0" >&2
  exit 1
fi
if [[ ! $SUDO_USER =~ ^[a-z_][a-z0-9_-]*$ ]]; then
  echo "unexpected user name: $SUDO_USER" >&2
  exit 1
fi

staged=$(mktemp)
trap 'rm -f "$staged"' EXIT
# The helper checks its own arguments; the * only lets the RPM path through to it.
printf '%s ALL=(root) NOPASSWD: %s install-candidate *, %s remove-test-install\n' \
  "$SUDO_USER" "$helper" "$helper" >"$staged"
# A sudoers file that does not parse disables sudo, so check it before it goes live.
visudo -cqf "$staged"

install -D -o root -g root -m 0755 "$(dirname "$(readlink -f "$0")")/linux-test-package.sh" "$helper"
install -o root -g root -m 0440 "$staged" "$rule"
visudo -cq
echo "installed $helper and $rule for $SUDO_USER"
