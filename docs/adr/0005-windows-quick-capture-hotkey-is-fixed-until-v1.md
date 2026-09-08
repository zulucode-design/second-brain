# ADR-0005: The Windows quick-capture hotkey is fixed until after v1

- **Status**: Accepted
- **Date**: 2026-09-08
- **Context**: Ticket #21, after [ADR-0003](./0003-windows-shortcut-capture-via-low-level-keyboard-hook.md) and [ADR-0004](./0004-windows-shortcut-is-typed-not-captured.md)

## Context

On Windows the app owns its hotkey through `RegisterHotKey`, so unlike Linux it *could* let
the user choose the combination. #21 built that: a key-capture field in Settings.

The field cannot see the combinations it most needs to. Once another process claims one,
Windows delivers it only to that process, so the field is never given the keystroke. Two
mechanisms were built to close that gap:

- **ADR-0003, a `WH_KEYBOARD_LL` hook.** Three rounds at a real desktop, a genuine defect
  fixed each round, and it never captured a single combination. Most sessions ended with
  `0 keys seen`.
- **ADR-0004, typing the combination instead.** Sound, reaches strictly more cases than the
  hook would have, and **never exercised on Windows** — written after the hook was removed
  and before the machine was next available.

That is two mechanisms for one setting, one measured not to work and one unverified, in a
feature whose actual purpose is *a hotkey that opens quick capture*. The hotkey itself works:
verified at the desktop on 2026-09-05 and again on 2026-09-07 — it fires the overlay, a note
saves to the chosen category, focus returns to the previous application.

## Decision

The Windows quick-capture hotkey is **`Ctrl+Alt+N`, fixed**. Choosing a different combination
from inside the app is removed until after v1.

- `hotkey::windows::configured_trigger` returns `PREFERRED_TRIGGER` — the same value Linux
  hints its compositor with, so both platforms behave identically on a first run.
- The `set_hotkey_trigger` command, its `api.ts` wrapper, the `hotkey_trigger` config field
  and the settings controls are all removed. Nothing writes a trigger, so nothing stores one.
- Settings shows the bound combination read-only, and says plainly that choosing your own is
  planned for after the first release.

Registration is untouched: it is still a real `RegisterHotKey` call that reports a genuine
conflict by name if something else already holds `Ctrl+Alt+N`. The user simply cannot pick a
different one yet.

### Why fixed rather than shipping the typed field

The typed field would probably work. But "probably" is the whole problem: this is the third
mechanism proposed for the same setting, the previous one consumed three rounds of
someone's time at a physical keyboard before being abandoned, and none of it can be verified
anywhere except that machine. Shipping an unverified third attempt into v1 to satisfy a
requirement no user has asked for is the wrong trade.

Fixing the combination removes the entire class of risk. What remains is the part that
demonstrably works.

### What is given up

`Ctrl+Alt+N` might be taken on someone's machine. If it is, the app says so by name and quick
capture still works from inside the app — the same position Linux is in when a compositor
declines the binding (ADR-0001), which is a state this project already considers acceptable
to ship.

#21's "a real conflict is reported by name" is still true, and still the thing this backend
can say that Linux cannot. What is deferred is *acting* on it by choosing another key.

## Alternatives considered

**Ship ADR-0004's typed field.** The status quo before this ADR. Rejected on the reasoning
above: unverified, and a third attempt at a setting that is not what the ticket is for.

**Make the trigger editable in the config file only.** A middle ground — no UI, but a power
user could change it. Rejected because a hand-edited value that fails to register produces
exactly the silent, unexplained dead hotkey this ticket has spent its whole life trying to
eliminate, and there would be no UI in which to explain it.

**Keep the capture field for unclaimed combinations only.** It does work for those. Rejected
because which combinations it can see is invisible to the user: it would work, then
inexplicably not, with the failure concentrated on exactly the combinations worth choosing
deliberately.

## Consequences

- One combination to support on Windows, and the only Windows-specific capture code left is
  the registration itself.
- `apply_trigger`'s replace-the-previous-trigger path is now unreachable, since it is only
  ever called with the same constant. Kept rather than deleted: the swap order is the subtle
  part, a rejected change got it wrong once already, and configurability is expected back.
  Its tests still run.
- Anyone whose machine already has `Ctrl+Alt+N` taken has no in-app recourse until this is
  revisited.
- **To revisit after v1**, start from ADR-0004 rather than ADR-0003. The hook is a dead end
  and ADR-0003 records why in detail.
