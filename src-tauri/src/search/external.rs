//! Indexing changes the app did not make.
//!
//! The app's own write paths keep the index current as they go. Anything else that
//! touches the vault — another editor, a hand-restored file, and soon the sync sidecar
//! (ADR-0002) — leaves notes on disk that search cannot see. This module closes that gap
//! by turning filesystem events into index updates.
//!
//! Two decisions shape it:
//!
//! **Intent is resolved at flush time, not read from the event.** A path is only ever
//! recorded as *touched*; when the batch flushes, the filesystem is asked whether that
//! path exists, and it becomes an upsert or a removal accordingly. `notify` reports an
//! external move as a rename on some platforms and as an unordered Remove + Create on
//! others, so trusting the event kind risks the removal landing last and deleting a note
//! that is really there. Asking the filesystem makes create, edit, delete and move the
//! same operation, and immune to those differences.
//!
//! **A burst becomes one commit.** A first sync can fire thousands of events; reindexing
//! per event would thrash. Paths coalesce into a set, and a path is only flushed once it
//! has been quiet — and unchanged — for [`SETTLE`], which is also what keeps a note that
//! another process is still writing from being indexed half-written.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tauri::{AppHandle, Manager};
use walkdir::WalkDir;

use crate::search::SearchIndex;
use crate::state::AppState;

/// How long a path must be quiet, and unchanged, before it is indexed.
///
/// This is the whole defence against indexing a half-written note: an editor writing in
/// several `write` calls keeps resetting the timer, so the read happens after the writer
/// has stopped rather than in the middle of it.
const SETTLE: Duration = Duration::from_millis(400);

/// How often the loop wakes to check for settled paths when no events are arriving.
const TICK: Duration = Duration::from_millis(100);

/// What a path looked like when it was last seen changing.
///
/// Size and mtime together are enough to tell "still being written" from "quiet": a slow
/// writer that pauses longer than [`SETTLE`] still moves one of them, so the path is held
/// back for another window instead of being read torn.
#[derive(PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

impl Fingerprint {
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

struct Seen {
    at: Instant,
    fingerprint: Option<Fingerprint>,
}

impl Seen {
    fn now(path: &Path) -> Self {
        Self {
            at: Instant::now(),
            fingerprint: Fingerprint::of(path),
        }
    }
}

/// Handle for reporting that something outside the app touched a path.
#[derive(Clone)]
pub struct ExternalChanges {
    tx: Sender<PathBuf>,
}

impl ExternalChanges {
    /// Record a touched path. Cheap and never blocks: the caller is the watcher thread,
    /// which must stay responsive to the OS event stream.
    pub fn touch(&self, path: PathBuf) {
        let _ = self.tx.send(path);
    }
}

/// Start the coalescing indexer for `vault_path`. The thread ends when the returned
/// [`ExternalChanges`] and all its clones are dropped, which happens when the vault closes.
pub fn start(app: AppHandle, vault_path: String, search: Arc<SearchIndex>) -> ExternalChanges {
    let (tx, rx) = mpsc::channel::<PathBuf>();
    std::thread::spawn(move || {
        let mut pending: HashMap<PathBuf, Seen> = HashMap::new();
        loop {
            // Only poll while something is waiting to settle; otherwise block, so an idle
            // vault costs nothing.
            let received = if pending.is_empty() {
                rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                rx.recv_timeout(TICK)
            };
            match received {
                Ok(path) => {
                    pending.insert(path.clone(), Seen::now(&path));
                    // Take whatever else is already queued before doing any work, so a
                    // burst collapses instead of flushing once per event.
                    while let Ok(path) = rx.try_recv() {
                        pending.insert(path.clone(), Seen::now(&path));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }

            if pending.is_empty() {
                continue;
            }

            // Defer rather than drop while a bulk writer holds the vault. Dropping would
            // leave the index silently missing whatever landed during the import.
            if app.state::<AppState>().importing.load(Ordering::Relaxed) {
                continue;
            }

            let settled = take_settled(&mut pending, Instant::now());
            if !settled.is_empty() {
                flush(&app, &vault_path, &search, settled);
            }
        }
    });
    ExternalChanges { tx }
}

/// Remove and return the paths that have gone quiet, holding back any still changing.
///
/// A path whose fingerprint moved since it was recorded is not settled: its timer is
/// restarted so it gets another full window. That is what stops a long write being read
/// half-finished.
fn take_settled(pending: &mut HashMap<PathBuf, Seen>, now: Instant) -> Vec<PathBuf> {
    let mut settled = Vec::new();
    pending.retain(|path, seen| {
        if now.duration_since(seen.at) < SETTLE {
            return true;
        }
        if Fingerprint::of(path) == seen.fingerprint {
            settled.push(path.clone());
            return false;
        }
        // Still moving, so give it a fresh window rather than reading it half-written.
        *seen = Seen::now(path);
        true
    });
    settled
}

/// Split settled paths by what is actually on disk, then apply them in one commit.
///
/// This is where "resolve intent at flush time" happens: the event that queued a path is
/// never consulted, only whether the path is a file right now.
fn apply(search: &SearchIndex, settled: Vec<PathBuf>) -> Result<(), ApplyFailure> {
    let mut upserts: Vec<String> = Vec::new();
    let mut gone = Vec::new();
    let mut folder_arrived = false;

    for path in settled {
        let text = path.to_string_lossy().to_string();
        if path.is_file() {
            upserts.push(text);
        } else if path.is_dir() {
            folder_arrived = true;
            // A folder that still exists means it arrived — renamed or moved into place.
            // Linux reports a child event per note and this would be redundant, but
            // Windows reports only the folder, so without walking it the notes inside
            // would leave the index at their old path and never return at the new one.
            upserts.extend(notes_within(&path));
        } else {
            gone.push(text);
        }
    }

    // A vanished path may be a folder, which the filesystem reports as one event while
    // leaving every note beneath it indexed. Resolve the whole batch in a single pass:
    // a sync burst can remove thousands of paths, and scanning per path would make one
    // flush thousands of full index scans.
    let mut removals: HashSet<String> = match search.indexed_paths_under_any(&gone) {
        Ok(descendants) => descendants.into_iter().collect(),
        Err(error) => {
            log::warn!("Could not list indexed notes under removed paths: {error}");
            HashSet::new()
        }
    };
    removals.extend(gone);

    // Windows reports a renamed folder as its new path only, never the old one, so the
    // entries left at the old path have nothing to retire them and search would return
    // each note twice — once pointing at a file that is no longer there. Retire whatever
    // the filesystem no longer backs. Only on a folder event, which is rare, and it makes
    // the index self-healing against any removal the watcher failed to see.
    if folder_arrived {
        match search.indexed_paths_missing_on_disk() {
            Ok(stale) => removals.extend(stale),
            Err(error) => log::warn!("Could not check the index for stale notes: {error}"),
        }
    }

    // One path can reach a batch twice — a folder event contributes every note inside it
    // while the notes may also arrive as their own events. `apply_note_changes` issues all
    // deletes before all adds, so a repeated path would be deleted once and then added
    // twice, leaving two documents and returning the note twice from search.
    let upserts: Vec<String> = {
        let mut seen = HashSet::new();
        upserts.retain(|path| seen.insert(path.clone()));
        upserts
    };

    if upserts.is_empty() && removals.is_empty() {
        return Ok(());
    }
    let removals: Vec<String> = removals.into_iter().collect();
    search
        .apply_note_changes(&removals, &upserts)
        .map_err(|error| ApplyFailure::new(error, removals, upserts))
}

/// Every note inside `directory`, for when the filesystem reports the folder rather than
/// its contents.
///
/// Bounded by the folder that moved rather than the vault, and only reached on a directory
/// event, so this does not run during ordinary note edits.
fn notes_within(directory: &Path) -> Vec<String> {
    WalkDir::new(directory)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("md")
        })
        .map(|entry| entry.path().to_string_lossy().to_string())
        .collect()
}

/// A failed batch, carrying the paths involved so a repair issue can name them.
#[derive(Debug)]
struct ApplyFailure {
    error: String,
    paths: Vec<String>,
}

impl ApplyFailure {
    /// Keeps a bounded sample of the paths: a failed sync burst could involve thousands,
    /// and a repair message listing them all helps nobody.
    fn new(error: String, removals: Vec<String>, upserts: Vec<String>) -> Self {
        let mut paths = removals;
        paths.extend(upserts);
        paths.sort();
        paths.truncate(20);
        Self { error, paths }
    }
}

fn flush(app: &AppHandle, vault_path: &str, search: &Arc<SearchIndex>, settled: Vec<PathBuf>) {
    let Err(ApplyFailure {
        error: incremental_error,
        paths,
    }) = apply(search, settled)
    else {
        return;
    };

    // Same recovery policy the app's own write paths use.
    let state = app.state::<AppState>();
    let _ = crate::commands::rebuild_after_failed_index_update(
        &state,
        vault_path,
        search,
        "Indexing changes made outside the app",
        &incremental_error,
        paths,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touched(path: &Path, ago: Duration) -> Seen {
        Seen {
            at: Instant::now() - ago,
            fingerprint: Fingerprint::of(path),
        }
    }

    fn note(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("external-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A vault with a real on-disk index, so these exercise the acceptance criteria
    /// end to end rather than asserting on the plan.
    fn indexed_vault(label: &str) -> (PathBuf, SearchIndex) {
        let vault = scratch(label);
        for category in ["Projects", "Archives"] {
            std::fs::create_dir_all(vault.join(category)).unwrap();
        }
        std::fs::create_dir_all(vault.join(".helixnotes")).unwrap();
        let index = SearchIndex::new(vault.to_str().unwrap()).unwrap();
        index.rebuild(vault.to_str().unwrap()).unwrap();
        (vault, index)
    }

    fn hits(index: &SearchIndex, query: &str) -> Vec<String> {
        index
            .search(query, 50)
            .unwrap()
            .into_iter()
            .map(|result| result.path)
            .collect()
    }

    #[test]
    fn a_note_created_outside_the_app_becomes_findable() {
        let (vault, index) = indexed_vault("created");
        let path = note(
            &vault.join("Projects"),
            "New.md",
            "body about zylophonic things",
        );
        assert!(hits(&index, "zylophonic").is_empty());

        apply(&index, vec![path.clone()]).unwrap();

        assert_eq!(hits(&index, "zylophonic"), vec![path.to_string_lossy()]);
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn an_edit_outside_the_app_replaces_the_old_content() {
        let (vault, index) = indexed_vault("edited");
        let path = note(
            &vault.join("Projects"),
            "Edit.md",
            "the original quixotrope text",
        );
        apply(&index, vec![path.clone()]).unwrap();
        assert_eq!(hits(&index, "quixotrope").len(), 1);

        std::fs::write(&path, "replaced with zylophonic text").unwrap();
        apply(&index, vec![path.clone()]).unwrap();

        assert_eq!(
            hits(&index, "zylophonic").len(),
            1,
            "new content must match"
        );
        assert!(
            hits(&index, "quixotrope").is_empty(),
            "old content must not"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_note_deleted_outside_the_app_disappears_from_search() {
        let (vault, index) = indexed_vault("deleted");
        let path = note(&vault.join("Projects"), "Doomed.md", "zylophonic");
        apply(&index, vec![path.clone()]).unwrap();
        assert_eq!(hits(&index, "zylophonic").len(), 1);

        std::fs::remove_file(&path).unwrap();
        apply(&index, vec![path]).unwrap();

        assert!(hits(&index, "zylophonic").is_empty());
        std::fs::remove_dir_all(vault).unwrap();
    }

    /// The move arrives as two touched paths in one batch, in the order that would break
    /// a naive implementation: the destination first, then the vanished source.
    #[test]
    fn a_note_moved_between_categories_is_found_once_at_its_new_home() {
        let (vault, index) = indexed_vault("moved");
        let source = note(&vault.join("Projects"), "Move.md", "zylophonic content");
        apply(&index, vec![source.clone()]).unwrap();

        let destination = vault.join("Archives").join("Move.md");
        std::fs::rename(&source, &destination).unwrap();
        apply(&index, vec![destination.clone(), source.clone()]).unwrap();

        assert_eq!(
            hits(&index, "zylophonic"),
            vec![destination.to_string_lossy()],
            "the note must be found once, at its new location"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    /// Deleting a folder reports only the folder, so its notes would otherwise survive in
    /// the index as results pointing at files that no longer exist.
    #[test]
    fn deleting_a_folder_outside_the_app_removes_the_notes_inside_it() {
        let (vault, index) = indexed_vault("folder-deleted");
        let notebook = vault.join("Projects").join("Launch");
        std::fs::create_dir_all(&notebook).unwrap();
        let inside = vec![
            note(&notebook, "One.md", "zylophonic one"),
            note(&notebook, "Two.md", "zylophonic two"),
        ];
        let outside = note(&vault.join("Projects"), "Keep.md", "zylophonic keeper");
        apply(&index, [inside.clone(), vec![outside.clone()]].concat()).unwrap();
        assert_eq!(hits(&index, "zylophonic").len(), 3);

        std::fs::remove_dir_all(&notebook).unwrap();
        apply(&index, vec![notebook]).unwrap();

        assert_eq!(
            hits(&index, "zylophonic"),
            vec![outside.to_string_lossy()],
            "only the note outside the deleted folder should remain"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    /// The coalescing claim: many events for one note become one indexed document, and a
    /// whole burst is a single commit rather than one per event.
    /// Windows reports a folder rename as a single directory event with no per-note events
    /// to follow, so the batch below contains only the two directory paths. Linux happens to
    /// send child events as well, which is why this only failed on Windows.
    #[test]
    fn a_folder_renamed_with_no_child_events_reindexes_the_notes_inside() {
        let (vault, index) = indexed_vault("folder-renamed");
        let source = vault.join("Projects").join("Launch");
        std::fs::create_dir_all(&source).unwrap();
        let names = ["One.md", "Two.md"];
        for name in names {
            note(&source, name, "zylophonic inside");
        }
        apply(&index, names.iter().map(|n| source.join(n)).collect()).unwrap();
        assert_eq!(hits(&index, "zylophonic").len(), 2);

        let destination = vault.join("Archives").join("Launch");
        std::fs::rename(&source, &destination).unwrap();

        // Only the destination: Windows reports the folder's new path and never the old
        // one, so nothing here identifies what to retire.
        apply(&index, vec![destination.clone()]).unwrap();

        let mut found = hits(&index, "zylophonic");
        found.sort();
        let mut expected: Vec<String> = names
            .iter()
            .map(|n| destination.join(n).to_string_lossy().to_string())
            .collect();
        expected.sort();
        assert_eq!(
            found, expected,
            "each note must appear once, at the folder's new location — not lost with the \
             old path, and not duplicated across both"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    /// A folder event contributes the notes inside it, and the same notes can arrive as
    /// their own events in the same batch. Both reaching one flush must still leave one
    /// document per note.
    #[test]
    fn a_note_reaching_one_batch_twice_is_indexed_once() {
        let (vault, index) = indexed_vault("duplicate-upsert");
        let notebook = vault.join("Projects").join("Launch");
        std::fs::create_dir_all(&notebook).unwrap();
        let note_path = note(&notebook, "One.md", "zylophonic once");

        apply(
            &index,
            vec![notebook.clone(), note_path.clone(), note_path.clone()],
        )
        .unwrap();

        assert_eq!(
            hits(&index, "zylophonic"),
            vec![note_path.to_string_lossy()],
            "the note must be indexed once, not once per event that mentioned it"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    /// Several folders removed in one burst are resolved together, not one scan each.
    #[test]
    fn several_folders_removed_at_once_are_all_swept() {
        let (vault, index) = indexed_vault("many-folders");
        let mut removed = Vec::new();
        for name in ["Alpha", "Beta", "Gamma"] {
            let notebook = vault.join("Projects").join(name);
            std::fs::create_dir_all(&notebook).unwrap();
            note(&notebook, "Note.md", "zylophonic inside");
            removed.push(notebook);
        }
        let keeper = note(&vault.join("Projects"), "Keep.md", "zylophonic keeper");
        let mut everything: Vec<PathBuf> = removed.iter().map(|dir| dir.join("Note.md")).collect();
        everything.push(keeper.clone());
        apply(&index, everything).unwrap();
        assert_eq!(hits(&index, "zylophonic").len(), 4);

        for notebook in &removed {
            std::fs::remove_dir_all(notebook).unwrap();
        }
        apply(&index, removed).unwrap();

        assert_eq!(
            hits(&index, "zylophonic"),
            vec![keeper.to_string_lossy()],
            "every removed folder should be swept in the one pass"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_burst_of_changes_collapses_into_one_batch() {
        let (vault, index) = indexed_vault("burst");
        let projects = vault.join("Projects");
        let mut pending = HashMap::new();
        let mut expected = Vec::new();
        for i in 0..50 {
            let path = note(&projects, &format!("Burst{i}.md"), "zylophonic burst");
            expected.push(path.to_string_lossy().to_string());
            // The same path touched repeatedly, as an editor writing in chunks would.
            for _ in 0..5 {
                pending.insert(path.clone(), touched(&path, SETTLE * 2));
            }
        }
        assert_eq!(pending.len(), 50, "repeat touches must coalesce per path");

        let settled = take_settled(&mut pending, Instant::now());
        assert_eq!(settled.len(), 50);
        apply(&index, settled).unwrap();

        let mut found = hits(&index, "zylophonic");
        found.sort();
        expected.sort();
        assert_eq!(
            found, expected,
            "every note in the burst is indexed exactly once"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_quiet_path_settles_and_a_fresh_one_waits() {
        let dir = scratch("settle");
        let quiet = note(&dir, "Quiet.md", "done");
        let fresh = note(&dir, "Fresh.md", "still going");
        let mut pending = HashMap::new();
        pending.insert(quiet.clone(), touched(&quiet, SETTLE * 2));
        pending.insert(fresh.clone(), touched(&fresh, Duration::ZERO));

        let settled = take_settled(&mut pending, Instant::now());

        assert_eq!(settled, vec![quiet]);
        assert!(
            pending.contains_key(&fresh),
            "a fresh path must be held back"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The half-written case: quiet long enough, but the bytes moved since we last looked.
    #[test]
    fn a_path_still_changing_is_held_back_for_another_window() {
        let dir = scratch("half-written");
        let path = note(&dir, "Growing.md", "first chunk");
        let mut pending = HashMap::new();
        pending.insert(path.clone(), touched(&path, SETTLE * 2));

        std::fs::write(&path, "first chunk and rather more of it").unwrap();
        let settled = take_settled(&mut pending, Instant::now());

        assert!(
            settled.is_empty(),
            "a file still growing must not be indexed"
        );
        assert!(pending.contains_key(&path), "it must stay queued");

        // Once it stops moving, the next window lets it through.
        let refreshed = pending.get_mut(&path).unwrap();
        refreshed.at = Instant::now() - SETTLE * 2;
        assert_eq!(take_settled(&mut pending, Instant::now()), vec![path]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_vanished_path_settles_so_it_can_be_removed_from_the_index() {
        let dir = scratch("vanished");
        let path = dir.join("Gone.md");
        let mut pending = HashMap::new();
        // Never existed on disk, so its fingerprint is None and stays None.
        pending.insert(path.clone(), touched(&path, SETTLE * 2));

        assert_eq!(take_settled(&mut pending, Instant::now()), vec![path]);
        assert!(pending.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
