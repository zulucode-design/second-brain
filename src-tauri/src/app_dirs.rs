//! Where Second Brain keeps machine-level state, and the one-time move out of the inherited
//! `helixnotes` directories (ADR-0011).
//!
//! Upstream HelixNotes writes `<config>/helixnotes` and `<local data>/helixnotes`. Sharing them
//! let a HelixNotes installation's configuration reach Second Brain and let Second Brain rewrite
//! it, so state lives under the application identifier instead: the same directories Tauri
//! already uses for window state, logs, and webview storage.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "io.github.zulucodedesign.SecondBrain";
const LEGACY_DIR: &str = "helixnotes";
/// Written once the legacy directories have been considered, so the move never repeats.
const MIGRATION_MARKER: &str = ".legacy-dirs-checked";

/// Configuration and default backups.
pub fn config_root() -> Result<PathBuf, String> {
    dirs::config_dir()
        .map(|dir| dir.join(APP_ID))
        .ok_or_else(|| "Config directory not available yet".to_string())
}

/// Machine-local vault state and search indexes. Never synced.
pub fn machine_root() -> Result<PathBuf, String> {
    dirs::data_local_dir()
        .map(|dir| dir.join(APP_ID))
        .ok_or_else(|| "No per-machine data directory is available on this system".to_string())
}

/// Move state left in the `helixnotes` directories by earlier Second Brain builds.
///
/// Must run before Tauri starts: it decides ownership from whether the identifier
/// directories exist, and Tauri creates them.
pub fn migrate_legacy_dirs() -> Result<(), String> {
    let config_home = dirs::config_dir().ok_or("Config directory not available yet")?;
    let data_home = dirs::data_local_dir().ok_or("No per-machine data directory is available")?;
    migrate(&config_home, &data_home).map_err(|error| {
        format!(
            "Settings from an earlier Second Brain build could not be moved to {APP_ID}: {error}"
        )
    })
}

fn migrate(config_home: &Path, data_home: &Path) -> io::Result<()> {
    let config_root = config_home.join(APP_ID);
    let machine_root = data_home.join(APP_ID);
    let marker = config_root.join(MIGRATION_MARKER);
    if marker.exists() {
        return Ok(());
    }
    // Only a machine where Second Brain already ran can hold mixed-name state. Otherwise a
    // `helixnotes` directory belongs to HelixNotes, a separate application, and stays untouched.
    if config_root.is_dir() || machine_root.is_dir() {
        move_children(&data_home.join(LEGACY_DIR), &machine_root)?;
        move_children(&config_home.join(LEGACY_DIR), &config_root)?;
    }
    fs::create_dir_all(&config_root)?;
    fs::write(marker, "")
}

/// Rename each entry into `to`. A folder that already exists there is merged, so per-vault
/// state under an existing `vaults/` still moves; an existing file wins and the older copy
/// stays behind. A failure leaves the marker unwritten, so the next launch resumes.
fn move_children(from: &Path, to: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    fs::create_dir_all(to)?;
    for entry in entries {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if !target.exists() {
            match fs::rename(entry.path(), &target) {
                // A second launch racing this one moved it first.
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                result => result?,
            }
        } else if target.is_dir() && entry.path().is_dir() {
            move_children(&entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Homes {
        root: PathBuf,
        config: PathBuf,
        data: PathBuf,
    }

    impl Homes {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("sb-app-dirs-{}", uuid::Uuid::new_v4()));
            let homes = Self {
                config: root.join("config"),
                data: root.join("data"),
                root,
            };
            fs::create_dir_all(&homes.config).unwrap();
            fs::create_dir_all(&homes.data).unwrap();
            homes
        }

        fn legacy_state(&self) {
            fs::create_dir_all(self.config.join(LEGACY_DIR).join("backups")).unwrap();
            fs::write(
                self.config.join(LEGACY_DIR).join("config.json"),
                "{\"v\":1}",
            )
            .unwrap();
            fs::create_dir_all(self.data.join(LEGACY_DIR).join("vaults").join("id")).unwrap();
        }

        fn migrate(&self) {
            migrate(&self.config, &self.data).unwrap();
        }
    }

    impl Drop for Homes {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn earlier_second_brain_state_moves_under_the_identifier() {
        let homes = Homes::new();
        homes.legacy_state();
        fs::create_dir_all(homes.config.join(APP_ID)).unwrap();
        fs::write(homes.config.join(APP_ID).join(".window-state.json"), "{}").unwrap();

        homes.migrate();

        let config = homes.config.join(APP_ID);
        assert_eq!(
            fs::read_to_string(config.join("config.json")).unwrap(),
            "{\"v\":1}"
        );
        assert!(config.join("backups").is_dir());
        assert!(config.join(".window-state.json").is_file());
        assert!(homes.data.join(APP_ID).join("vaults").join("id").is_dir());
        assert!(!homes.config.join(LEGACY_DIR).join("config.json").exists());
    }

    #[test]
    fn a_helixnotes_installation_is_never_touched() {
        let homes = Homes::new();
        homes.legacy_state();

        homes.migrate();

        assert!(homes.config.join(LEGACY_DIR).join("config.json").is_file());
        assert!(homes.config.join(LEGACY_DIR).join("backups").is_dir());
        assert!(homes
            .data
            .join(LEGACY_DIR)
            .join("vaults")
            .join("id")
            .is_dir());
        assert!(!homes.config.join(APP_ID).join("config.json").exists());

        // The decision is permanent: once Second Brain has run, a later HelixNotes
        // configuration is still not adopted.
        fs::create_dir_all(homes.data.join(APP_ID)).unwrap();
        homes.migrate();
        assert!(homes.config.join(LEGACY_DIR).join("config.json").is_file());
        assert!(!homes.config.join(APP_ID).join("config.json").exists());
    }

    #[test]
    fn per_vault_state_merges_into_an_existing_folder() {
        let homes = Homes::new();
        homes.legacy_state();
        fs::create_dir_all(homes.data.join(APP_ID).join("vaults").join("newer")).unwrap();

        homes.migrate();

        let vaults = homes.data.join(APP_ID).join("vaults");
        assert!(vaults.join("id").is_dir());
        assert!(vaults.join("newer").is_dir());
    }

    #[test]
    fn existing_identifier_state_is_never_overwritten() {
        let homes = Homes::new();
        homes.legacy_state();
        fs::create_dir_all(homes.config.join(APP_ID)).unwrap();
        fs::write(homes.config.join(APP_ID).join("config.json"), "{\"v\":2}").unwrap();

        homes.migrate();

        assert_eq!(
            fs::read_to_string(homes.config.join(APP_ID).join("config.json")).unwrap(),
            "{\"v\":2}"
        );
        assert_eq!(
            fs::read_to_string(homes.config.join(LEGACY_DIR).join("config.json")).unwrap(),
            "{\"v\":1}"
        );
    }
}
