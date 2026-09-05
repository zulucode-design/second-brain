//! Windows global hotkey backend: the app owns the key directly through
//! `tauri-plugin-global-shortcut`, unlike the Linux portal (`super::portal`; see ADR-0001
//! for why the two do not share an abstraction).
//!
//! Because the app owns the key here, a failed registration is a real, specific fact —
//! another process already holds that combination — not a permission the user granted or
//! withheld. That is the one failure mode this backend can report that Linux structurally
//! cannot, and it is also why the settings UI shows a key-capture field here instead of a
//! read-only trigger: the app can act on what the user types, because it is the one asking
//! the OS for the key, not a compositor deciding on its own.

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use super::{Availability, Cause, HotkeyStatus, PREFERRED_TRIGGER, STATUS_EVENT};
use crate::state::AppState;

/// Why the hotkey is not registered, in terms that map to something the user can do.
///
/// Linux's `Unavailable` is a different type with different variants, deliberately: see
/// `Cause` in `mod.rs` for why the two failure domains are not merged into one enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// The shortcut cannot be useful because its capture surface could not be prepared.
    CaptureWindow { detail: String },
    /// The configured trigger does not parse into a real key combination — a stale or
    /// hand-edited setting, since the in-app capture field can only ever produce one that
    /// parses.
    InvalidTrigger { trigger: String, detail: String },
    /// Another application already holds this exact key combination. Named as its own case
    /// because it is the one thing the app can say precisely here that it could never say
    /// on Linux: which key, and that it is genuinely taken, not merely undecided.
    KeyTaken { trigger: String },
    /// Anything else the plugin or the underlying OS call reported.
    PluginError { detail: String },
}

impl Cause for Unavailable {
    fn reason(&self) -> String {
        match self {
            Self::CaptureWindow { detail } => format!(
                "Quick capture could not prepare its capture window ({detail}). Restart the \
                 app; if this continues, report this error."
            ),
            Self::InvalidTrigger { trigger, detail } => format!(
                "The configured shortcut \"{trigger}\" is not a valid key combination \
                 ({detail}). Open Settings and set a new one."
            ),
            Self::KeyTaken { trigger } => format!(
                "The shortcut \"{trigger}\" is already used by another application, so quick \
                 capture has no hotkey. Open Settings and choose a different combination."
            ),
            Self::PluginError { detail } => {
                format!("The global hotkey could not be registered: {detail}")
            }
        }
    }
}

/// Classify what the plugin reported into something the user can act on.
///
/// The plugin's own error type stringifies whatever `global-hotkey` returned before it
/// reaches here (`tauri_plugin_global_shortcut::Error::GlobalHotkey(String)`), so — the
/// same shape as Linux's `classify_portal_error` — the only thing to match on is the text.
/// `global-hotkey`'s Windows backend maps `ERROR_HOTKEY_ALREADY_REGISTERED` to a dedicated
/// `AlreadyRegistered(HotKey)` variant whose `Display` starts "HotKey already registered:",
/// confirmed against its source rather than guessed from a message this app has not
/// actually seen fail; every other registration failure goes through `FailedToRegister`
/// instead, worded differently.
fn classify_registration_error(trigger: &str, message: &str) -> Unavailable {
    if message.starts_with("HotKey already registered") {
        Unavailable::KeyTaken {
            trigger: trigger.to_string(),
        }
    } else {
        Unavailable::PluginError {
            detail: message.to_string(),
        }
    }
}

/// Parse a trigger string into a shortcut the plugin can register, wrapping the parse
/// failure into this backend's own vocabulary rather than the plugin's `HotKeyParseError`.
fn parse_trigger(trigger: &str) -> Result<Shortcut, Unavailable> {
    trigger
        .parse::<Shortcut>()
        .map_err(|error| Unavailable::InvalidTrigger {
            trigger: trigger.to_string(),
            detail: error.to_string(),
        })
}

/// Register `trigger` as the quick-capture hotkey, replacing whatever this app previously
/// held. Returns the status to publish either way, never an error: a failed registration is
/// an ordinary outcome to report, not something the caller needs to additionally handle.
///
/// The old binding is dropped first and unconditionally. `tauri-plugin-global-shortcut`
/// tracks registrations per shortcut value, not per "the app's current hotkey", so calling
/// `unregister_all` before registering the new one is what makes changing the shortcut in
/// Settings not silently leave the previous key also still bound.
pub fn apply_trigger(app: &AppHandle, trigger: &str) -> HotkeyStatus {
    let shortcut = match parse_trigger(trigger) {
        Ok(shortcut) => shortcut,
        Err(cause) => return HotkeyStatus::unavailable(&cause),
    };

    let manager = app.global_shortcut();
    let _ = manager.unregister_all();

    let result = manager.on_shortcut(shortcut, |app_handle, _shortcut, event| {
        if event.state() != ShortcutState::Pressed {
            return;
        }
        on_activation(app_handle);
    });

    match result {
        Ok(()) => {
            let status = HotkeyStatus::registered(Some(trigger.to_string()), false);
            store_and_publish(app, status.clone());
            status
        }
        Err(error) => {
            let status =
                HotkeyStatus::unavailable(&classify_registration_error(trigger, &error.to_string()));
            store_and_publish(app, status.clone());
            status
        }
    }
}

/// Register the hotkey at startup and keep answering it for as long as the app runs.
///
/// Unlike the Linux portal, registration here is synchronous and cannot prompt, so there is
/// no first-run consideration analogous to `needs_binding` — it either succeeds or reports
/// exactly why, every launch, the same way.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        resync_autostart(&app);

        if let Err(detail) = super::window::ensure_window(&app).await {
            let status = HotkeyStatus::unavailable(&Unavailable::CaptureWindow { detail });
            store_and_publish(&app, status);
            log::warn!("Quick capture hotkey unavailable: the capture window could not be prepared");
            return;
        }

        if !a_vault_was_configured(&app) {
            store_and_publish(&app, HotkeyStatus::unknown());
            log::info!("Quick capture hotkey not registered: no vault is configured yet");
            return;
        }

        let trigger = configured_trigger(&app);
        let status = apply_trigger(&app, &trigger);
        match (&status.availability, &status.reason) {
            (Availability::Available, _) => {
                log::info!("Quick capture hotkey registered: {trigger}")
            }
            (_, Some(reason)) => log::warn!("Quick capture hotkey unavailable: {reason}"),
            (_, None) => {}
        }
    });
}

fn on_activation(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match super::vault_status::activation_plan(
            configured_vault_path(&app),
            super::vault_status::vault_is_reachable,
        ) {
            super::vault_status::ActivationPlan::ShowCaptureWindow => {
                if let Err(detail) = super::window::show_capture_window(&app) {
                    log::warn!("Quick capture could not show the overlay: {detail}");
                }
            }
            super::vault_status::ActivationPlan::NotifyVaultUnavailable { path } => {
                notify_vault_unavailable(&app, &path);
            }
            super::vault_status::ActivationPlan::Nothing => {}
        }
    });
}

/// Tell the user why the hotkey did not open the overlay, rather than nothing happening.
///
/// Through `tauri-plugin-notification` rather than hand-rolled Win32 toast calls: unlike
/// the Linux path (ADR-0001), where that plugin's desktop backend nests a Tokio runtime and
/// panics on every call, its Windows backend goes through WinRT's own notification queue
/// and does not start a runtime of its own. Reused rather than re-litigated, but this is
/// the one claim in this file that needs confirming on a real machine, since it cannot be
/// exercised from here — see the ticket's verification checklist.
fn notify_vault_unavailable(app: &AppHandle, path: &str) {
    use tauri_plugin_notification::NotificationExt;

    // A stable id, matching the Linux path's intent: repeated presses against a vault that
    // is still missing replace one notification rather than stacking identical copies.
    const VAULT_UNAVAILABLE_NOTIFICATION_ID: i32 = 1;

    let notice = super::vault_status::vault_unavailable_notice(path);
    if let Err(error) = app
        .notification()
        .builder()
        .id(VAULT_UNAVAILABLE_NOTIFICATION_ID)
        .title(&notice.title)
        .body(&notice.body)
        .show()
    {
        log::warn!("Could not show the vault-unavailable notification: {error}");
    }
}

fn store_and_publish(app: &AppHandle, status: HotkeyStatus) {
    if let Ok(mut stored) = app.state::<AppState>().hotkey_status.lock() {
        *stored = status.clone();
    }
    let _ = tauri::Emitter::emit(app, STATUS_EVENT, status);
}

/// Make the on-disk autostart entry match the stored preference. Mirrors
/// `startup::resync_autostart`'s Linux shape; the two never call the same `autostart::sync`
/// because the two backends' signatures genuinely differ (Windows needs no config
/// directory), not because of an accident of naming.
fn resync_autostart(app: &AppHandle) {
    let Some(exec) = std::env::current_exe().ok() else {
        log::warn!("Could not resolve this app's own executable path for autostart");
        return;
    };
    let Ok(app_id) = super::configured_application_id() else {
        return;
    };
    let enabled = app
        .state::<AppState>()
        .config
        .lock()
        .map(|config| config.autostart)
        .unwrap_or(false);

    if let Err(error) = crate::autostart::sync(enabled, &app_id, &exec) {
        log::warn!("Could not update the autostart entry: {error}");
    }
}

fn a_vault_was_configured(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .config
        .lock()
        .map(|config| config.active_vault.is_some())
        .unwrap_or(false)
}

fn configured_vault_path(app: &AppHandle) -> Option<String> {
    app.state::<AppState>()
        .config
        .lock()
        .ok()
        .and_then(|config| config.active_vault.clone())
}

/// The trigger to register: whatever the user set in Settings, or the same default Linux
/// hints the compositor with, so a first run behaves identically before either platform's
/// user has ever touched the setting.
fn configured_trigger(app: &AppHandle) -> String {
    app.state::<AppState>()
        .config
        .lock()
        .ok()
        .and_then(|config| config.hotkey_trigger.clone())
        .filter(|trigger| !trigger.is_empty())
        .unwrap_or_else(|| PREFERRED_TRIGGER.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conflicting_key_names_the_trigger_that_is_taken() {
        let cause = classify_registration_error(
            "Ctrl+Alt+N",
            "HotKey already registered: HotKey { mods: ..., key: KeyN, id: 1 }",
        );
        assert_eq!(
            cause,
            Unavailable::KeyTaken {
                trigger: "Ctrl+Alt+N".to_string()
            }
        );
        assert!(cause.reason().contains("Ctrl+Alt+N"));
        assert!(cause.reason().contains("already used by another"));
    }

    #[test]
    fn an_unrecognised_plugin_error_is_passed_through_rather_than_guessed_at() {
        let cause = classify_registration_error("Ctrl+Alt+N", "Something new went wrong");
        assert_eq!(
            cause,
            Unavailable::PluginError {
                detail: "Something new went wrong".to_string()
            }
        );
    }

    #[test]
    fn an_unparseable_trigger_names_itself_rather_than_the_parser_internals() {
        let result = parse_trigger("not a real shortcut");
        let Err(cause) = result else {
            panic!("a nonsense trigger must not parse");
        };
        assert!(matches!(cause, Unavailable::InvalidTrigger { .. }));
        assert!(cause.reason().contains("not a real shortcut"));
    }

    #[test]
    fn a_valid_trigger_parses() {
        assert!(parse_trigger("Ctrl+Alt+N").is_ok());
    }
}
