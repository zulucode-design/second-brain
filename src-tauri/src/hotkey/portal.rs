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
    Activated, BindShortcutsOptions, GlobalShortcuts, ListShortcutsOptions, NewShortcut, Shortcut,
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
    session: Session<GlobalShortcuts>,
    trigger: Option<String>,
}

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

    // Harmless and skipped inside a sandbox, where the app id is already known to the portal.
    ashpd::register_host_app(parsed)
        .await
        .map_err(|err| classify(app_id, &err))?;

    let proxy = GlobalShortcuts::new()
        .await
        .map_err(|err| classify(app_id, &err))?;
    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(|err| classify(app_id, &err))?;

    let listed = proxy
        .list_shortcuts(&session, ListShortcutsOptions::default())
        .await
        .and_then(|request| request.response())
        .map_err(|err| classify(app_id, &err))?;

    let bound: Vec<Shortcut> = if needs_binding(listed.shortcuts().iter().map(Shortcut::id)) {
        let shortcut = NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION)
            .preferred_trigger(PREFERRED_TRIGGER);
        proxy
            .bind_shortcuts(&session, &[shortcut], None, BindShortcutsOptions::default())
            .await
            .and_then(|request| request.response())
            .map_err(|err| classify(app_id, &err))?
            .shortcuts()
            .to_vec()
    } else {
        listed.shortcuts().to_vec()
    };

    Ok(Registration {
        trigger: trigger_of(&bound),
        proxy,
        session,
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
