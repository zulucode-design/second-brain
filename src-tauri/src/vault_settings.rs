//! Small, synced settings whose values must agree across every copy of a vault.
//!
//! Machine-specific preferences remain in `AppConfig`. This file is deliberately inside
//! the vault because different history retention values would make paired machines delete
//! one another's snapshots.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSettings {
    version: u8,
    pub max_versions_per_note: u32,
}

fn path(vault: &Path) -> PathBuf {
    vault.join(".helixnotes").join("settings.json")
}

pub fn load_or_create(vault: &Path, legacy_max_versions: u32) -> Result<VaultSettings, String> {
    let path = path(vault);
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let settings: VaultSettings = serde_json::from_str(&contents)
                .map_err(|error| format!("Shared vault settings are invalid: {error}"))?;
            if settings.version != VERSION || settings.max_versions_per_note == 0 {
                return Err("Shared vault settings are incompatible".to_string());
            }
            Ok(settings)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let settings = VaultSettings {
                version: VERSION,
                max_versions_per_note: legacy_max_versions.max(1),
            };
            write(vault, &settings)?;
            Ok(settings)
        }
        Err(error) => Err(format!("Could not read shared vault settings: {error}")),
    }
}

fn write(vault: &Path, settings: &VaultSettings) -> Result<(), String> {
    let destination = path(vault);
    let parent = destination
        .parent()
        .ok_or_else(|| "Shared vault settings have no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let data = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    crate::durable::replace(&destination, &data, crate::durable::Mode::Shared)
}

#[cfg(test)]
mod tests {
    use super::load_or_create;

    #[test]
    fn first_open_migrates_retention_and_later_machines_adopt_it() {
        let vault = std::env::temp_dir().join(format!("vault-settings-{}", uuid::Uuid::new_v4()));
        let first = load_or_create(&vault, 37).unwrap();
        let second = load_or_create(&vault, 9).unwrap();
        assert_eq!(first.max_versions_per_note, 37);
        assert_eq!(second.max_versions_per_note, 37);
        assert!(vault.join(".helixnotes/settings.json").is_file());
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn invalid_shared_settings_fail_closed() {
        let vault = std::env::temp_dir().join(format!("vault-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(vault.join(".helixnotes")).unwrap();
        std::fs::write(vault.join(".helixnotes/settings.json"), "not json").unwrap();
        assert!(load_or_create(&vault, 20).unwrap_err().contains("invalid"));
        std::fs::remove_dir_all(vault).unwrap();
    }
}
