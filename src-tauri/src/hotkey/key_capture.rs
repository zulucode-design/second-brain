//! Reading a shortcut out of the keyboard on Windows, for the settings field to save.
//!
//! See ADR-0003. The short version: once any process claims a combination through
//! `RegisterHotKey`, Windows delivers it only to that process as `WM_HOTKEY` — never as an
//! ordinary keystroke to the focused window. The settings field listened for a DOM `keydown`,
//! so for exactly the conflicts worth reporting it was never given the key, and the backend's
//! own "already used by another application" message could not be reached.
//!
//! A `WH_KEYBOARD_LL` hook runs in the raw input path, ahead of hotkey dispatch, so it sees
//! those keys. It is the only mechanism that does, which is the whole reason for taking on a
//! Win32 hook rather than keeping a webview event handler.
//!
//! The hook is installed only while the user is actively pressing a shortcut into the field,
//! and torn down on every path out — a captured combination, Escape, the panel closing, or
//! the caller's timeout. It is deliberately not a hook the app holds for its lifetime.

use std::sync::mpsc;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostQuitMessage, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

/// Emitted once per capture attempt, whatever its outcome, so the field never waits forever.
pub const CAPTURE_EVENT: &str = "hotkey-capture";

/// The result of one capture attempt. `trigger: None` means it ended without one — Escape,
/// or the caller cancelling — which the field shows as "nothing captured" rather than an error.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureOutcome {
    pub trigger: Option<String>,
}

/// The capture thread's id while one is armed. `None` means no hook is installed.
///
/// The id rather than a `JoinHandle` because the only thing another thread ever needs to do
/// is post `WM_QUIT` at it; the thread owns its own hook and cleans up after itself.
static CAPTURE_THREAD: Mutex<Option<u32>> = Mutex::new(None);

thread_local! {
    /// Set by the capture thread before its message loop, read by the hook callback, which
    /// the OS calls on that same thread. Not shared, so no lock is taken in the callback —
    /// see the timeout note on [`hook_proc`].
    static CAPTURED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Install the hook and start listening. Idempotent: arming while already armed is a no-op
/// rather than a second hook.
pub fn arm(app: AppHandle) -> Result<(), String> {
    let mut armed = CAPTURE_THREAD
        .lock()
        .map_err(|_| "the shortcut capture lock was poisoned".to_string())?;
    if armed.is_some() {
        return Ok(());
    }

    let (ready, started) = mpsc::channel::<Result<u32, String>>();
    std::thread::Builder::new()
        .name("hotkey-capture".to_string())
        .spawn(move || capture_thread(app, ready))
        .map_err(|error| format!("could not start the shortcut capture thread: {error}"))?;

    match started.recv() {
        Ok(Ok(thread_id)) => {
            *armed = Some(thread_id);
            log::info!("Shortcut capture armed");
            Ok(())
        }
        Ok(Err(detail)) => Err(detail),
        Err(_) => Err("the shortcut capture thread stopped before it was ready".to_string()),
    }
}

/// Tear the hook down. Safe to call when nothing is armed, which is what every "on close"
/// path does — the point is that no route out of the settings panel leaves a hook installed.
pub fn disarm() {
    let Ok(mut armed) = CAPTURE_THREAD.lock() else {
        return;
    };
    if let Some(thread_id) = armed.take() {
        // Ends the thread's GetMessageW loop; it unhooks and emits on its way out.
        let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}

/// Owns the hook for its whole lifetime: installs it, pumps messages so the OS can call it,
/// and unhooks before reporting. The hook must be installed and removed on the same thread,
/// and that thread has to be pumping messages, which is why this is a bare thread rather
/// than a Tauri async task.
fn capture_thread(app: AppHandle, ready: mpsc::Sender<Result<u32, String>>) {
    let hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) } {
        Ok(hook) => hook,
        Err(error) => {
            // Reported rather than swallowed: a capture field that silently never responds
            // is the "appears to work" failure ADR-0001 exists to prevent.
            let _ = ready.send(Err(format!(
                "the keyboard hook could not be installed: {error}"
            )));
            return;
        }
    };

    if ready.send(Ok(unsafe { GetCurrentThreadId() })).is_err() {
        let _ = unsafe { UnhookWindowsHookEx(hook) };
        return;
    }

    let mut message = MSG::default();
    // GetMessageW returns 0 on WM_QUIT, which is what both the callback and `disarm` post.
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    let _ = unsafe { UnhookWindowsHookEx(hook) };

    // Only now, off the callback's timing budget and with the hook already gone, does this
    // touch Tauri at all.
    let trigger = CAPTURED.with(|captured| captured.borrow_mut().take());
    if let Ok(mut armed) = CAPTURE_THREAD.lock() {
        *armed = None;
    }
    match &trigger {
        Some(trigger) => log::info!("Shortcut capture read {trigger}"),
        None => log::info!("Shortcut capture ended without a combination"),
    }
    let _ = app.emit(CAPTURE_EVENT, CaptureOutcome { trigger });
}

/// Called by the OS for every keystroke on the machine while armed.
///
/// **This must return quickly.** Windows silently drops a low-level hook whose callback
/// exceeds `LowLevelHooksTimeout` (300 ms by default) and does not say so, which would leave
/// the field looking exactly as broken as it did before any of this. So it reads the key,
/// formats a short string, and posts — no locks another thread holds, no I/O, and nothing
/// that calls into Tauri.
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let is_key_down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
    if !is_key_down {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let event = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let virtual_key = event.vkCode as u16;

    // Escape cancels, matching what the field already did with a DOM keydown. Swallowed so
    // it cancels the capture rather than also closing the settings panel behind it.
    if virtual_key == VK_ESCAPE.0 {
        unsafe { PostQuitMessage(0) };
        return LRESULT(1);
    }

    let Some(trigger) = trigger_from_key(virtual_key, current_modifiers()) else {
        // A bare modifier, an unmodified key, or one this format cannot spell: not a
        // combination yet, so keep listening and let the keystroke through untouched.
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    };

    CAPTURED.with(|captured| *captured.borrow_mut() = Some(trigger));
    unsafe { PostQuitMessage(0) };
    // Swallowed: pressing a combination to *record* it must not also fire it in whatever
    // application already owns it.
    LRESULT(1)
}

/// Which modifiers are held right now, as `(ctrl, alt, shift, win)`.
///
/// Read from the keyboard state rather than accumulated across events: the hook may be armed
/// with Ctrl already down, and there is no earlier event to have seen it.
fn current_modifiers() -> (bool, bool, bool, bool) {
    let held = |key: i32| (unsafe { GetAsyncKeyState(key) } as u16 & 0x8000) != 0;
    (
        held(VK_CONTROL.0 as i32),
        held(VK_MENU.0 as i32),
        held(VK_SHIFT.0 as i32),
        held(VK_LWIN.0 as i32) || held(VK_RWIN.0 as i32),
    )
}

/// Spell a virtual key plus its modifiers the way `tauri-plugin-global-shortcut` parses it
/// back, which is also what the settings field has always displayed: modifiers joined by `+`,
/// main key last.
///
/// `None` when there is nothing worth saving yet, for the same three reasons the DOM version
/// had: the key *is* a modifier, no modifier is held, or the key has no spelling this format
/// accepts. Every string this returns is asserted to round-trip through `parse_trigger` in
/// the tests below, so the mapping cannot drift away from the parser.
fn trigger_from_key(virtual_key: u16, modifiers: (bool, bool, bool, bool)) -> Option<String> {
    let key = key_name(virtual_key)?;

    let (ctrl, alt, shift, win) = modifiers;
    let mut parts = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    if win {
        parts.push("Super");
    }
    // An unmodified key would capture every press of it system-wide — almost certainly not
    // what was intended, and the plugin would register it happily. Refused here, exactly as
    // the DOM path refused it, rather than letting a mis-press break the keyboard.
    if parts.is_empty() {
        return None;
    }

    parts.push(key.as_str());
    Some(parts.join("+"))
}

/// The name for a virtual key, or `None` for keys this field does not offer.
///
/// Deliberately narrow: letters, digits and function keys. A shortcut picker that accepts
/// every OEM punctuation key would produce triggers whose spelling varies by keyboard
/// layout, and the parser on the other side would reject them at registration time — after
/// the user thought they had set one.
fn key_name(virtual_key: u16) -> Option<String> {
    match virtual_key {
        // Modifiers are not a combination on their own; keep listening for the key they
        // are being held for.
        code if is_modifier(code) => None,
        code @ 0x30..=0x39 => Some(((code as u8) as char).to_string()), // 0-9
        code @ 0x41..=0x5A => Some(((code as u8) as char).to_string()), // A-Z
        code @ 0x70..=0x87 => Some(format!("F{}", code - 0x70 + 1)),    // F1-F24
        _ => None,
    }
}

fn is_modifier(virtual_key: u16) -> bool {
    const VK_LSHIFT: u16 = 0xA0;
    const VK_RMENU: u16 = 0xA5;
    matches!(virtual_key, code if code == VK_CONTROL.0
        || code == VK_MENU.0
        || code == VK_SHIFT.0
        || code == VK_LWIN.0
        || code == VK_RWIN.0
        || (VK_LSHIFT..=VK_RMENU).contains(&code))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_ALT: (bool, bool, bool, bool) = (true, true, false, false);
    const NOTHING: (bool, bool, bool, bool) = (false, false, false, false);

    #[test]
    fn a_letter_with_modifiers_spells_the_combination_the_parser_expects() {
        assert_eq!(
            trigger_from_key(0x4E, CTRL_ALT).as_deref(), // VK_N
            Some("Ctrl+Alt+N")
        );
    }

    #[test]
    fn every_key_this_offers_round_trips_through_the_registration_parser() {
        // The mapping and the parser are on opposite sides of the same string. If they ever
        // disagree, the field accepts a combination that then fails to register — after the
        // user believes it is set. This is the test that stops that.
        let all_modifiers = [
            (true, false, false, false),
            (false, true, false, false),
            (true, true, true, false),
            (true, false, false, true),
        ];
        for virtual_key in (0x30..=0x39).chain(0x41..=0x5A).chain(0x70..=0x87) {
            for modifiers in all_modifiers {
                let trigger = trigger_from_key(virtual_key, modifiers)
                    .unwrap_or_else(|| panic!("{virtual_key:#x} should map to a trigger"));
                assert!(
                    super::super::windows::parse_trigger(&trigger).is_ok(),
                    "the parser rejects {trigger:?}, which this field would have offered"
                );
            }
        }
    }

    #[test]
    fn a_key_with_no_modifier_is_refused() {
        // Otherwise a single mis-press claims that key system-wide.
        assert_eq!(trigger_from_key(0x4E, NOTHING), None);
    }

    #[test]
    fn a_bare_modifier_is_not_a_combination_yet() {
        for modifier in [VK_CONTROL.0, VK_MENU.0, VK_SHIFT.0, VK_LWIN.0, VK_RWIN.0] {
            assert_eq!(
                trigger_from_key(modifier, CTRL_ALT),
                None,
                "{modifier:#x} is a modifier, not a key to save"
            );
        }
    }

    #[test]
    fn keys_this_format_cannot_spell_are_left_alone() {
        // OEM punctuation varies by layout, so its spelling would not survive the parser.
        for virtual_key in [0xBA_u16, 0xBD, 0xDB] {
            assert_eq!(trigger_from_key(virtual_key, CTRL_ALT), None);
        }
    }
}
