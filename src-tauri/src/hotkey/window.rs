//! Showing the capture overlay — the one piece of hotkey handling with no opinion about
//! Linux or Windows at all, since it is pure Tauri window management either way.
//!
//! Returns a bare `String` rather than either backend's own `Unavailable`, so a caller on
//! either platform wraps it into its own `CaptureWindow { detail }` variant. Sharing the
//! *error type* here would mean choosing one backend's enum to be the "real" one and the
//! other importing it — exactly the coupling ADR-0001 argues against — when what is
//! actually shared is only this function's behaviour, not either platform's taxonomy of
//! what can go wrong.

use tauri::{AppHandle, Emitter, Manager};

use super::{SHOWN_EVENT, WINDOW_LABEL};

/// Make sure the capture window exists, building it if Tauri did not.
///
/// **Not called from `setup`.** Building a window synchronously there deadlocks on Linux: the
/// call waits on a GTK event loop that has not started yet, `setup` never returns, and the app
/// runs on looking healthy because the main window already exists and background tasks are on
/// their own threads. Observed 2026-09-02, and it costs an afternoon to find, because nothing
/// errors — the builder simply never comes back. Untested on Windows, but nothing about this
/// call is Linux-specific, so it runs after the event loop is up there too rather than
/// re-deriving that this particular deadlock does not apply.
pub async fn ensure_window(app: &AppHandle) -> Result<(), String> {
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
        return Err(format!(
            "the application configuration declares no {WINDOW_LABEL:?} window"
        ));
    };
    tauri::WebviewWindowBuilder::from_config(app, &config)
        .and_then(|builder| builder.build())
        .map_err(|error| error.to_string())?;
    log::debug!("Capture window ready");
    Ok(())
}

/// Bring the overlay up and put the caret in it.
///
/// The window already exists, hidden, created at startup: showing it is a compositor
/// operation, where creating it would be a WebView load with the user waiting on it.
pub fn show_capture_window(app: &AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return Err(format!(
            "the capture window is missing; available windows: {:?}",
            app.webview_windows().keys().collect::<Vec<_>>()
        ));
    };
    window
        .show()
        .and_then(|()| window.set_focus())
        .map_err(|error| error.to_string())?;
    // The field decides its own focus: the window may have been shown while the webview was
    // still mounting, and only it knows when the textarea exists.
    window
        .emit(SHOWN_EVENT, ())
        .map_err(|error| error.to_string())
}
