//! The global hotkey that opens quick capture, and what to tell the user when it is absent.
//!
//! The two backends diverge because the hotkey model itself does, not because one is more
//! finished than the other — see ADR-0001. On Linux this goes through the XDG
//! `GlobalShortcuts` portal rather than an X11 grab: an X11 grab under Wayland fires only
//! while an X11 window holds focus and is silent otherwise, which is worse than not
//! shipping the feature. **The app does not own the keybinding** — it sends a preferred
//! trigger as a hint, the compositor decides, and there is no such thing as a registration
//! conflict to report, only a shortcut that is bound or is not.
//!
//! On Windows the app registers the key directly through `tauri-plugin-global-shortcut`
//! (`windows.rs`). **The app does own the keybinding** there, so a failure is a real,
//! specific conflict — another application already holds that combination — and the
//! shortcut is configured from inside the app rather than handed to the OS.
//!
//! Both backends report through the same [`HotkeyStatus`], so the settings UI and the
//! activation path above this module (`capture.rs`, `vault_status.rs`) never need to know
//! which one is running underneath.

use serde::{Deserialize, Serialize};
use std::fmt;

pub mod capture;
#[cfg(target_os = "linux")]
pub mod desktop_entry;
#[cfg(target_os = "linux")]
pub mod portal;
#[cfg(target_os = "linux")]
pub mod startup;
pub mod vault_status;
pub mod window;
#[cfg(target_os = "windows")]
pub mod windows;

/// The shortcut's identity with the portal. Stable: the compositor remembers bindings against
/// it, so changing it would silently orphan whatever the user has already assigned.
pub const SHORTCUT_ID: &str = "quick-capture";

/// Shown to the user in the system's shortcut settings, so it is user-facing copy.
/// Linux-only: Windows has no equivalent system-level description to populate, since the
/// shortcut is configured entirely inside the app.
#[cfg(target_os = "linux")]
pub const SHORTCUT_DESCRIPTION: &str = "Quick capture a note";

/// A hint only. The portal is free to ignore it, and on GNOME the user confirms or changes it
/// in a dialog. Never display this as though it were the active binding — display the
/// `trigger` the portal returns.
pub const PREFERRED_TRIGGER: &str = "CTRL+ALT+n";

/// The capture window's label, as declared in `tauri.conf.json`. Created hidden at startup so
/// the hotkey shows an existing window rather than waiting on a WebView to load.
pub const WINDOW_LABEL: &str = "capture";

/// Emitted to the capture window when it is shown, so the field can take the caret. The
/// window cannot infer this: it is shown and hidden repeatedly without ever being reloaded.
pub const SHOWN_EVENT: &str = "quick-capture-shown";

/// Emitted when the hotkey's registration state changes, so settings never shows a stale one.
pub const STATUS_EVENT: &str = "hotkey-status-changed";

/// A reverse-DNS application identifier accepted by Tauri's bundler *and* the portal.
///
/// Three sets of rules apply at once, and they contradict each other, so the intersection is
/// narrower than any one of them. Measured, each against the thing that enforces it:
///
/// - **Tauri's bundler** rejects `_` outright: "must contain only alphanumeric characters
///   (A-Z, a-z, and 0-9), hyphens (-), and periods (.)". A `.deb` build fails before it starts.
/// - **The Flatpak app-id rules**, which `ashpd` enforces client-side, allow `_` anywhere but
///   permit `-` **only in the final segment**. `ashpd` refuses to send anything else, so the
///   portal never sees it.
/// - **The portal daemon itself** is more lenient than `ashpd` — it accepted a mid-segment
///   hyphen when probed directly on 2026-09-02 — but that is not something to depend on.
///
/// So a middle segment may hold letters and digits only. `io.github.zulucode_design.X` cannot
/// be packaged; `io.github.zulucode-design.X` cannot be registered; both were shipped and both
/// failed. The check below is what stops a third attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationId(String);

impl ApplicationId {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let segments: Vec<&str> = value.split('.').collect();
        let last = segments.len().saturating_sub(1);
        // A hyphen is allowed only in the final segment, and only between alphanumerics.
        let valid_segment = |(index, segment): (usize, &&str)| {
            let hyphens_allowed = index == last;
            !segment.is_empty()
                && segment.chars().all(|character| {
                    character.is_ascii_alphanumeric() || (hyphens_allowed && character == '-')
                })
                && segment
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_alphanumeric())
                && segment
                    .chars()
                    .last()
                    .is_some_and(|character| character.is_ascii_alphanumeric())
        };

        if segments.len() < 3 || !segments.iter().enumerate().all(valid_segment) {
            return Err(format!(
                "{value:?} is not a Tauri-compatible reverse-DNS application identifier"
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApplicationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The single configured source of truth for this build's application identifier.
pub fn configured_application_id() -> Result<ApplicationId, String> {
    let config: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json"))
        .map_err(|error| format!("tauri.conf.json is invalid: {error}"))?;
    let identifier = config["identifier"]
        .as_str()
        .ok_or_else(|| "tauri.conf.json has no string identifier".to_string())?;
    ApplicationId::parse(identifier)
}

/// Reused from `ai_health`, not redeclared. The two surfaces answer the same question — can
/// this thing be used, and if not, why — so the front end gets one discriminator to switch on
/// rather than two enums that happen to share a name.
pub use crate::ai_health::Availability;

/// Something that explains why the hotkey is not registered, in the user's own terms.
///
/// Each backend has its own enum — `Unavailable` here for Linux, `windows::Unavailable` for
/// Windows — because the two failure domains share nothing: a portal refusing a permission
/// dialog and a registry key already claimed by another process are not the same kind of
/// fact wearing different words, and forcing them into one enum would mean every match
/// arm on one platform reasoning about cases that can never happen there. This trait is the
/// only thing they share: enough for [`HotkeyStatus::unavailable`] to accept either.
pub trait Cause {
    fn reason(&self) -> String;
}

/// Why the hotkey is not registered, in terms that map to something the user can do.
///
/// These are kept as distinct cases rather than one opaque string because they call for
/// genuinely different actions, and because the portal reports several of them identically.
/// Linux-specific: see [`Cause`] for why this is not the one type both platforms share.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// The shortcut cannot be useful because its capture surface could not be prepared.
    CaptureWindow { detail: String },
    /// An AppImage could not create the user-level desktop entry the portal requires.
    AppImageIntegration { detail: String },
    /// No `GlobalShortcuts` implementation on this desktop.
    NoPortal,
    /// The portal will not accept our app id, which in practice means it cannot find an
    /// installed desktop entry matching it.
    AppIdRejected { app_id: String, detail: String },
    /// The user dismissed or denied the permission dialog.
    ///
    /// Named as its own case because the generic message ("the portal refused the global
    /// shortcut") tells the user nothing they can act on, and this is the one refusal they
    /// caused and can undo.
    ///
    /// Whether it *persists* varies by desktop. GNOME 50 records nothing and asks again on the
    /// next launch — measured 2026-09-02, dismissing the real dialog twice and finding no entry
    /// in either the permission store or dconf. Portals that do persist a refusal need
    /// `flatpak permission-reset`, so the message covers both without claiming either.
    PermissionDenied { app_id: String },
    /// Anything else the portal reported.
    PortalError { detail: String },
}

#[cfg(target_os = "linux")]
impl Cause for Unavailable {
    fn reason(&self) -> String {
        Unavailable::reason(self)
    }
}

#[cfg(target_os = "linux")]
impl Unavailable {
    /// The message shown to the user. Each one ends with the thing to actually do.
    pub fn reason(&self) -> String {
        match self {
            Self::CaptureWindow { detail } => format!(
                "Quick capture could not prepare its capture window ({detail}). Restart the app; \
                 if this continues, report this error."
            ),
            Self::AppImageIntegration { detail } => format!(
                "Quick capture could not prepare the AppImage desktop entry ({detail}). Check \
                 that your user data directory is writable, then restart the app."
            ),
            Self::NoPortal => "This desktop has no global shortcuts portal, so a system-wide \
                 hotkey cannot be registered. Quick capture still works from inside the app."
                .to_string(),
            Self::AppIdRejected { app_id, detail } => format!(
                "The desktop portal would not accept the application id \"{app_id}\", so the \
                 global hotkey could not be registered. This means no installed desktop entry \
                 matches that id: if you installed Second Brain from a package, reinstalling it \
                 should restore the entry; if you are running a development build, run \
                 scripts/dev-desktop-entry.sh. ({detail})"
            ),
            Self::PermissionDenied { app_id } => format!(
                "Permission for the global shortcut was declined, so quick capture has no \
                 hotkey. Restart Second Brain to be asked again. If your desktop remembers the \
                 refusal and stops asking, clear it with: flatpak permission-reset {app_id}"
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
    /// Whether this desktop can open its own shortcut editor on request.
    ///
    /// False on `GlobalShortcuts` portals older than version 2, which have no
    /// `ConfigureShortcuts` at all — including GNOME 50, the target machine. The settings UI
    /// reads this to decide between offering a button and explaining where to go instead:
    /// a button that can only fail is the same "appears to work" failure ADR-0001 exists to
    /// prevent, one layer up.
    pub can_configure: bool,
}

impl HotkeyStatus {
    pub fn unknown() -> Self {
        Self {
            availability: Availability::Unknown,
            reason: None,
            trigger: None,
            can_configure: false,
        }
    }

    pub fn registered(trigger: Option<String>, can_configure: bool) -> Self {
        Self {
            availability: Availability::Available,
            reason: None,
            trigger,
            can_configure,
        }
    }

    pub fn unavailable(cause: &impl Cause) -> Self {
        Self {
            availability: Availability::Unavailable,
            reason: Some(cause.reason()),
            trigger: None,
            can_configure: false,
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
#[cfg(target_os = "linux")]
pub fn classify_portal_error(app_id: &ApplicationId, message: &str) -> Unavailable {
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
    fn application_ids_accept_the_packaged_reverse_dns_identifier() {
        let id = ApplicationId::parse("io.github.zulucodedesign.SecondBrain")
            .expect("the shipped identifier is valid");
        assert_eq!(id.as_str(), "io.github.zulucodedesign.SecondBrain");
    }

    #[test]
    fn application_ids_reject_values_tauri_or_the_portal_cannot_use() {
        for invalid in [
            "",
            "Second Brain",
            "io.github.zulucode_design.SecondBrain",
            // Shipped once and rejected by ashpd: a hyphen outside the final segment.
            "io.github.zulucode-design.SecondBrain",
            "single-label",
            ".io.github.App",
            "io..App",
        ] {
            assert!(
                ApplicationId::parse(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    fn example_app_id() -> ApplicationId {
        ApplicationId::parse("io.github.example.App").expect("example id is valid")
    }

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

    #[cfg(target_os = "linux")]
    #[test]
    fn a_missing_desktop_entry_is_reported_as_an_app_id_problem() {
        // Both of these are what the portal actually says; verified against
        // xdg-desktop-portal 1.22.1 on 2026-08-30.
        for message in [
            "Could not register app ID: App info not found for 'io.github.example.App'",
            "An app id is required",
        ] {
            let cause = classify_portal_error(&example_app_id(), message);
            assert!(
                matches!(cause, Unavailable::AppIdRejected { .. }),
                "{message}"
            );
            let reason = cause.reason();
            // Both audiences, because both hit this. A packaged user has no repo to run a
            // script from, and telling them to is worse than saying nothing: it sounds like
            // an answer while pointing at a file they do not have.
            assert!(
                reason.contains("reinstalling it"),
                "should tell a packaged user what to do"
            );
            assert!(
                reason.contains("dev-desktop-entry.sh"),
                "should say how to fix it"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_declined_permission_says_how_to_be_asked_again() {
        // Two desktops, two behaviours, and the message has to be true on both.
        //
        // Measured on GNOME 50 / xdg-desktop-portal-gnome 50.0, 2026-09-02: a dismissed
        // dialog is recorded nowhere — not in the permission store, not in dconf — and the
        // next launch simply asks again. An earlier version of this message asserted the
        // opposite ("the desktop remembers that decision, so asking again does nothing"),
        // taken from portal documentation rather than from this machine, and was therefore
        // telling users to run a recovery command for a state they were not in.
        //
        // The reset command stays, because portals that *do* persist a refusal exist and
        // leave no other way back — but it is now the conditional half of the advice.
        let cause = Unavailable::PermissionDenied {
            app_id: "io.github.example.App".to_string(),
        };
        let reason = cause.reason();
        assert!(
            reason.contains("Restart"),
            "the common case is simply being asked again"
        );
        assert!(reason.contains("flatpak permission-reset io.github.example.App"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_appimage_integration_failure_names_the_failed_local_setup() {
        let cause = Unavailable::AppImageIntegration {
            detail: "permission denied while writing the desktop entry".to_string(),
        };
        let reason = cause.reason();

        assert!(reason.contains("AppImage desktop entry"));
        assert!(reason.contains("permission denied"));
        assert!(!reason.contains("dev-desktop-entry.sh"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_unrecognised_portal_error_is_passed_through_rather_than_guessed_at() {
        let cause = classify_portal_error(&example_app_id(), "Something new went wrong");
        assert_eq!(
            cause,
            Unavailable::PortalError {
                detail: "Something new went wrong".to_string()
            }
        );
    }

    #[test]
    fn a_registered_hotkey_offers_no_reason() {
        let status = HotkeyStatus::registered(Some("Ctrl+Alt+N".to_string()), true);
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
