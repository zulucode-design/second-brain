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
    if status.issues.is_empty() {
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
