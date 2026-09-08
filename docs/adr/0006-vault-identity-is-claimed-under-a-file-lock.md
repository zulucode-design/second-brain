# ADR-0006: The vault identity is claimed under a file lock

- **Status**: Accepted
- **Date**: 2026-09-08
- **Context**: Ticket #5 follow-up; the flaky crash test from the #27 / #33 relocation work
- **Supersedes**: the in-code decision in `machine_local.rs` that a `create_new` claim is the
  whole claim, and the comment justifying why a stalled claim is never taken over

## Context

Every vault has an identity file, `<vault>/.helixnotes/vault_id`. Machine-local state — the
search index, relocation manifests, per-machine bookkeeping (§12) — is keyed by it, so it
survives the vault folder being moved or renamed.

It was claimed with `create_new`: exactly one process creates the file, everyone else adopts
what that process wrote. The comment was explicit that this made the write "a single atomic
claim, so two processes opening the same vault at once cannot end up with two different ids."

Creating a file and writing it are two steps. Between them the file exists and is empty.

A process killed in that window leaves it empty forever. Every later open then read an empty
file, concluded a claim was in flight, polled for 500 ms, and gave up. **The vault became
unopenable permanently** — not for one run, but for every run afterwards, because nothing
ever filled the file.

The code knew about the window and chose to wait rather than take over, for a good reason:

> An empty file is a claim in flight, and waiting is the only safe response. A writer stalled
> on a slow mount is indistinguishable from a dead one, so replacing the file on a timeout
> would let two processes key the same vault to two directories.

That reasoning is correct. It is also why the bug survived: the failure mode is the *safe*
branch of a deliberate trade.

This surfaced as `killing_the_app_mid_notebook_move_still_recovers` failing about one run in
five. That test spawns 40 child processes and kills each at 0–78 ms, so it lands in the
window regularly. It looked like a flaky test. It was a real defect, reported honestly.

## Decision

**Keep `create_new` to decide who creates the file, and add an exclusive lock for everything
after that.**

The creator holds the lock across its write. Any other process that finds the file empty
attempts the lock:

- **The lock is held** — a writer is alive, so waiting is right, exactly as before.
- **The lock is free** — the kernel released it when its holder died, so the claim is
  abandoned rather than in flight, and may be taken over.

This removes the ambiguity the old comment names instead of overruling it. A lock is released
by the kernel on process death, which is precisely the question "is that writer still alive?"
that a timeout cannot answer.

Ordering matters and is load-bearing: `create_new` is attempted **first**, before anything
touches the lock. It answers, without needing a lock to work at all, the one question a
filesystem that cannot lock can still settle — has anyone started a claim here? Only the
creator may write to a file no one else can be holding. An earlier draft of this change
created the marker before testing lockability, which on a lockless filesystem left every
process staring at an empty file none of them was allowed to fill: the original bug,
reintroduced for those filesystems. That draft was caught in review.

`fs4` provides the lock. It is already in the dependency tree via Tantivy, so making it a
direct dependency adds no crates to the build.

## Consequences

- A crash mid-claim costs one run, not the vault.
- The one-vault-one-id guarantee is preserved. Beyond the creator, nothing writes without the
  lock, and even the creator re-reads under the lock before writing, so a process that claimed
  the file in the gap is adopted rather than overwritten.
- **Filesystems that cannot lock** fall back to the previous behaviour rather than refusing to
  open the vault. `flock` over NFS is historically unreliable; where it is a no-op, an empty
  marker is waited out and the open fails, as it did before. No worse, not better.
- **Read-only vaults still open.** Claiming needs write access, so an id already recorded is
  read without writing. Without this, a vault on a read-only mount would have stopped opening
  entirely — a regression this change was one review away from shipping.
- Evidence: 55 consecutive full-suite runs with no failure, 8 under saturated CPU, against 2
  failures in 9 before. Detail, including the alternatives rejected, is in
  `docs/reports/ticket-5-clipping-verification-2026-09-08.html`.

## Alternatives rejected

| Option | Why not |
| --- | --- |
| Put the id in the filename (`vault-id-<uuid>`) | Removes the empty window but not the race: two processes create two different names and both succeed, so uniqueness weakens from guaranteed to eventually converged, and a process could switch ids after building an index keyed to the first. Needs an on-disk format change and a migration. |
| Age heuristic — "empty for N seconds means abandoned" | Does not fix the observed failure at all. Recovery happens immediately after the crash, so the file is always fresh. Looks reasonable, achieves nothing. |
| Delete the empty file and re-claim | Clean unless a live writer still holds the descriptor — the exact case the wait could not rule out, which is the ambiguity this decision exists to remove. |
| `hard_link` for atomic no-clobber creation | Already rejected in-code for FAT/USB portability, and the marker lives inside the vault. `flock` is VFS-level on Linux, so that constraint does not block the chosen option. |

## Note on the remaining flake

The crash test ends with two sampling assertions, `interrupted > 0` and `restored > 0`, which
depend on a kill landing inside a narrow window across the 40 delays. It can therefore fail
with nothing wrong in the vault. That is a defect in the test rather than the code under it,
it is untouched by this ADR, and no number of passing runs retires it.
