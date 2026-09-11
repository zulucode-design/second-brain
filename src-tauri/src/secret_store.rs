use crate::types::AppConfig;
use std::collections::HashMap;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SecretId {
    AnthropicApiKey,
    OpenAiApiKey,
    OllamaApiKey,
    OpenAiCompatibleApiKey,
    WebdavPassword(String),
    #[allow(dead_code)] // The credential slot is consumed by the in-flight issue #12 branch.
    NotionToken(String),
}

pub(crate) trait SecretStore {
    fn get(&self, id: &SecretId) -> Result<Option<String>, String>;
    fn set(&self, id: &SecretId, value: &str) -> Result<(), String>;
    fn delete(&self, id: &SecretId) -> Result<(), String>;
}

pub(crate) struct OsSecretStore;

const SERVICE: &str = "io.github.zulucodedesign.SecondBrain";

impl SecretId {
    fn account(&self) -> String {
        match self {
            Self::AnthropicApiKey => "ai:anthropic".to_string(),
            Self::OpenAiApiKey => "ai:openai".to_string(),
            Self::OllamaApiKey => "ai:ollama".to_string(),
            Self::OpenAiCompatibleApiKey => "ai:openai-compatible".to_string(),
            Self::WebdavPassword(vault) => format!("sync:webdav:{vault}"),
            Self::NotionToken(vault) => format!("integration:notion:{vault}"),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl OsSecretStore {
    fn entry(id: &SecretId) -> Result<keyring::v1::Entry, String> {
        keyring::v1::Entry::new(SERVICE, &id.account()).map_err(store_error)
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn store_error(error: keyring::Error) -> String {
    format!("The OS secret store is unavailable or locked: {error}")
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl SecretStore for OsSecretStore {
    fn get(&self, id: &SecretId) -> Result<Option<String>, String> {
        match Self::entry(id)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(store_error(error)),
        }
    }

    fn set(&self, id: &SecretId, value: &str) -> Result<(), String> {
        Self::entry(id)?.set_password(value).map_err(store_error)
    }

    fn delete(&self, id: &SecretId) -> Result<(), String> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(store_error(error)),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
impl SecretStore for OsSecretStore {
    fn get(&self, _id: &SecretId) -> Result<Option<String>, String> {
        Err("The OS secret store is unavailable on this platform".to_string())
    }

    fn set(&self, _id: &SecretId, _value: &str) -> Result<(), String> {
        Err("The OS secret store is unavailable on this platform".to_string())
    }

    fn delete(&self, _id: &SecretId) -> Result<(), String> {
        Err("The OS secret store is unavailable on this platform".to_string())
    }
}

pub(crate) struct LoadOutcome {
    pub(crate) migrated_plaintext: bool,
    pub(crate) changes: AppliedChanges,
}

pub(crate) fn hydrate_config(
    config: &mut AppConfig,
    store: &dyn SecretStore,
) -> Result<LoadOutcome, String> {
    let had_plaintext = has_plaintext_credentials(config);
    if let Some(error) = unaddressable_plaintext(config) {
        clear_credentials(config);
        return Err(error);
    }
    let plaintext = credential_values(config);
    clear_credentials(config);

    // Read every expected entry before changing anything. A locked or unavailable store
    // must not leave the process using plaintext values or a half-migrated keyring.
    let mut previous = HashMap::new();
    for id in credential_ids(config) {
        previous.insert(id.clone(), store.get(&id)?);
    }

    let mut changed = Vec::new();
    for (id, value) in &plaintext {
        if let Err(error) = store.set(id, value) {
            rollback(store, &previous, &changed);
            return Err(error);
        }
        changed.push(id.clone());
    }

    let migration_snapshots = changed
        .iter()
        .map(|id| (id.clone(), previous.get(id).cloned().flatten()))
        .collect();
    for (id, old_value) in &previous {
        assign(
            config,
            id,
            plaintext.get(id).cloned().or_else(|| old_value.clone()),
        );
    }

    Ok(LoadOutcome {
        migrated_plaintext: had_plaintext,
        changes: AppliedChanges(migration_snapshots),
    })
}

pub(crate) fn redacted_config(config: &AppConfig) -> AppConfig {
    let mut redacted = config.clone();
    clear_credentials(&mut redacted);
    redacted.secret_store_error = None;
    redacted
}

/// Preserve an as-yet-unmigrated recovery copy while allowing non-secret settings to save.
///
/// These values are never put back into the running config. They remain only in the file
/// that already held them until a later startup can complete the keyring migration.
pub(crate) fn copy_plaintext_recovery(from: &AppConfig, to: &mut AppConfig) {
    to.ai_api_key.clone_from(&from.ai_api_key);
    to.openai_api_key.clone_from(&from.openai_api_key);
    to.ollama_api_key.clone_from(&from.ollama_api_key);
    to.openai_compatible_api_key
        .clone_from(&from.openai_compatible_api_key);
    to.legacy_sync
        .credentials
        .webdav
        .password
        .clone_from(&from.legacy_sync.credentials.webdav.password);

    for source in &from.vaults {
        let Some(password) = source.sync.credentials.webdav.password.as_ref() else {
            continue;
        };
        if let Some(target) = to.vaults.iter_mut().find(|target| {
            source
                .vault_id
                .as_deref()
                .zip(target.vault_id.as_deref())
                .is_some_and(|(source, target)| source == target)
                || source
                    .bookmark_id
                    .as_deref()
                    .zip(target.bookmark_id.as_deref())
                    .is_some_and(|(source, target)| source == target)
                || (source.bookmark_id.is_none()
                    && target.bookmark_id.is_none()
                    && source.path == target.path)
        }) {
            target.sync.credentials.webdav.password = Some(password.clone());
        }
    }
}

pub(crate) fn has_plaintext_credentials(config: &AppConfig) -> bool {
    !credential_values(config).is_empty()
        || config
            .legacy_sync
            .credentials
            .webdav
            .password
            .as_deref()
            .is_some_and(|password| !password.is_empty())
}

#[derive(Debug)]
pub(crate) struct AppliedChanges(Vec<(SecretId, Option<String>)>);

pub(crate) fn apply_config_changes(
    before: &AppConfig,
    after: &AppConfig,
    store: &dyn SecretStore,
) -> Result<AppliedChanges, String> {
    if let Some(error) = unaddressable_plaintext(before).or_else(|| unaddressable_plaintext(after))
    {
        return Err(error);
    }
    let before_values = credential_values(before);
    let after_values = credential_values(after);
    let mut ids = credential_ids(before);
    for id in credential_ids(after) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let changed: Vec<_> = ids
        .into_iter()
        .filter(|id| before_values.get(id) != after_values.get(id))
        .collect();

    // Snapshot the store itself, not the in-memory config, so rollback restores exactly
    // what was present before this transaction even after an interrupted older migration.
    let mut snapshots = Vec::with_capacity(changed.len());
    for id in &changed {
        snapshots.push((id.clone(), store.get(id)?));
    }

    for (applied, id) in changed.iter().enumerate() {
        let result = match after_values.get(id) {
            Some(value) => store.set(id, value),
            None => store.delete(id),
        };
        if let Err(error) = result {
            let rollback = restore_snapshots(store, &snapshots[..applied]);
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback_error) => {
                    format!("{error}; restoring the previous secret also failed: {rollback_error}")
                }
            });
        }
    }

    Ok(AppliedChanges(snapshots))
}

pub(crate) fn rollback_changes(
    store: &dyn SecretStore,
    changes: &AppliedChanges,
) -> Result<(), String> {
    restore_snapshots(store, &changes.0)
}

fn restore_snapshots(
    store: &dyn SecretStore,
    snapshots: &[(SecretId, Option<String>)],
) -> Result<(), String> {
    for (id, value) in snapshots.iter().rev() {
        match value {
            Some(value) => store.set(id, value)?,
            None => store.delete(id)?,
        }
    }
    Ok(())
}

fn vault_identity(config: &crate::types::VaultConfig) -> Option<&str> {
    config.vault_id.as_deref()
}

fn credential_ids(config: &AppConfig) -> Vec<SecretId> {
    credential_bindings(config)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

fn credential_values(config: &AppConfig) -> HashMap<SecretId, String> {
    credential_bindings(config)
        .into_iter()
        .filter_map(|(id, value)| {
            value
                .filter(|value| !value.is_empty())
                .map(|value| (id, value))
        })
        .collect()
}

fn credential_bindings(config: &AppConfig) -> Vec<(SecretId, Option<String>)> {
    let mut bindings = vec![
        (SecretId::AnthropicApiKey, config.ai_api_key.clone()),
        (SecretId::OpenAiApiKey, config.openai_api_key.clone()),
        (SecretId::OllamaApiKey, config.ollama_api_key.clone()),
        (
            SecretId::OpenAiCompatibleApiKey,
            config.openai_compatible_api_key.clone(),
        ),
    ];
    bindings.extend(config.vaults.iter().filter_map(|vault| {
        vault_identity(vault).map(|identity| {
            (
                SecretId::WebdavPassword(identity.to_string()),
                vault.sync.credentials.webdav.password.clone(),
            )
        })
    }));
    bindings
}

fn clear_credentials(config: &mut AppConfig) {
    let ids = credential_ids(config);
    for id in ids {
        assign(config, &id, None);
    }
    config.legacy_sync.credentials.webdav.password = None;
}

fn assign(config: &mut AppConfig, id: &SecretId, value: Option<String>) {
    match id {
        SecretId::AnthropicApiKey => config.ai_api_key = value,
        SecretId::OpenAiApiKey => config.openai_api_key = value,
        SecretId::OllamaApiKey => config.ollama_api_key = value,
        SecretId::OpenAiCompatibleApiKey => config.openai_compatible_api_key = value,
        SecretId::WebdavPassword(identity) => {
            if let Some(vault) = config
                .vaults
                .iter_mut()
                .find(|vault| vault_identity(vault) == Some(identity.as_str()))
            {
                vault.sync.credentials.webdav.password = value;
            }
        }
        // Reserved for the Notion integration. Its config type lands in issue #12; the
        // stable key is available now so that branch never needs to persist its token.
        SecretId::NotionToken(_) => {}
    }
}

fn unaddressable_plaintext(config: &AppConfig) -> Option<String> {
    if config.vaults.iter().any(|vault| {
        vault
            .sync
            .credentials
            .webdav
            .password
            .as_deref()
            .is_some_and(|password| !password.is_empty())
            && vault_identity(vault).is_none()
    }) {
        return Some(
            "A WebDAV password cannot migrate until its vault identity is available".to_string(),
        );
    }

    let legacy = config
        .legacy_sync
        .credentials
        .webdav
        .password
        .as_deref()
        .filter(|password| !password.is_empty());
    if legacy.is_some_and(|legacy| {
        !config.vaults.iter().any(|vault| {
            vault.sync.credentials.webdav.password.as_deref() == Some(legacy)
                && vault_identity(vault).is_some()
        })
    }) {
        return Some(
            "The legacy WebDAV password cannot migrate until it is associated with an identifiable vault"
                .to_string(),
        );
    }
    None
}

fn rollback(
    store: &dyn SecretStore,
    previous: &HashMap<SecretId, Option<String>>,
    changed: &[SecretId],
) {
    for id in changed.iter().rev() {
        let _ = match previous.get(id).cloned().flatten() {
            Some(value) => store.set(id, &value),
            None => store.delete(id),
        };
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{SecretId, SecretStore};
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    pub(crate) struct MemoryStore(pub(crate) RefCell<HashMap<SecretId, String>>);

    impl SecretStore for MemoryStore {
        fn get(&self, id: &SecretId) -> Result<Option<String>, String> {
            Ok(self.0.borrow().get(id).cloned())
        }

        fn set(&self, id: &SecretId, value: &str) -> Result<(), String> {
            self.0.borrow_mut().insert(id.clone(), value.to_string());
            Ok(())
        }

        fn delete(&self, id: &SecretId) -> Result<(), String> {
            self.0.borrow_mut().remove(id);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::MemoryStore;
    use super::*;
    use crate::sync_config::{ProviderCredentials, SyncSettings, WebdavCredentials};
    use crate::types::VaultConfig;

    #[test]
    fn plaintext_credentials_migrate_to_the_secret_store() {
        let mut config = AppConfig {
            ai_api_key: Some("anthropic-secret".to_string()),
            openai_api_key: Some("openai-secret".to_string()),
            ollama_api_key: Some("ollama-secret".to_string()),
            openai_compatible_api_key: Some("compatible-secret".to_string()),
            vaults: vec![VaultConfig {
                path: "/vaults/life".to_string(),
                name: "Life".to_string(),
                vault_id: Some("life-vault-id".to_string()),
                sync: SyncSettings {
                    credentials: ProviderCredentials {
                        webdav: WebdavCredentials {
                            password: Some("webdav-secret".to_string()),
                            ..Default::default()
                        },
                    },
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let store = MemoryStore::default();

        let outcome = hydrate_config(&mut config, &store).expect("migration should succeed");

        assert!(outcome.migrated_plaintext);
        assert_eq!(config.ai_api_key.as_deref(), Some("anthropic-secret"));
        assert_eq!(config.openai_api_key.as_deref(), Some("openai-secret"));
        assert_eq!(config.ollama_api_key.as_deref(), Some("ollama-secret"));
        assert_eq!(
            config.openai_compatible_api_key.as_deref(),
            Some("compatible-secret")
        );
        assert_eq!(
            config.vaults[0].sync.credentials.webdav.password.as_deref(),
            Some("webdav-secret")
        );
        let stored = store.0.borrow();
        assert_eq!(
            stored.get(&SecretId::AnthropicApiKey).map(String::as_str),
            Some("anthropic-secret")
        );
        assert_eq!(
            stored.get(&SecretId::OpenAiApiKey).map(String::as_str),
            Some("openai-secret")
        );
        assert_eq!(
            stored.get(&SecretId::OllamaApiKey).map(String::as_str),
            Some("ollama-secret")
        );
        assert_eq!(
            stored
                .get(&SecretId::OpenAiCompatibleApiKey)
                .map(String::as_str),
            Some("compatible-secret")
        );
        assert_eq!(stored.len(), 5, "the WebDAV password must migrate too");
    }

    #[test]
    fn persisted_config_contains_no_credentials() {
        let config = AppConfig {
            ai_api_key: Some("anthropic-secret".to_string()),
            openai_api_key: Some("openai-secret".to_string()),
            ollama_api_key: Some("ollama-secret".to_string()),
            openai_compatible_api_key: Some("compatible-secret".to_string()),
            vaults: vec![VaultConfig {
                path: "/vaults/life".to_string(),
                vault_id: Some("life-vault-id".to_string()),
                sync: SyncSettings {
                    credentials: ProviderCredentials {
                        webdav: WebdavCredentials {
                            password: Some("webdav-secret".to_string()),
                            ..Default::default()
                        },
                    },
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        };

        let json = serde_json::to_string(&redacted_config(&config)).unwrap();

        for secret in [
            "anthropic-secret",
            "openai-secret",
            "ollama-secret",
            "compatible-secret",
            "webdav-secret",
        ] {
            assert!(!json.contains(secret), "persisted plaintext: {secret}");
        }
    }

    struct FailsOnOpenAiStore(MemoryStore);

    impl SecretStore for FailsOnOpenAiStore {
        fn get(&self, id: &SecretId) -> Result<Option<String>, String> {
            self.0.get(id)
        }

        fn set(&self, id: &SecretId, value: &str) -> Result<(), String> {
            if id == &SecretId::OpenAiApiKey {
                return Err("keyring locked".to_string());
            }
            self.0.set(id, value)
        }

        fn delete(&self, id: &SecretId) -> Result<(), String> {
            self.0.delete(id)
        }
    }

    #[test]
    fn failed_secret_update_rolls_back_earlier_keyring_changes() {
        let store = FailsOnOpenAiStore(MemoryStore::default());
        store
            .0
            .set(&SecretId::AnthropicApiKey, "old-anthropic")
            .unwrap();
        store.0.set(&SecretId::OpenAiApiKey, "old-openai").unwrap();
        let before = AppConfig {
            ai_api_key: Some("old-anthropic".to_string()),
            openai_api_key: Some("old-openai".to_string()),
            ..Default::default()
        };
        let after = AppConfig {
            ai_api_key: Some("new-anthropic".to_string()),
            openai_api_key: Some("new-openai".to_string()),
            ..Default::default()
        };

        let error = apply_config_changes(&before, &after, &store).unwrap_err();

        assert_eq!(error, "keyring locked");
        assert_eq!(
            store.0.get(&SecretId::AnthropicApiKey).unwrap().as_deref(),
            Some("old-anthropic")
        );
        assert_eq!(
            store.0.get(&SecretId::OpenAiApiKey).unwrap().as_deref(),
            Some("old-openai")
        );
    }

    #[test]
    fn webdav_password_follows_the_vault_id_when_the_folder_moves() {
        let store = MemoryStore::default();
        let mut before_move = AppConfig {
            vaults: vec![VaultConfig {
                path: "/old/location".to_string(),
                vault_id: Some("stable-vault-id".to_string()),
                sync: SyncSettings {
                    credentials: ProviderCredentials {
                        webdav: WebdavCredentials {
                            password: Some("follow-the-vault".to_string()),
                            ..Default::default()
                        },
                    },
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        hydrate_config(&mut before_move, &store).unwrap();
        let mut after_move = AppConfig {
            vaults: vec![VaultConfig {
                path: "/new/location".to_string(),
                vault_id: Some("stable-vault-id".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        hydrate_config(&mut after_move, &store).unwrap();

        assert_eq!(
            after_move.vaults[0]
                .sync
                .credentials
                .webdav
                .password
                .as_deref(),
            Some("follow-the-vault")
        );
    }
}
