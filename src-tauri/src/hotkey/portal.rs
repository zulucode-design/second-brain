//! The Linux side of the hotkey: the XDG `GlobalShortcuts` portal driver.
//!
//! Everything here talks to the desktop; the decisions it makes are in the parent module,
//! where they can be tested without a session bus. This file is the part that cannot be
//! unit-tested, so it is kept as thin as the protocol allows.
//!
//! The call order is fixed and each step exists for a reason:
//!
//! 1. `Registry.Register` — a non-sandboxed app has no app id the portal can see, so
//!    without this the portal answers "An app id is required" and nothing else works.
//! 2. `CreateSession` — where GNOME validates the app id against installed desktop entries.
//! 3. `ListShortcuts` — asks what is already bound, so step 4 can be skipped.
//! 4. `BindShortcuts`, **only if not already bound** — this is the call that shows the
//!    permission dialog, and a dialog shown on every launch is the thing users remember.
//!
//! Verified against xdg-desktop-portal on GNOME/Wayland, 2026-09-01: the first run prompts
//! and binds (18.8s, nearly all of it the dialog); the second run finds the shortcut already
//! bound, skips `BindShortcuts`, and completes in 0.03s without prompting. GNOME honoured the
//! preferred trigger and described it as "Press <Control><Alt>n" — a sentence, not an
//! accelerator, which is what the settings UI has to render.
//!
//! The session must outlive registration: closing it drops the binding and ends the
//! `Activated` signal stream. So [`Registration`] owns it, and the hotkey lasts exactly as
//! long as the value is held.

use ashpd::desktop::global_shortcuts::{
    Activated, BindShortcutsOptions, ConfigureShortcutsOptions, GlobalShortcuts,
    ListShortcutsOptions, NewShortcut, Shortcut,
};
use ashpd::desktop::{CreateSessionOptions, ResponseError, Session};
use ashpd::{AppID, Error as PortalError};
use futures::{Stream, StreamExt};

use super::{
    classify_portal_error, needs_binding, ApplicationId, Unavailable, PREFERRED_TRIGGER,
    SHORTCUT_DESCRIPTION, SHORTCUT_ID,
};

/// A live registration: the session that holds the binding, plus what the compositor bound.
///
/// There is no explicit teardown, and no `Drop` impl: the portal ends the session when our
/// bus connection goes, which for a desktop app is when the process exits. Holding this value
/// is what keeps [`Registration::activations`] producing.
pub struct Registration {
    proxy: GlobalShortcuts,
    /// Doubles as a liveness guard: the portal ends the session when this value drops, and
    /// with it the binding and the `Activated` stream, so its lifetime *is* the hotkey's
    /// lifetime. Also read directly by [`Registration::configure`].
    session: Session<GlobalShortcuts>,
    trigger: Option<String>,
    supports_configuration: bool,
}

/// The `GlobalShortcuts` interface version that first offered `ConfigureShortcuts`.
///
/// Below this, there is no way for an app to open the desktop's shortcut editor, and the
/// settings UI must not offer a button that can only fail. GNOME 50 / xdg-desktop-portal-gnome
/// 50.0 publishes version 1 — measured on the target machine, 2026-09-02, by clicking the
/// button and getting "This interface requires version 2, but 1 is available".
const CONFIGURE_SHORTCUTS_VERSION: u32 = 2;

impl Registration {
    /// What the compositor bound, as it described it. For display only, never parsed: the
    /// app asked for a trigger, it did not choose one.
    pub fn trigger(&self) -> Option<&str> {
        self.trigger.as_deref()
    }

    /// Fires once per press of the shortcut.
    ///
    /// Filtered by shortcut id rather than session handle: this process holds exactly one
    /// session, and the portal delivers `Activated` only to the session's owner, so the id
    /// is the only distinction that can matter here.
    pub async fn activations(&self) -> Result<impl Stream<Item = ()> + use<>, PortalError> {
        let stream = self.proxy.receive_activated().await?;
        Ok(stream.filter_map(|activated: Activated| async move {
            (activated.shortcut_id() == SHORTCUT_ID).then_some(())
        }))
    }

    /// Whether this desktop can open its own shortcut editor on request.
    ///
    /// False on portals older than `ConfigureShortcuts`, where the user has to reach the
    /// desktop's keyboard settings themselves. Asked once, at registration, so the settings
    /// UI can decide what to offer instead of finding out when a button fails.
    pub fn supports_configuration(&self) -> bool {
        self.supports_configuration
    }

    /// Open the desktop's own shortcut-configuration UI for this session.
    ///
    /// The app cannot change the binding itself — ADR-0001 — so this is the entirety of
    /// "change the hotkey": hand the user to the compositor that actually owns it. The call
    /// blocks until the dialog closes, which is what lets a caller safely close a temporary
    /// session right after this returns without cutting the dialog off.
    pub async fn configure(&self) -> Result<(), PortalError> {
        self.proxy
            .configure_shortcuts(&self.session, None, ConfigureShortcutsOptions::default())
            .await
    }

    /// End this session on the portal side, rather than leaving it to be cleaned up only when
    /// the process exits. For the long-lived registration that holds the `Activated` stream,
    /// letting it live for the process is correct and this is never called. For a temporary
    /// session created solely to call [`Registration::configure`], leaving it dangling for
    /// the rest of the run would accumulate one dead session per settings-button press.
    pub async fn close(&self) -> Result<(), PortalError> {
        self.session.close().await
    }
}

/// Run the handshake described in the module docs, binding the shortcut if it is not
/// already bound.
///
/// The `Err` side is always something the user can be told, never a raw protocol error.
pub async fn register(app_id: &ApplicationId) -> Result<Registration, Unavailable> {
    let parsed: AppID = app_id
        .as_str()
        .try_into()
        .map_err(|_| Unavailable::AppIdRejected {
            app_id: app_id.to_string(),
            detail: "not a valid application id".to_string(),
        })?;

    log::debug!("portal: registering host app id {app_id}");
    // Harmless and skipped inside a sandbox, where the app id is already known to the portal.
    match ashpd::register_host_app(parsed).await {
        Ok(()) => {}
        // Already done, on this same connection, by an earlier handshake in this process.
        Err(err) if is_already_registered(&err) => {
            log::debug!("portal: app id already registered on this connection");
        }
        Err(err) => return Err(classify(app_id, &err)),
    }

    log::debug!("portal: opening the GlobalShortcuts proxy");
    let proxy = GlobalShortcuts::new()
        .await
        .map_err(|err| classify(app_id, &err))?;
    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(|err| classify(app_id, &err))?;

    log::debug!("portal: session created, listing shortcuts");
    let listed = proxy
        .list_shortcuts(&session, ListShortcutsOptions::default())
        .await
        .and_then(|request| request.response())
        .map_err(|err| classify(app_id, &err))?;

    log::debug!(
        "portal: {} shortcut(s) already bound",
        listed.shortcuts().len()
    );
    let bound: Vec<Shortcut> = if needs_binding(listed.shortcuts().iter().map(Shortcut::id)) {
        let shortcut = NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION)
            .preferred_trigger(PREFERRED_TRIGGER);
        proxy
            .bind_shortcuts(&session, &[shortcut], None, BindShortcutsOptions::default())
            .await
            .and_then(|request| request.response())
            .map_err(|err| classify_bind(app_id, &err))?
            .shortcuts()
            .to_vec()
    } else {
        listed.shortcuts().to_vec()
    };

    let supports_configuration = proxy.version() >= CONFIGURE_SHORTCUTS_VERSION;
    log::debug!(
        "portal: GlobalShortcuts version {}, configuration UI {}",
        proxy.version(),
        if supports_configuration {
            "available"
        } else {
            "unavailable"
        }
    );

    Ok(Registration {
        trigger: trigger_of(&bound),
        proxy,
        session,
        supports_configuration,
    })
}

/// The description of our shortcut's trigger, if the portal reported one.
///
/// A reply that does not mention the shortcut is not an error: GNOME has answered a bind
/// with an empty list before the user has finished with the dialog. The binding exists;
/// there is just nothing to display yet.
fn trigger_of(shortcuts: &[Shortcut]) -> Option<String> {
    shortcuts
        .iter()
        .find(|shortcut| shortcut.id() == SHORTCUT_ID)
        .map(|shortcut| shortcut.trigger_description().to_string())
        .filter(|description| !description.is_empty())
}

/// Whether this connection has already claimed an app id.
///
/// `Registry.Register` is once per bus connection, and `ashpd` hands every caller in a process
/// the same session connection. So the second handshake — the one the settings button runs to
/// reach `ConfigureShortcuts` — always hits this, and it means "already done", not "failed".
/// Treating it as an error made the button report the shortcut as refused while the shortcut
/// was working perfectly. Observed 2026-09-02, on the first click.
///
/// Matched on text because the portal reports it as a generic `Failed`, the same shape as
/// genuinely fatal registration problems.
fn is_already_registered(error: &PortalError) -> bool {
    error
        .to_string()
        .to_ascii_lowercase()
        .contains("already associated with an application id")
}

/// Classify a failure of `BindShortcuts` specifically.
///
/// `BindShortcuts` is the only call in the handshake that shows a dialog, so **any**
/// unsuccessful response to it is the user declining that dialog. That matters because the
/// decision is remembered: every later attempt then fails without prompting, and a message
/// that does not name `flatpak permission-reset` leaves the user with a hotkey that can never
/// come back and no way to find out why.
///
/// The general classifier is not enough here. It maps `Cancelled` to a denial, which is what
/// the response type was assumed to be — but GNOME answers a dismissed dialog with
/// `Other` (response type 2), not `Cancelled` (1). Measured on 2026-09-02 by dismissing the
/// real dialog: the reason shown was the generic "Portal request didn't succeed with no
/// information", precisely the vague message ADR-0001 exists to prevent.
fn classify_bind(app_id: &ApplicationId, err: &PortalError) -> Unavailable {
    match err {
        PortalError::Response(_) => Unavailable::PermissionDenied {
            app_id: app_id.to_string(),
        },
        other => classify(app_id, other),
    }
}

/// Turn a portal error into something the user can act on.
///
/// Two shapes get named before falling back to the message-text classifier: a desktop with
/// no `GlobalShortcuts` implementation at all, and a cancelled request. A cancelled request
/// is the permission dialog being dismissed, which the desktop remembers — the case that
/// otherwise looks like the hotkey silently not working.
fn classify(app_id: &ApplicationId, err: &PortalError) -> Unavailable {
    match err {
        PortalError::PortalNotFound(_) => Unavailable::NoPortal,
        PortalError::Response(ResponseError::Cancelled) => Unavailable::PermissionDenied {
            app_id: app_id.to_string(),
        },
        other => classify_portal_error(app_id, &other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ashpd::zvariant::{serialized, to_bytes, Endian, Value};
    use std::collections::HashMap;

    /// `Shortcut` has no public constructor — it only ever arrives deserialized from the
    /// portal — so build one the way the bus would.
    fn shortcut(id: &str, trigger_description: &str) -> Shortcut {
        let info = HashMap::from([
            ("description", Value::from(SHORTCUT_DESCRIPTION)),
            ("trigger_description", Value::from(trigger_description)),
        ]);
        let ctx = serialized::Context::new_dbus(Endian::native(), 0);
        let bytes = to_bytes(ctx, &(id.to_string(), info)).expect("shortcut encodes");
        bytes.deserialize::<Shortcut>().expect("shortcut decodes").0
    }

    #[test]
    fn the_trigger_shown_is_the_one_the_compositor_reported_for_our_shortcut() {
        // Other apps' shortcuts can appear in the same reply; ours is the only one to show.
        let bound = vec![
            shortcut("someone-elses", "Super+K"),
            shortcut(SHORTCUT_ID, "Ctrl+Alt+N"),
        ];
        assert_eq!(trigger_of(&bound).as_deref(), Some("Ctrl+Alt+N"));
    }

    #[test]
    fn a_bind_reply_that_says_nothing_about_the_trigger_displays_nothing() {
        // Not an error: the binding exists, there is just no description to show yet, and
        // an empty string in the settings UI would read as a broken hotkey.
        assert_eq!(trigger_of(&[]), None);
        assert_eq!(trigger_of(&[shortcut(SHORTCUT_ID, "")]), None);
    }

    /// The app id the app will actually ship with, read from the config rather than repeated
    /// here. A test that registers a different id from the product proves nothing.
    fn configured_app_id() -> ApplicationId {
        super::super::configured_application_id().expect("configured app id is valid")
    }

    /// The whole handshake against the real portal. Not run by default, and it cannot be:
    /// it needs a desktop session, an installed desktop entry matching the app id
    /// (`scripts/dev-desktop-entry.sh`), and a person to answer the permission dialog. The
    /// dialog's answer is remembered per app id, so a dismissal here is undone only with
    /// `flatpak permission-reset $(the id below)`.
    ///
    ///     cargo test --lib -- --ignored registers_against_the_real_portal --nocapture
    #[test]
    #[ignore = "needs a desktop session and a person to answer the permission dialog"]
    fn registers_against_the_real_portal() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime starts");
        match runtime.block_on(register(&configured_app_id())) {
            Ok(registration) => println!("registered; trigger: {:?}", registration.trigger()),
            Err(cause) => panic!("{}", cause.reason()),
        }
    }

    #[test]
    fn a_desktop_without_the_portal_is_reported_as_such_rather_than_as_an_error() {
        let missing = PortalError::PortalNotFound(
            ashpd::zbus::names::InterfaceName::try_from("org.freedesktop.portal.GlobalShortcuts")
                .unwrap()
                .into(),
        );
        assert_eq!(
            classify(
                &ApplicationId::parse("io.github.example.App").expect("example id is valid"),
                &missing
            ),
            Unavailable::NoPortal
        );
    }

    #[test]
    fn a_second_handshake_on_the_same_connection_is_not_a_failure() {
        // Verbatim from the portal on 2026-09-02, when the settings button ran a second
        // handshake to reach ConfigureShortcuts. Reporting this as an error told the user the
        // shortcut had been refused while it was working.
        let already = PortalError::Portal(ashpd::PortalError::Failed(
            "Could not register app ID: Connection already associated with an application ID"
                .to_string(),
        ));
        assert!(is_already_registered(&already));
    }

    #[test]
    fn a_genuine_registration_failure_is_still_a_failure() {
        let missing_entry = PortalError::Portal(ashpd::PortalError::Failed(
            "Could not register app ID: App info not found for 'io.github.example.App'".to_string(),
        ));
        assert!(!is_already_registered(&missing_entry));
    }

    #[test]
    fn every_unsuccessful_bind_response_is_a_denial_naming_the_reset_command() {
        // BindShortcuts is the only call that shows a dialog, so any unsuccessful response to
        // it is the user declining. Both variants, because the assumption that a dismissal
        // arrives as Cancelled was wrong: GNOME answers with Other, and that fell through to
        // "Portal request didn't succeed with no information" — measured 2026-09-02 against
        // the real dialog, and exactly the vague message this case exists to avoid.
        for response in [ResponseError::Cancelled, ResponseError::Other] {
            let cause = classify_bind(
                &ApplicationId::parse("io.github.example.App").expect("example id is valid"),
                &PortalError::Response(response),
            );
            assert_eq!(
                cause,
                Unavailable::PermissionDenied {
                    app_id: "io.github.example.App".to_string()
                },
                "{response:?} must be reported as a denial"
            );
            assert!(cause.reason().contains("flatpak permission-reset"));
        }
    }

    #[test]
    fn a_bind_failure_that_is_not_a_response_keeps_its_own_cause() {
        // A missing portal during bind is still a missing portal, not a denial.
        let missing = PortalError::PortalNotFound(
            ashpd::zbus::names::InterfaceName::try_from("org.freedesktop.portal.GlobalShortcuts")
                .unwrap()
                .into(),
        );
        assert_eq!(
            classify_bind(
                &ApplicationId::parse("io.github.example.App").expect("example id is valid"),
                &missing
            ),
            Unavailable::NoPortal
        );
    }

    #[test]
    fn a_dismissed_permission_dialog_is_a_denial_the_user_can_undo() {
        // The portal reports a dismissed dialog as a cancelled request, and remembers it,
        // so anything less specific here leaves the user with a hotkey that never works.
        let cancelled = PortalError::Response(ResponseError::Cancelled);
        assert_eq!(
            classify(
                &ApplicationId::parse("io.github.example.App").expect("example id is valid"),
                &cancelled
            ),
            Unavailable::PermissionDenied {
                app_id: "io.github.example.App".to_string()
            }
        );
    }
}
