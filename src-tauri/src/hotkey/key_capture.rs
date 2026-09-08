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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostQuitMessage,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT,
    MSG, PM_NOREMOVE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WM_USER,
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
static CAPTURE_THREAD: Mutex<Option<u32>> = Mutex::new(None);

/// Everything the hook callback touches is a process-wide atomic, not thread-local state.
///
/// The first version of this module kept the captured key and the modifier state in
/// `thread_local!`, on the documented understanding that a low-level hook is called on the
/// thread that installed it. Measured on 2026-09-07, it is not: the callback ran, counted
/// keys, and every one of them read modifiers as "none held" and left the captured key
/// unset, because it was reading a different thread's copy. The combination was then never
/// recognised, never swallowed, and passed straight through — so `Ctrl+Alt+N` reached the
/// registered hotkey and opened the overlay, and Alt+Z reached ShadowPlay.
///
/// Atomics also suit the callback better than the alternative: no lock to contend, no
/// allocation, nothing that can block. The string is built afterwards, on the capture thread.
mod shared {
    use std::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize};

    /// How many times the OS called the hook. Zero means it was installed and never called,
    /// which is a different failure from seeing keys and keeping none — telling those apart
    /// is what this exists for.
    pub static CALLBACKS: AtomicUsize = AtomicUsize::new(0);
    /// Modifiers currently held, as [`super::MOD_CTRL`] and friends.
    pub static MODIFIERS: AtomicU8 = AtomicU8::new(0);
    /// The captured combination as `(modifier bits) << 16 | virtual key`, or 0 for none.
    /// Packed rather than a `Mutex<String>` so the callback allocates nothing.
    pub static CAPTURED: AtomicU32 = AtomicU32::new(0);
    /// The thread running the message loop, so the callback can end it from wherever the OS
    /// happens to call it. `PostQuitMessage` cannot: it targets the *calling* thread, which
    /// is why capture used to run on for the full five seconds instead of stopping.
    pub static LOOP_THREAD: AtomicU32 = AtomicU32::new(0);
    /// The thread the OS actually called the hook on, recorded so the mismatch above is
    /// visible in the log rather than inferred from symptoms a second time.
    pub static CALLBACK_THREAD: AtomicU32 = AtomicU32::new(0);
}

const MOD_CTRL: u8 = 1;
const MOD_ALT: u8 = 2;
const MOD_SHIFT: u8 = 4;
const MOD_WIN: u8 = 8;

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
        end_message_loop(thread_id);
    }
}

/// Stop the capture thread's `GetMessageW` loop, from any thread.
fn end_message_loop(thread_id: u32) {
    if thread_id != 0 {
        let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}

/// Owns the hook for its whole lifetime: installs it, pumps messages so the OS can call it,
/// and unhooks before reporting. The hook must be installed and removed on the same thread,
/// and that thread has to be pumping messages, which is why this is a bare thread rather
/// than a Tauri async task.
fn capture_thread(app: AppHandle, ready: mpsc::Sender<Result<u32, String>>) {
    // Force this thread's message queue into existence before the hook goes on, and before
    // anyone can `PostThreadMessageW` at it. A thread has no queue until it first asks for a
    // message, and `PostThreadMessageW` is documented to fail against one that never has.
    let mut discard = MSG::default();
    let _ = unsafe { PeekMessageW(&mut discard, None, WM_USER, WM_USER, PM_NOREMOVE) };

    let thread_id = unsafe { GetCurrentThreadId() };
    shared::LOOP_THREAD.store(thread_id, Ordering::Relaxed);
    shared::CALLBACKS.store(0, Ordering::Relaxed);
    shared::CAPTURED.store(0, Ordering::Relaxed);
    shared::CALLBACK_THREAD.store(0, Ordering::Relaxed);
    // Trustworthy here, outside the callback: this is the one place the real key state can
    // be read, so a hook armed while Ctrl is already held starts out knowing it.
    shared::MODIFIERS.store(held_modifiers_now(), Ordering::Relaxed);

    // A real module handle rather than `None`. The documentation permits null for a
    // low-level hook, and `SetWindowsHookExW` accepted it — but then installed a hook the
    // system never called, 0 keys seen across every session on 2026-09-07.
    let module = unsafe { GetModuleHandleW(None) }.ok();

    let hook = match unsafe {
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module.map(Into::into), 0)
    } {
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

    if ready.send(Ok(thread_id)).is_err() {
        let _ = unsafe { UnhookWindowsHookEx(hook) };
        return;
    }

    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    let _ = unsafe { UnhookWindowsHookEx(hook) };
    shared::LOOP_THREAD.store(0, Ordering::Relaxed);

    // Only now, off the callback's timing budget and with the hook already gone, is the
    // string built and Tauri touched at all.
    let trigger = unpack_captured(shared::CAPTURED.swap(0, Ordering::Relaxed));
    if let Ok(mut armed) = CAPTURE_THREAD.lock() {
        *armed = None;
    }
    let callbacks = shared::CALLBACKS.load(Ordering::Relaxed);
    let callback_thread = shared::CALLBACK_THREAD.load(Ordering::Relaxed);
    let same_thread = callback_thread == 0 || callback_thread == thread_id;
    match &trigger {
        Some(trigger) => log::info!("Shortcut capture read {trigger} ({callbacks} keys seen)"),
        None => log::info!("Shortcut capture ended without a combination ({callbacks} keys seen)"),
    }
    if !same_thread {
        log::debug!(
            "The keyboard hook was called on thread {callback_thread}, not the {thread_id} that \
             installed it"
        );
    }
    let _ = app.emit(CAPTURE_EVENT, CaptureOutcome { trigger });
}

/// Called by the OS for every keystroke on the machine while armed.
///
/// **This must return quickly.** Windows silently drops a low-level hook whose callback
/// exceeds `LowLevelHooksTimeout` (300 ms by default) and does not say so. So this touches
/// only atomics, allocates nothing, takes no lock, and never calls into Tauri.
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Counted before anything can return, so "the system never called this" is a count of
    // zero and nothing else.
    shared::CALLBACKS.fetch_add(1, Ordering::Relaxed);
    shared::CALLBACK_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);

    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let message = wparam.0 as u32;
    let is_key_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
    let is_key_up = message == WM_KEYUP || message == WM_SYSKEYUP;
    if !is_key_down && !is_key_up {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let event = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let virtual_key = event.vkCode as u16;

    // Modifiers are tracked as they happen, both directions, so the state is current by the
    // time the key they are held for arrives.
    if let Some(modifier) = modifier_of(virtual_key) {
        let bit = modifier_bit(modifier);
        if is_key_down {
            shared::MODIFIERS.fetch_or(bit, Ordering::Relaxed);
        } else {
            shared::MODIFIERS.fetch_and(!bit, Ordering::Relaxed);
        }
        // Never a combination on its own, and passed through so holding a modifier does not
        // interfere with anything else on the machine.
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    if !is_key_down {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    // Escape cancels. Swallowed so it cancels the capture rather than also closing the
    // settings panel behind it.
    if virtual_key == VK_ESCAPE.0 {
        end_message_loop(shared::LOOP_THREAD.load(Ordering::Relaxed));
        return LRESULT(1);
    }

    let modifiers = shared::MODIFIERS.load(Ordering::Relaxed);
    // A bare key, or one this format cannot spell: not a combination, so keep listening and
    // let the keystroke through untouched.
    if modifiers == 0 || !is_capturable(virtual_key) {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    shared::CAPTURED.store(
        (modifiers as u32) << 16 | virtual_key as u32,
        Ordering::Relaxed,
    );
    end_message_loop(shared::LOOP_THREAD.load(Ordering::Relaxed));
    // Swallowed: pressing a combination to *record* it must not also fire it in whatever
    // application already owns it.
    LRESULT(1)
}

/// Rebuild the trigger string from what the callback packed. Runs on the capture thread,
/// where allocating is fine.
fn unpack_captured(packed: u32) -> Option<String> {
    if packed == 0 {
        return None;
    }
    let virtual_key = (packed & 0xFFFF) as u16;
    let modifiers = ((packed >> 16) & 0xFF) as u8;
    trigger_from_key(virtual_key, modifiers)
}

/// Which modifiers are physically held, as [`MOD_CTRL`] and friends.
///
/// Only ever called outside the hook callback, to seed `shared::MODIFIERS`. Inside a
/// low-level hook this call reports a state that has not been updated for the keystroke
/// being handled, which is why the callback tracks its own.
fn held_modifiers_now() -> u8 {
    let held = |key: i32| (unsafe { GetAsyncKeyState(key) } as u16 & 0x8000) != 0;
    let mut modifiers = 0;
    if held(VK_CONTROL.0 as i32) {
        modifiers |= MOD_CTRL;
    }
    if held(VK_MENU.0 as i32) {
        modifiers |= MOD_ALT;
    }
    if held(VK_SHIFT.0 as i32) {
        modifiers |= MOD_SHIFT;
    }
    if held(VK_LWIN.0 as i32) || held(VK_RWIN.0 as i32) {
        modifiers |= MOD_WIN;
    }
    modifiers
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Win,
}

fn modifier_bit(modifier: Modifier) -> u8 {
    match modifier {
        Modifier::Ctrl => MOD_CTRL,
        Modifier::Alt => MOD_ALT,
        Modifier::Shift => MOD_SHIFT,
        Modifier::Win => MOD_WIN,
    }
}

/// Which modifier a virtual key is, if it is one.
///
/// A low-level hook reports the handed variants — `VK_LMENU` rather than `VK_MENU` — so
/// matching only the generic codes would miss every modifier the user actually presses.
fn modifier_of(virtual_key: u16) -> Option<Modifier> {
    const VK_LSHIFT: u16 = 0xA0;
    const VK_RSHIFT: u16 = 0xA1;
    const VK_LCONTROL: u16 = 0xA2;
    const VK_RCONTROL: u16 = 0xA3;
    const VK_LMENU: u16 = 0xA4;
    const VK_RMENU: u16 = 0xA5;
    match virtual_key {
        key if key == VK_CONTROL.0 || key == VK_LCONTROL || key == VK_RCONTROL => {
            Some(Modifier::Ctrl)
        }
        key if key == VK_MENU.0 || key == VK_LMENU || key == VK_RMENU => Some(Modifier::Alt),
        key if key == VK_SHIFT.0 || key == VK_LSHIFT || key == VK_RSHIFT => Some(Modifier::Shift),
        key if key == VK_LWIN.0 || key == VK_RWIN.0 => Some(Modifier::Win),
        _ => None,
    }
}

/// Whether this field offers a key at all. Allocation-free, because the callback asks it.
///
/// Deliberately narrow: letters, digits and function keys. A picker that accepted every OEM
/// punctuation key would produce triggers whose spelling varies by keyboard layout, and the
/// parser on the other side would reject them at registration — after the user believed they
/// had set one.
fn is_capturable(virtual_key: u16) -> bool {
    modifier_of(virtual_key).is_none()
        && matches!(virtual_key, 0x30..=0x39 | 0x41..=0x5A | 0x70..=0x87)
}

/// Spell a virtual key plus its modifiers the way `tauri-plugin-global-shortcut` parses it
/// back, which is also what the settings field has always displayed: modifiers joined by
/// `+`, main key last.
///
/// `None` when there is nothing worth saving: the key is a modifier, no modifier is held, or
/// the key has no spelling this format accepts. Every string this returns is asserted to
/// round-trip through `parse_trigger` in the tests below, so the mapping cannot drift away
/// from the parser.
fn trigger_from_key(virtual_key: u16, modifiers: u8) -> Option<String> {
    if !is_capturable(virtual_key) {
        return None;
    }

    let mut parts = Vec::new();
    if modifiers & MOD_CTRL != 0 {
        parts.push("Ctrl");
    }
    if modifiers & MOD_ALT != 0 {
        parts.push("Alt");
    }
    if modifiers & MOD_SHIFT != 0 {
        parts.push("Shift");
    }
    if modifiers & MOD_WIN != 0 {
        parts.push("Super");
    }
    // An unmodified key would capture every press of it system-wide — almost certainly not
    // what was intended, and the plugin would register it happily.
    if parts.is_empty() {
        return None;
    }

    let key = match virtual_key {
        code @ 0x70..=0x87 => format!("F{}", code - 0x70 + 1),
        code => ((code as u8) as char).to_string(),
    };
    let mut trigger = parts.join("+");
    trigger.push('+');
    trigger.push_str(&key);
    Some(trigger)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_ALT: u8 = MOD_CTRL | MOD_ALT;

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
        // user believes it is set.
        let all_modifiers = [
            MOD_CTRL,
            MOD_ALT,
            MOD_CTRL | MOD_ALT | MOD_SHIFT,
            MOD_CTRL | MOD_WIN,
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
        assert_eq!(trigger_from_key(0x4E, 0), None);
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
            assert!(!is_capturable(virtual_key));
        }
    }

    #[test]
    fn the_handed_modifier_codes_a_hook_actually_reports_are_recognised() {
        // The hook is given VK_LMENU, not VK_MENU. Matching only the generic codes would
        // miss every modifier a user physically presses, so the field would see Alt+Z as a
        // bare Z and refuse it.
        for (code, expected) in [
            (0xA2_u16, Modifier::Ctrl), // VK_LCONTROL
            (0xA3, Modifier::Ctrl),     // VK_RCONTROL
            (0xA4, Modifier::Alt),      // VK_LMENU
            (0xA5, Modifier::Alt),      // VK_RMENU
            (0xA0, Modifier::Shift),    // VK_LSHIFT
            (0xA1, Modifier::Shift),    // VK_RSHIFT
            (VK_CONTROL.0, Modifier::Ctrl),
            (VK_MENU.0, Modifier::Alt),
            (VK_SHIFT.0, Modifier::Shift),
            (VK_LWIN.0, Modifier::Win),
            (VK_RWIN.0, Modifier::Win),
        ] {
            assert_eq!(modifier_of(code), Some(expected), "{code:#x}");
        }
        for code in [0x41_u16, 0x5A, 0x30, 0x39] {
            assert_eq!(modifier_of(code), None, "{code:#x}");
        }
    }

    #[test]
    fn a_captured_combination_survives_the_trip_through_the_callback_packing() {
        // The callback cannot allocate, so it packs the key and modifiers into one atomic
        // and the capture thread spells it afterwards. Alt+Z is the combination the
        // 2026-09-07 pass could never capture, so it is the one pinned here.
        let packed = (CTRL_ALT as u32) << 16 | 0x5A_u32;
        assert_eq!(unpack_captured(packed).as_deref(), Some("Ctrl+Alt+Z"));

        assert_eq!(unpack_captured(0), None, "0 means nothing was captured");
    }
}
