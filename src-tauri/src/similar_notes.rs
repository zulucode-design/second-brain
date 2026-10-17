//! The capture-time similarity check (#9): after a note is saved, find existing notes that say
//! the same thing, so the user can fold the new one in instead of keeping a near-duplicate.
//!
//! The check only ever runs after the note is safely on disk, and every failure ends it quietly:
//! a capture must never wait on it or be lost to it.

use crate::semantic_search::SemanticIndex;
use crate::state::AppState;
use crate::vault::operations;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use tauri::{AppHandle, Emitter, Manager};

/// The score a note must reach to count as saying the same thing. Calibrated against live
/// embeddinggemma on scripts/similarity-fixture.json (#9). With captures as a title and a body
/// (#213), its 8 reworded duplicates scored 0.778 to 0.917 against their notes, and no other
/// pair, same-topic notes included, passed 0.571. The bar sits in that gap. Recalibrate when the
/// embedding model changes.
const SIMILAR_SCORE: f32 = 0.65;
/// How many similar notes a card shows; the rest are only counted.
const SHOWN_PER_CARD: usize = 3;
/// Shorter text embeds too loosely to call anything a duplicate.
const MIN_WORDS: usize = 3;

/// A just-written note and the existing notes most like it, each with the revision it had when
/// checked, so nothing is appended to a note that changed since the user saw it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarityCheck {
    /// The vault the check ran in, so a result that lands after a vault switch is dropped.
    pub vault: String,
    pub capture: CheckedNote,
    pub matches: Vec<SimilarMatch>,
    /// Every note that passed the bar, of which `matches` holds the best few.
    pub total: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckedNote {
    pub path: String,
    pub title: String,
    pub revision: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarMatch {
    #[serde(flatten)]
    pub note: CheckedNote,
    /// The passage that matched best: why the note was picked.
    pub excerpt: String,
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
    )?;
    let mut matches: Vec<SimilarMatch> = found
        .into_iter()
        // Deleted or unreadable since it was indexed: nothing to append to. Changed since: the
        // passage may no longer be in it, and the card would vouch for text the user never saw.
        .filter_map(|similar| {
            let existing = operations::read_note(vault, &similar.found.path).ok()?;
            (existing.revision == similar.indexed_revision).then_some(SimilarMatch {
                note: CheckedNote {
                    path: similar.found.path,
                    title: similar.found.title,
                    revision: existing.revision,
                },
                excerpt: similar.found.snippet,
            })
        })
        .collect();
    if matches.is_empty() {
        return Ok(None);
    }
    // Counted after the drops, so the card never counts a note it cannot show.
    let total = matches.len();
    matches.truncate(SHOWN_PER_CARD);
    Ok(Some(SimilarityCheck {
        vault: vault.to_string(),
        capture: CheckedNote {
            path: note.path,
            title: note.meta.title,
            revision: note.revision,
        },
        total,
        matches,
    }))
}

/// [`check`] against the open vault, with every failure treated as "nothing found": no index,
/// an unreachable embedding backend, or a note that vanished. A check that fails is kept, to
/// run once the backend is back (#216).
pub fn check_now(state: &AppState, path: &str) -> Option<SimilarityCheck> {
    let index = state.semantic_index.lock().ok()?.clone()?;
    let vault = state.config.lock().ok()?.active_vault.clone()?;
    // Read before the check, while the note is surely at `path`: a failing check can wait out
    // the backend's timeout, and the id finds the note after a move.
    let id = operations::read_note(&vault, path)
        .map(|note| note.meta.id)
        .unwrap_or_default();
    match check(&index, &vault, path) {
        Ok(found) => found,
        Err(_) => {
            // Without the error: an embedding failure carries the backend's response body,
            // which docs/log-privacy.md keeps out of the log.
            log::info!("Skipped the similarity check for a new note");
            defer(
                state,
                &vault,
                DeferredCheck {
                    id,
                    path: path.to_string(),
                },
            );
            None
        }
    }
}

/// The file in the vault's machine-local folder that keeps deferred checks across a restart.
const DEFERRED_FILE: &str = "deferred_similarity.json";

/// A note whose similarity check failed, so it never ran (#216).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DeferredCheck {
    /// The note's frontmatter id, which finds it again after a rename or a move. Empty when it
    /// has none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) id: String,
    /// Where the note was when its check failed: how a note without an id is found.
    pub(crate) path: String,
}

impl DeferredCheck {
    fn same_note(&self, other: &Self) -> bool {
        if self.id.is_empty() || other.id.is_empty() {
            self.path == other.path
        } else {
            self.id == other.id
        }
    }
}

/// The open vault's deferred checks, oldest first (#216). Every change rewrites a file in the
/// vault's machine-local folder, never inside the vault, so a restart keeps them and sync never
/// carries them.
#[derive(Debug, Default)]
pub struct DeferredChecks {
    /// The vault they belong to. `None` until a vault opens.
    vault: Option<String>,
    checks: Vec<DeferredCheck>,
}

/// The file's shape: `{"checks": [...]}`.
#[derive(Deserialize, Serialize)]
struct DeferredFile<C> {
    checks: C,
}

fn deferred_file(vault: &str) -> Result<PathBuf, String> {
    crate::machine_local::vault_dir(Path::new(vault)).map(|dir| dir.join(DEFERRED_FILE))
}

impl DeferredChecks {
    /// The checks kept for `vault`. A missing file is an empty list, and so is an unreadable
    /// one, with one log line: a lost deferred check never stops a vault from opening.
    pub fn load(vault: &str) -> Self {
        let read = deferred_file(vault)
            .ok()
            .and_then(|file| match std::fs::read(file) {
                Ok(bytes) => serde_json::from_slice::<DeferredFile<Vec<DeferredCheck>>>(&bytes)
                    .ok()
                    .map(|file| file.checks),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
                Err(_) => None,
            });
        let checks = read.unwrap_or_else(|| {
            // The outcome only: the error can name the file.
            log::warn!("The deferred similarity checks could not be read, so none are waiting");
            Vec::new()
        });
        Self {
            vault: Some(vault.to_string()),
            checks,
        }
    }

    /// Rewrites the file. When that fails, the checks still wait in memory until the app quits.
    fn save(&self) {
        let Some(vault) = self.vault.as_deref() else {
            return;
        };
        let saved = deferred_file(vault).and_then(|file| {
            let bytes = serde_json::to_vec_pretty(&DeferredFile {
                checks: &self.checks,
            })
            .map_err(|error| error.to_string())?;
            crate::durable::replace(&file, &bytes, crate::durable::Mode::Private)
        });
        if saved.is_err() {
            // Without the error, which can name the file.
            log::warn!("Could not save the deferred similarity checks");
        }
    }

    /// Keeps `check` as the newest, in place of an earlier one for the same note.
    fn keep(&mut self, check: DeferredCheck) {
        self.checks.retain(|kept| !kept.same_note(&check));
        self.checks.push(check);
        self.save();
    }

    #[cfg(test)]
    pub(crate) fn checks(&self) -> &[DeferredCheck] {
        &self.checks
    }
}

/// Keeps a check that just failed, to run once the backend is back.
fn defer(state: &AppState, vault: &str, check: DeferredCheck) {
    let Ok(mut open) = state.deferred_checks.lock() else {
        return;
    };
    match open.vault.as_deref().map(|open_vault| open_vault == vault) {
        Some(true) => open.keep(check),
        // The vault was switched while the check ran. The check belongs to the vault it ran in,
        // whose file keeps it for that vault's next open.
        Some(false) => {
            drop(open);
            DeferredChecks::load(vault).keep(check);
        }
        None => {
            *open = DeferredChecks::load(vault);
            open.keep(check);
        }
    }
}

/// Where a deferred check's note is now.
enum Location {
    At(String),
    /// Deleted: neither the index nor the vault has it any more.
    Gone,
    /// Not readable where the index or the record puts it, and not shown gone either: the file
    /// is there but cannot be read just now, or the index has not caught up yet with a move
    /// made while the app was closed. It is looked for again after the next pass.
    Unsure,
}

/// Finds a deferred check's note: by its id through the index, then at its recorded path, then
/// by its id across the vault.
fn locate(
    index: &SemanticIndex,
    vault: &str,
    deferred: &DeferredCheck,
) -> Result<Location, String> {
    let indexed = if deferred.id.is_empty() {
        Vec::new()
    } else {
        index.paths_of_note(&deferred.id)?
    };
    let mut unsure = !indexed.is_empty();
    for path in indexed.iter().chain(std::iter::once(&deferred.path)) {
        // Not `read_note`, which reads the trash too: a note in the trash has left the vault.
        match operations::read_vault_note(vault, path) {
            Ok(note) if deferred.id.is_empty() || note.meta.id == deferred.id => {
                return Ok(Location::At(path.clone()));
            }
            // Another note has that path now.
            Ok(_) => {}
            Err(_) if Path::new(path).is_file() => unsure = true,
            Err(_) => {}
        }
    }
    if unsure {
        return Ok(Location::Unsure);
    }
    // The index can miss a note moved while the app was closed, so the vault has the last word
    // before the check is dropped.
    if !deferred.id.is_empty() {
        if let Some(note) = operations::scan_notes(vault, None)?
            .into_iter()
            .find(|note| note.meta.id == deferred.id)
        {
            return Ok(Location::At(note.path));
        }
    }
    Ok(Location::Gone)
}

/// One rerun at a time, so two passes that finish close together never run a check twice.
static RERUNNING: Mutex<()> = Mutex::new(());

/// Runs each of the open vault's deferred checks once, oldest first, on the note's current
/// text, and hands every result with matches to `deliver` (#216). A check that runs leaves the
/// list, matches or not; one that fails again stays; a deleted note's is dropped.
pub(crate) fn rerun_deferred_checks(state: &AppState, mut deliver: impl FnMut(SimilarityCheck)) {
    let _rerunning = RERUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    // Read together, in the order opening a vault takes them, so they never straddle a switch.
    let open = || {
        let slot = state.semantic_index.lock().ok()?;
        let config = state.config.lock().ok()?;
        let index = slot.clone()?;
        let vault = config.active_vault.clone()?;
        Some((index, vault))
    };
    let Some((index, vault)) = open() else {
        return;
    };
    let waiting = match state.deferred_checks.lock() {
        Ok(list) if list.vault.as_deref() == Some(vault.as_str()) => list.checks.clone(),
        _ => return,
    };
    if waiting.is_empty() {
        return;
    }
    let mut settled = Vec::new();
    let (mut found, mut dropped) = (0, 0);
    for deferred in &waiting {
        match locate(&index, &vault, deferred) {
            Ok(Location::At(path)) => match check(&index, &vault, &path) {
                Ok(result) => {
                    settled.push(deferred);
                    if let Some(result) = result {
                        found += 1;
                        deliver(result);
                    }
                }
                // Most likely the backend is away again: the rest wait for the next pass rather
                // than each waiting out its timeout.
                Err(_) => break,
            },
            Ok(Location::Gone) => {
                dropped += 1;
                settled.push(deferred);
            }
            Ok(Location::Unsure) | Err(_) => {}
        }
    }
    let ran = settled.len() - dropped;
    let still_waiting = waiting.len() - settled.len();
    if !settled.is_empty() {
        if let Ok(mut list) = state.deferred_checks.lock() {
            // Unless the vault was switched meanwhile: its file then keeps every check for its
            // next open, and those that ran run once more.
            if list.vault.as_deref() == Some(vault.as_str()) {
                list.checks.retain(|kept| !settled.contains(&kept));
                list.save();
            }
        }
    }
    // Counts only, never a note (docs/log-privacy.md).
    log::info!(
        "Deferred similarity checks: ran={ran} found={found} dropped={dropped} \
         waiting={still_waiting}"
    );
}

/// Reruns the deferred checks off the indexing worker's thread and shows each result with
/// matches as the ordinary card. Oldest first, so the newest lands on top, since cards show
/// newest first. No notification and no summary: the user did not just capture anything.
fn rerun_deferred(app: AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        rerun_deferred_checks(&app.state::<AppState>(), |found| {
            // The vault can close while the checks run; a card belongs to the vault it ran in.
            if !vault_still_open(&app, &found.vault) {
                return;
            }
            if let Err(error) = app.emit_to("main", crate::events::SIMILAR_NOTES_FOUND, &found) {
                log::warn!("Could not show the similar-notes card: {error}");
            }
        });
    });
}

/// Reruns the open vault's deferred checks after each pass of `index` that shows the embedding
/// backend back (#216). For every new index, before its worker starts.
pub fn rerun_when_caught_up(app: &AppHandle, index: &SemanticIndex) {
    let app = app.clone();
    index.on_caught_up(move || rerun_deferred(app.clone()));
}

/// `existing` with the capture added at the end under a dated marker, title line included.
pub fn with_capture_appended(
    existing: &str,
    capture_title: &str,
    capture_body: &str,
    date: &str,
) -> String {
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
    // Read now, while it still describes this capture: the next overlay overwrites it.
    let focused_at_capture = MAIN_FOCUSED_AT_CAPTURE.load(Ordering::SeqCst);
    tauri::async_runtime::spawn_blocking(move || {
        let Some(found) = check_now(&app.state::<AppState>(), &path) else {
            return;
        };
        // A check can outlast a vault switch; its card and its nudge belong to the old vault.
        if !vault_still_open(&app, &found.vault) {
            return;
        }
        if let Err(error) = app.emit_to("main", crate::events::SIMILAR_NOTES_FOUND, &found) {
            log::warn!("Could not show the similar-notes card: {error}");
            return;
        }
        if !focused_at_capture && !main_window_focused(&app) {
            notify(&app, found.vault);
        }
    });
}

/// Whether the main window had focus when the capture overlay last opened. The overlay takes
/// focus, so a check that finishes before it hands focus back would otherwise notify a user who
/// was in the app all along.
static MAIN_FOCUSED_AT_CAPTURE: AtomicBool = AtomicBool::new(false);

/// The capture overlay is about to open.
pub fn capture_opening(app: &AppHandle) {
    MAIN_FOCUSED_AT_CAPTURE.store(main_window_focused(app), Ordering::SeqCst);
}

fn vault_still_open(app: &AppHandle, vault: &str) -> bool {
    app.state::<AppState>()
        .config
        .lock()
        .is_ok_and(|config| config.active_vault.as_deref() == Some(vault))
}

/// A queued notification whose turn comes after the user went back to the app, or after a
/// vault switch, is moot.
fn notification_moot(app: &AppHandle, vault: &str) -> bool {
    main_window_focused(app) || !vault_still_open(app, vault)
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
pub fn main_window_seen() {
    UNSEEN.store(0, Ordering::SeqCst);
}

/// One more capture the user has not seen, and the title that counts it.
fn next_notification_title() -> String {
    notification_title(UNSEEN.fetch_add(1, Ordering::SeqCst) + 1)
}

/// The notification's wording. Generic on purpose: it can show on a lock screen and stays in
/// the notification history, so it never names a note.
fn notification_title(unseen: usize) -> String {
    if unseen <= 1 {
        "Similar note found for your capture".to_string()
    } else {
        format!("{unseen} captures have similar notes")
    }
}

const NOTIFICATION_BODY: &str = "Open Second Brain to review.";

/// Bring the main window forward, from a notification click on whatever thread delivers it.
///
/// `activation_token` is the compositor's permission to take focus, which the notification
/// service sends just before a click. Without it GNOME on Wayland refuses the focus request and
/// shows "Second Brain is ready" instead of raising the window.
fn open_main_window(app: &AppHandle, activation_token: Option<String>) {
    main_window_seen();
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

/// The id GNOME gave the last similar-notes notification, so a newer one replaces it and a click
/// can be matched to it. 0 is none yet.
#[cfg(target_os = "linux")]
static NOTIFICATION_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The desktop's notification service, `org.freedesktop.Notifications`, over the session bus
/// ashpd already uses.
///
/// Not the notification portal that `hotkey::startup::notify_vault_unavailable` uses: a click on
/// a portal notification reaches the app without an activation token (GNOME 50 sends its action
/// with no platform data), and without one GNOME on Wayland refuses to raise the window and shows
/// "… is ready" instead. This service sends the token (`ActivationToken`) just before the click.
///
/// One connection for the app's lifetime: GNOME Shell withdraws an app's notifications the moment
/// the connection that sent them closes, so a connection per notification lost each one at once.
#[cfg(target_os = "linux")]
async fn notification_service() -> ashpd::zbus::Result<&'static ashpd::zbus::Proxy<'static>> {
    static SERVICE: tokio::sync::OnceCell<ashpd::zbus::Proxy<'static>> =
        tokio::sync::OnceCell::const_new();
    SERVICE
        .get_or_try_init(|| async {
            let connection = ashpd::zbus::Connection::session().await?;
            ashpd::zbus::Proxy::new(
                &connection,
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
            )
            .await
        })
        .await
}

#[cfg(target_os = "linux")]
fn notify(app: &AppHandle, vault: String) {
    use ashpd::zbus::zvariant::Value;
    use std::collections::HashMap;

    let desktop_entry = app.config().identifier.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // One send at a time: a second capture must wait for the first's id, or both would go
        // out as new notifications instead of one replacing the other. Counted inside, so the
        // notification that replaces another never shows the smaller count.
        static SENDING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _sending = SENDING.lock().await;
        let sent = async {
            let service = notification_service().await?;
            // Checked after connecting, which can wait: focus or the vault may have changed.
            if notification_moot(&app, &vault) {
                return Ok(());
            }
            let title = next_notification_title();
            let hints = HashMap::from([("desktop-entry", Value::from(desktop_entry.as_str()))]);
            let id: u32 = service
                .call(
                    "Notify",
                    &(
                        "Second Brain",
                        NOTIFICATION_ID.load(Ordering::SeqCst),
                        "",
                        title.as_str(),
                        NOTIFICATION_BODY,
                        vec!["default", "Open"],
                        hints,
                        -1i32,
                    ),
                )
                .await?;
            NOTIFICATION_ID.store(id, Ordering::SeqCst);
            // Only once a notification is out, so a missing service logs one line, not two.
            listen_for_clicks(&app);
            Ok::<(), ashpd::zbus::Error>(())
        }
        .await;
        // A nudge only: the card is already waiting in the main window.
        if let Err(error) = sent {
            log::warn!("Could not show the similar-notes notification: {error}");
        }
    });
}

/// One listener for the app's lifetime. The service announces a click as `ActivationToken` and
/// then `ActionInvoked` for the notification's id; both come on one stream, in order.
#[cfg(target_os = "linux")]
fn listen_for_clicks(app: &AppHandle) {
    use futures::StreamExt;
    static LISTENING: AtomicBool = AtomicBool::new(false);

    if LISTENING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let listening = async {
            let service = notification_service().await?;
            let mut signals = service.receive_all_signals().await?;
            let mut token = None;
            while let Some(signal) = signals.next().await {
                let header = signal.header();
                let Some(member) = header.member() else {
                    continue;
                };
                let Ok((id, value)) = signal.body().deserialize::<(u32, String)>() else {
                    continue;
                };
                if id != NOTIFICATION_ID.load(Ordering::SeqCst) {
                    continue;
                }
                match member.as_str() {
                    "ActivationToken" => token = Some(value),
                    "ActionInvoked" if value == "default" => {
                        if token.is_none() {
                            log::info!("A notification click came without an activation token");
                        }
                        open_main_window(&app, token.take());
                    }
                    _ => {}
                }
            }
            Ok::<(), ashpd::zbus::Error>(())
        }
        .await;
        if let Err(error) = listening {
            // Let the next notification try again.
            LISTENING.store(false, Ordering::SeqCst);
            log::warn!("Notification clicks will not open the app: {error}");
        }
    });
}

/// A toast under a fixed tag, so a newer one replaces the last (#153). Clicking it raises the
/// main window while the app runs, which it does whenever a capture was just made.
#[cfg(target_os = "windows")]
fn notify(app: &AppHandle, vault: String) {
    use windows::core::IInspectable;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::ToastNotification;

    // Checks finish on their own threads; one toast at a time keeps the newest count on top.
    static SENDING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _sending = SENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if notification_moot(app, &vault) {
        return;
    }
    let title = next_notification_title();
    let handle = app.clone();
    let shown =
        crate::hotkey::windows::toast(&title, NOTIFICATION_BODY, "similar-notes", "capture")
            .and_then(|toast| {
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
fn notify(_app: &AppHandle, _vault: String) {}

#[cfg(test)]
mod tests {
    use super::{
        deferred_file, notification_title, with_capture_appended, DeferredCheck, DeferredChecks,
    };

    #[test]
    fn an_unreadable_deferred_file_gives_an_empty_list_and_the_vault_still_opens() {
        let vault = std::env::temp_dir().join(format!("sb-deferred-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&vault).unwrap();
        let vault_text = vault.to_str().unwrap();
        let file = deferred_file(vault_text).unwrap();
        assert!(
            DeferredChecks::load(vault_text).checks().is_empty(),
            "a missing file is an empty list"
        );

        // `load` has no error to return, so opening the vault goes on with an empty list.
        std::fs::write(&file, b"{ not json").unwrap();
        let mut loaded = DeferredChecks::load(vault_text);
        assert!(loaded.checks().is_empty(), "a damaged file");
        // The next change writes a whole file over the damaged one.
        loaded.keep(DeferredCheck {
            id: "note-id".to_string(),
            path: "Areas/Note.md".to_string(),
        });
        assert_eq!(DeferredChecks::load(vault_text).checks(), loaded.checks());

        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        assert!(
            DeferredChecks::load(vault_text).checks().is_empty(),
            "a file that cannot be read at all"
        );
        std::fs::remove_dir(&file).unwrap();
        std::fs::remove_dir_all(&vault).unwrap();
    }

    #[test]
    fn an_append_keeps_the_note_and_marks_where_the_capture_starts() {
        assert_eq!(
            with_capture_appended("Existing body.\n\n", "Cedar arrived", "Stacked by the shed.", "2026-10-05"),
            "Existing body.\n\n---\n\n*Added from capture, 2026-10-05:*\n\nCedar arrived\n\nStacked by the shed.\n"
        );
        assert_eq!(
            with_capture_appended("Existing.", "Title only", "", "2026-10-05"),
            "Existing.\n\n---\n\n*Added from capture, 2026-10-05:*\n\nTitle only\n"
        );
    }

    #[test]
    fn the_notification_never_names_a_note_and_counts_unseen_captures() {
        assert_eq!(notification_title(1), "Similar note found for your capture");
        assert_eq!(notification_title(3), "3 captures have similar notes");
    }
}
