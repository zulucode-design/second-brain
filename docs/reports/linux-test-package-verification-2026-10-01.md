# Fedora test package helper: manual verification

Date: 2026-10-01. Machine: Nicolas's Fedora 44 laptop (rpm 6.0.2). Branch
`feat/linux-test-package-helper`. See [linux-test-package.md](../linux-test-package.md) for the
design.

The tests cover the package checks and the staging around the install with the privileged
commands stubbed. This record covers what only the live, root-owned helper can show.

## Setup

Nicolas first ran `sudo scripts/install-linux-test-package.sh` from the branch at 447eadd.

- `/usr/local/libexec/second-brain-test-package` is `root:root`, mode 755, and matched
  `scripts/linux-test-package.sh` at that commit.
- `sudo -n -l` lists exactly the two helper commands as `NOPASSWD`. Everything else still asks
  for a password.

## Refused calls

Each of these exited non-zero and changed nothing:

| Call | Result |
| --- | --- |
| `sudo -n /usr/bin/rpm -q second-brain` | `sudo: a password is required` |
| `sudo -n /bin/bash -c id` | `sudo: a password is required` |
| helper with no arguments | `sudo: a password is required` (not in the rule) |
| `remove-test-install extra` | `sudo: a password is required` (not in the rule) |
| `install-candidate` with no path | `sudo: a password is required` (not in the rule) |
| `install-candidate relative.rpm` | exit 2, `candidate must be an absolute path to an .rpm file` |
| `install-candidate /tmp/a.rpm /tmp/b.rpm` | exit 2, usage |
| `install-candidate /tmp/manifest-probe.rpm`, a text file naming `/etc/shadow` | exit 1, `not an RPM package` |
| `install-candidate /tmp/shadow-link.rpm`, a symlink to `/etc/shadow` | exit 1: `head` running as Nicolas cannot read it |
| `install-candidate /etc/shadow.rpm`, which does not exist | exit 1 |
| a package with the sysusers entry `u evil -` | exit 1, `package creates users or groups` |
| a package with a `%transfiletriggerpostun` | exit 1, `package has scriptlets or triggers` |

## Install and removal of a real candidate

The candidate was built from the branch the way CI builds it:
`pnpm exec tauri build --bundles rpm --ci --no-sign` with the Syncthing sidecar, giving
`Second Brain-0.1.0-alpha.1-1.x86_64.rpm`. Its path contains a space.

- The unprivileged `check_candidate`, as CI runs it, accepted it, and
  `scripts/verify-linux-package.sh` passed.
- `sudo -n … install-candidate "<path>"` installed it: `rpm -q` reported
  `second-brain-0.1.0-alpha.1-1.x86_64`, the installed header's `SHA256HEADER` matched the file,
  and `rpm -V second-brain` was clean.
- `sudo -n … remove-test-install` exited 0, and afterwards `rpm -q second-brain` reported it not
  installed. `/usr/bin/second-brain` and the desktop entry were both gone.

## Defect found and fixed

The live install above exited 1 after installing, with
`line 1: stage: unbound variable`. The helper's `EXIT` trap removes the staging directory, but it
ran after `install_candidate` had returned, when its `local stage` no longer existed. Under
`set -u` the trap failed, so a good install reported failure and left its staging copy in
`/var/tmp`. Every gate would have failed at its first install.

Commit 74c8718 keeps `stage` global and adds a test that stubs the privileged commands and
fails on the old code. Because the installed helper is a copy, the fix takes effect only after
the setup runs again.

Five root-only `second-brain-candidate.*` directories remain in `/var/tmp` from these runs.
They hold copies of the test probes and the candidate, readable only by root.
`systemd-tmpfiles` removes them after 30 days, or `sudo rm -rf /var/tmp/second-brain-candidate.*`
clears them now.

## Probe package installed by mistake

One of the review's probe packages, whose only file was a `%ghost` `/usr/bin/second-brain`, was
built to show that ghost files pass the check. During the refused-call runs it was sent
through the live helper. It passed, as designed, and replaced the previously installed test
build (`second-brain-0.1.0-alpha.1-1`, from an earlier candidate) with
`second-brain-1-1`. `remove-test-install` removed it, and the real candidate was installed and
removed afterwards as described above. The laptop was left with no `second-brain` package
installed.

## After the setup was rerun with 74c8718

Nicolas reran `sudo scripts/install-linux-test-package.sh` from the branch. The installed
helper then matched `scripts/linux-test-package.sh` byte for byte and was still `root:root`,
mode 755. With the same candidate RPM:

- `install-candidate` exited 0. The installed header matched the file, and `rpm -V second-brain`
  was clean.
- No new staging directory was left: `/var/tmp` still held only the five from the earlier runs.
- `remove-test-install` exited 0. `rpm -q second-brain` reported it not installed, and
  `/usr/bin/second-brain` and the desktop entry were gone.
- `install-candidate relative.rpm` was still refused with exit 2.

The laptop was left with no `second-brain` package installed.
