# Establish an independent Second Brain product identity

- **Status**: Accepted
- **Date**: 2026-09-13

Second Brain is the only user-facing product name. Its executable and distributable package
name become `second-brain`, its application identifier remains
`io.github.zulucodedesign.SecondBrain`, and its release line begins at `0.1.0-alpha.1`.
The stable identifier preserves this fork's configuration, data directories, keyring,
desktop integration, and Linux portal permissions while the executable rename removes
the inherited name from installation and runtime surfaces. Purely internal names may remain
when they neither affect users nor define package behavior.

HelixNotes is a separate application. Second Brain never imports or adopts a HelixNotes
installation's configuration or credentials automatically; a user may explicitly open a
portable Markdown vault. Migration is limited to mixed-name development artifacts that
already use the Second Brain identifier: preserve their data and keyring entries, replace
stale `helixnotes` launch, desktop-entry, and autostart references when found, and never
delete a vault. The first external-alpha package is the first supported installer baseline,
so unofficial development packages receive no broader upgrade guarantee.

The inherited `1.3.x` version line, `https://helixnotes.com/latest.json` endpoint, and
HelixNotes signing key are not Second Brain release infrastructure. The in-app updater stays
disabled until this project has its own GitHub release endpoint and signing key and verifies
a signed update between two Second Brain versions on supported Windows and Linux packages.
Useful upstream changes remain eligible for review and integration without carrying forward
the upstream product's version number.
