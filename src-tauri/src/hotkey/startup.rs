//! Wiring the hotkey into a running app: register at launch, show the overlay when it fires.
//!
//! Split from [`super::portal`] so that module stays a driver with no opinion about windows
//! or app state, and this one holds the decisions that need both.
//!
//! Registration happens once, at startup, because binding can prompt and a prompt is only
//! acceptable as a first-run event. What it decides is stored, and the settings UI reads the
//! stored answer rather than re-running a handshake to redraw a panel.

use tauri::{AppHandle, Emitter, Manager};

use super::{portal, Availability, HotkeyStatus, Unavailable, STATUS_EVENT};
use crate::state::AppState;

type ActivationListener = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

struct RegistrationAttempt {
    status: HotkeyStatus,
    listener: Option<ActivationListener>,
}

impl RegistrationAttempt {
    fn inactive(status: HotkeyStatus) -> Self {
        Self {
            status,
            listener: None,
        }
    }

    fn active(status: HotkeyStatus, listener: ActivationListener) -> Self {
        Self {
            status,
            listener: Some(listener),
        }
    }

    /// Publish the initial truth before the listener can replace it with a later status.
    fn launch<Publish, Start>(self, publish_initial: Publish, start_listener: Start)
    where
        Publish: FnOnce(HotkeyStatus),
        Start: FnOnce(ActivationListener),
    {
        publish_initial(self.status);
        if let Some(listener) = self.listener {
            start_listener(listener);
        }
    }
}

/// Register the hotkey and keep answering it for as long as the app runs.
///
/// Never fails loudly: a desktop without the portal is an ordinary state, not an error, and
/// the app is fully usable without a global hotkey. What it must not do is fail *silently* —
/// every outcome is stored and announced, so the settings UI can say which one happened.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Independent of the hotkey itself, but piggybacks on this task rather than spawning
        // a second one for what is a handful of file operations at startup.
        resync_autostart(&app);

        // The window has to exist before the hotkey can be answered, and it is cheap: it is
        // created hidden and never loaded again, so the first press is a show, not a load.
        let readiness = super::window::ensure_window(&app)
            .await
            .map_err(|detail| Unavailable::CaptureWindow { detail });
        let attempt =
            match register_when_ready(readiness, || async { Ok(register(&app).await) }).await {
                Ok(attempt) => attempt,
                Err(cause) => RegistrationAttempt::inactive(HotkeyStatus::unavailable(&cause)),
            };
        // Logged, not only stored. An unregistered hotkey is invisible by nature — nothing
        // happens when the key is pressed — so the reason has to be somewhere a person can
        // read it without attaching a debugger.
        match (&attempt.status.availability, &attempt.status.reason) {
            (Availability::Available, _) => log::info!(
                "Quick capture hotkey registered: {}",
                attempt
                    .status
                    .trigger
                    .as_deref()
                    .unwrap_or("trigger not described")
            ),
            (_, Some(reason)) => log::warn!("Quick capture hotkey unavailable: {reason}"),
            (_, None) => {
                log::info!("Quick capture hotkey not registered: no vault is configured yet")
            }
        }
        attempt.launch(
            |status| publish(&app, status),
            |listener| {
                tauri::async_runtime::spawn(listener);
            },
        );
    });
}

/// Run portal registration only after every local prerequisite can answer the shortcut.
async fn register_when_ready<T, Register, Future>(
    readiness: Result<(), Unavailable>,
    register: Register,
) -> Result<T, Unavailable>
where
    Register: FnOnce() -> Future,
    Future: std::future::Future<Output = Result<T, Unavailable>>,
{
    readiness?;
    register().await
}

async fn register(app: &AppHandle) -> RegistrationAttempt {
    // No vault was ever configured: this is first run, the user has not set the app up yet,
    // and a permission dialog before they have chosen a vault is a prompt about a feature
    // they have not met. Say nothing and leave the hotkey unregistered.
    if !a_vault_was_configured(app) {
        return RegistrationAttempt::inactive(HotkeyStatus::unknown());
    }

    let app_id = match super::configured_application_id() {
        Ok(app_id) => app_id,
        Err(detail) => {
            return RegistrationAttempt::inactive(HotkeyStatus::unavailable(
                &Unavailable::AppIdRejected {
                    app_id: "<invalid>".to_string(),
                    detail,
                },
            ))
        }
    };

    // An AppImage installs no desktop entry, so the portal cannot resolve its app id until it
    // writes one for itself. This is a registration prerequisite, not a best-effort side effect:
    // proceeding after it fails would misreport the later portal rejection as an app-id problem.
    let appimage = super::desktop_entry::running_as_appimage();
    let data_home = data_home();
    if let Err(cause) = prepare_appimage_entry(
        appimage.as_deref(),
        data_home.as_deref(),
        |data_home, appimage| {
            super::desktop_entry::ensure_appimage_entry(data_home, &app_id, appimage)
        },
    ) {
        return RegistrationAttempt::inactive(HotkeyStatus::unavailable(&cause));
    }

    match portal::register(&app_id).await {
        Ok(registration) => {
            let status = HotkeyStatus::registered(
                registration.trigger().map(str::to_string),
                registration.supports_configuration(),
            );
            match registration.activations().await {
                Ok(activations) => {
                    let app = app.clone();
                    // The registration owns the portal session, so the listener has to own it:
                    // dropping it here would end the binding. Return the listener unspawned so
                    // startup can publish Available before any later status can replace it.
                    let listener: ActivationListener = Box::pin(async move {
                        use futures::StreamExt;
                        let mut activations = Box::pin(activations);
                        while activations.next().await.is_some() {
                            match on_activation(&app).await {
                                ActivationOutcome::Continue => {}
                                ActivationOutcome::EndSession(status) => {
                                    publish(&app, status);
                                    drop(registration);
                                    return;
                                }
                            }
                        }
                        // The stream ending means the session is gone, and with it the
                        // binding. Say so rather than leaving a stale "registered".
                        publish(
                            &app,
                            HotkeyStatus::unavailable(&Unavailable::PortalError {
                                detail: "The desktop ended the global shortcut session."
                                    .to_string(),
                            }),
                        );
                        drop(registration);
                    });
                    RegistrationAttempt::active(status, listener)
                }
                Err(error) => RegistrationAttempt::inactive(HotkeyStatus::unavailable(
                    &Unavailable::PortalError {
                        detail: error.to_string(),
                    },
                )),
            }
        }
        Err(cause) => RegistrationAttempt::inactive(HotkeyStatus::unavailable(&cause)),
    }
}

fn prepare_appimage_entry<Ensure, Error>(
    appimage: Option<&std::path::Path>,
    data_home: Option<&std::path::Path>,
    ensure: Ensure,
) -> Result<(), Unavailable>
where
    Ensure: FnOnce(
        &std::path::Path,
        &std::path::Path,
    ) -> Result<super::desktop_entry::Integration, Error>,
    Error: std::fmt::Display,
{
    let Some(appimage) = appimage else {
        return Ok(());
    };
    let data_home = data_home.ok_or_else(|| Unavailable::AppImageIntegration {
        detail: "no user data directory is available".to_string(),
    })?;
    let outcome =
        ensure(data_home, appimage).map_err(|error| Unavailable::AppImageIntegration {
            detail: error.to_string(),
        })?;
    log::info!("AppImage desktop entry: {outcome:?}");
    Ok(())
}

fn show_capture_window(app: &AppHandle) -> Result<(), Unavailable> {
    super::window::show_capture_window(app).map_err(|detail| Unavailable::CaptureWindow { detail })
}

/// The notification's id with the portal. Stable, so repeated presses against a vault that is
/// still missing replace one notification rather than stacking identical copies.
const VAULT_UNAVAILABLE_NOTIFICATION_ID: &str = "quick-capture-vault-unavailable";

/// Decide what a single press of the hotkey should do, and do it.
///
/// A vault becoming unreachable is not a hotkey failure — the shortcut is still bound and
/// still worth having — so unlike [`show_capture_window`]'s errors, this branch never ends
/// the session. It only ever asks the question fresh and says what it found.
async fn on_activation(app: &AppHandle) -> ActivationOutcome {
    match super::vault_status::activation_plan(
        configured_vault_path(app),
        super::vault_status::vault_is_reachable,
    ) {
        super::vault_status::ActivationPlan::ShowCaptureWindow => {
            activation_outcome(show_capture_window(app))
        }
        super::vault_status::ActivationPlan::NotifyVaultUnavailable { path } => {
            notify_vault_unavailable(&path).await;
            ActivationOutcome::Continue
        }
        super::vault_status::ActivationPlan::Nothing => ActivationOutcome::Continue,
    }
}

/// The vault path a capture would file into right now, if one was ever configured.
///
/// Read fresh on every activation rather than cached: this is a different question from "was
/// a vault ever configured", which only gates whether registration happens at all.
fn configured_vault_path(app: &AppHandle) -> Option<String> {
    app.state::<AppState>()
        .config
        .lock()
        .ok()
        .and_then(|config| config.active_vault.clone())
}

/// Tell the user why the hotkey did not open the overlay, rather than nothing happening.
///
/// Sent through the **portal**, like the shortcut itself, rather than through
/// `tauri-plugin-notification`.
///
/// That plugin was tried first and cannot work here. Its desktop path wraps `notify-rust`,
/// which drives a runtime of its own, and the plugin schedules that onto the tokio runtime
/// before calling it — so every press panicked with "Cannot start a runtime from within a
/// runtime" and the user saw nothing. Moving the *call site* to a blocking pool thread, and
/// then to a plain OS thread, changed nothing, because the hop onto the runtime happens
/// inside the plugin. Measured on 2026-09-02 across all three attempts; the backtrace names
/// `tauri_plugin_notification::desktop::imp::Notification::show` polled as a tokio task.
///
/// `ashpd`'s notification portal is async to begin with, so it composes with the runtime
/// instead of fighting it, and it is the same dependency and the same bus connection the
/// shortcut already uses.
///
/// The id is stable, so leaning on the hotkey repeatedly replaces one notification rather
/// than stacking up identical copies.
async fn notify_vault_unavailable(path: &str) {
    let notice = super::vault_status::vault_unavailable_notice(path);
    let sent = async {
        ashpd::desktop::notification::NotificationProxy::new()
            .await?
            .add_notification(
                VAULT_UNAVAILABLE_NOTIFICATION_ID,
                ashpd::desktop::notification::Notification::new(&notice.title)
                    .body(notice.body.as_str())
                    .priority(ashpd::desktop::notification::Priority::Normal),
            )
            .await
    }
    .await;

    // Logged and swallowed, not escalated: a notification that did not get through does not
    // mean the hotkey is broken, only that this one explanation was missed.
    if let Err(error) = sent {
        log::warn!("Could not show the vault-unavailable notification: {error}");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActivationOutcome {
    Continue,
    EndSession(HotkeyStatus),
}

fn activation_outcome(result: Result<(), Unavailable>) -> ActivationOutcome {
    match result {
        Ok(()) => ActivationOutcome::Continue,
        Err(cause) => ActivationOutcome::EndSession(HotkeyStatus::unavailable(&cause)),
    }
}

fn publish(app: &AppHandle, status: HotkeyStatus) {
    if let Ok(mut stored) = app.state::<AppState>().hotkey_status.lock() {
        *stored = status.clone();
    }
    let _ = app.emit(STATUS_EVENT, status);
}

/// Make the on-disk autostart entry match the stored preference.
///
/// Run once per launch, unconditionally of whether the hotkey itself registers: the two are
/// unrelated features that happen to share a settings panel. Failure is logged and swallowed
/// — a missing autostart entry is an inconvenience the user can retoggle, not a reason to
/// affect anything else the app is doing at startup.
fn resync_autostart(app: &AppHandle) {
    let Some(config_home) = dirs::config_dir() else {
        return;
    };
    let Some(exec) = super::desktop_entry::current_executable() else {
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

    if let Err(error) = crate::autostart::sync(enabled, &config_home, &app_id, &exec) {
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

fn data_home() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(dirs::data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn a_failed_startup_prerequisite_prevents_portal_registration() {
        let registration_attempted = Arc::new(AtomicBool::new(false));
        let observed = registration_attempted.clone();
        let runtime = tokio::runtime::Runtime::new().expect("runtime starts");
        let result = runtime.block_on(register_when_ready::<(), _, _>(
            Err(Unavailable::CaptureWindow {
                detail: "capture window config is missing".to_string(),
            }),
            move || async move {
                observed.store(true, Ordering::SeqCst);
                Ok(())
            },
        ));

        assert!(matches!(result, Err(Unavailable::CaptureWindow { .. })));
        assert!(!registration_attempted.load(Ordering::SeqCst));
    }

    #[test]
    fn an_appimage_without_a_user_data_directory_is_not_ready_to_register() {
        let result = prepare_appimage_entry(
            Some(std::path::Path::new("/home/u/SecondBrain.AppImage")),
            None,
            |_, _| {
                Ok::<_, std::io::Error>(super::super::desktop_entry::Integration::AlreadyCurrent)
            },
        );

        assert!(matches!(
            result,
            Err(Unavailable::AppImageIntegration { ref detail })
                if detail.contains("user data directory")
        ));
    }

    #[test]
    fn a_failed_activation_ends_the_registered_session_with_an_unavailable_status() {
        let outcome = activation_outcome(Err(Unavailable::CaptureWindow {
            detail: "the compositor refused to focus the capture window".to_string(),
        }));

        let ActivationOutcome::EndSession(status) = outcome else {
            panic!("an unusable capture window must end the portal session");
        };
        assert_eq!(status.availability, Availability::Unavailable);
        assert!(status
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("refused to focus")));
    }

    #[test]
    fn available_is_published_before_the_activation_listener_can_run() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener_events = events.clone();
        let attempt = RegistrationAttempt {
            status: HotkeyStatus::registered(Some("Ctrl+Alt+N".to_string()), true),
            listener: Some(Box::pin(async move {
                listener_events.lock().unwrap().push("listener");
            })),
        };

        let publish_events = events.clone();
        attempt.launch(
            move |_| publish_events.lock().unwrap().push("available"),
            |listener| {
                tokio::runtime::Runtime::new()
                    .expect("runtime starts")
                    .block_on(listener)
            },
        );

        assert_eq!(*events.lock().unwrap(), ["available", "listener"]);
    }
}
