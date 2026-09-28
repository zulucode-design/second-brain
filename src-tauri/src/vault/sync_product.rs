//! Refuses vaults inside folders another sync product manages (ADR-0012). The bundled
//! Syncthing sidecar owns synchronization; a second syncer racing it turns every edit into
//! conflict copies and resurrected deletions.
//!
//! Detection is best-effort: the markers common clients keep at their sync root, plus the
//! OneDrive roots Windows publishes in the environment. The vault picker's confirmation
//! covers products this cannot see.

use std::path::{Path, PathBuf};

/// The sync product managing `vault`, if one is detected.
pub fn managing_product(vault: &Path) -> Option<&'static str> {
    managing_product_with(vault, &onedrive_roots())
}

fn managing_product_with(vault: &Path, onedrive_roots: &[PathBuf]) -> Option<&'static str> {
    let vault = canonical(vault);
    if onedrive_roots
        .iter()
        .any(|root| vault.starts_with(canonical(root)))
    {
        return Some("OneDrive");
    }
    vault.ancestors().find_map(marker_product)
}

fn marker_product(dir: &Path) -> Option<&'static str> {
    // The sync root holds a `.dropbox` file; `~/.dropbox` is the client's config directory
    // and says nothing about folders under the home directory.
    if dir.join(".dropbox").is_file() || dir.join(".dropbox.cache").is_dir() {
        return Some("Dropbox");
    }
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let journal =
            (name.starts_with(".sync_") || name.starts_with("._sync_")) && name.ends_with(".db");
        (journal || name == ".owncloudsync.log").then_some("Nextcloud or ownCloud")
    })
}

fn onedrive_roots() -> Vec<PathBuf> {
    ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("sb-sync-product-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(root.join("sync").join("notes")).unwrap();
            Self(root)
        }

        fn vault(&self) -> PathBuf {
            self.0.join("sync").join("notes")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_plain_local_folder_is_accepted() {
        let scratch = Scratch::new();
        assert_eq!(managing_product_with(&scratch.vault(), &[]), None);
    }

    #[test]
    fn a_dropbox_client_config_directory_is_not_a_sync_root() {
        let scratch = Scratch::new();
        fs::create_dir(scratch.0.join(".dropbox")).unwrap();
        assert_eq!(managing_product_with(&scratch.vault(), &[]), None);
    }

    #[test]
    fn folders_inside_a_dropbox_root_are_rejected() {
        let scratch = Scratch::new();
        fs::write(scratch.0.join("sync").join(".dropbox"), "{}").unwrap();
        assert_eq!(
            managing_product_with(&scratch.vault(), &[]),
            Some("Dropbox")
        );
    }

    #[test]
    fn folders_inside_a_nextcloud_root_are_rejected() {
        let scratch = Scratch::new();
        fs::write(scratch.0.join("sync").join(".sync_0123abcd.db"), "").unwrap();
        assert_eq!(
            managing_product_with(&scratch.vault(), &[]),
            Some("Nextcloud or ownCloud")
        );
    }

    #[test]
    fn folders_inside_a_onedrive_root_are_rejected() {
        let scratch = Scratch::new();
        let roots = [scratch.0.join("sync")];
        assert_eq!(
            managing_product_with(&scratch.vault(), &roots),
            Some("OneDrive")
        );
        assert_eq!(managing_product_with(&scratch.0, &roots), None);
    }
}
