//! PARA categories: the four buckets every note is filed into.
//!
//! Named for Tiago Forte's *Building a Second Brain*: Projects, Areas, Resources,
//! Archives. The names are fixed by the method, so they are not user-configurable.
//!
//! A note's own `category` field is the single source of truth. Folders are only where
//! an already-categorised note is stored, so the folder never decides what a note is.
//!
//! When the two disagree — an external program dropped a file somewhere, or a sync wrote
//! it to the wrong place — the file moves to match the note. A note carrying no category
//! cannot be filed at all, so it goes to a holding area for the user to resolve.

use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Component, Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParaCategory {
    Projects,
    Areas,
    Resources,
    Archives,
}

impl ParaCategory {
    /// Every category, in the order the method presents them.
    pub const ALL: [ParaCategory; 4] = [
        ParaCategory::Projects,
        ParaCategory::Areas,
        ParaCategory::Resources,
        ParaCategory::Archives,
    ];

    /// The vault folder holding this category's notes. Also the value written to
    /// frontmatter, so the two never need translating between.
    pub fn folder_name(self) -> &'static str {
        match self {
            ParaCategory::Projects => "Projects",
            ParaCategory::Areas => "Areas",
            ParaCategory::Resources => "Resources",
            ParaCategory::Archives => "Archives",
        }
    }

    /// Parse a folder or frontmatter value back into a category.
    ///
    /// Case-insensitive: a vault edited by hand or synced through another tool may
    /// arrive with `projects` rather than `Projects`, and that is still the same bucket.
    pub fn from_name(name: &str) -> Option<ParaCategory> {
        ParaCategory::ALL
            .into_iter()
            .find(|c| c.folder_name().eq_ignore_ascii_case(name.trim()))
    }
}

/// Folder holding notes that have no category and so cannot be filed.
///
/// Not a category and not a place the user files things: a holding area the app empties
/// by asking the user to categorise what landed there. It sits inside the app's own
/// metadata folder so the vault root keeps exactly the four categories.
pub const UNFILED_DIR: &str = "unfiled";

/// The category implied by where a file sits, taken from the first component of its
/// vault-relative path.
///
/// This is **not** the note's category — the note's own `category` field is the only
/// source of truth for that. This reports where a file is *stored*, so the app can move
/// a misplaced file to the folder its category calls for.
pub fn category_for_relative_path(relative_path: &str) -> Option<ParaCategory> {
    let first = Path::new(relative_path)
        .components()
        .find_map(|c| match c {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })?;
    ParaCategory::from_name(first)
}

/// Whether a vault-relative path is one of the four category folders themselves.
///
/// The category names are fixed by the method, so the folders cannot be renamed, moved,
/// or deleted. Sub-folders inside a category are ordinary folders and are not protected.
pub fn is_category_root(relative_path: &str) -> bool {
    let mut components = Path::new(relative_path)
        .components()
        .filter(|c| matches!(c, Component::Normal(_)));

    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) => {
            name.to_str().and_then(ParaCategory::from_name).is_some()
        }
        _ => false,
    }
}

/// Create the four category folders in a vault, if they are not already there.
///
/// Safe to run on an existing vault: it only adds the missing folders and never touches
/// notes already present, so opening a pre-PARA vault gains the structure without
/// disturbing its contents.
pub fn ensure_scaffold(vault_path: &str) -> Result<(), String> {
    for category in ParaCategory::ALL {
        let dir = Path::new(vault_path).join(category.folder_name());
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {}", category.folder_name(), e))?;
    }
    Ok(())
}

/// What a reconciliation pass did, so the user can be told what changed.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ReconcileReport {
    /// Notes whose file was moved to the folder their category calls for.
    pub relocated: usize,
    /// Vault-relative paths of notes now sitting in the holding area, awaiting a
    /// category from the user. Non-empty means the user has something to resolve.
    pub unfiled: Vec<String>,
    /// Uncategorized notes moved into the Holding Area during this pass.
    pub moved_to_holding: usize,
    /// Notes or directories that could not be inspected or corrected. Startup must
    /// surface these instead of presenting a partial pass as fully successful.
    pub failures: Vec<ReconcileFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconcileFailure {
    pub path: String,
    pub message: String,
}

impl ReconcileReport {
    pub fn needs_attention(&self) -> bool {
        !self.unfiled.is_empty() || !self.failures.is_empty()
    }
}

/// Move every note to the folder its own category calls for, and set aside the ones that
/// have no category.
///
/// Runs on vault open, so a note written by an external program — a file sync, a file
/// manager — cannot sit in a folder that contradicts it. The note is never rewritten:
/// only its location changes, because the note is what is true.
pub fn reconcile_vault(vault_path: &str) -> Result<ReconcileReport, String> {
    let root = Path::new(vault_path);
    let unfiled_dir = ensure_holding_area(root)?;
    let mut report = ReconcileReport::default();

    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_app_metadata(entry.path()))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report.failures.push(ReconcileFailure {
                    path: error
                        .path()
                        .map(|path| relative_to(root, path))
                        .unwrap_or_else(|| "<vault>".to_string()),
                    message: error.to_string(),
                });
                continue;
            }
        };
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }

        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) => {
                report.failures.push(ReconcileFailure {
                    path: relative_to(root, path),
                    message: error.to_string(),
                });
                continue;
            }
        };
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let category = crate::vault::frontmatter::parse_note(&raw, &filename)
            .0
            .category;

        let destination_dir = match category {
            Some(category) => root.join(category.folder_name()),
            None => unfiled_dir.clone(),
        };

        // Already where it belongs: anywhere inside the right category, including a
        // sub-folder the user made for their own organisation.
        if category.is_some() && path.starts_with(&destination_dir) {
            continue;
        }

        match relocate_note(root, path, &destination_dir) {
            Ok(_) if category.is_some() => report.relocated += 1,
            Ok(_) => report.moved_to_holding += 1,
            Err(error) => report.failures.push(ReconcileFailure {
                path: relative_to(root, path),
                message: error,
            }),
        }
    }

    // Read the holding area rather than counting only what this pass moved there: notes
    // left unresolved from an earlier open still need the user's attention.
    match list_unfiled(root, &unfiled_dir) {
        Ok(paths) => report.unfiled = paths,
        Err(error) => report.failures.push(ReconcileFailure {
            path: relative_to(root, &unfiled_dir),
            message: error,
        }),
    }

    Ok(report)
}

fn ensure_holding_area(root: &Path) -> Result<std::path::PathBuf, String> {
    let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
    let app_data = root.join(".helixnotes");
    let metadata = std::fs::symlink_metadata(&app_data)
        .map_err(|error| format!("Invalid vault app-data directory: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Vault app-data directory must be a real directory".to_string());
    }
    let holding = app_data.join(UNFILED_DIR);
    match std::fs::create_dir(&holding) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("Could not create Holding Area: {error}")),
    }
    let metadata = std::fs::symlink_metadata(&holding)
        .map_err(|error| format!("Invalid Holding Area: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Holding Area must be a real directory".to_string());
    }
    let holding = std::fs::canonicalize(&holding).map_err(|error| error.to_string())?;
    if holding.parent() != Some(app_data.as_path()) {
        return Err("Holding Area escaped the active vault".to_string());
    }
    Ok(holding)
}

/// Vault-relative paths of every note waiting in the holding area.
fn list_unfiled(root: &Path, unfiled_dir: &Path) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(unfiled_dir).map_err(|error| error.to_string())?;

    let mut paths = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_file() && path.extension().and_then(|extension| extension.to_str()) == Some("md")
        {
            paths.push(relative_to(root, &path));
        }
    }
    paths.sort();
    Ok(paths)
}

/// The app's own metadata folder, and any hidden folder, is not note storage.
fn is_app_metadata(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Move a file into a directory without overwriting anything already there.
pub fn relocate_note(
    vault_root: &Path,
    src: &Path,
    dest_dir: &Path,
) -> Result<std::path::PathBuf, String> {
    let filename = src.file_name().unwrap_or_else(|| OsStr::new("note.md"));
    crate::vault::relocation::relocate_file(vault_root, src, dest_dir, filename, |raw| {
        Ok(raw.to_vec())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_round_trips_through_its_folder_name() {
        for category in ParaCategory::ALL {
            assert_eq!(
                ParaCategory::from_name(category.folder_name()),
                Some(category)
            );
        }
    }

    #[test]
    fn category_names_are_the_four_from_the_book() {
        let names: Vec<&str> = ParaCategory::ALL.iter().map(|c| c.folder_name()).collect();
        assert_eq!(names, ["Projects", "Areas", "Resources", "Archives"]);
    }

    #[test]
    fn parses_hand_edited_names_regardless_of_case() {
        assert_eq!(
            ParaCategory::from_name("projects"),
            Some(ParaCategory::Projects)
        );
        assert_eq!(
            ParaCategory::from_name("  ARCHIVES  "),
            Some(ParaCategory::Archives)
        );
    }

    #[test]
    fn rejects_names_that_are_not_categories() {
        assert_eq!(ParaCategory::from_name(""), None);
        assert_eq!(ParaCategory::from_name("Inbox"), None);
        // Near-misses must not be coerced into a real category.
        assert_eq!(ParaCategory::from_name("Project"), None);
        assert_eq!(ParaCategory::from_name("Archive"), None);
    }

    #[test]
    fn derives_category_from_the_top_level_folder() {
        assert_eq!(
            category_for_relative_path("Projects/launch.md"),
            Some(ParaCategory::Projects)
        );
        assert_eq!(
            category_for_relative_path("Areas/health/running.md"),
            Some(ParaCategory::Areas)
        );
    }

    #[test]
    fn a_nested_category_name_does_not_change_the_category() {
        // The top-level folder decides. A sub-folder that happens to share a category
        // name is just a sub-folder.
        assert_eq!(
            category_for_relative_path("Projects/Archives/old.md"),
            Some(ParaCategory::Projects)
        );
    }

    #[test]
    fn notes_outside_the_para_folders_have_no_category() {
        // A vault predating PARA: notes load as uncategorised rather than being
        // guessed into a bucket.
        assert_eq!(category_for_relative_path("loose-note.md"), None);
        assert_eq!(category_for_relative_path("Inbox/thought.md"), None);
        assert_eq!(category_for_relative_path(""), None);
    }

    #[test]
    fn ignores_leading_path_noise_when_deriving_the_category() {
        assert_eq!(
            category_for_relative_path("./Projects/launch.md"),
            Some(ParaCategory::Projects)
        );
    }

    #[test]
    fn scaffold_creates_the_four_folders() {
        let vault = std::env::temp_dir().join(format!("para-scaffold-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&vault).unwrap();

        ensure_scaffold(&vault.to_string_lossy()).unwrap();

        for category in ParaCategory::ALL {
            assert!(
                vault.join(category.folder_name()).is_dir(),
                "{} folder missing",
                category.folder_name()
            );
        }
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn scaffold_leaves_an_existing_vault_untouched() {
        // Opening a pre-PARA vault must add structure without disturbing what is there.
        let vault = std::env::temp_dir().join(format!("para-existing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(vault.join("Projects")).unwrap();
        std::fs::write(vault.join("Projects/kept.md"), "already here").unwrap();
        std::fs::write(vault.join("loose.md"), "uncategorised").unwrap();

        ensure_scaffold(&vault.to_string_lossy()).unwrap();

        assert_eq!(
            std::fs::read_to_string(vault.join("Projects/kept.md")).unwrap(),
            "already here"
        );
        assert_eq!(
            std::fs::read_to_string(vault.join("loose.md")).unwrap(),
            "uncategorised"
        );
        assert!(vault.join("Areas").is_dir());
        std::fs::remove_dir_all(vault).unwrap();
    }

    /// A vault with the four folders and a holding area, plus a helper to write notes.
    fn reconcile_fixture(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let vault =
            std::env::temp_dir().join(format!("para-rec-{}-{}", label, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(vault.join(".helixnotes")).unwrap();
        ensure_scaffold(&vault.to_string_lossy()).unwrap();
        let unfiled = vault.join(".helixnotes").join(UNFILED_DIR);
        (vault, unfiled)
    }

    fn write_note(path: &Path, category: Option<&str>, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let category_line = category
            .map(|c| format!("category: {}\n", c))
            .unwrap_or_default();
        std::fs::write(
            path,
            format!(
                "---\nid: \"x\"\ntitle: \"T\"\n{}---\n{}",
                category_line, body
            ),
        )
        .unwrap();
    }

    #[test]
    fn a_note_in_the_wrong_folder_moves_to_match_its_category() {
        // The note is the truth; the folder is corrected to agree with it.
        let (vault, _unfiled) = reconcile_fixture("wrong-folder");
        write_note(
            &vault.join("Archives/misplaced.md"),
            Some("Projects"),
            "body\n",
        );

        let report = reconcile_vault(&vault.to_string_lossy()).unwrap();

        assert_eq!(report.relocated, 1);
        assert!(vault.join("Projects/misplaced.md").is_file());
        assert!(!vault.join("Archives/misplaced.md").exists());
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn reconciling_never_rewrites_the_note() {
        let (vault, _unfiled) = reconcile_fixture("no-rewrite");
        write_note(
            &vault.join("Areas/moved.md"),
            Some("Resources"),
            "precious\n",
        );
        let before = std::fs::read_to_string(vault.join("Areas/moved.md")).unwrap();

        reconcile_vault(&vault.to_string_lossy()).unwrap();

        let after = std::fs::read_to_string(vault.join("Resources/moved.md")).unwrap();
        assert_eq!(before, after, "only the location may change");
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_note_with_no_category_goes_to_the_holding_area() {
        let (vault, unfiled) = reconcile_fixture("no-category");
        write_note(&vault.join("Projects/orphan.md"), None, "body\n");

        let report = reconcile_vault(&vault.to_string_lossy()).unwrap();

        assert!(report.needs_attention());
        assert_eq!(report.unfiled.len(), 1);
        assert!(unfiled.join("orphan.md").is_file());
        assert!(!vault.join("Projects/orphan.md").exists());
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_note_already_under_its_category_is_left_alone() {
        let (vault, _unfiled) = reconcile_fixture("already-right");
        write_note(&vault.join("Projects/fine.md"), Some("Projects"), "body\n");
        // A sub-folder the user made is still inside the category.
        write_note(&vault.join("Areas/health/run.md"), Some("Areas"), "body\n");

        let report = reconcile_vault(&vault.to_string_lossy()).unwrap();

        assert_eq!(report.relocated, 0);
        assert!(vault.join("Projects/fine.md").is_file());
        assert!(
            vault.join("Areas/health/run.md").is_file(),
            "a sub-folder inside the right category must not be flattened"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn notes_left_unresolved_are_reported_again_on_the_next_pass() {
        // The user is reminded until they act, not only on the run that set the note aside.
        let (vault, _unfiled) = reconcile_fixture("still-waiting");
        write_note(&vault.join("Projects/orphan.md"), None, "body\n");

        reconcile_vault(&vault.to_string_lossy()).unwrap();
        let second = reconcile_vault(&vault.to_string_lossy()).unwrap();

        assert_eq!(second.unfiled.len(), 1);
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn moving_a_note_never_overwrites_one_already_there() {
        let (vault, _unfiled) = reconcile_fixture("collision");
        write_note(
            &vault.join("Projects/same.md"),
            Some("Projects"),
            "original\n",
        );
        write_note(
            &vault.join("Archives/same.md"),
            Some("Projects"),
            "incoming\n",
        );

        reconcile_vault(&vault.to_string_lossy()).unwrap();

        let original = std::fs::read_to_string(vault.join("Projects/same.md")).unwrap();
        assert!(
            original.contains("original"),
            "the existing note must survive"
        );
        assert!(
            vault.join("Projects/same 1.md").is_file(),
            "the incoming note is kept under a free name"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn scaffold_is_safe_to_run_repeatedly() {
        let vault = std::env::temp_dir().join(format!("para-idempotent-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&vault).unwrap();

        ensure_scaffold(&vault.to_string_lossy()).unwrap();
        std::fs::write(vault.join("Areas/note.md"), "content").unwrap();
        ensure_scaffold(&vault.to_string_lossy()).unwrap();

        assert_eq!(
            std::fs::read_to_string(vault.join("Areas/note.md")).unwrap(),
            "content"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn reconciliation_reports_mixed_success_instead_of_hiding_failed_notes() {
        let (vault, _unfiled) = reconcile_fixture("mixed-result");
        write_note(
            &vault.join("Archives/movable.md"),
            Some("Projects"),
            "body\n",
        );
        std::fs::write(vault.join("Areas/unreadable.md"), [0xff, 0xfe]).unwrap();

        let report = reconcile_vault(&vault.to_string_lossy()).unwrap();

        assert_eq!(report.relocated, 1);
        assert!(vault.join("Projects/movable.md").is_file());
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].path, "Areas/unreadable.md");
        assert!(report.needs_attention());
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_dot_prefixed_vault_root_is_still_reconciled() {
        let parent =
            std::env::temp_dir().join(format!("dot-vault-parent-{}", uuid::Uuid::new_v4()));
        let vault = parent.join(".second-brain");
        std::fs::create_dir_all(vault.join(".helixnotes")).unwrap();
        ensure_scaffold(&vault.to_string_lossy()).unwrap();
        write_note(
            &vault.join("Archives/movable.md"),
            Some("Projects"),
            "body\n",
        );

        let report = reconcile_vault(&vault.to_string_lossy()).unwrap();

        assert_eq!(report.relocated, 1);
        assert!(vault.join("Projects/movable.md").is_file());
        std::fs::remove_dir_all(parent).unwrap();
    }
}
