use notify::{Config, EventKind, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::search::{external, SearchIndex};
use crate::state::AppState;
use crate::types::FileEvent;
use crate::vault::operations::helixnotes_dir;
use std::sync::Arc;

const IOS_POLL_INTERVAL: Duration = Duration::from_secs(10);
const NATIVE_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatcherBackend {
    Recommended,
    Poll,
}

const fn watcher_backend_for_target(is_ios: bool) -> WatcherBackend {
    if is_ios {
        WatcherBackend::Poll
    } else {
        WatcherBackend::Recommended
    }
}

const fn poll_interval_for_backend(backend: WatcherBackend) -> Duration {
    match backend {
        WatcherBackend::Recommended => NATIVE_POLL_INTERVAL,
        WatcherBackend::Poll => IOS_POLL_INTERVAL,
    }
}

pub enum VaultWatcher {
    Recommended(RecommendedWatcher),
    Poll(PollWatcher),
}

impl VaultWatcher {
    fn watch(&mut self, path: &Path, recursive_mode: RecursiveMode) -> notify::Result<()> {
        match self {
            Self::Recommended(watcher) => watcher.watch(path, recursive_mode),
            Self::Poll(watcher) => watcher.watch(path, recursive_mode),
        }
    }
}

/// Whether the indexer should be told about `path`.
///
/// Hidden paths and `.helixnotes` are excluded through the same predicate the full rebuild
/// uses, so a note indexed live can never be one a rebuild would drop.
///
/// Notes are forwarded so they can be read; anything that no longer exists is forwarded so
/// it can be removed, because a vanished path may be a note *or* a folder full of them and
/// the event cannot tell us which. An existing non-note is deliberately not forwarded:
/// indexing reads files as text, so handing it an image would fail the whole batch.
///
/// The indexer is told about paths even mid-import. It defers the work rather than doing it
/// then; dropping the path here instead would leave search silently missing whatever the
/// import wrote.
fn should_index(path: &Path, vault_root: &Path) -> bool {
    if crate::search::is_ignored_by_index(path, vault_root) {
        return false;
    }
    // An existing directory is forwarded too: a folder renamed into place is reported as
    // one event on Windows, with no per-note events to follow it.
    path.extension().and_then(|value| value.to_str()) == Some("md")
        || !path.exists()
        || path.is_dir()
}

pub fn start_watcher(
    app: AppHandle,
    vault_path: String,
    search: Arc<SearchIndex>,
) -> Result<VaultWatcher, String> {
    let (tx, rx) = mpsc::channel();

    let backend = watcher_backend_for_target(cfg!(target_os = "ios"));
    let config = Config::default().with_poll_interval(poll_interval_for_backend(backend));
    let mut watcher = match backend {
        WatcherBackend::Recommended => VaultWatcher::Recommended(
            RecommendedWatcher::new(tx, config).map_err(|e| e.to_string())?,
        ),
        WatcherBackend::Poll => {
            VaultWatcher::Poll(PollWatcher::new(tx, config).map_err(|e| e.to_string())?)
        }
    };

    watcher
        .watch(Path::new(&vault_path), RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;

    let hn_dir = helixnotes_dir(&vault_path);
    let vault_root = std::path::PathBuf::from(&vault_path);
    let changes = external::start(app.clone(), vault_path.clone(), search);

    std::thread::spawn(move || {
        while let Ok(result) = rx.recv() {
            match result {
                Ok(event) => {
                    // Skip .helixnotes directory events
                    let dominated_by_hn = event.paths.iter().all(|p| p.starts_with(&hn_dir));
                    if dominated_by_hn {
                        continue;
                    }

                    // The indexer needs a laxer rule than the UI, and needs it before the
                    // UI gate below drops anything. A folder deleted or moved out of the
                    // vault no longer reports as a directory and never had a `.md`
                    // extension, so the UI gate discards it — leaving every note that was
                    // inside it in the index, findable but gone from disk.
                    for path in &event.paths {
                        if should_index(path, &vault_root) {
                            changes.touch(path.clone());
                        }
                    }

                    // Only care about .md file events
                    let has_md = event.paths.iter().any(|p| {
                        p.extension().and_then(|x| x.to_str()) == Some("md") || p.is_dir()
                    });

                    if !has_md {
                        continue;
                    }

                    let event_type = match event.kind {
                        EventKind::Create(_) => "create",
                        EventKind::Modify(_) => "modify",
                        EventKind::Remove(_) => "remove",
                        _ => continue,
                    };

                    // The UI event is what floods the IPC channel during a bulk write, so
                    // that is what the importing flag suppresses.
                    let state = app.state::<AppState>();
                    if state.importing.load(std::sync::atomic::Ordering::Relaxed) {
                        continue;
                    }

                    for path in &event.paths {
                        let fe = FileEvent {
                            event_type: event_type.to_string(),
                            path: path.to_string_lossy().to_string(),
                        };
                        let _ = app.emit("file-changed", &fe);
                    }

                    // On mobile, throttle event emission to prevent IPC flooding
                    // on FUSE filesystems where Syncthing/other apps generate
                    // constant file activity that blocks the Tauri command channel.
                    #[cfg(mobile)]
                    {
                        std::thread::sleep(Duration::from_secs(2));
                        while rx.try_recv().is_ok() {}
                    }
                }
                Err(e) => {
                    log::error!("File watcher error: {}", e);
                }
            }
        }
    });

    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("watcher-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The case a `.md`-only filter gets wrong: the folder is gone, so it no longer reports
    /// as a directory and never had a note's extension, yet every note it held is still
    /// indexed and must be swept.
    #[test]
    fn a_vanished_folder_reaches_the_indexer() {
        let vault = scratch("vanished-folder");
        let notebook = vault.join("Projects").join("Launch");

        assert!(
            should_index(&notebook, &vault),
            "a folder that no longer exists must reach the indexer"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn notes_reach_the_indexer_and_other_existing_files_do_not() {
        let vault = scratch("filter");
        let hn = vault.join(".helixnotes");
        std::fs::create_dir_all(&hn).unwrap();
        let note = vault.join("Note.md");
        let image = vault.join("photo.png");
        std::fs::write(&note, "body").unwrap();
        std::fs::write(&image, [0x89, 0x50, 0x4e, 0x47]).unwrap();

        assert!(should_index(&note, &vault));
        assert!(
            !should_index(&image, &vault),
            "indexing reads files as text, so an existing binary must not be forwarded"
        );
        assert!(
            !should_index(&hn.join("state.json"), &vault),
            "machine-local state is never indexed"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    /// The watcher and a full rebuild must agree on what counts as a note. If the watcher
    /// were laxer, a hidden file would be searchable until the next open and then vanish.
    #[test]
    fn hidden_paths_are_withheld_because_a_rebuild_would_drop_them() {
        let vault = scratch("hidden");
        std::fs::create_dir_all(vault.join("Projects")).unwrap();
        let hidden_note = vault.join("Projects").join(".draft.md");
        std::fs::write(&hidden_note, "scratch").unwrap();
        let in_hidden_folder = vault.join(".backup").join("Note.md");

        assert!(
            !should_index(&hidden_note, &vault),
            "a dot-prefixed note is not something a rebuild would index"
        );
        assert!(
            !should_index(&in_hidden_folder, &vault),
            "nor is a note inside a hidden folder"
        );
        // The ordinary note beside it still is.
        let real = vault.join("Projects").join("Real.md");
        std::fs::write(&real, "body").unwrap();
        assert!(should_index(&real, &vault));
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn ios_uses_polling_backend() {
        assert_eq!(watcher_backend_for_target(true), WatcherBackend::Poll);
    }

    #[test]
    fn other_platforms_keep_recommended_backend() {
        assert_eq!(
            watcher_backend_for_target(false),
            WatcherBackend::Recommended
        );
    }

    #[test]
    fn watcher_backends_use_the_expected_intervals() {
        assert_eq!(
            poll_interval_for_backend(WatcherBackend::Poll),
            Duration::from_secs(10)
        );
        assert_eq!(
            poll_interval_for_backend(WatcherBackend::Recommended),
            Duration::from_secs(1)
        );
    }
}
