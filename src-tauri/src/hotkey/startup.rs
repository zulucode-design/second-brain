//! Wiring the hotkey into a running app: register at launch, show the overlay when it fires.
//!
//! Split from [`super::portal`] so that module stays a driver with no opinion about windows
//! or app state, and this one holds the decisions that need both.
//!
//! Registration happens once, at startup, because binding can prompt and a prompt is only
//! acceptable as a first-run event. What it decides is stored, and the settings UI reads the
//! stored answer rather than re-running a handshake to redraw a panel.

use tauri::{AppHandle, Emitter, Manager};

use super::{
    portal, Availability, HotkeyStatus, Unavailable, SHOWN_EVENT, STATUS_EVENT, WINDOW_LABEL,
};
use crate::state::AppState;

/// Make sure the capture window exists, building it if Tauri did not.
///
/// **Not called from `setup`.** Building a window synchronously there deadlocks on Linux: the
/// call waits on a GTK event loop that has not started yet, `setup` never returns, and the app
/// runs on looking healthy because the main window already exists and background tasks are on
/// their own threads. Observed 2026-09-02, and it costs an afternoon to find, because nothing
/// errors — the builder simply never comes back.
///
/// So this runs on the async runtime, after the loop is up.
async fn ensure_window(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return Ok(());
    }
    let Some(config) = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == WINDOW_LABEL)
        .cloned()
    else {
        log::error!("tauri.conf.json declares no {WINDOW_LABEL:?} window");
        return Ok(());
    };
    tauri::WebviewWindowBuilder::from_config(app, &config)?.build()?;
    log::debug!("Capture window ready");
    Ok(())
}

/// Register the hotkey and keep answering it for as long as the app runs.
///
/// Never fails loudly: a desktop without the portal is an ordinary state, not an error, and
/// the app is fully usable without a global hotkey. What it must not do is fail *silently* —
/// every outcome is stored and announced, so the settings UI can say which one happened.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // The window has to exist before the hotkey can be answered, and it is cheap: it is
        // created hidden and never loaded again, so the first press is a show, not a load.
        if let Err(error) = ensure_window(&app).await {
            log::error!("Could not create the capture window: {error}");
        }
        let status = register(&app).await;
        // Logged, not only stored. An unregistered hotkey is invisible by nature — nothing
        // happens when the key is pressed — so the reason has to be somewhere a person can
        // read it without attaching a debugger.
        match (&status.availability, &status.reason) {
            (Availability::Available, _) => log::info!(
                "Quick capture hotkey registered: {}",
                status.trigger.as_deref().unwrap_or("trigger not described")
            ),
            (_, Some(reason)) => log::warn!("Quick capture hotkey unavailable: {reason}"),
            (_, None) => {
                log::info!("Quick capture hotkey not registered: no vault is configured yet")
            }
        }
        publish(&app, status);
    });
}

async fn register(app: &AppHandle) -> HotkeyStatus {
    // No vault was ever configured: this is first run, the user has not set the app up yet,
    // and a permission dialog before they have chosen a vault is a prompt about a feature
    // they have not met. Say nothing and leave the hotkey unregistered.
    if !a_vault_was_configured(app) {
        return HotkeyStatus::unknown();
    }

    let app_id = match super::configured_application_id() {
        Ok(app_id) => app_id,
        Err(detail) => {
            return HotkeyStatus::unavailable(&Unavailable::AppIdRejected {
                app_id: "<invalid>".to_string(),
                detail,
            })
        }
    };

    // An AppImage installs no desktop entry, so the portal cannot resolve its app id until it
    // writes one for itself. Failing here is not fatal: registration then reports
    // AppIdRejected, which names the cause.
    if let Some(appimage) = super::desktop_entry::running_as_appimage() {
        if let Some(data_home) = data_home() {
            match super::desktop_entry::ensure_appimage_entry(&data_home, &app_id, &appimage) {
                Ok(outcome) => log::info!("AppImage desktop entry: {outcome:?}"),
                Err(error) => log::warn!("Could not write the AppImage desktop entry: {error}"),
            }
        }
    }

    match portal::register(&app_id).await {
        Ok(registration) => {
            let status = HotkeyStatus::registered(registration.trigger().map(str::to_string));
            match registration.activations().await {
                Ok(activations) => {
                    let app = app.clone();
                    // The registration owns the portal session, so the task that listens has
                    // to own the registration: dropping it here would end the binding.
                    tauri::async_runtime::spawn(async move {
                        use futures::StreamExt;
                        let mut activations = Box::pin(activations);
                        while activations.next().await.is_some() {
                            show_capture_window(&app);
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
                    status
                }
                Err(error) => HotkeyStatus::unavailable(&Unavailable::PortalError {
                    detail: error.to_string(),
                }),
            }
        }
        Err(cause) => HotkeyStatus::unavailable(&cause),
    }
}

/// Bring the overlay up and put the caret in it.
///
/// The window already exists, hidden, created at startup: showing it is a compositor
/// operation, where creating it would be a WebView load with the user waiting on it.
fn show_capture_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        log::error!(
            "The capture window is missing, so the hotkey has nothing to show. Windows: {:?}",
            app.webview_windows().keys().collect::<Vec<_>>()
        );
        return;
    };
    if let Err(error) = window.show().and_then(|()| window.set_focus()) {
        log::warn!("Could not show the capture window: {error}");
        return;
    }
    // The field decides its own focus: the window may have been shown while the webview was
    // still mounting, and only it knows when the textarea exists.
    let _ = window.emit(SHOWN_EVENT, ());
}

fn publish(app: &AppHandle, status: HotkeyStatus) {
    if let Ok(mut stored) = app.state::<AppState>().hotkey_status.lock() {
        *stored = status.clone();
    }
    let _ = app.emit(STATUS_EVENT, status);
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
