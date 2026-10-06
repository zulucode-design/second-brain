//! The capture-time similarity check (#9): after a note is saved, find existing notes that say
//! the same thing, so the user can fold the new one in instead of keeping a near-duplicate.
//!
//! The check only ever runs after the note is safely on disk, and every failure ends it quietly:
//! a capture must never wait on it or be lost to it.

use crate::semantic_search::SemanticIndex;
use crate::state::AppState;
use crate::vault::operations;
use serde::Serialize;
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::{AppHandle, Emitter, Manager};

/// The score a note must reach to count as saying the same thing. Calibrated against live
/// embeddinggemma on scripts/similarity-fixture.json (#9): its 8 reworded duplicates scored
/// 0.714 to 0.850 against their notes, and no other pair, same-topic notes included, passed
/// 0.533. The bar sits midway. Recalibrate when the embedding model changes.
const SIMILAR_SCORE: f32 = 0.62;
/// How many similar notes a card shows; the rest are only counted.
const SHOWN: usize = 3;
/// Shorter text embeds too loosely to call anything a duplicate.
const MIN_WORDS: usize = 3;

/// A just-written note and the existing notes most like it, each with the revision it had when
/// checked, so nothing is appended to a note that changed since the user saw it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarityCheck {
    pub capture: CheckedNote,
    pub matches: Vec<SimilarMatch>,
    /// Every note that passed the bar, of which `matches` holds the best few.
    pub total: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckedNote {
    pub path: String,
    pub title: String,
    pub revision: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarMatch {
    pub path: String,
    pub title: String,
    /// The passage that matched best: why the note was picked.
    pub excerpt: String,
    pub revision: String,
}

/// The existing notes most like the note at `path`, or `None` when there are none or the note
/// is too short to judge.
pub fn check(
    index: &SemanticIndex,
    vault: &str,
    path: &str,
) -> Result<Option<SimilarityCheck>, String> {
    let note = operations::read_note(vault, path)?;
    let words =
        note.meta.title.split_whitespace().count() + note.content.split_whitespace().count();
    if words < MIN_WORDS {
        return Ok(None);
    }
    let found = index.similar(
        &note.meta.title,
        &note.content,
        &note.meta.id,
        SIMILAR_SCORE,
        SHOWN,
    )?;
    let matches: Vec<SimilarMatch> = found
        .notes
        .into_iter()
        // Deleted or unreadable since it was indexed: nothing to append to.
        .filter_map(|result| {
            let existing = operations::read_note(vault, &result.path).ok()?;
            Some(SimilarMatch {
                path: result.path,
                title: result.title,
                excerpt: result.snippet,
                revision: existing.revision,
            })
        })
        .collect();
    if matches.is_empty() {
        return Ok(None);
    }
    Ok(Some(SimilarityCheck {
        capture: CheckedNote {
            path: note.path,
            title: note.meta.title,
            revision: note.revision,
        },
        matches,
        total: found.total,
    }))
}

/// [`check`] against the open vault, with every failure logged and treated as "nothing found":
/// no index, an unreachable embedding backend, or a note that vanished.
pub fn check_now(state: &AppState, path: &str) -> Option<SimilarityCheck> {
    let index = state.semantic_index.lock().ok()?.clone()?;
    let vault = state.config.lock().ok()?.active_vault.clone()?;
    match check(&index, &vault, path) {
        Ok(found) => found,
        Err(error) => {
            log::info!("Skipped the similarity check for a new note: {error}");
            None
        }
    }
}

/// `existing` with the capture added at the end under a dated marker, title line included.
pub fn appended(existing: &str, capture_title: &str, capture_body: &str, date: &str) -> String {
    let capture = [capture_title.trim(), capture_body.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "{}\n\n---\n\n*Added from capture, {date}:*\n\n{capture}\n",
        existing.trim_end()
    )
}

/// Run the check for a quick capture in the background, show what it found in the main
/// window, and raise a notification when the user is looking elsewhere.
pub fn after_capture(app: AppHandle, path: String) {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(found) = check_now(&app.state::<AppState>(), &path) else {
            return;
        };
        if let Err(error) = app.emit_to("main", crate::events::SIMILAR_NOTES_FOUND, &found) {
            log::warn!("Could not show the similar-notes card: {error}");
            return;
        }
        if !main_window_focused(&app) {
            notify(&app);
        }
    });
}

fn main_window_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main").is_some_and(|window| {
        window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
    })
}

/// Captures with a card the user has not looked at since the last notification. Reset when the
/// main window gains focus, so the next notification counts only what is new.
static UNSEEN: AtomicUsize = AtomicUsize::new(0);

/// The main window was focused: every card is in front of the user.
pub fn seen() {
    UNSEEN.store(0, Ordering::SeqCst);
}

/// The notification's wording. Generic on purpose: it can show on a lock screen and stays in
/// the notification history, so it never names a note.
fn notice(unseen: usize) -> (String, &'static str) {
    let title = if unseen <= 1 {
        "Similar note found for your capture".to_string()
    } else {
        format!("{unseen} captures have similar notes")
    };
    (title, "Open Second Brain to review.")
}

/// Bring the main window forward, from a notification click on whatever thread delivers it.
///
/// `activation_token` is the compositor's permission to take focus, which a click on a portal
/// notification carries. Without it GNOME on Wayland refuses the focus request and shows
/// "Second Brain is ready" instead of raising the window.
fn open_main_window(app: &AppHandle, activation_token: Option<String>) {
    seen();
    let handle = app.clone();
    let opened = app.run_on_main_thread(move || {
        #[cfg(target_os = "linux")]
        if let Some(token) = activation_token {
            use gtk::prelude::GtkWindowExt;
            if let Some(Ok(window)) = handle
                .get_webview_window("main")
                .map(|main| main.gtk_window())
            {
                window.set_startup_id(&token);
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = activation_token;
        crate::show_main_window(&handle);
    });
    if let Err(error) = opened {
        log::warn!("Could not open the main window from the notification: {error}");
    }
}

#[cfg(target_os = "linux")]
const NOTIFICATION_ID: &str = "capture-similar-notes";

/// Through the notification portal, like the hotkey's own notice (see
/// `hotkey::startup::notify_vault_unavailable` for why not `tauri-plugin-notification`). The
/// fixed id makes a newer notification replace the last.
#[cfg(target_os = "linux")]
fn notify(app: &AppHandle) {
    use ashpd::desktop::notification::{Notification, NotificationProxy, Priority};

    listen_for_clicks(app);
    let (title, body) = notice(UNSEEN.fetch_add(1, Ordering::SeqCst) + 1);
    tauri::async_runtime::spawn(async move {
        let sent = async {
            NotificationProxy::new()
                .await?
                .add_notification(
                    NOTIFICATION_ID,
                    Notification::new(&title)
                        .body(body)
                        .default_action("open")
                        .priority(Priority::Normal),
                )
                .await
        }
        .await;
        // A nudge only: the card is already waiting in the main window.
        if let Err(error) = sent {
            log::warn!("Could not show the similar-notes notification: {error}");
        }
    });
}

/// One listener for the app's lifetime: the portal reports a click on any of its notifications
/// as an action on its id.
#[cfg(target_os = "linux")]
fn listen_for_clicks(app: &AppHandle) {
    use futures::StreamExt;
    static STARTED: std::sync::Once = std::sync::Once::new();

    let app = app.clone();
    STARTED.call_once(move || {
        tauri::async_runtime::spawn(async move {
            let listening = async {
                let proxy = ashpd::desktop::notification::NotificationProxy::new().await?;
                let mut actions = proxy.receive_action_invoked().await?;
                while let Some(action) = actions.next().await {
                    if action.id() == NOTIFICATION_ID {
                        let token = activation_token(action.parameter());
                        if token.is_none() {
                            log::info!("A notification click came without an activation token");
                        }
                        open_main_window(&app, token);
                    }
                }
                Ok::<(), ashpd::Error>(())
            }
            .await;
            if let Err(error) = listening {
                log::warn!("Notification clicks will not open the app: {error}");
            }
        });
    });
}

/// The activation token in a portal action's parameters. Since version 2 of the notification
/// portal they end with platform data, a dictionary that holds it as `activation-token`.
#[cfg(target_os = "linux")]
fn activation_token(parameters: &[ashpd::zvariant::OwnedValue]) -> Option<String> {
    use std::collections::HashMap;
    parameters.iter().find_map(|parameter| {
        let data =
            HashMap::<String, ashpd::zvariant::OwnedValue>::try_from(parameter.try_clone().ok()?)
                .ok()?;
        String::try_from(data.get("activation-token")?.try_clone().ok()?).ok()
    })
}

/// A toast under a fixed tag, so a newer one replaces the last (#153). Clicking it raises the
/// main window while the app runs, which it does whenever a capture was just made.
#[cfg(target_os = "windows")]
fn notify(app: &AppHandle) {
    use windows::core::IInspectable;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::ToastNotification;

    let (title, body) = notice(UNSEEN.fetch_add(1, Ordering::SeqCst) + 1);
    let handle = app.clone();
    let shown =
        crate::hotkey::windows::toast(&title, body, "similar-notes", "capture").and_then(|toast| {
            toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
                move |_, _| {
                    open_main_window(&handle, None);
                    Ok(())
                },
            ))?;
            crate::hotkey::windows::show_toast(&app.config().identifier, &toast)
        });
    // A nudge only: the card is already waiting in the main window.
    if let Err(error) = shown {
        log::warn!("Could not show the similar-notes notification: {error}");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn notify(_app: &AppHandle) {}

#[cfg(test)]
mod tests {
    use super::{appended, notice};

    #[test]
    fn an_append_keeps_the_note_and_marks_where_the_capture_starts() {
        assert_eq!(
            appended("Existing body.\n\n", "Cedar arrived", "Stacked by the shed.", "2026-10-05"),
            "Existing body.\n\n---\n\n*Added from capture, 2026-10-05:*\n\nCedar arrived\n\nStacked by the shed.\n"
        );
        assert_eq!(
            appended("Existing.", "Title only", "", "2026-10-05"),
            "Existing.\n\n---\n\n*Added from capture, 2026-10-05:*\n\nTitle only\n"
        );
    }

    #[test]
    fn the_notification_never_names_a_note_and_counts_unseen_captures() {
        assert_eq!(notice(1).0, "Similar note found for your capture");
        assert_eq!(notice(3).0, "3 captures have similar notes");
        assert_eq!(notice(1).1, "Open Second Brain to review.");
    }
}
