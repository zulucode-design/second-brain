# ADR-0004: A conflicting Windows shortcut is typed, not captured

- **Status**: Superseded by [ADR-0005](./0005-windows-quick-capture-hotkey-is-fixed-until-v1.md) before it shipped
- **Date**: 2026-09-07
- **Context**: Ticket #46, after [ADR-0003](./0003-windows-shortcut-capture-via-low-level-keyboard-hook.md) was built and removed

> **Superseded, 2026-09-08.** Typed entry was built but never exercised on Windows. Rather
> than carry a third capture mechanism into v1 on the strength of an untested path, the
> Windows hotkey was fixed to a single combination and configurability deferred until after
> the first release. The reasoning below still holds and is the starting point for whoever
> picks this up again.

## Context

The Windows settings panel offers a key-capture field: click it, press a combination, and it
is registered. That works for any combination nothing else has claimed, and it is what most
users will do.

It cannot work for the combinations most worth setting deliberately. Once another process
claims one through `RegisterHotKey`, Windows delivers it only to that process — never as an
ordinary keystroke to the focused window. So for exactly the conflicts #21 promises to report
by name, the field is never given the key, `invoke` is never called, and the backend is never
asked.

ADR-0003 chose a `WH_KEYBOARD_LL` hook to close that gap, since a low-level hook sits ahead
of hotkey dispatch and is the only mechanism that can see those keys. It was built and tested
across three rounds at a real desktop, fixing a genuine defect each round, and **it never
captured a single combination**. The full record is in that ADR.

## Decision

Keep the press-to-capture field, and add a text box next to it. The user types the
combination — `Ctrl+Alt+N` — and the backend answers with the real outcome.

Nothing about the backend changes. `set_hotkey_trigger` already parses the string, attempts
registration, and reports a conflict named by combination, refusing to persist a trigger that
did not register. That path was verified working on 2026-09-05, invoked directly against a
real `RegisterHotKey` lock. The only thing that was ever missing is a way for the user to
reach it.

### Why this is not a consolation prize

Typing reaches **strictly more** combinations than the hook was ever going to:

- A combination claimed by another process — the case the hook was built for.
- A combination claimed by an **elevated** process, which UIPI hides from a low-level hook
  installed by a normal-integrity one. ADR-0003 recorded this as a permanent limitation of
  its own approach. Typing has no such limit, because the combination never has to survive
  the keyboard at all.

It is also the only version whose correctness can be established away from the target
machine. The parsing and the conflict classification are covered by unit tests that run
anywhere; the hook's behaviour could only ever be observed on one desktop with one particular
set of software installed on it.

### Cost

Typing a shortcut is worse than pressing one. That is why it is the second option on the
panel rather than the only one: press works for the common case, and the box is there for the
case press structurally cannot reach, with the reason stated next to it rather than left for
the user to deduce from silence.

The press path also still needs its timeout — five seconds, then a message. It now points at
the text box instead of merely explaining the silence, which is the difference between naming
a dead end and offering the way through it.

## Alternatives considered

**Keep only press-to-capture, and accept the gap.** What the branch did before #46. The
message it shows is an apology, and #21's stated differentiator stays unreachable through any
route a user actually takes.

**Retry the hook.** Rejected on evidence rather than on principle: three rounds, three real
defects fixed, still `0 keys seen` in most sessions. Each round costs a person at a physical
keyboard, because none of it is observable from CI, from Linux, or over SSH — a low-level
hook needs an interactive desktop session, and the SSH link lands in session 0. The next
round would have had the same cost and no better reason to expect a different result.

**A dropdown of known-safe combinations.** Avoids both typing and hooks, but cannot express
what a given user's machine has free, which varies by whatever else they have installed —
precisely the thing this feature exists to discover.

## Consequences

- The panel has two controls where it had one. Worth it: they cover different, complementary
  sets of combinations, and neither covers both.
- Typos are possible in a way they are not when pressing. The backend rejects anything that
  does not parse and says so, naming the string it was given, so a typo is a message rather
  than a silently wrong setting.
- `#46`'s acceptance criterion — a user can reach the by-name conflict through normal use —
  is met by the typed path, not the pressed one.
- The hook code is removed rather than left disabled. It is recoverable from git history, and
  ADR-0003 records what it did, which is more useful than dead code carrying a warning.
