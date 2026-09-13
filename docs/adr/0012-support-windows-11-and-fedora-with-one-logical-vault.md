# Support Windows 11 and Fedora with one logical vault

- **Status**: Accepted
- **Date**: 2026-09-13

Second Brain supports Windows 11 x86-64 and Fedora 44 Workstation x86-64. A newer Fedora
release joins the support contract only after its package and verification matrix pass.
Linux CI runs inside a pinned Fedora container; the GitHub-hosted Ubuntu machine underneath
is infrastructure, not a supported user platform. macOS, Android, iOS, and Ubuntu are not
supported. Their inherited dependencies, packaging, capabilities, runtime branches, and
platform-specific tests are removed. Responsive narrow-window behavior remains under the
platform-neutral term **compact layout**.

Each installation configures one logical vault. The Windows and Fedora machines hold
synchronized replicas of that same vault; PARA categories organize its contents. When an
inherited configuration contains several vault entries, migration retains the active entry
and drops the other configuration references without modifying or deleting any referenced
directory. There is no permanent multi-vault switcher or recovery-list UI.

External alpha supports vaults on local filesystems and directly attached drives. Putting a
vault inside another synchronization product's managed folder is unsupported because the
bundled Syncthing sidecar owns machine-to-machine synchronization. NAS vaults are deferred
for post-v1 investigation rather than rejected. A 10,000-note fixture is the external-alpha
tested baseline, not a hard limit; larger vaults remain openable and the verified scale may
increase after measurement.
