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
# paths a Second Brain build ships, and may carry no scriptlets, triggers, sysusers, or
# obsoletes. That stops a bad build from writing /etc or running code as root. It does not make
# the app itself safe to run; the app runs as the ordinary user.

set -euo pipefail
export PATH=/usr/sbin:/usr/bin
umask 077

# Returns 0 for an RPM this helper may install; otherwise prints the reason and returns 1.
check_candidate() {
  local rpm=$1 magic identity scripts sysusers obsoletes files mode user group caps path
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
  scripts=$(rpm -qp --scripts --triggers --filetriggers -- "$rpm") || return 1
  if [[ -n $scripts ]]; then
    echo 'package has scriptlets or triggers' >&2
    return 1
  fi
  # rpm -U creates these users and groups, and group memberships, in /etc as root.
  sysusers=$(rpm -qp --qf '[%{SYSUSERS}\n]' -- "$rpm") || return 1
  if [[ -n $sysusers ]]; then
    echo 'package creates users or groups' >&2
    return 1
  fi
  # rpm -U erases every installed package an Obsoletes names.
  obsoletes=$(rpm -qp --obsoletes -- "$rpm") || return 1
  if [[ -n $obsoletes ]]; then
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
  local source=$1 stage size
  if [[ $source != /*.rpm ]]; then
    echo "candidate must be an absolute path to an .rpm file: $source" >&2
    exit 2
  fi
  stage=$(mktemp -d -p /var/tmp second-brain-candidate.XXXXXX)
  trap 'rm -rf "$stage"' EXIT
  # Read as the calling user, so root never reads a file that user could not, and checked and
  # installed from a root-owned copy, so the caller cannot swap the package after the check.
  # Capped, so a path to /dev/zero cannot fill the disk; a real candidate is about 30 MiB.
  runuser -u "$SUDO_USER" -- head -c 1073741825 -- "$source" >"$stage/candidate.rpm"
  size=$(stat -c %s "$stage/candidate.rpm")
  if ((size > 1073741824)); then
    echo "candidate is larger than 1 GiB: $source" >&2
    exit 2
  fi
  check_candidate "$stage/candidate.rpm" || exit 1
  # --nosysusers as well as the check above: nothing in the package may change accounts.
  rpm -U --replacepkgs --oldpackage --nosysusers -- "$stage/candidate.rpm"
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
