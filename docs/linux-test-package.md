# Passwordless test installs on the Fedora laptop

Agents (Claude, Codex, and the alpha-harness Actions job) install and remove Second Brain test
builds on Nicolas's Fedora laptop without him. Before this, every gate stopped twice for him to
type `sudo rpm`: once to install the candidate and once to remove it.

This page explains how agents use it, how much it lets them do, and what it does not protect.

## Using it

Two commands, and nothing else, run as root without a password:

```sh
sudo -n /usr/local/libexec/second-brain-test-package install-candidate /absolute/path/to/candidate.rpm
sudo -n /usr/local/libexec/second-brain-test-package remove-test-install
```

Always pass `-n`. If the rule is missing, sudo then fails at once instead of waiting for a
password that no one will type.

The harness calls these commands itself:

- **walkthrough** (Gate 3) and **acceptance** (#28) install `--fedora-rpm` before the run
  starts. After a passing run they remove it, then check that the package, `/usr/bin/second-brain`
  and the desktop entry are gone and that the run's vault is unchanged.
- **sync** (Gate 1) and **restore** (Gate 2) take no RPM. Install the candidate with the helper
  before running them, and remove it afterwards.
- A failed run, or a `walkthrough --machine windows` run, leaves the candidate installed for
  inspection. Remove it with `remove-test-install` once it is no longer needed.

The app, `tauri-driver`, and the harness all run as the ordinary user. Only installing and
removing the package runs as root.

## How it is set up

There are two pieces, both owned by root:

| Piece | Path | Source |
| --- | --- | --- |
| Helper | `/usr/local/libexec/second-brain-test-package` | copy of `scripts/linux-test-package.sh` |
| Sudoers rule | `/etc/sudoers.d/second-brain-test-package` | written by `scripts/install-linux-test-package.sh` |

The rule lets one account, the one that ran the setup, run the helper with exactly those
arguments:

```
nicolaszuloaga ALL=(root) NOPASSWD: /usr/local/libexec/second-brain-test-package install-candidate *, /usr/local/libexec/second-brain-test-package remove-test-install
```

The `*` only passes the RPM path through. The helper refuses any call that is not exactly one of
the two forms above.

Setting it up needs Nicolas's password once:

```sh
sudo scripts/install-linux-test-package.sh
```

The script checks the rule with `visudo` before it goes live, because a sudoers file that does
not parse disables sudo entirely.

The installed helper is a copy. Editing `scripts/linux-test-package.sh` changes nothing on the
laptop until the setup runs again, with his password. This is deliberate: if the repository
copy were live, anyone who could change the repository could change what runs as root.

To undo the setup:

```sh
sudo rm /etc/sudoers.d/second-brain-test-package /usr/local/libexec/second-brain-test-package
```

## What `install-candidate` checks

1. The path is absolute and ends in `.rpm`.
2. The file is read as the calling user, so root never reads a file that user could not. It is
   copied, up to 1 GiB, into a fresh root-only directory under `/var/tmp`. Every later check,
   and the install itself, uses that copy, so the caller cannot swap the package halfway.
3. The copy starts with the RPM magic bytes. Without this, `rpm` reads any other file as a
   manifest naming more files to open, as root.
4. The package is `second-brain` for `x86_64`.
5. It has no scriptlets of any kind, no triggers, no file triggers, no sysusers entries (which
   create users, groups, and group memberships), and no `Obsoletes` (which would erase other
   installed packages).
6. Every file in it is a plain root-owned file: not a directory, symlink, or device, not setuid
   or setgid, not writable by anyone but root, with no file capabilities, and at one of the
   paths a Second Brain build ships:
   - `/usr/bin/second-brain` and `/usr/bin/syncthing`
   - `/usr/share/applications/Second Brain.desktop` and
     `/usr/share/applications/io.github.zulucodedesign.SecondBrain.desktop`
   - `/usr/share/icons/hicolor/<N>x<N>[@2]/apps/second-brain.png`
7. It installs with `rpm -U --replacepkgs --oldpackage --nosysusers`.

`remove-test-install` runs `rpm -e second-brain` and nothing else.

`tests/linux-test-package.test.mjs` builds small packages that each break one of rules 3 to 6 and
checks that the helper refuses every one. CI runs the same check on every RPM it builds, so a packaging
change that adds a path, a scriptlet, or a dependency on account creation fails in CI before
any gate reaches the helper. When that happens on purpose, update the allowlist in
`scripts/linux-test-package.sh`, then run the setup again.

## What it reaches

What a caller can do as root through the helper:

- write the listed files under `/usr/bin`, `/usr/share/applications` and
  `/usr/share/icons/hicolor`;
- replace or remove the installed `second-brain` package;
- trigger the scripts that Fedora's own packages run when files land in those directories, such
  as the icon-cache and desktop-database updates.

What it cannot do through the helper:

- run a shell, `rpm`, or `dnf` with arguments of its own;
- run package scripts or triggers, or install anything outside the listed paths: no `/etc`, no
  systemd units, no sudoers files, no PAM or polkit configuration;
- create or change users and groups;
- install a setuid binary or one with file capabilities;
- remove any package except `second-brain`;
- change the helper or the rule, both of which are root-owned and outside the repository.

## What it does not protect

- **The app itself.** An installed build is still code that Nicolas's account runs, with his
  files, keyring, and network. A bad build cannot become root through the helper, but it can do
  anything his account can. Installing an agent-built package means trusting the agent that
  built it.
- **`/usr/bin/syncthing`.** The package ships this path, so a candidate can replace it.
  Anything on the laptop that runs `syncthing` from `PATH` runs the candidate's copy.
- **Who calls it.** Every process running as `nicolaszuloaga` can use the rule, not just agents:
  a browser exploit or a malicious npm install script could too. They would get the reach
  described above, and nothing more.
- **Unknown RPM features.** The checks cover everything RPM 6.0 (Fedora 44) writes during an
  install. A future RPM feature that writes outside the payload would need a new check. Rerun the
  tests when Fedora moves to a new RPM major version.

## Why this design

Sol compared graphical authentication (`pkexec`, `sudo -A`), rootless Podman, a disposable VM,
and a task-specific `NOPASSWD` rule. Only the last one meets both requirements: no one present,
and the native GNOME session that Gate 3 tests. A rule for unrestricted `rpm` or `dnf` would
have been simpler, but it would have handed root to anything running as this account. The helper
keeps the rule usable for its one job only.
