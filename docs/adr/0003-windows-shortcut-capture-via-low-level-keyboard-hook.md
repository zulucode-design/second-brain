# ADR-0003: Windows shortcut capture uses a low-level keyboard hook, not DOM key events

- **Status**: Proposed
- **Date**: 2026-09-07
- **Context**: Ticket #46, found while verifying #21 (Windows global hotkey backend)

## Context

ADR-0001 settled how the hotkey is *registered* on each platform. It said nothing about how
the settings UI *captures* the combination the user wants, because on Linux there is nothing
to capture — the compositor owns the binding and the app only hints at it. On Windows the app
owns the key, so #21 added a key-capture field, and that field listens for a DOM `keydown`.

That does not work for the case the field exists to handle.

Once any process claims a combination through `RegisterHotKey`, Windows stops delivering it
as an ordinary keystroke. It goes to the registering process as `WM_HOTKEY` and nowhere else
— not to the focused window, not to the webview, not to us. So for **exactly** the conflicts
worth reporting, the field is never given the keystroke, `invoke` is never called, and the
backend is never asked the question.

Measured at the Windows desktop 2026-09-05, against a real `RegisterHotKey` lock held from an
interactive session:

| | |
| --- | --- |
| Backend asked directly | Returns `availability: 'unavailable'`, `"The shortcut "Ctrl+Alt+P" is already used by another application…"`, naming the combination, and does not persist it |
| Same combination pressed into the capture field | Nothing. No event, no `invoke`, no message |

This is what #21 bills as its differentiator — "a real conflict is reported by name, the one
thing this backend can say that Linux structurally cannot". The backend can say it. The field
cannot ask.

It also invalidated four conflict tests in that session, each of which looked like a UI bug
and was not. ShadowPlay's Alt+Z opened its overlay while our field never saw the key, because
a `RegisterHotKey` claim and a low-level hook behave completely differently — which is the
first thing this ADR is here to record.

`8051508` mitigated the symptom: the field now stops after five seconds and says what the
silence means, instead of sitting on "Press a key combination…" forever looking like a hang.
That is a dead end naming itself, not a fix.

## Decision

Capture keys on Windows with a `WH_KEYBOARD_LL` hook owned by Rust, and feed the resulting
combination to the existing `set_hotkey_trigger` command. DOM `keydown` stops being the
capture mechanism on Windows; it remains untouched everywhere else.

A low-level keyboard hook runs in the raw input path, ahead of hotkey dispatch, so it sees
combinations that `RegisterHotKey` would otherwise consume. That is the whole reason to take
on a Win32 hook rather than keep a webview event handler: it is the only mechanism that
observes the keys this feature is about.

### Shape

A new `hotkey/key_capture.rs`, sitting beside `hotkey/windows.rs` rather than inside it —
registration and capture are different concerns that happen to share a platform. (`capture.rs`
under `hotkey/` is already taken by the note-capture flow, hence the longer name.)

- **A dedicated thread**, spawned when capture is armed and joined when it is disarmed. Low-
  level hooks are dispatched by the OS onto the thread that installed them, and that thread
  must be pumping messages, so this cannot live on a Tauri async task. The thread installs
  the hook, runs a `GetMessage` loop, and tears the hook down on the way out.
- **`SetWindowsHookExW(WH_KEYBOARD_LL, proc, NULL, 0)`.** Unlike most global hooks, a
  low-level one needs no injected DLL and takes a null module handle.
- **The callback classifies and forwards.** It reads the virtual key and the current modifier
  state, builds the same `Ctrl+Alt+N` string the existing parser already round-trips, and
  hands it to the Tauri side. It does no other work — see the timeout below.
- **Armed state is explicit and single.** Capture is either armed or not; arming twice is a
  no-op rather than a second hook.

### Swallow while armed, pass through otherwise

While armed, the callback returns non-zero for anything it treats as part of a combination,
so the key does not reach the application that would otherwise act on it. Pressing a
combination to *record* it must not also *fire* it — otherwise choosing ShadowPlay's Alt+Z
opens ShadowPlay's overlay over our settings window.

While not armed, the hook is not installed at all. This is deliberately not a
permanently-installed hook that decides per keystroke: the window in which we interfere with
the entire system's keyboard is the few seconds a user is actively pressing a shortcut into a
field, and nothing else.

### The safety rules, and why each one is here

A low-level hook is a global, system-wide interception point. A bad one does not degrade this
app — it degrades typing everywhere. These are the constraints the implementation is held to:

1. **Disarm on a timeout, always.** The existing five-second timer becomes the hook's
   lifetime, not just a UI message. A hook that outlives its window is the failure that
   breaks the machine.
2. **Disarm on window close, blur, and app exit.** Every path out of the settings panel
   uninstalls the hook, including the ones nobody thinks about.
3. **The callback must return fast.** Windows silently drops a hook whose callback exceeds
   `LowLevelHooksTimeout` (default 300 ms), and it does so without telling anyone — so the
   callback allocates nothing it can avoid, takes no lock that another thread holds while
   doing I/O, and never calls into Tauri synchronously. It posts and returns.
4. **Never swallow what must not be swallowed.** Ctrl+Alt+Del is a secure attention sequence
   and is not hookable at all, which is a guarantee rather than something to implement. The
   Windows key alone, and Escape, pass through so there is always a way out.
5. **Escape cancels**, matching the current field's behaviour.
6. **A failed `SetWindowsHookExW` is reported, not ignored.** If the hook cannot be installed,
   the field says so and falls back to the current DOM behaviour, which is worse but not
   broken. This is ADR-0001's own principle: fail loudly rather than appear to work.

### What this does not do

- **It does not change registration.** `RegisterHotKey`, via `tauri-plugin-global-shortcut`,
  is still how the app owns the key. The hook is for capture only. Replacing registration
  with a hook would mean holding a system-wide hook for the entire life of the app, which is
  a much larger claim on the machine than this feature justifies.
- **It does not apply to Linux or macOS.** Both are `#[cfg]`-gated out. On Linux the
  compositor owns the binding and there is nothing to capture (ADR-0001).
- **It does not see keys destined for elevated windows.** UIPI blocks a normal-integrity
  process from hooking a higher-integrity one. A combination claimed only by an elevated
  process will still not be captured, and that limitation stays.

## Alternatives considered

**Keep DOM `keydown` and the five-second explanation.** Costs nothing and ships today. It is
what is on the branch. But it permanently gives up the one capability that distinguishes this
backend, and the message it shows is an apology rather than an answer.

**Let the user type the combination as text.** No hook, no risk, and it reaches every
combination including elevated-process conflicts, because the backend is asked directly rather
than through the keyboard. Rejected as the primary mechanism because a shortcut picker that
demands you spell `Ctrl+Alt+N` correctly is a worse experience than every comparable app's.
Worth keeping in mind as a fallback for the UIPI case above.

**`RegisterHotKey`-probe each candidate.** Try to register what the user picked and report the
result — which is what already happens once a combination reaches the backend. This does not
help: the problem is getting the combination out of the keyboard in the first place.

**`RawInput` (`WM_INPUT`) instead of a hook.** Sees hardware key events without a hook's
global reach, and cannot be silently dropped on a timeout. Rejected because raw input is
delivered to a window that has registered for it and does not suppress the keystroke — so the
conflicting application still acts on the key while we record it, which rule 2 above exists to
prevent.

## Consequences

- The app installs a system-wide keyboard hook for a few seconds at a time, during an action
  the user explicitly started. Bugs in that window affect the whole machine, which is why the
  safety rules are listed as constraints rather than suggestions.
- `hotkey/key_capture.rs` is unit-testable only in its pure parts — virtual key to trigger
  string, and the armed-state machine. That the hook installs, fires, swallows and uninstalls
  is verifiable only by a person at a Windows desktop, like the rest of #21.
- Verification must state which mechanism holds any conflicting key. A `RegisterHotKey` claim
  and a low-level hook behave differently enough that a test which does not say which one it
  used has not reported anything. Four tests on 2026-09-05 were void for exactly this.
- If it proves unreliable in daily use, the fallback is the text-entry alternative above,
  which reaches strictly more combinations than the hook does.
