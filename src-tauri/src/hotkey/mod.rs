//! The global hotkey that opens quick capture, and what to tell the user when it is absent.
//!
//! On Linux this goes through the XDG `GlobalShortcuts` portal rather than an X11 grab. The
//! reasoning, and the rejected alternatives, are in `docs/adr/0001-linux-global-shortcuts-via-xdg-portal.md`;
//! the short version is that an X11 grab under Wayland fires only while an X11 window holds
//! focus and is silent otherwise, which is worse than not shipping the feature.
//!
//! The consequence that shapes this module: **the app does not own the keybinding.** It sends
//! a preferred trigger as a hint, the compositor decides, and hands back a description to
//! display. So there is no such thing as a registration conflict to report here — only a
//! shortcut that is bound or is not, and a reason the user can act on.

use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
pub mod portal;

/// The shortcut's identity with the portal. Stable: the compositor remembers bindings against
/// it, so changing it would silently orphan whatever the user has already assigned.
pub const SHORTCUT_ID: &str = "quick-capture";

/// Shown to the user in the system's shortcut settings, so it is user-facing copy.
pub const SHORTCUT_DESCRIPTION: &str = "Quick capture a note";

/// A hint only. The portal is free to ignore it, and on GNOME the user confirms or changes it
/// in a dialog. Never display this as though it were the active binding — display the
/// `trigger` the portal returns.
pub const PREFERRED_TRIGGER: &str = "CTRL+ALT+n";

/// Reused from `ai_health`, not redeclared. The two surfaces answer the same question — can
/// this thing be used, and if not, why — so the front end gets one discriminator to switch on
/// rather than two enums that happen to share a name.
pub use crate::ai_health::Availability;

/// Why the hotkey is not registered, in terms that map to something the user can do.
///
/// These are kept as distinct cases rather than one opaque string because they call for
/// genuinely different actions, and because the portal reports several of them identically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// No `GlobalShortcuts` implementation on this desktop.
    NoPortal,
    /// The portal will not accept our app id, which in practice means it cannot find an
    /// installed desktop entry matching it.
    AppIdRejected { app_id: String, detail: String },
    /// The user dismissed or denied the permission dialog. This decision persists, and every
    /// later attempt fails without prompting again, so it needs naming explicitly.
    PermissionDenied { app_id: String },
    /// Anything else the portal reported.
    PortalError { detail: String },
}

impl Unavailable {
    /// The message shown to the user. Each one ends with the thing to actually do.
    pub fn reason(&self) -> String {
        match self {
            Self::NoPortal => "This desktop has no global shortcuts portal, so a system-wide \
                 hotkey cannot be registered. Quick capture still works from inside the app."
                .to_string(),
            Self::AppIdRejected { app_id, detail } => format!(
                "The desktop portal would not accept the application id \"{app_id}\" ({detail}). \
                 This usually means no installed desktop entry matches it. For a development \
                 build, run scripts/dev-desktop-entry.sh."
            ),
            Self::PermissionDenied { app_id } => format!(
                "Permission for the global shortcut was declined, and the desktop remembers that \
                 decision, so asking again does nothing. To be asked once more, run: \
                 flatpak permission-reset {app_id}"
            ),
            Self::PortalError { detail } => {
                format!("The desktop portal refused the global shortcut: {detail}")
            }
        }
    }
}

/// Whether the hotkey is registered, and what to say when it is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyStatus {
    pub availability: Availability,
    /// `None` when registered.
    pub reason: Option<String>,
    /// What the compositor actually bound, as it described it — for display only. `None` until
    /// a binding exists.
    pub trigger: Option<String>,
}

impl HotkeyStatus {
    pub fn unknown() -> Self {
        Self {
            availability: Availability::Unknown,
            reason: None,
            trigger: None,
        }
    }

    pub fn registered(trigger: Option<String>) -> Self {
        Self {
            availability: Availability::Available,
            reason: None,
            trigger,
        }
    }

    pub fn unavailable(cause: &Unavailable) -> Self {
        Self {
            availability: Availability::Unavailable,
            reason: Some(cause.reason()),
            trigger: None,
        }
    }
}

impl Default for HotkeyStatus {
    fn default() -> Self {
        Self::unknown()
    }
}

/// Whether [`SHORTCUT_ID`] still needs binding, given what the portal says is already bound.
///
/// Bindings outlive the session that made them, so binding unconditionally at startup would
/// show the permission dialog on every launch. Asking first turns it into a first-run event.
pub fn needs_binding<'a>(already_bound: impl IntoIterator<Item = &'a str>) -> bool {
    !already_bound.into_iter().any(|id| id == SHORTCUT_ID)
}

/// Classify a portal error into something the user can act on.
///
/// The portal reports a missing desktop entry and a rejected app id with the same
/// `NotAllowed`/`Failed` shapes, so the text is what distinguishes them.
pub fn classify_portal_error(app_id: &str, message: &str) -> Unavailable {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("app info not found") || lowered.contains("an app id is required") {
        Unavailable::AppIdRejected {
            app_id: app_id.to_string(),
            detail: message.to_string(),
        }
    } else if lowered.contains("not allowed") || lowered.contains("denied") {
        Unavailable::PermissionDenied {
            app_id: app_id.to_string(),
        }
    } else {
        Unavailable::PortalError {
            detail: message.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_already_bound_is_not_bound_again() {
        // Binding again would re-prompt, and the dialog is the thing users remember.
        assert!(!needs_binding(vec![SHORTCUT_ID]));
        assert!(!needs_binding(vec!["something-else", SHORTCUT_ID]));
    }

    #[test]
    fn a_shortcut_that_is_not_bound_yet_needs_binding() {
        assert!(needs_binding(Vec::<&str>::new()));
        assert!(needs_binding(vec!["something-else"]));
    }

    #[test]
    fn a_missing_desktop_entry_is_reported_as_an_app_id_problem() {
        // Both of these are what the portal actually says; verified against
        // xdg-desktop-portal 1.22.1 on 2026-08-30.
        for message in [
            "Could not register app ID: App info not found for 'io.github.example.App'",
            "An app id is required",
        ] {
            let cause = classify_portal_error("io.github.example.App", message);
            assert!(
                matches!(cause, Unavailable::AppIdRejected { .. }),
                "{message}"
            );
            assert!(
                cause.reason().contains("dev-desktop-entry.sh"),
                "should say how to fix it"
            );
        }
    }

    #[test]
    fn a_declined_permission_names_the_reset_command() {
        // The decision persists, so without the command the user has no way back.
        let cause = Unavailable::PermissionDenied {
            app_id: "io.github.example.App".to_string(),
        };
        let reason = cause.reason();
        assert!(reason.contains("flatpak permission-reset io.github.example.App"));
    }

    #[test]
    fn an_unrecognised_portal_error_is_passed_through_rather_than_guessed_at() {
        let cause = classify_portal_error("io.github.example.App", "Something new went wrong");
        assert_eq!(
            cause,
            Unavailable::PortalError {
                detail: "Something new went wrong".to_string()
            }
        );
    }

    #[test]
    fn a_registered_hotkey_offers_no_reason() {
        let status = HotkeyStatus::registered(Some("Ctrl+Alt+N".to_string()));
        assert_eq!(status.availability, Availability::Available);
        assert!(status.reason.is_none());
    }

    #[test]
    fn nothing_is_claimed_before_startup_decides() {
        // Unknown must not be mistaken for unavailable, or the UI reports a broken hotkey
        // during the moment before the portal has answered.
        let status = HotkeyStatus::unknown();
        assert_eq!(status.availability, Availability::Unknown);
        assert!(status.reason.is_none());
        assert!(status.trigger.is_none());
    }
}
