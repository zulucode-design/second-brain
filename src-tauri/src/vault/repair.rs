use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RepairStage {
    Search,
    Reconciliation,
    /// Notices, not failures: an interrupted restore was recovered, or restore folders no
    /// record accounts for were found. Retrying leaves them; only dismissing clears them.
    Restore,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepairIssue {
    pub key: String,
    pub stage: RepairStage,
    pub message: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepairStatus {
    #[serde(default)]
    pub issues: Vec<RepairIssue>,
    /// Unowned restore folders the user has already been told about and dismissed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dismissed_restore_folders: Vec<String>,
}

const RESTORE_RECOVERED: &str = "restore:recovered";
const RESTORE_UNOWNED: &str = "restore:unowned";

pub fn unreadable_ledger_issue(vault_path: &str, error: &str) -> RepairIssue {
    RepairIssue {
        key: "reconciliation:ledger".to_string(),
        stage: RepairStage::Reconciliation,
        message: format!("The repair ledger was unreadable and has been replaced: {error}"),
        paths: vec![ledger_location(vault_path)],
    }
}

/// Record what an interrupted-restore recovery has to tell the user (#142). Restore folders
/// no journal accounts for are raised until dismissed, and again whenever the set changes,
/// since one may hold the only copy of a vault's notes.
pub fn apply_restore_recovery(
    status: &mut RepairStatus,
    recovery: &crate::backup::RestoreRecovery,
) {
    use crate::backup::RecoveredRestore;
    if let Some(outcome) = recovery.outcomes.last() {
        let message = match outcome {
            RecoveredRestore::Undone => {
                "A restore was interrupted before it finished. Your vault is back exactly as it \
                 was before the restore; run the restore again if you still want it."
            }
            RecoveredRestore::KeptRestored => {
                "A restore was interrupted after it had finished copying. Your vault was kept as \
                 restored, and the files the restore left behind were cleaned up."
            }
        };
        status.record(RepairIssue {
            key: RESTORE_RECOVERED.to_string(),
            stage: RepairStage::Restore,
            message: message.to_string(),
            paths: Vec::new(),
        });
    }
    let strays: Vec<String> = recovery
        .strays
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    status
        .dismissed_restore_folders
        .retain(|path| strays.contains(path));
    status.issues.retain(|issue| issue.key != RESTORE_UNOWNED);
    if strays
        .iter()
        .any(|path| !status.dismissed_restore_folders.contains(path))
    {
        status.issues.push(RepairIssue {
            key: RESTORE_UNOWNED.to_string(),
            stage: RepairStage::Restore,
            message: "Restore folders were found next to your vault that no restore record \
                      accounts for. They may hold an earlier copy of your notes, so they were \
                      left untouched; move them somewhere safe or delete them once you have \
                      checked them."
                .to_string(),
            paths: strays,
        });
    }
}

/// Clear one restore notice. Dismissed folders are remembered so they are not raised again.
pub fn dismiss_restore_notice(status: &mut RepairStatus, key: &str) {
    if let Some(issue) = status
        .issues
        .iter()
        .find(|issue| issue.key == key && issue.stage == RepairStage::Restore)
    {
        if key == RESTORE_UNOWNED {
            status
                .dismissed_restore_folders
                .extend(issue.paths.iter().cloned());
        }
    }
    status
        .issues
        .retain(|issue| !(issue.key == key && issue.stage == RepairStage::Restore));
}

impl RepairStatus {
    pub fn record(&mut self, issue: RepairIssue) {
        if let Some(existing) = self.issues.iter_mut().find(|value| value.key == issue.key) {
            *existing = issue;
        } else {
            self.issues.push(issue);
        }
    }

    pub fn clear_stage(&mut self, stage: RepairStage) {
        self.issues.retain(|issue| issue.stage != stage);
    }

    pub fn has_stage(&self, stage: RepairStage) -> bool {
        self.issues.iter().any(|issue| issue.stage == stage)
    }
}

/// The ledger records what went wrong on *this* machine, so it lives outside the vault
/// alongside the rest of the machine-local state. All three paths share a parent, which
/// is what keeps the rename dance below atomic.
fn ledger_path(vault_path: &str) -> Result<PathBuf, String> {
    crate::machine_local::repair_ledger_path(Path::new(vault_path))
}

fn backup_path(vault_path: &str) -> Result<PathBuf, String> {
    ledger_path(vault_path).map(|path| path.with_extension("json.backup"))
}

fn temporary_path(vault_path: &str) -> Result<PathBuf, String> {
    ledger_path(vault_path).map(|path| path.with_extension("json.tmp"))
}

/// Where the ledger lives, for showing the user in a repair warning. Falls back to the
/// bare filename when the machine-local directory cannot be resolved — the message is
/// more useful with an approximate location than with none.
pub fn ledger_location(vault_path: &str) -> String {
    ledger_path(vault_path)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| "repair_issues.json".to_string())
}

pub fn load(vault_path: &str) -> Result<RepairStatus, String> {
    let path = ledger_path(vault_path)?;
    let backup = backup_path(vault_path)?;
    if !path.exists() && backup.exists() {
        fs::rename(&backup, &path).map_err(|error| error.to_string())?;
    }
    if !path.exists() {
        return Ok(RepairStatus::default());
    }
    let data = fs::read(path).map_err(|error| error.to_string())?;
    let status = serde_json::from_slice(&data).map_err(|error| error.to_string())?;
    let _ = fs::remove_file(backup);
    Ok(status)
}

pub fn save(vault_path: &str, status: &RepairStatus) -> Result<(), String> {
    let path = ledger_path(vault_path)?;
    if status.issues.is_empty() && status.dismissed_restore_folders.is_empty() {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        }
    }
    let data = serde_json::to_vec_pretty(status).map_err(|error| error.to_string())?;
    let temporary = temporary_path(vault_path)?;
    let backup = backup_path(vault_path)?;
    let _ = fs::remove_file(&temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(&data).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);

    if path.exists() {
        let _ = fs::remove_file(&backup);
        fs::rename(&path, &backup).map_err(|error| error.to_string())?;
    }
    if let Err(error) = fs::rename(&temporary, &path) {
        if backup.exists() {
            let _ = fs::rename(&backup, &path);
        }
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    let _ = fs::remove_file(backup);
    sync_parent(&path).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> std::io::Result<()> {
    File::open(path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strays(paths: &[&str]) -> crate::backup::RestoreRecovery {
        crate::backup::RestoreRecovery {
            outcomes: Vec::new(),
            strays: paths.iter().map(std::path::PathBuf::from).collect(),
        }
    }

    fn unowned(status: &RepairStatus) -> Option<Vec<String>> {
        status
            .issues
            .iter()
            .find(|issue| issue.key == "restore:unowned")
            .map(|issue| issue.paths.clone())
    }

    #[test]
    fn dismissed_restore_folders_stay_quiet_until_the_set_changes() {
        let mut status = RepairStatus::default();
        apply_restore_recovery(
            &mut status,
            &strays(&["/v/.second-brain-restore-rollback-a"]),
        );
        assert_eq!(
            unowned(&status),
            Some(vec!["/v/.second-brain-restore-rollback-a".to_string()])
        );

        dismiss_restore_notice(&mut status, "restore:unowned");
        apply_restore_recovery(
            &mut status,
            &strays(&["/v/.second-brain-restore-rollback-a"]),
        );
        assert_eq!(
            unowned(&status),
            None,
            "a dismissed folder is not raised again"
        );

        apply_restore_recovery(
            &mut status,
            &strays(&[
                "/v/.second-brain-restore-rollback-a",
                "/v/.second-brain-restore-stage-b",
            ]),
        );
        assert_eq!(
            unowned(&status),
            Some(vec![
                "/v/.second-brain-restore-rollback-a".to_string(),
                "/v/.second-brain-restore-stage-b".to_string()
            ]),
            "a new folder raises the notice again, listing every folder"
        );

        apply_restore_recovery(&mut status, &strays(&[]));
        assert_eq!(
            unowned(&status),
            None,
            "folders that are gone clear the notice"
        );
        assert!(status.dismissed_restore_folders.is_empty());
    }

    #[test]
    fn dismissing_one_restore_notice_keeps_the_other() {
        let mut status = RepairStatus::default();
        apply_restore_recovery(
            &mut status,
            &crate::backup::RestoreRecovery {
                outcomes: vec![crate::backup::RecoveredRestore::Undone],
                strays: vec!["/v/.second-brain-restore-rollback-a".into()],
            },
        );
        dismiss_restore_notice(&mut status, "restore:recovered");
        let keys: Vec<&str> = status
            .issues
            .iter()
            .map(|issue| issue.key.as_str())
            .collect();
        assert_eq!(keys, vec!["restore:unowned"]);
    }

    #[test]
    fn dismissed_restore_folders_persist_when_no_issue_remains() {
        let vault =
            std::env::temp_dir().join(format!("repair-dismissed-restore-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(vault.join(".helixnotes")).unwrap();
        let vault = vault.to_string_lossy().to_string();
        let stray = "/v/.second-brain-restore-rollback-a";
        let mut status = RepairStatus::default();
        apply_restore_recovery(&mut status, &strays(&[stray]));
        dismiss_restore_notice(&mut status, RESTORE_UNOWNED);

        save(&vault, &status).unwrap();
        let mut loaded = load(&vault).unwrap();
        apply_restore_recovery(&mut loaded, &strays(&[stray]));

        assert_eq!(unowned(&loaded), None);
        assert_eq!(loaded.dismissed_restore_folders, [stray]);
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn issues_are_deduplicated_and_persist_across_reload() {
        let vault = std::env::temp_dir().join(format!("repair-ledger-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(vault.join(".helixnotes")).unwrap();
        let vault = vault.to_string_lossy().to_string();
        let mut status = RepairStatus::default();
        status.record(RepairIssue {
            key: "search:index".to_string(),
            stage: RepairStage::Search,
            message: "first".to_string(),
            paths: vec!["Projects/Plan.md".to_string()],
        });
        status.record(RepairIssue {
            key: "search:index".to_string(),
            stage: RepairStage::Search,
            message: "latest".to_string(),
            paths: vec!["Archives/Plan.md".to_string()],
        });

        save(&vault, &status).unwrap();
        let loaded = load(&vault).unwrap();

        assert_eq!(loaded.issues.len(), 1);
        assert_eq!(loaded.issues[0].message, "latest");
        assert_eq!(loaded.issues[0].paths, ["Archives/Plan.md"]);
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn clearing_one_repair_stage_preserves_unresolved_issues() {
        let mut status = RepairStatus {
            issues: vec![
                RepairIssue {
                    key: "search:index".to_string(),
                    stage: RepairStage::Search,
                    message: "search".to_string(),
                    paths: Vec::new(),
                },
                RepairIssue {
                    key: "reconciliation:note".to_string(),
                    stage: RepairStage::Reconciliation,
                    message: "note".to_string(),
                    paths: vec!["Projects/Plan.md".to_string()],
                },
            ],
            ..Default::default()
        };

        status.clear_stage(RepairStage::Search);

        assert_eq!(status.issues.len(), 1);
        assert_eq!(status.issues[0].stage, RepairStage::Reconciliation);
    }

    #[test]
    fn load_recovers_a_ledger_left_in_the_backup_slot() {
        let vault = std::env::temp_dir().join(format!("repair-recovery-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(vault.join(".helixnotes")).unwrap();
        let vault = vault.to_string_lossy().to_string();
        let expected = RepairStatus {
            issues: vec![RepairIssue {
                key: "search:index".to_string(),
                stage: RepairStage::Search,
                message: "recover me".to_string(),
                paths: Vec::new(),
            }],
            ..Default::default()
        };
        fs::write(
            backup_path(&vault).unwrap(),
            serde_json::to_vec_pretty(&expected).unwrap(),
        )
        .unwrap();

        assert_eq!(load(&vault).unwrap(), expected);
        assert!(ledger_path(&vault).unwrap().is_file());
        fs::remove_dir_all(vault).unwrap();
    }
}
