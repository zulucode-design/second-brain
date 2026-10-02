#!/usr/bin/env bash
# Installs and removes Second Brain test builds on Nicolas's Fedora laptop without a password.
#
# scripts/install-linux-test-package.sh copies this file, root-owned, to
# /usr/local/libexec/second-brain-test-package and adds a sudoers rule that lets his account run
# exactly these two actions without a password:
#
#   sudo -n /usr/local/libexec/second-brain-test-package install-candidate /absolute/path/to.rpm
#   sudo -n /usr/local/libexec/second-brain-test-package remove-test-install
#
# Any process running as that account can call it, and an agent built the RPM, so
# check_candidate is the boundary: the package may only install plain root-owned files at the
# paths a Second Brain build ships, and may carry no scriptlets, triggers, or obsoletes. That stops
# a bad build from writing /etc or running code as root. It does not make the app itself safe to
# run; the app runs as the ordinary user.

set -euo pipefail
export PATH=/usr/sbin:/usr/bin
umask 077

# Returns 0 for an RPM this helper may install; otherwise prints the reason and returns 1.
check_candidate() {
  local rpm=$1 magic identity files mode user group caps path
  # Without this, rpm reads any other file as a manifest naming packages to open, as root.
  magic=$(od -An -tx1 -N4 -- "$rpm" | tr -d ' \n')
  if [[ $magic != edabeedb ]]; then
    echo "not an RPM package: $rpm" >&2
    return 1
  fi
  identity=$(rpm -qp --qf '%{NAME} %{ARCH}' -- "$rpm") || return 1
  if [[ $identity != 'second-brain x86_64' ]]; then
    echo "not a second-brain x86_64 package: $identity" >&2
    return 1
  fi
  if [[ -n $(rpm -qp --scripts --triggers --filetriggers -- "$rpm") ]]; then
    echo 'package has scriptlets or triggers' >&2
    return 1
  fi
  # rpm -U erases every installed package an Obsoletes names.
  if [[ -n $(rpm -qp --obsoletes -- "$rpm") ]]; then
    echo 'package obsoletes other packages' >&2
    return 1
  fi
  files=$(rpm -qp --qf '[%{FILEMODES:perms}\t%{FILEUSERNAME}\t%{FILEGROUPNAME}\t%{FILECAPS}\t%{FILENAMES}\n]' -- "$rpm") || return 1
  while IFS=$'\t' read -r mode user group caps path; do
    # A plain file, not setuid, setgid, or writable by anyone but root, and with no capabilities.
    if [[ ! $mode =~ ^-rw[-x]r-[-x]r-[-x]$ || $user != root || $group != root || ($caps != '(none)' && -n $caps) ]]; then
      echo "file not a plain root-owned file: $mode $user $group $caps $path" >&2
      return 1
    fi
    case $path in
      /usr/bin/second-brain | /usr/bin/syncthing) ;;
      '/usr/share/applications/Second Brain.desktop') ;;
      /usr/share/applications/io.github.zulucodedesign.SecondBrain.desktop) ;;
      *)
        if [[ ! $path =~ ^/usr/share/icons/hicolor/[0-9]+x[0-9]+(@2)?/apps/second-brain\.png$ ]]; then
          echo "file outside the Second Brain paths: $path" >&2
          return 1
        fi
        ;;
    esac
  done <<<"$files"
}

install_candidate() {
  local source=$1 stage
  if [[ $source != /*.rpm ]]; then
    echo "candidate must be an absolute path to an .rpm file: $source" >&2
    exit 2
  fi
  stage=$(mktemp -d -p /var/tmp second-brain-candidate.XXXXXX)
  trap 'rm -rf "$stage"' EXIT
  # Read as the calling user, so root never reads a file that user could not, and checked and
  # installed from a root-owned copy, so the caller cannot swap the package after the check.
  runuser -u "$SUDO_USER" -- cat -- "$source" >"$stage/candidate.rpm"
  check_candidate "$stage/candidate.rpm" || exit 1
  rpm -U --replacepkgs --oldpackage -- "$stage/candidate.rpm"
  rpm -q second-brain
}

main() {
  if [[ $EUID -ne 0 || -z ${SUDO_USER:-} ]]; then
    echo "run through sudo: sudo -n $0 install-candidate <rpm> | remove-test-install" >&2
    exit 1
  fi
  if [[ $# -eq 2 && $1 == install-candidate ]]; then
    install_candidate "$2"
  elif [[ $# -eq 1 && $1 == remove-test-install ]]; then
    rpm -e second-brain
  else
    echo "usage: $0 install-candidate <absolute rpm path> | remove-test-install" >&2
    exit 2
  fi
}

# Sourcing defines check_candidate without running anything, for the tests and CI.
if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
  main "$@"
fi
