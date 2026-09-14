//! One-way retirement of the removed WebDAV provider.
//!
//! Older releases stored WebDAV settings in `config.json` and passwords in the OS
//! credential store. Production no longer contains a WebDAV client, command, or active
//! configuration type. This module exists only to consume those historical fields once.
//!
//! Retirement is config-first and keyring-second: the cleaned configuration and a durable
//! marker are synced to disk before any credential is deleted. The marker keeps the stable
//! vault IDs whose old keyring entries still need deletion, so an interrupted run retries
//! safely without opening a vault or contacting the former remote.

use crate::secret_store::{SecretId, SecretStore};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

const RETIREMENT_VERSION: u8 = 1;
const RETIREMENT_MARKER: &str = "webdav-retirement-v1.json";
const LEGACY_FLAT_KEYS: &[&str] = &[
    "sync_provider",
    "webdav_url",
    "webdav_username",
    "webdav_password",
    "sync_on_open",
    "sync_on_change",
    "sync_interval_minutes",
    "last_sync_time",
];

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct RetirementMarker {
    version: u8,
    legacy_configuration_found: bool,
    configuration_retired: bool,
    pending_vault_ids: Vec<String>,
    complete: bool,
}

/// Remove historical WebDAV configuration and retire every addressable old password.
///
/// The returned JSON is always safe to deserialize into the current `AppConfig`; it has no
/// WebDAV provider, credential, or schedule fields. No path below a vault is read or written.
pub(crate) fn retire_webdav(
    config_path: &Path,
    contents: &str,
    store: &dyn SecretStore,
) -> Result<String, String> {
    let mut document: Value = serde_json::from_str(contents)
        .map_err(|error| format!("Legacy WebDAV configuration could not be inspected: {error}"))?;
    let (legacy_configuration_found, vault_ids) = strip_webdav(&mut document)?;
    let cleaned = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    let marker_path = marker_path(config_path)?;
    let existing = read_marker(&marker_path)?;
    if let Some(marker) = &existing {
        if marker.version != RETIREMENT_VERSION {
            return Err(format!(
                "Unsupported WebDAV retirement marker version {}",
                marker.version
            ));
        }
    }

    if existing.as_ref().is_some_and(|marker| marker.complete) && !legacy_configuration_found {
        return Ok(cleaned);
    }

    if existing.is_none() && !legacy_configuration_found && vault_ids.is_empty() {
        return Ok(cleaned);
    }

    let mut marker = existing.unwrap_or_else(|| RetirementMarker {
        version: RETIREMENT_VERSION,
        ..Default::default()
    });

    // A downgrade may have written the removed fields again after a completed migration.
    // Re-open retirement in that case and delete any newly-created stable entries too.
    if legacy_configuration_found && marker.complete {
        marker.configuration_retired = false;
        marker.complete = false;
    }
    marker.legacy_configuration_found |= legacy_configuration_found;
    for vault_id in vault_ids {
        if !marker.pending_vault_ids.contains(&vault_id) {
            marker.pending_vault_ids.push(vault_id);
        }
    }
    marker.pending_vault_ids.sort();

    // Persist the recovery plan before changing config.json. If the config write fails,
    // credentials remain untouched and the next startup can retry the same plan.
    write_marker(&marker_path, &marker)?;
    write_durable_private_file(config_path, cleaned.as_bytes())?;

    // This synced marker is the durable boundary: only after it records that production
    // configuration is clean may old credentials be removed.
    marker.configuration_retired = true;
    write_marker(&marker_path, &marker)?;

    while let Some(vault_id) = marker.pending_vault_ids.first().cloned() {
        store.delete(&SecretId::WebdavPassword(vault_id.clone()))?;
        marker.pending_vault_ids.remove(0);
        // A crash between deletion and this write is harmless: deletion is idempotent.
        write_marker(&marker_path, &marker)?;
    }

    marker.complete = true;
    write_marker(&marker_path, &marker)?;
    Ok(cleaned)
}

/// Durably retain the keyring address before a vault is removed from `config.json`.
///
/// An invalid or unreadable marker blocks the removal rather than allowing the last stable
/// vault ID for a historical credential to be lost. Cleanup itself remains startup-owned.
pub(crate) fn reserve_webdav_retirement(config_path: &Path, vault_id: &str) -> Result<(), String> {
    if vault_id.is_empty() {
        return Ok(());
    }

    let marker_path = marker_path(config_path)?;
    let mut marker = read_marker(&marker_path)?.unwrap_or_else(|| RetirementMarker {
        version: RETIREMENT_VERSION,
        ..Default::default()
    });
    if marker.version != RETIREMENT_VERSION {
        return Err(format!(
            "Unsupported WebDAV retirement marker version {}",
            marker.version
        ));
    }
    if !marker
        .pending_vault_ids
        .iter()
        .any(|pending| pending == vault_id)
    {
        marker.pending_vault_ids.push(vault_id.to_string());
        marker.pending_vault_ids.sort();
    }
    marker.complete = false;
    write_marker(&marker_path, &marker)
}

fn marker_path(config_path: &Path) -> Result<PathBuf, String> {
    let parent = config_path
        .parent()
        .ok_or_else(|| "Config path has no parent directory".to_string())?;
    Ok(parent.join(RETIREMENT_MARKER))
}

fn read_marker(path: &Path) -> Result<Option<RetirementMarker>, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map(Some)
            .map_err(|error| format!("WebDAV retirement marker is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn write_marker(path: &Path, marker: &RetirementMarker) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(marker).map_err(|error| error.to_string())?;
    write_durable_private_file(path, &data)
}

fn write_durable_private_file(path: &Path, data: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Private file path has no parent directory".to_string())?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Private file path has no valid filename".to_string())?;
    let temporary = parent.join(format!(".{filename}.{}.tmp", uuid::Uuid::new_v4()));

    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let result = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        file.write_all(data).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);

        atomic_replace(&temporary, path)?;

        // Unix rename durability also requires the containing directory to be synced.
        #[cfg(unix)]
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|error| error.to_string())
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing_filename: *const u16, new_filename: *const u16, flags: u32) -> i32;
    }

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both buffers are NUL-terminated, remain alive for the call, and the flags
    // request an atomic replacement with write-through durability.
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|error| error.to_string())
}

fn strip_webdav(document: &mut Value) -> Result<(bool, Vec<String>), String> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| "config.json must contain a JSON object".to_string())?;
    let mut found = strip_settings(root);
    let mut vault_ids = Vec::new();

    if let Some(vaults) = root.get_mut("vaults").and_then(Value::as_array_mut) {
        for vault in vaults {
            let Some(vault) = vault.as_object_mut() else {
                continue;
            };
            if let Some(vault_id) = vault
                .get("vault_id")
                .and_then(Value::as_str)
                .filter(|vault_id| !vault_id.is_empty())
            {
                vault_ids.push(vault_id.to_string());
            }
            found |= strip_settings(vault);
        }
    }

    vault_ids.sort();
    vault_ids.dedup();
    Ok((found, vault_ids))
}

fn strip_settings(object: &mut Map<String, Value>) -> bool {
    let mut found = false;
    for key in LEGACY_FLAT_KEYS {
        found |= object.remove(*key).is_some();
    }
    found |= object.remove("schedule").is_some();

    let mut remove_credentials = false;
    if let Some(credentials) = object.get_mut("credentials").and_then(Value::as_object_mut) {
        found |= credentials.remove("webdav").is_some();
        remove_credentials = credentials.is_empty();
    }
    if remove_credentials {
        object.remove("credentials");
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::test_support::MemoryStore;
    use std::cell::RefCell;

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "second-brain-webdav-retirement-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn durable_private_write_replaces_existing_content() {
        let dir = temp_dir();
        let path = dir.join("replacement.json");
        std::fs::write(&path, b"old-content-that-is-longer").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        write_durable_private_file(&path, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let temporary_prefix = ".replacement.json.";
        assert!(
            std::fs::read_dir(&dir).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(temporary_prefix)
            }),
            "the sibling temporary file must be consumed by the atomic replacement"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_marker_blocks_vault_id_loss_and_retry_cleans_the_credential() {
        let dir = temp_dir();
        let config_path = dir.join("config.json");
        let marker_path = marker_path(&config_path).unwrap();
        let original = r#"{"vaults":[{"path":"/missing","name":"Old","vault_id":"stable-id"}]}"#;
        std::fs::write(&config_path, original).unwrap();
        std::fs::write(&marker_path, "not valid json").unwrap();
        let store = MemoryStore::default();
        store
            .set(
                &SecretId::WebdavPassword("stable-id".to_string()),
                "legacy-password",
            )
            .unwrap();

        let error = reserve_webdav_retirement(&config_path, "stable-id").unwrap_err();

        assert!(error.contains("marker is invalid"));
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
        assert!(store
            .get(&SecretId::WebdavPassword("stable-id".to_string()))
            .unwrap()
            .is_some());

        std::fs::remove_file(&marker_path).unwrap();
        reserve_webdav_retirement(&config_path, "stable-id").unwrap();
        let without_vault = r#"{"vaults":[]}"#;
        write_durable_private_file(&config_path, without_vault.as_bytes()).unwrap();

        let persisted = std::fs::read_to_string(&config_path).unwrap();
        retire_webdav(&config_path, &persisted, &store).unwrap();

        assert!(store
            .get(&SecretId::WebdavPassword("stable-id".to_string()))
            .unwrap()
            .is_none());
        let marker = read_marker(&marker_path).unwrap().unwrap();
        assert!(marker.complete);
        assert!(marker.pending_vault_ids.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn flat_global_nested_and_per_vault_settings_are_removed_without_touching_other_data() {
        let dir = temp_dir();
        let config_path = dir.join("config.json");
        let original = r#"{
            "theme": "dark",
            "sync_provider": "webdav",
            "webdav_url": "http://legacy.example/dav",
            "webdav_password": "plaintext-global",
            "schedule": {"on_open": true},
            "vaults": [{
                "path": "/path/that/must/not/be/opened",
                "name": "Brain",
                "vault_id": "stable-id",
                "sync_provider": "webdav",
                "credentials": {
                    "webdav": {"url": "https://example.test", "password": "plaintext-vault"},
                    "future-provider": {"value": 1}
                },
                "sync_interval_minutes": 15,
                "unrelated": {"preserved": true}
            }]
        }"#;
        std::fs::write(&config_path, original).unwrap();
        let store = MemoryStore::default();
        store
            .set(
                &SecretId::WebdavPassword("stable-id".to_string()),
                "stored-password",
            )
            .unwrap();

        let cleaned = retire_webdav(&config_path, original, &store).unwrap();
        let cleaned: Value = serde_json::from_str(&cleaned).unwrap();
        let vault = &cleaned["vaults"][0];

        assert_eq!(cleaned["theme"], "dark");
        assert_eq!(vault["unrelated"]["preserved"], true);
        assert_eq!(vault["credentials"]["future-provider"]["value"], 1);
        for forbidden in ["sync_provider", "webdav_url", "webdav_password", "schedule"] {
            assert!(cleaned.get(forbidden).is_none(), "retained {forbidden}");
        }
        assert!(vault.get("sync_provider").is_none());
        assert!(vault.get("sync_interval_minutes").is_none());
        assert!(vault["credentials"].get("webdav").is_none());
        assert!(store
            .get(&SecretId::WebdavPassword("stable-id".to_string()))
            .unwrap()
            .is_none());

        let marker = read_marker(&marker_path(&config_path).unwrap())
            .unwrap()
            .unwrap();
        assert!(marker.legacy_configuration_found);
        assert!(marker.configuration_retired);
        assert!(marker.pending_vault_ids.is_empty());
        assert!(marker.complete);
        std::fs::remove_dir_all(dir).unwrap();
    }

    struct CountingStore {
        deletes: RefCell<usize>,
    }

    impl SecretStore for CountingStore {
        fn get(&self, _id: &SecretId) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn set(&self, _id: &SecretId, _value: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _id: &SecretId) -> Result<(), String> {
            *self.deletes.borrow_mut() += 1;
            Ok(())
        }
    }

    #[test]
    fn a_config_write_failure_happens_before_any_credential_delete() {
        let dir = temp_dir();
        let config_path = dir.join("config.json");
        std::fs::create_dir(&config_path).unwrap();
        let original =
            r#"{"vaults":[{"vault_id":"stable-id","webdav_url":"https://example.test"}]}"#;
        let store = CountingStore {
            deletes: RefCell::new(0),
        };

        let error = retire_webdav(&config_path, original, &store).unwrap_err();

        assert!(!error.is_empty());
        assert_eq!(*store.deletes.borrow(), 0);
        let marker = read_marker(&marker_path(&config_path).unwrap())
            .unwrap()
            .unwrap();
        assert!(!marker.configuration_retired);
        assert_eq!(marker.pending_vault_ids, ["stable-id"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    struct FailingDeleteStore;

    impl SecretStore for FailingDeleteStore {
        fn get(&self, _id: &SecretId) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn set(&self, _id: &SecretId, _value: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _id: &SecretId) -> Result<(), String> {
            Err("keyring locked".to_string())
        }
    }

    #[test]
    fn a_delete_failure_leaves_a_durable_retry_after_configuration_is_clean() {
        let dir = temp_dir();
        let config_path = dir.join("config.json");
        let original = r#"{"theme":"light","vaults":[{"vault_id":"stable-id","credentials":{"webdav":{"password":"plaintext"}}}]}"#;
        std::fs::write(&config_path, original).unwrap();

        let error = retire_webdav(&config_path, original, &FailingDeleteStore).unwrap_err();

        assert_eq!(error, "keyring locked");
        let persisted = std::fs::read_to_string(&config_path).unwrap();
        assert!(!persisted.contains("webdav"));
        assert!(!persisted.contains("plaintext"));
        let marker = read_marker(&marker_path(&config_path).unwrap())
            .unwrap()
            .unwrap();
        assert!(marker.configuration_retired);
        assert_eq!(marker.pending_vault_ids, ["stable-id"]);
        assert!(!marker.complete);

        let store = MemoryStore::default();
        retire_webdav(&config_path, &persisted, &store).unwrap();
        let completed = read_marker(&marker_path(&config_path).unwrap())
            .unwrap()
            .unwrap();
        assert!(completed.complete);
        assert!(completed.pending_vault_ids.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod vault_path_tests {
    use super::*;
    use crate::secret_store::test_support::MemoryStore;

    #[test]
    fn retirement_never_opens_or_creates_the_legacy_vault_path() {
        let dir = std::env::temp_dir().join(format!(
            "second-brain-webdav-vault-boundary-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.json");
        let missing_vault = dir.join("vault-that-must-stay-missing");
        let original = serde_json::json!({
            "vaults": [{
                "path": missing_vault,
                "name": "Moved vault",
                "vault_id": "stable-id",
                "sync_provider": "webdav",
                "credentials": {"webdav": {"password": "plaintext"}}
            }]
        })
        .to_string();
        std::fs::write(&config_path, &original).unwrap();

        retire_webdav(&config_path, &original, &MemoryStore::default()).unwrap();

        assert!(!missing_vault.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
