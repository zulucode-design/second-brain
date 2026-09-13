# Fedora as the GitHub Actions environment

Date: 2026-09-13

## Question

Can the project replace Ubuntu with Fedora as its GitHub Actions environment now that only
Windows 11 and Fedora are supported user platforms?

## Findings

1. **Standard GitHub-hosted runners do not offer Fedora.** GitHub documents its standard
   hosted operating systems as Ubuntu, Windows, and macOS. Fedora is therefore unavailable as
   a direct `runs-on` image for this public repository.
   - Publisher: GitHub
   - Source: https://docs.github.com/en/actions/reference/runners/github-hosted-runners
   - Confidence: high

2. **Fedora is supported for self-hosted Actions runners.** GitHub lists Fedora 29 or later
   among supported self-hosted runner operating systems. A self-hosted runner would make the
   project responsible for machine availability, updates, isolation, and execution of
   untrusted workflow code.
   - Publisher: GitHub
   - Source: https://docs.github.com/en/actions/reference/runners/self-hosted-runners
   - Confidence: high

3. **A Fedora container can run on a standard Ubuntu-hosted job.** This keeps GitHub's
   disposable hosted worker while moving compilation and tests into a pinned Fedora userspace.
   It does not reproduce a complete installed Fedora desktop session, so package installation,
   portals, hotkeys, keyring, tray, and launch behavior still require verification on the real
   Fedora machine.
   - Source: inference from GitHub-hosted Linux/container execution and the project's packaged
     desktop requirements
   - Confidence: high for build/test use; medium for the exact workflow until implemented

## Recommendation

Use a pinned Fedora container on a standard GitHub-hosted Linux runner for Linux compilation,
tests, and RPM construction. Treat the Ubuntu host as CI infrastructure, not a supported
product platform. Do not maintain a self-hosted runner for the external alpha. Record manual
installation and desktop smoke evidence from the actual Fedora target in package H's
verification matrix.
