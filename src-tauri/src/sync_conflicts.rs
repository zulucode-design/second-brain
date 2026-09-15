use crate::state::AppState;
use crate::vault::conflicts::original_for_conflict;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflict {
    conflict_path: String,
    original_path: String,
    relative_path: String,
    conflict_content: String,
    original_content: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConflictChoice {
    Original,
    Conflict,
}

impl TryFrom<&str> for ConflictChoice {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "original" => Ok(Self::Original),
            "conflict" => Ok(Self::Conflict),
            _ => Err("Choice must be original or conflict".to_string()),
        }
    }
}

fn scan(vault: &Path) -> Result<Vec<SyncConflict>, String> {
    let mut conflicts = Vec::new();
    for entry in walkdir::WalkDir::new(vault)
        .into_iter()
        .filter_entry(|entry| !entry.path().starts_with(vault.join(".helixnotes")))
    {
        let entry = entry.map_err(|error| format!("Could not scan conflicts: {error}"))?;
        let path = entry.path();
        let Some(original) = original_for_conflict(path) else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        conflicts.push(SyncConflict {
            conflict_path: path.to_string_lossy().into_owned(),
            original_path: original.to_string_lossy().into_owned(),
            relative_path: path
                .strip_prefix(vault)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/"),
            conflict_content: std::fs::read_to_string(path)
                .map_err(|error| format!("Could not read conflict: {error}"))?,
            original_content: std::fs::read_to_string(original).ok(),
        });
    }
    conflicts.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(conflicts)
}

#[tauri::command]
pub fn list_sync_conflicts(state: State<'_, AppState>) -> Result<Vec<SyncConflict>, String> {
    let vault = state
        .config
        .lock()
        .map_err(|error| error.to_string())?
        .active_vault
        .clone()
        .ok_or_else(|| "No active vault".to_string())?;
    scan(Path::new(&vault))
}

fn trash_path(vault: &Path, source: &Path) -> Result<PathBuf, String> {
    let trash = vault.join(".helixnotes").join("trash");
    std::fs::create_dir_all(&trash).map_err(|error| error.to_string())?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Conflict has no valid filename".to_string())?;
    Ok(trash.join(format!(
        "{}_{}",
        chrono::Utc::now().format("%Y%m%d%H%M%S%3f"),
        name
    )))
}

fn resolve_files(vault: &Path, conflict: &Path, choice: ConflictChoice) -> Result<(), String> {
    let original = original_for_conflict(conflict)
        .ok_or_else(|| "Not a Syncthing conflict copy".to_string())?;
    match choice {
        ConflictChoice::Original => {
            if !original.exists() {
                return Err(
                    "The current version no longer exists; choose the conflict version".to_string(),
                );
            }
            std::fs::rename(conflict, trash_path(vault, conflict)?)
                .map_err(|error| format!("Could not archive conflict copy: {error}"))?
        }
        ConflictChoice::Conflict => {
            let archived = original
                .exists()
                .then(|| trash_path(vault, &original))
                .transpose()?;
            if let Some(archived) = &archived {
                std::fs::rename(&original, archived)
                    .map_err(|error| format!("Could not preserve original: {error}"))?;
            }
            if let Err(error) = std::fs::rename(conflict, &original) {
                if let Some(archived) = archived {
                    let _ = std::fs::rename(archived, &original);
                }
                return Err(format!("Could not choose conflict version: {error}"));
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn resolve_sync_conflict(
    state: State<'_, AppState>,
    conflict_path: String,
    choice: String,
) -> Result<(), String> {
    let vault_string = state
        .config
        .lock()
        .map_err(|error| error.to_string())?
        .active_vault
        .clone()
        .ok_or_else(|| "No active vault".to_string())?;
    let vault = std::fs::canonicalize(&vault_string).map_err(|error| error.to_string())?;
    let conflict = std::fs::canonicalize(&conflict_path).map_err(|error| error.to_string())?;
    if !conflict.starts_with(&vault) {
        return Err("Conflict is outside the active vault".to_string());
    }
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    resolve_files(
        &vault,
        &conflict,
        ConflictChoice::try_from(choice.as_str())?,
    )?;
    crate::commands::reconcile_bulk_projections(&state, &vault_string)
}

#[cfg(test)]
mod tests {
    use super::{resolve_files, scan, ConflictChoice};
    use crate::vault::conflicts::original_for_conflict;

    fn conflict_fixture() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let vault = std::env::temp_dir().join(format!("conflict-choice-{}", uuid::Uuid::new_v4()));
        let original = vault.join("Projects/Plan.md");
        let conflict = vault.join("Projects/Plan.sync-conflict-20260913-142233-ABCDEF.md");
        std::fs::create_dir_all(original.parent().unwrap()).unwrap();
        std::fs::write(&original, "current").unwrap();
        std::fs::write(&conflict, "conflict").unwrap();
        (vault, original, conflict)
    }
    #[test]
    fn conflict_pattern_pairs_with_its_original_and_near_misses_do_not() {
        let path = std::path::Path::new("Projects/Plan.sync-conflict-20260913-142233-ABCDEF.md");
        assert_eq!(
            original_for_conflict(path).unwrap(),
            std::path::Path::new("Projects/Plan.md")
        );
        assert!(original_for_conflict(std::path::Path::new(
            "Projects/sync-conflict-not-a-copy.md"
        ))
        .is_none());
    }
    #[test]
    fn scanner_returns_both_versions_without_treating_metadata_as_a_conflict() {
        let vault = std::env::temp_dir().join(format!("conflict-scan-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(vault.join("Projects")).unwrap();
        std::fs::create_dir_all(vault.join(".helixnotes")).unwrap();
        std::fs::write(vault.join("Projects/Plan.md"), "local").unwrap();
        std::fs::write(
            vault.join("Projects/Plan.sync-conflict-20260913-142233-ABCDEF.md"),
            "remote",
        )
        .unwrap();
        std::fs::write(
            vault.join(".helixnotes/Hidden.sync-conflict-20260913-142233-ABCDEF.md"),
            "hidden",
        )
        .unwrap();
        let found = scan(&vault).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].original_content.as_deref(), Some("local"));
        assert_eq!(found[0].conflict_content, "remote");
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn keeping_current_archives_the_conflict_copy() {
        let (vault, original, conflict) = conflict_fixture();
        resolve_files(&vault, &conflict, ConflictChoice::Original).unwrap();
        assert_eq!(std::fs::read_to_string(original).unwrap(), "current");
        assert!(!conflict.exists());
        assert_eq!(
            std::fs::read_dir(vault.join(".helixnotes/trash"))
                .unwrap()
                .count(),
            1
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn choosing_conflict_preserves_current_and_promotes_conflict() {
        let (vault, original, conflict) = conflict_fixture();
        resolve_files(&vault, &conflict, ConflictChoice::Conflict).unwrap();
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "conflict");
        assert!(!conflict.exists());
        let archived = std::fs::read_dir(vault.join(".helixnotes/trash"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read_to_string(archived).unwrap(), "current");
        std::fs::remove_dir_all(vault).unwrap();
    }
}
