//! The Notion publisher's front door: setup, status, and running a push.
//!
//! Everything here is glue. The decisions live in [`super::plan`], the API in
//! [`super::client`], and the order of calls in [`super::publish`] — none of which know
//! about Tauri. That separation is what lets the same engine run inside the background
//! process #57 introduces, which has no window to emit events to.
//!
//! ## Enumeration, and why it does not read every note
//!
//! The map is keyed by note id, and a note's id lives inside its file. Taken literally that
//! would mean opening every note on every poll just to discover which map entry it belongs
//! to — twelve times an hour, to learn nothing.
//!
//! So a map entry also records the note's path. Enumeration walks the four category folders
//! for names and modification times, matches each path against the map, and opens only the
//! files that are new or whose timestamp moved. A note that has not changed is never read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use super::client::{NotionClient, NotionError, VisiblePage};
use super::config::{self, DatabaseRegistry, NotionSettings};
use super::map;
use super::plan::NoteSnapshot;
use super::publish::{self, NoteSource, Progress, RunSources, Summary};
use crate::state::AppState;
use crate::types::AppConfig;
use crate::vault::para::ParaCategory;

/// What the Settings panel shows.
#[derive(Debug, Clone, Default, Serialize)]
pub struct NotionStatus {
    /// Whether this machine is the publisher.
    pub enabled: bool,
    /// Whether this machine is currently publishing.
    pub publishing: bool,
    /// Latest progress, including for a timer-driven run started outside Settings.
    pub progress: Option<Progress>,
    /// Whether a token is stored. Never carries the token itself.
    pub connected: bool,
    /// The connection's name in Notion, once it has been checked.
    pub connection_name: Option<String>,
    /// Whether all four databases exist.
    pub setup_complete: bool,
    pub poll_minutes: u32,
    pub last_run: Option<String>,
    pub last_summary: Option<Summary>,
    /// Why the last run stopped, when it stopped badly.
    pub last_error: Option<String>,
    /// How many notes are currently recorded as failing, so a single unpublishable note is
    /// visible rather than silently retried forever.
    pub failing_notes: u32,
}

fn active_vault(config: &AppConfig) -> Result<PathBuf, String> {
    config
        .active_vault
        .clone()
        .map(PathBuf::from)
        .ok_or_else(|| "No active vault".to_string())
}

fn settings_of(config: &AppConfig) -> NotionSettings {
    config
        .active_vault
        .as_ref()
        .and_then(|active| config.vaults.iter().find(|vault| &vault.path == active))
        .map(|vault| vault.notion.clone())
        .unwrap_or_default()
}

fn update_settings(
    state: &State<'_, AppState>,
    change: impl FnOnce(&mut NotionSettings) -> Result<(), String>,
) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|error| error.to_string())?;
    let mut candidate = config.clone();
    let active = candidate
        .active_vault
        .clone()
        .ok_or_else(|| "No active vault".to_string())?;
    let vault = candidate
        .vaults
        .iter_mut()
        .find(|vault| vault.path == active)
        .ok_or_else(|| "The active vault is not in the vault list".to_string())?;
    change(&mut vault.notion)?;
    crate::commands::commit_secret_config(&mut config, candidate)
}

/// Commit settings from an async command without running the synchronous Linux keyring
/// backend on a Tokio worker. That backend drives its own runtime and panics if it is
/// entered directly from another runtime.
async fn update_settings_async(
    app: AppHandle,
    change: impl FnOnce(&mut NotionSettings) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    run_sync_from_async(move || {
        let state = app.state::<AppState>();
        update_settings(&state, change)
    })
    .await
}

async fn run_sync_from_async<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| error.to_string())?
}

fn client_for(config: &AppConfig) -> Result<NotionClient, String> {
    let settings = settings_of(config);
    let token = settings
        .token
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| "Notion is not connected".to_string())?;
    Ok(NotionClient::new(token))
}

/// Everything the Settings panel needs, and nothing secret.
#[tauri::command]
pub fn notion_status(state: State<'_, AppState>) -> Result<NotionStatus, String> {
    let config = state.config.lock().map_err(|error| error.to_string())?;
    let settings = settings_of(&config);
    let vault = active_vault(&config).ok();

    let registry = vault
        .as_ref()
        .map(|path| config::load_registry(path))
        .unwrap_or_default();

    let failing = vault
        .as_ref()
        .map(|path| {
            map::all(path)
                .iter()
                .filter(|(_, entry)| entry.last_error.is_some())
                .count() as u32
        })
        .unwrap_or(0);

    Ok(NotionStatus {
        enabled: settings.enabled,
        publishing: state.notion_publishing.load(Ordering::SeqCst),
        progress: *state
            .notion_progress
            .lock()
            .map_err(|error| error.to_string())?,
        connected: settings.token.is_some(),
        connection_name: None,
        setup_complete: registry.is_complete(),
        poll_minutes: settings.poll_interval_minutes(),
        last_run: settings.last_run.clone(),
        last_summary: vault.as_ref().and_then(|path| read_last_summary(path)),
        last_error: vault.as_ref().and_then(|path| read_last_error(path)),
        failing_notes: failing,
    })
}

/// Store a token after checking it works.
///
/// Checked before it is stored, so a mistyped token fails here rather than as a silent
/// publisher that never publishes.
#[tauri::command]
pub async fn notion_connect(app: AppHandle, token: String) -> Result<String, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("Paste the integration token from Notion".into());
    }

    let name = NotionClient::new(token.clone())
        .whoami()
        .await
        .map_err(|error| error.message())?;

    update_settings_async(app, move |settings| {
        settings.token = Some(token);
        settings.enabled = true;
        Ok(())
    })
    .await?;
    Ok(name)
}

/// Forget the token. The databases and the map are left alone.
///
/// Deliberate: disconnecting is not deleting. The pages stay readable in Notion, and
/// reconnecting later finds the map intact rather than republishing the whole vault.
#[tauri::command]
pub fn notion_disconnect(state: State<'_, AppState>) -> Result<(), String> {
    update_settings(&state, |settings| {
        settings.token = None;
        settings.enabled = false;
        Ok(())
    })
}

/// Choose whether this connected machine is the one that publishes.
#[tauri::command]
pub fn notion_set_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    update_settings(&state, |settings| {
        if enabled && settings.token.is_none() {
            return Err("Connect Notion before enabling publishing on this machine".into());
        }
        settings.enabled = enabled;
        Ok(())
    })
}

/// Pages the integration can see, for the setup picker.
#[tauri::command]
pub async fn notion_visible_pages(app: AppHandle) -> Result<Vec<VisiblePage>, String> {
    let client = {
        let state = app.state::<AppState>();
        let config = state.config.lock().map_err(|error| error.to_string())?;
        client_for(&config)?
    };
    client
        .visible_pages()
        .await
        .map_err(|error| error.message())
}

/// Create the four PARA databases under the chosen page.
///
/// Resumable: only the categories without a database are created, so a run interrupted
/// after two does not produce two more of them on the next attempt.
#[tauri::command]
pub async fn notion_setup(app: AppHandle, parent_page_id: String) -> Result<(), String> {
    let (client, vault) = {
        let state = app.state::<AppState>();
        let config = state.config.lock().map_err(|error| error.to_string())?;
        (client_for(&config)?, active_vault(&config)?)
    };

    setup_databases(&client, &vault, &parent_page_id)
        .await
        .map(|_| ())
}

/// Create whichever of the four databases do not exist yet, under `parent_page_id`.
///
/// Separate from the command so the same code path is the one the live verification drives,
/// rather than a copy of it that could drift.
pub(crate) async fn setup_databases(
    client: &NotionClient,
    vault: &Path,
    parent_page_id: &str,
) -> Result<DatabaseRegistry, String> {
    let mut registry = config::load_registry(vault);
    // A different parent means starting again: the databases recorded live somewhere the
    // user no longer means to use.
    if registry.parent_page_id.as_deref() != Some(parent_page_id) {
        registry = DatabaseRegistry {
            parent_page_id: Some(parent_page_id.to_string()),
            ..Default::default()
        };
    }

    for category in registry.missing() {
        let link = client
            .create_database(parent_page_id, category.folder_name())
            .await
            .map_err(|error| error.message())?;
        registry.set_link(category, link);
        // Saved per database rather than at the end, so an interruption keeps what
        // succeeded and the next attempt creates only the rest.
        config::save_registry(vault, &registry)?;
    }

    config::save_registry(vault, &registry)?;
    Ok(registry)
}

/// Push now, in the background.
///
/// Returns immediately. Progress and the result arrive as events, because a first sync can
/// be thousands of notes and must never block the window.
#[tauri::command]
pub fn notion_publish_now(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    // A manual push, the poll timer, and a push at startup can all collide.
    if state.notion_publishing.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    if let Ok(mut progress) = state.notion_progress.lock() {
        *progress = None;
    }

    let prepared = {
        let config = match state.config.lock() {
            Ok(config) => config,
            Err(error) => {
                state.notion_publishing.store(false, Ordering::SeqCst);
                return Err(error.to_string());
            }
        };
        let settings = settings_of(&config);
        if !settings.is_configured() {
            state.notion_publishing.store(false, Ordering::SeqCst);
            return Err("Notion is not connected on this machine".into());
        }
        client_for(&config).and_then(|client| active_vault(&config).map(|vault| (client, vault)))
    };

    let (client, vault) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            state.notion_publishing.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };

    // Connected but not set up: every note would be read only to be skipped for want of a
    // database, every five minutes, and the panel would report it as a run. Refusing is
    // both cheaper and more honest about what state the machine is in.
    if !config::load_registry(&vault).is_complete() {
        state.notion_publishing.store(false, Ordering::SeqCst);
        return Err("Finish setting up Notion before publishing".into());
    }

    tauri::async_runtime::spawn(async move {
        let outcome = publish_once(&app, &client, &vault).await;

        let state = app.state::<AppState>();
        state.notion_publishing.store(false, Ordering::SeqCst);
        if let Ok(mut progress) = state.notion_progress.lock() {
            *progress = None;
        }

        match outcome {
            Ok(summary) => {
                write_last_run(&vault, Some(&summary), None);
                let _ = update_settings_async(app.clone(), |settings| {
                    settings.last_run = Some(chrono::Utc::now().to_rfc3339());
                    Ok(())
                })
                .await;
                let _ = app.emit("notion-publish-finished", &summary);
            }
            Err(error) => {
                // A fatal error is almost always a revoked token, and the only useful
                // response is to tell the user to reconnect rather than retry quietly.
                write_last_run(&vault, None, Some(&error.message()));
                let _ = app.emit(
                    "notion-publish-failed",
                    serde_json::json!({ "error": error.message(), "fatal": error.is_fatal() }),
                );
            }
        }
    });

    Ok(())
}

async fn publish_once(
    app: &AppHandle,
    client: &NotionClient,
    vault: &Path,
) -> Result<Summary, NotionError> {
    let registry = config::load_registry(vault);
    let (snapshots, mut prepared) = enumerate(vault);

    publish::run(
        vault,
        client,
        &registry,
        &app.state::<AppState>().notion_deletions,
        RunSources {
            snapshots,
            read: |snapshot: &NoteSnapshot| {
                prepared
                    .remove(&snapshot.note_id)
                    .ok_or_else(|| format!("{} could not be read", snapshot.relative_path))
            },
            source_is_current: |note: &NoteSource| {
                publish::read_note(vault, &note.snapshot.relative_path)
                    .map(|(meta, _, _)| meta.id == note.snapshot.note_id)
                    .unwrap_or(false)
            },
        },
        |progress: Progress| {
            if let Ok(mut current) = app.state::<AppState>().notion_progress.lock() {
                *current = Some(progress);
            }
            let _ = app.emit("notion-publish-progress", progress);
        },
    )
    .await
}

/// A file's modification time, in nanoseconds since the epoch.
///
/// Nanoseconds, not seconds. At one-second resolution an autosave, a publish, and a second
/// autosave can all land in the same second: the publisher records that second, the later
/// edit leaves the timestamp unchanged, and the gate treats the note as untouched — so the
/// last edit of a burst is never published until the note is edited again. Filesystems
/// with coarse timestamps (FAT, some network mounts) still have that window; ext4, btrfs,
/// APFS, and NTFS do not.
pub(crate) fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_nanos() as i64)
        .unwrap_or(0)
}

/// Walk the vault, reading only what the map cannot already account for.
///
/// Returns the notes to consider, plus the ones that had to be opened — so the publisher's
/// reader serves them from memory rather than opening each file a second time.
pub(crate) fn enumerate(vault: &Path) -> (Vec<NoteSnapshot>, HashMap<String, NoteSource>) {
    // path -> (note id, mtime at last publish)
    let known: HashMap<String, (String, Option<i64>)> = map::all(vault)
        .into_iter()
        .filter_map(|(note_id, entry)| {
            entry
                .relative_path
                .clone()
                .map(|path| (path, (note_id, entry.source_mtime)))
        })
        .collect();

    let mut snapshots = Vec::new();
    let mut prepared = HashMap::new();

    for category in ParaCategory::ALL {
        let root = vault.join(category.folder_name());
        if !root.exists() {
            continue;
        }

        for entry in walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            collect_snapshot(
                vault,
                path,
                Some(category),
                &known,
                &mut snapshots,
                &mut prepared,
            );
        }
    }

    // The holding area is deliberately not a fifth category. Its direct Markdown files
    // still have to reach the planner so they are counted as uncategorised and the Settings
    // panel can explain why they were not published.
    let holding = vault.join(".helixnotes").join("unfiled");
    if let Ok(entries) = std::fs::read_dir(holding) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                collect_snapshot(vault, &path, None, &known, &mut snapshots, &mut prepared);
            }
        }
    }

    (snapshots, prepared)
}

fn collect_snapshot(
    vault: &Path,
    path: &Path,
    known_category: Option<ParaCategory>,
    known: &HashMap<String, (String, Option<i64>)>,
    snapshots: &mut Vec<NoteSnapshot>,
    prepared: &mut HashMap<String, NoteSource>,
) {
    let Ok(relative) = path.strip_prefix(vault) else {
        return;
    };
    let relative_path = relative.to_string_lossy().replace('\\', "/");
    let mtime = path
        .metadata()
        .ok()
        .map(|meta| mtime_of(&meta))
        .unwrap_or(0);

    // Known and unmoved: the note id comes from the map, and the file stays shut.
    if let Some((note_id, recorded)) = known.get(&relative_path) {
        if *recorded == Some(mtime) {
            snapshots.push(NoteSnapshot {
                note_id: note_id.clone(),
                relative_path,
                category: known_category,
                mtime,
            });
            return;
        }
    }

    let Ok((meta, body, _)) = publish::read_note(vault, &relative_path) else {
        return;
    };
    if meta.id.trim().is_empty() {
        // Nothing can be mapped to a note with no id, and inventing one would mean
        // writing to a note this feature has no business modifying.
        log::warn!("Not publishing {relative_path}: the note has no id");
        return;
    }

    let snapshot = NoteSnapshot {
        note_id: meta.id.clone(),
        relative_path,
        // The note's own category is the source of truth, never the folder.
        category: meta.category,
        mtime,
    };
    snapshots.push(snapshot.clone());
    prepared.insert(
        meta.id.clone(),
        NoteSource {
            snapshot,
            meta,
            body,
        },
    );
}

// ── hooks the vault calls when a note comes or goes ──

/// Record that a note was deleted, so its Notion page can be trashed later.
///
/// Must be called *before* the note is removed: the page id lives in the map, but the note
/// id that finds it lives inside the file, and after deletion there is nothing left to read.
/// This is the whole reason deletion is driven by a tombstone rather than by noticing a
/// missing note — and it has to be, because with sync a missing note may simply not have
/// arrived yet (ADR-0002).
pub fn note_deleted(vault_path: &Path, note_path: &str) -> Result<(), String> {
    // Vaults that never set Notion up pay nothing for this.
    if !config::notion_dir(vault_path).exists() {
        return Ok(());
    }
    let Some(note_id) = note_id_at(note_path) else {
        return Ok(());
    };
    map::tombstone(vault_path, &note_id).map_err(|error| {
        format!("Could not record that {note_id} needs removing from Notion: {error}")
    })
}

/// Undo a tombstone for a note restored from the app's trash.
///
/// Costs no API call and keeps the page identity: the page was never touched, so a restore
/// that happens before the publisher runs nets out to nothing at all.
pub fn note_restored(vault_path: &Path, note_path: &str) -> Result<(), String> {
    if !config::notion_dir(vault_path).exists() {
        return Ok(());
    }
    let Some(note_id) = note_id_at(note_path) else {
        return Ok(());
    };
    map::restore(vault_path, &note_id)
        .map_err(|error| format!("Could not clear the Notion tombstone for {note_id}: {error}"))
}

/// Run a local deletion only after every affected note has a durable tombstone.
///
/// If either a marker write or the deletion fails, markers are rolled back while the source
/// files still exist. The same boundary serves one note and all descendants of a notebook.
pub fn with_notes_deleted<T>(
    vault_path: &Path,
    note_paths: &[String],
    delete: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let mut marked: Vec<String> = Vec::new();
    for path in note_paths {
        if let Err(error) = note_deleted(vault_path, path) {
            return match notes_restored(vault_path, &marked) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "{error}. Could not roll back earlier Notion deletion markers: {rollback_error}"
                )),
            };
        }
        marked.push(path.clone());
    }
    match delete() {
        Ok(value) => Ok(value),
        Err(error) => match notes_restored(vault_path, &marked) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(format!(
                "{error}. Could not roll back Notion deletion markers: {rollback_error}"
            )),
        },
    }
}

/// Clear tombstones for notes that remain local or have just been restored.
pub fn notes_restored(vault_path: &Path, note_paths: &[String]) -> Result<(), String> {
    let failures: Vec<String> = note_paths
        .iter()
        .filter_map(|path| note_restored(vault_path, path).err())
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn notes_deleted(vault_path: &Path, note_paths: &[String]) -> Result<(), String> {
    let failures: Vec<String> = note_paths
        .iter()
        .filter_map(|path| note_deleted(vault_path, path).err())
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// Run a local restore only after every pending tombstone has been cleared.
///
/// Source paths still point into trash while markers change. If the restore fails, those
/// same files can re-establish every marker before the caller releases the lifecycle lock.
pub fn with_notes_restored<T>(
    vault_path: &Path,
    note_paths: &[String],
    restore: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let mut cleared: Vec<String> = Vec::new();
    for path in note_paths {
        if let Err(error) = note_restored(vault_path, path) {
            return match notes_deleted(vault_path, &cleared) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "{error}. Could not restore earlier Notion deletion markers: {rollback_error}"
                )),
            };
        }
        cleared.push(path.clone());
    }
    match restore() {
        Ok(value) => Ok(value),
        Err(error) => match notes_deleted(vault_path, &cleared) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(format!(
                "{error}. Could not restore Notion deletion markers: {rollback_error}"
            )),
        },
    }
}

fn note_id_at(note_path: &str) -> Option<String> {
    let raw = std::fs::read_to_string(note_path).ok()?;
    let (meta, _) = crate::vault::frontmatter::parse_note(&raw, "");
    Some(meta.id).filter(|id| !id.trim().is_empty())
}

// ── the last run, recorded where both the app and #57's process can read it ──
//
// Machine-local: it describes what *this* machine's publisher did, and the vault is synced.

fn status_path(vault: &Path) -> Option<PathBuf> {
    crate::machine_local::vault_dir(vault)
        .ok()
        .map(|dir| dir.join("notion_last_run.json"))
}

fn write_last_run(vault: &Path, summary: Option<&Summary>, error: Option<&str>) {
    let Some(path) = status_path(vault) else {
        return;
    };
    let payload = serde_json::json!({
        "at": chrono::Utc::now().to_rfc3339(),
        "summary": summary,
        "error": error,
    });
    if let Ok(encoded) = serde_json::to_string_pretty(&payload) {
        let _ = std::fs::write(path, encoded);
    }
}

fn read_last_run(vault: &Path) -> Option<serde_json::Value> {
    let path = status_path(vault)?;
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn read_last_summary(vault: &Path) -> Option<Summary> {
    let value = read_last_run(vault)?;
    serde_json::from_value(value.get("summary")?.clone()).ok()
}

fn read_last_error(vault: &Path) -> Option<String> {
    read_last_run(vault)?
        .get("error")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notion::map::{EntryState, MapEntry};

    #[tokio::test]
    async fn synchronous_runtime_drivers_run_outside_the_command_runtime() {
        let value = run_sync_from_async(|| {
            let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
            Ok(runtime.block_on(async { 42 }))
        })
        .await
        .unwrap();

        assert_eq!(value, 42);
    }

    fn vault() -> PathBuf {
        let path = std::env::temp_dir().join(format!("notion-enumerate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(path.join("Projects")).unwrap();
        std::fs::create_dir_all(path.join("Areas")).unwrap();
        path
    }

    fn write_note(vault: &Path, category: &str, name: &str, id: &str, body: &str) -> PathBuf {
        let path = vault.join(category).join(format!("{name}.md"));
        let raw =
            format!("---\nid: \"{id}\"\ntitle: \"{name}\"\ncategory: {category}\n---\n{body}\n");
        std::fs::write(&path, raw).unwrap();
        path
    }

    fn file_mtime(path: &Path) -> i64 {
        super::mtime_of(&std::fs::metadata(path).unwrap())
    }

    fn published_entry(path: &str) -> MapEntry {
        MapEntry {
            state: EntryState::Published,
            page_id: Some("page-1".into()),
            data_source_id: Some("ds".into()),
            content_hash: Some("hash".into()),
            source_mtime: Some(1),
            relative_path: Some(path.into()),
            last_error: None,
        }
    }

    #[test]
    fn deleting_a_note_records_the_page_that_still_needs_trashing() {
        let vault = vault();
        let path = write_note(&vault, "Projects", "One", "id-1", "body");
        map::save(&vault, "id-1", &published_entry("Projects/One.md")).unwrap();

        note_deleted(&vault, path.to_str().unwrap()).unwrap();

        let entry = map::load(&vault, "id-1").expect("the tombstone must survive the note");
        assert_eq!(entry.state, EntryState::Deleted);
        assert_eq!(
            entry.page_id.as_deref(),
            Some("page-1"),
            "without this the page could never be found again"
        );
    }

    #[test]
    fn restoring_a_note_before_the_publisher_runs_clears_the_tombstone() {
        let vault = vault();
        let path = write_note(&vault, "Projects", "One", "id-1", "body");
        map::save(&vault, "id-1", &published_entry("Projects/One.md")).unwrap();

        note_deleted(&vault, path.to_str().unwrap()).unwrap();
        note_restored(&vault, path.to_str().unwrap()).unwrap();

        let entry = map::load(&vault, "id-1").unwrap();
        assert_eq!(entry.state, EntryState::Published);
        assert_eq!(
            entry.content_hash.as_deref(),
            Some("hash"),
            "a delete and restore that netted out to nothing should force no re-upload"
        );
    }

    #[test]
    fn a_failed_local_delete_restores_every_tombstone() {
        let vault = vault();
        let first = write_note(&vault, "Projects", "One", "id-1", "body");
        let second = write_note(&vault, "Projects", "Two", "id-2", "body");
        map::save(&vault, "id-1", &published_entry("Projects/One.md")).unwrap();
        map::save(&vault, "id-2", &published_entry("Projects/Two.md")).unwrap();
        let paths = vec![
            first.to_string_lossy().to_string(),
            second.to_string_lossy().to_string(),
        ];

        let result = with_notes_deleted(&vault, &paths, || {
            Err::<(), _>("the local delete failed".to_string())
        });

        assert_eq!(result.unwrap_err(), "the local delete failed");
        assert_eq!(
            map::load(&vault, "id-1").unwrap().state,
            EntryState::Published
        );
        assert_eq!(
            map::load(&vault, "id-2").unwrap().state,
            EntryState::Published
        );
    }

    #[test]
    fn a_failed_local_restore_reinstates_every_tombstone() {
        let vault = vault();
        let first = write_note(&vault, "Projects", "One", "id-1", "body");
        let second = write_note(&vault, "Projects", "Two", "id-2", "body");
        map::save(&vault, "id-1", &published_entry("Projects/One.md")).unwrap();
        map::save(&vault, "id-2", &published_entry("Projects/Two.md")).unwrap();
        note_deleted(&vault, first.to_str().unwrap()).unwrap();
        note_deleted(&vault, second.to_str().unwrap()).unwrap();
        let paths = vec![
            first.to_string_lossy().to_string(),
            second.to_string_lossy().to_string(),
        ];

        let result = with_notes_restored(&vault, &paths, || {
            Err::<(), _>("the local restore failed".to_string())
        });

        assert_eq!(result.unwrap_err(), "the local restore failed");
        assert_eq!(
            map::load(&vault, "id-1").unwrap().state,
            EntryState::Deleted
        );
        assert_eq!(
            map::load(&vault, "id-2").unwrap().state,
            EntryState::Deleted
        );
    }

    #[test]
    fn a_vault_that_never_set_notion_up_pays_nothing_on_delete() {
        // The hook runs on every deletion in the app, including for users who will never
        // connect Notion at all.
        let vault = vault();
        let path = write_note(&vault, "Projects", "One", "id-1", "body");

        note_deleted(&vault, path.to_str().unwrap()).unwrap();

        assert!(
            !config::notion_dir(&vault).exists(),
            "nothing should be created"
        );
    }

    #[test]
    fn deleting_a_note_that_was_never_published_leaves_no_tombstone() {
        let vault = vault();
        let path = write_note(&vault, "Projects", "One", "id-1", "body");
        // The directory exists — another note is published — but this note is not in it.
        map::save(&vault, "other", &published_entry("Projects/Other.md")).unwrap();

        note_deleted(&vault, path.to_str().unwrap()).unwrap();

        assert!(map::load(&vault, "id-1").is_none());
    }

    #[test]
    fn enumeration_finds_notes_in_every_category() {
        let vault = vault();
        write_note(&vault, "Projects", "One", "id-1", "body");
        write_note(&vault, "Areas", "Two", "id-2", "body");

        let (snapshots, prepared) = enumerate(&vault);

        assert_eq!(snapshots.len(), 2);
        assert_eq!(prepared.len(), 2, "both are new, so both are read");
        let ids: Vec<&str> = snapshots.iter().map(|s| s.note_id.as_str()).collect();
        assert!(ids.contains(&"id-1") && ids.contains(&"id-2"));
    }

    #[test]
    fn a_known_note_that_has_not_changed_is_not_opened() {
        // The reason map entries record a path at all: without it, learning that nothing
        // changed would mean opening every note on every poll.
        let vault = vault();
        let path = write_note(&vault, "Projects", "One", "id-1", "body");

        map::save(
            &vault,
            "id-1",
            &MapEntry {
                state: EntryState::Published,
                page_id: Some("page-1".into()),
                data_source_id: Some("ds".into()),
                content_hash: Some("hash".into()),
                source_mtime: Some(file_mtime(&path)),
                relative_path: Some("Projects/One.md".into()),
                last_error: None,
            },
        )
        .unwrap();

        let (snapshots, prepared) = enumerate(&vault);

        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].note_id, "id-1",
            "resolved from the map, not the file"
        );
        assert!(prepared.is_empty(), "the file should not have been read");
    }

    #[test]
    fn a_known_note_whose_timestamp_moved_is_read_again() {
        let vault = vault();
        let path = write_note(&vault, "Projects", "One", "id-1", "body");
        map::save(
            &vault,
            "id-1",
            &MapEntry {
                state: EntryState::Published,
                page_id: Some("page-1".into()),
                data_source_id: Some("ds".into()),
                content_hash: Some("hash".into()),
                source_mtime: Some(file_mtime(&path) - 500),
                relative_path: Some("Projects/One.md".into()),
                last_error: None,
            },
        )
        .unwrap();

        let (_snapshots, prepared) = enumerate(&vault);
        assert_eq!(prepared.len(), 1);
    }

    #[test]
    fn a_note_with_no_id_is_left_alone_rather_than_given_one() {
        // Writing an id would modify a note this feature has no business touching, and the
        // local id scheme is deliberately read-only here.
        let vault = vault();
        std::fs::write(
            vault.join("Projects").join("Legacy.md"),
            "---\ntitle: \"Legacy\"\n---\nbody\n",
        )
        .unwrap();

        let (snapshots, _prepared) = enumerate(&vault);
        assert!(snapshots.is_empty());
    }

    #[test]
    fn the_notes_own_category_decides_where_it_goes_not_the_folder_it_sits_in() {
        // A file in the wrong folder is a reconciliation problem the vault fixes; the
        // publisher must not encode the folder as truth in the meantime.
        let vault = vault();
        let path = vault.join("Projects").join("Misfiled.md");
        std::fs::write(
            &path,
            "---\nid: \"id-9\"\ntitle: \"Misfiled\"\ncategory: Areas\n---\nbody\n",
        )
        .unwrap();

        let (snapshots, _) = enumerate(&vault);

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].category, Some(ParaCategory::Areas));
    }

    #[test]
    fn files_that_are_not_notes_are_ignored() {
        let vault = vault();
        std::fs::write(vault.join("Projects").join("notes.txt"), "not a note").unwrap();
        std::fs::write(vault.join("Projects").join("image.png"), [0u8; 8]).unwrap();

        let (snapshots, _) = enumerate(&vault);
        assert!(snapshots.is_empty());
    }

    #[test]
    fn holding_notes_reach_the_planner_as_uncategorised() {
        let vault = vault();
        let holding = vault.join(".helixnotes").join("unfiled");
        std::fs::create_dir_all(&holding).unwrap();
        std::fs::write(
            holding.join("Stray.md"),
            "---\nid: \"id-stray\"\n---\nbody\n",
        )
        .unwrap();

        let (snapshots, prepared) = enumerate(&vault);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].note_id, "id-stray");
        assert_eq!(snapshots[0].category, None);
        assert_eq!(prepared.len(), 1);
    }
}
