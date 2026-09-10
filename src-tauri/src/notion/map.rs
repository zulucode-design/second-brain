//! Which local note owns which Notion page, and what still has to happen to it.
//!
//! This is the file that stops a second run creating a second copy of every note. It is
//! also the file most exposed to the vault being synced, so its shape is chosen for that
//! rather than for convenience.
//!
//! **One file per note**, `.helixnotes/notion/<note-id>.json`, not one map. A single map
//! file is rewritten on every run by whichever machine publishes, and the sync engine
//! resolves two versions of a file by writing a conflict copy beside it — so the one file
//! that exists to prevent duplicate pages would be the file most likely to be replaced by
//! a machine-generated copy of itself. Per-note files confine that to one note, and they
//! are the shape ADR-0002 already settled on for `history/`, which had the same problem.
//!
//! **The map is a cache, not the truth.** Every page carries the note's id in a `Note ID`
//! property, so a vault that has lost this directory entirely rebuilds it by querying
//! Notion. That is what makes "the mapping can be rebuilt if lost" a property of the
//! design rather than a recovery routine somebody has to remember to test.
//!
//! **Nothing here is written into the note.** The note's frontmatter is read for its id and
//! never modified; the association lives at the two ends — this file locally, the `Note ID`
//! property in Notion — so the local id scheme is untouched by any of this.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::config::notion_dir;
use crate::history::safe_path_component;

/// What the publisher still owes this note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    /// A page creation was started and its outcome is unknown.
    ///
    /// Written *before* the API call, so a process killed mid-create leaves this behind
    /// rather than nothing. Without it, the next run sees no mapping, creates a second
    /// page, and the first is orphaned with no record that it exists. On restart this
    /// state means "ask Notion", not "create".
    Creating,

    /// The page exists and matches the note as of `content_hash`.
    Published,

    /// The note was deleted locally and its page still has to be trashed.
    ///
    /// The tombstone has to outlive the note because the publisher runs later, by which
    /// time the note is gone and nothing else records which page belonged to it. Deleting
    /// this file at the same time as the note would leak the page permanently: it would sit
    /// in Notion with no way left to find it.
    Deleted,
}

/// What is known about one note's Notion page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapEntry {
    pub state: EntryState,

    /// `None` only while [`EntryState::Creating`] — every other state knows its page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,

    /// The data source the page currently sits in, which is how a category change is
    /// noticed: the note says Areas, the map says the page is in the Projects data source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source_id: Option<String>,

    /// The note's content hash at the last successful push, so an unchanged note costs no
    /// API call at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,

    /// The file's modification time when that hash was taken.
    ///
    /// Checked before the hash so an unchanged note costs no *file read* either. Polling
    /// every five minutes would otherwise re-read the whole vault twelve times an hour to
    /// learn nothing. The hash is still what decides — a file synced from the other machine
    /// gets a new mtime with identical bytes, and must not be republished for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_mtime: Option<i64>,

    /// Why the last attempt failed, if it did.
    ///
    /// A note that cannot be pushed — malformed, or rejected by Notion — is skipped and
    /// retried next run rather than halting the publisher. Recording the reason is what
    /// keeps that from being silent: one unpublishable note failing invisibly for weeks is
    /// a worse outcome than the note not being published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl MapEntry {
    /// A note about to have its page created for the first time.
    pub fn creating() -> Self {
        Self {
            state: EntryState::Creating,
            page_id: None,
            data_source_id: None,
            content_hash: None,
            source_mtime: None,
            last_error: None,
        }
    }

    /// Whether this note needs an API call, given what the vault now holds.
    ///
    /// The three reasons are deliberately distinct: content changed, category changed, or
    /// a previous run left the entry unfinished. A caller that only compared hashes would
    /// miss a category change, because moving a note between categories need not alter a
    /// single byte of it.
    pub fn needs_push(&self, content_hash: &str, data_source_id: &str) -> bool {
        match self.state {
            EntryState::Creating | EntryState::Deleted => true,
            EntryState::Published => {
                self.content_hash.as_deref() != Some(content_hash)
                    || self.data_source_id.as_deref() != Some(data_source_id)
            }
        }
    }

    /// Whether the page has to move to a different data source.
    pub fn needs_move(&self, data_source_id: &str) -> bool {
        self.page_id.is_some()
            && self.data_source_id.as_deref() != Some(data_source_id)
            && self.state != EntryState::Deleted
    }
}

fn entry_path(vault_path: &Path, note_id: &str) -> Result<PathBuf, String> {
    Ok(notion_dir(vault_path).join(format!(
        "{}.json",
        safe_path_component(note_id, "note ID")?
    )))
}

/// Read one note's entry. A missing entry is not an error: it means the note has never
/// been published, which is where every note starts.
pub fn load(vault_path: &Path, note_id: &str) -> Option<MapEntry> {
    let path = entry_path(vault_path, note_id).ok()?;
    let contents = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&contents) {
        Ok(entry) => Some(entry),
        Err(error) => {
            // A damaged entry reads as absent, which sends the note down the create path.
            // That would duplicate a page were the create path naive — it is not: it writes
            // `Creating` first and asks Notion by `Note ID` before creating anything.
            log::warn!("Ignoring an unreadable Notion map entry for {note_id}: {error}");
            None
        }
    }
}

pub fn save(vault_path: &Path, note_id: &str, entry: &MapEntry) -> Result<(), String> {
    let path = entry_path(vault_path, note_id)?;
    let dir = notion_dir(vault_path);
    std::fs::create_dir_all(&dir).map_err(|error| format!("Cannot create {dir:?}: {error}"))?;
    let encoded = serde_json::to_string_pretty(entry).map_err(|error| error.to_string())?;
    std::fs::write(&path, encoded)
        .map_err(|error| format!("Cannot write the Notion map entry for {note_id}: {error}"))
}

pub fn remove(vault_path: &Path, note_id: &str) -> Result<(), String> {
    let path = entry_path(vault_path, note_id)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot remove the Notion map entry: {error}")),
    }
}

/// Mark a deleted note's page for trashing.
///
/// Called when the note is deleted locally, which is the last moment anything knows the
/// note existed. A note that was never published has nothing to trash, so its entry is
/// removed outright rather than left as a tombstone for a page that does not exist.
pub fn tombstone(vault_path: &Path, note_id: &str) -> Result<(), String> {
    let Some(mut entry) = load(vault_path, note_id) else {
        return Ok(());
    };

    if entry.page_id.is_none() {
        return remove(vault_path, note_id);
    }

    entry.state = EntryState::Deleted;
    save(vault_path, note_id, &entry)
}

/// Undo a tombstone, for a note restored from the app's trash before the publisher acted.
///
/// The page was never touched, so this costs no API call and keeps the page identity that
/// the whole move design exists to protect. Letting the page be trashed and recreated
/// instead would give a new page id for a note that never actually left.
pub fn restore(vault_path: &Path, note_id: &str) -> Result<(), String> {
    let Some(mut entry) = load(vault_path, note_id) else {
        return Ok(());
    };

    if entry.state != EntryState::Deleted {
        return Ok(());
    }

    entry.state = EntryState::Published;
    save(vault_path, note_id, &entry)
}

/// Every entry currently in the map, as `(note id, entry)`.
///
/// Used for the two sweeps that are not driven by walking the vault: trashing tombstoned
/// pages, and resolving entries a killed process left `Creating`.
pub fn all(vault_path: &Path) -> Vec<(String, MapEntry)> {
    let dir = notion_dir(vault_path);
    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    for item in read_dir.flatten() {
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // The registry shares this directory and is not a note.
        if stem == "databases" {
            continue;
        }
        if let Some(entry) = load(vault_path, stem) {
            entries.push((stem.to_string(), entry));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("notion-map-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn published(hash: &str, data_source: &str) -> MapEntry {
        MapEntry {
            state: EntryState::Published,
            page_id: Some("page-1".into()),
            data_source_id: Some(data_source.into()),
            content_hash: Some(hash.into()),
            source_mtime: Some(1_000),
            last_error: None,
        }
    }

    #[test]
    fn a_note_never_published_has_no_entry() {
        let vault = tempdir();
        assert!(load(&vault, "note-1").is_none());
    }

    #[test]
    fn an_entry_survives_a_save_and_reload() {
        let vault = tempdir();
        let entry = published("hash-1", "ds-projects");
        save(&vault, "note-1", &entry).unwrap();
        assert_eq!(load(&vault, "note-1"), Some(entry));
    }

    #[test]
    fn an_unchanged_note_needs_no_api_call() {
        let entry = published("hash-1", "ds-projects");
        assert!(!entry.needs_push("hash-1", "ds-projects"));
    }

    #[test]
    fn an_edited_note_needs_a_push() {
        let entry = published("hash-1", "ds-projects");
        assert!(entry.needs_push("hash-2", "ds-projects"));
    }

    #[test]
    fn a_recategorised_note_needs_a_push_even_though_its_bytes_are_identical() {
        // Moving a note between categories changes no content, so a hash comparison alone
        // would decide nothing needs doing and the page would stay in the wrong database.
        let entry = published("hash-1", "ds-projects");
        assert!(entry.needs_push("hash-1", "ds-areas"));
        assert!(entry.needs_move("ds-areas"));
    }

    #[test]
    fn a_note_in_the_right_place_needs_no_move() {
        let entry = published("hash-1", "ds-projects");
        assert!(!entry.needs_move("ds-projects"));
    }

    #[test]
    fn an_interrupted_creation_is_retried_rather_than_assumed_lost() {
        // The window this exists for: page created, process killed before the map was
        // updated. The entry must send the next run to ask Notion, not to skip the note.
        let entry = MapEntry::creating();
        assert!(entry.needs_push("any-hash", "ds-projects"));
        assert_eq!(entry.state, EntryState::Creating);
        assert!(entry.page_id.is_none());
    }

    #[test]
    fn deleting_a_note_leaves_a_tombstone_naming_its_page() {
        let vault = tempdir();
        save(&vault, "note-1", &published("hash-1", "ds-projects")).unwrap();

        tombstone(&vault, "note-1").unwrap();

        let entry = load(&vault, "note-1").expect("the tombstone must outlive the note");
        assert_eq!(entry.state, EntryState::Deleted);
        assert_eq!(
            entry.page_id.as_deref(),
            Some("page-1"),
            "without the page id the page can never be trashed"
        );
    }

    #[test]
    fn deleting_a_note_that_was_never_published_leaves_nothing_behind() {
        let vault = tempdir();
        let mut entry = MapEntry::creating();
        entry.state = EntryState::Creating;
        save(&vault, "note-1", &entry).unwrap();

        tombstone(&vault, "note-1").unwrap();

        assert!(
            load(&vault, "note-1").is_none(),
            "there is no page to trash, so there is nothing to remember"
        );
    }

    #[test]
    fn restoring_a_note_from_trash_clears_the_tombstone_and_keeps_the_page() {
        let vault = tempdir();
        save(&vault, "note-1", &published("hash-1", "ds-projects")).unwrap();
        tombstone(&vault, "note-1").unwrap();

        restore(&vault, "note-1").unwrap();

        let entry = load(&vault, "note-1").unwrap();
        assert_eq!(entry.state, EntryState::Published);
        assert_eq!(entry.page_id.as_deref(), Some("page-1"));
        assert_eq!(
            entry.content_hash.as_deref(),
            Some("hash-1"),
            "a restore that netted out to nothing should not force a re-upload"
        );
    }

    #[test]
    fn restoring_a_note_that_was_not_deleted_changes_nothing() {
        let vault = tempdir();
        let entry = published("hash-1", "ds-projects");
        save(&vault, "note-1", &entry).unwrap();

        restore(&vault, "note-1").unwrap();

        assert_eq!(load(&vault, "note-1"), Some(entry));
    }

    #[test]
    fn a_tombstoned_note_is_never_treated_as_needing_a_move() {
        // Its page is about to be trashed; moving it first would be a wasted call against
        // a rate-limited API, and would move a page into a database only to remove it.
        let mut entry = published("hash-1", "ds-projects");
        entry.state = EntryState::Deleted;
        assert!(!entry.needs_move("ds-areas"));
    }

    #[test]
    fn a_note_id_that_looks_like_a_path_cannot_escape_the_map_directory() {
        // Ids come from frontmatter, which is user-editable and arrives from other
        // machines. This is the same containment the history directory enforces.
        let vault = tempdir();
        assert!(save(&vault, "../../escape", &MapEntry::creating()).is_err());
        assert!(load(&vault, "../../escape").is_none());
    }

    #[test]
    fn a_damaged_entry_reads_as_absent() {
        let vault = tempdir();
        std::fs::create_dir_all(notion_dir(&vault)).unwrap();
        std::fs::write(
            notion_dir(&vault).join("note-1.json"),
            "{ not json at all",
        )
        .unwrap();

        assert!(load(&vault, "note-1").is_none());
    }

    #[test]
    fn listing_entries_ignores_the_database_registry_sharing_the_directory() {
        let vault = tempdir();
        save(&vault, "note-1", &published("h", "ds")).unwrap();
        super::super::config::save_registry(&vault, &Default::default()).unwrap();

        let all = all(&vault);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, "note-1");
    }

    #[test]
    fn removing_an_entry_that_is_already_gone_is_not_an_error() {
        // The publisher removes a tombstone after trashing its page, and may be
        // interrupted between the two. Retrying must not fail.
        let vault = tempdir();
        assert!(remove(&vault, "never-existed").is_ok());
    }
}
