//! Showing the capture overlay — the one piece of hotkey handling with no opinion about
//! Linux or Windows at all, since it is pure Tauri window management either way.
//!
//! `ensure_window`/`show_capture_window` return a bare `String` rather than either
//! backend's own `Unavailable`, so a caller on either platform wraps it into its own
//! `CaptureWindow(CaptureWindowUnavailable)` variant. Sharing the *error enum* would mean
//! choosing one backend's taxonomy to be the "real" one and the other importing it —
//! exactly the coupling ADR-0001 argues against — when what is actually shared is only
//! this function's behaviour, not either platform's classification of what else can go
//! wrong. [`CaptureWindowUnavailable`] is the one exception: both platforms' failure here
//! is identical in cause and in the message to show, so it is a single leaf type both
//! enums hold rather than two copies of the same variant and the same wording.

use tauri::{AppHandle, Emitter, Manager};

use super::{Cause, SHOWN_EVENT, WINDOW_LABEL};

/// The capture window could not be prepared or shown. Shared by both backends' `Unavailable`
/// enums (`super::Unavailable::CaptureWindow` on Linux, `super::windows::Unavailable::CaptureWindow`
/// on Windows) because the cause and the message are the same regardless of which backend
/// asked: this module is what does the work, and it fails the same way either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureWindowUnavailable(pub String);

impl Cause for CaptureWindowUnavailable {
    fn reason(&self) -> String {
        format!(
            "Quick capture could not prepare its capture window ({}). Restart the app; if \
             this continues, report this error.",
            self.0
        )
    }
}

/// Make sure the capture window exists and is hidden, building it if Tauri did not.
///
/// **Not called from `setup`.** Building a window synchronously there deadlocks on Linux: the
/// call waits on a GTK event loop that has not started yet, `setup` never returns, and the app
/// runs on looking healthy because the main window already exists and background tasks are on
/// their own threads. Observed 2026-09-02, and it costs an afternoon to find, because nothing
/// errors — the builder simply never comes back. Nothing about this call is Linux-specific, so
/// it runs after the event loop is up there too rather than re-deriving that this particular
/// deadlock does not apply.
///
/// Hides the window explicitly rather than trusting the config's own `"visible": false` to
/// have held: on Windows, confirmed 2026-09-05, Tauri creates this window from config *before*
/// this function ever runs — the `is_some()` branch below is what finds it — and it comes up
/// visible regardless of that flag, parked over whatever else is on screen and outliving
/// `main`'s close since it is a real top-level window Tauri does not otherwise know to tie to
/// the app's lifecycle. Asserting the hidden state here rather than assuming it holds is what
/// this function's own name already promises.
pub async fn ensure_window(app: &AppHandle) -> Result<(), String> {
    let window = if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        window
    } else {
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
        let window = tauri::WebviewWindowBuilder::from_config(app, &config)
            .and_then(|builder| builder.build())
            .map_err(|error| error.to_string())?;
        log::debug!("Capture window ready");
        window
    };
    window.hide().map_err(|error| error.to_string())
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
