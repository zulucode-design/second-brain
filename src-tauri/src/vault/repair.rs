use serde::{Deserialize, Serialize};
use std::fs;
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

fn ledger_path(vault_path: &str) -> PathBuf {
    Path::new(vault_path).join(".helixnotes/repair_issues.json")
}

pub fn load(vault_path: &str) -> Result<RepairStatus, String> {
    let path = ledger_path(vault_path);
    if !path.exists() {
        return Ok(RepairStatus::default());
    }
    let data = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&data).map_err(|error| error.to_string())
}

pub fn save(vault_path: &str, status: &RepairStatus) -> Result<(), String> {
    let path = ledger_path(vault_path);
    if status.issues.is_empty() {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        }
    }
    let data = serde_json::to_vec_pretty(status).map_err(|error| error.to_string())?;
    fs::write(path, data).map_err(|error| error.to_string())
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
}
