//! How a vault's sync is configured, kept in one place instead of loose fields.
//!
//! Sync settings used to sit as individual fields spread across the vault config, with the
//! same set duplicated on the app config as a legacy global. Adding a second provider that
//! way means adding more loose fields in both, and every consumer learning which prefix
//! belongs to which provider.
//!
//! Here a provider's credentials live in that provider's own type, and everything not
//! specific to a provider — when to sync, when it last ran — is shared. Adding a provider
//! is a new field on [`ProviderCredentials`], touching nothing else.
//!
//! On disk, settings written before this grouping existed are still read: the wire form
//! accepts the old flat keys and the new nested ones, preferring the nested. Saving writes
//! the nested form, so a config migrates the first time it is saved and never needs the
//! user to reconfigure.

use serde::{Deserialize, Serialize};

/// Where a WebDAV server is and how to authenticate to it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebdavCredentials {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

impl WebdavCredentials {
    /// Whether anything has actually been configured.
    ///
    /// A URL is the part that cannot be defaulted, so its absence means unconfigured even
    /// if a stray username survived.
    pub fn is_configured(&self) -> bool {
        self.url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty())
    }
}

/// Credentials for each provider the app can sync with.
///
/// One field per provider rather than one flat set: a provider's settings cannot then be
/// confused for another's, and adding one does not disturb the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCredentials {
    #[serde(default)]
    pub webdav: WebdavCredentials,
}

/// When syncing happens. Not tied to any provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSchedule {
    #[serde(default)]
    pub on_open: bool,
    #[serde(default)]
    pub on_change: bool,
    #[serde(default)]
    pub interval_minutes: u32,
    #[serde(default)]
    pub last_sync_time: Option<String>,
}

/// A vault's complete sync configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SyncSettingsWire", into = "SyncSettingsWire")]
pub struct SyncSettings {
    /// Which provider is in use, or `None` when sync is off.
    pub provider: Option<String>,
    pub credentials: ProviderCredentials,
    pub schedule: SyncSchedule,
}

impl SyncSettings {
    /// Whether this vault syncs with the named provider.
    pub fn uses(&self, provider: &str) -> bool {
        self.provider.as_deref() == Some(provider)
    }

    /// Whether anything has been configured at all, for deciding if a legacy global should
    /// be migrated in.
    pub fn is_configured(&self) -> bool {
        self.provider.is_some() || self.credentials.webdav.is_configured()
    }
}

/// The on-disk form, which accepts both the pre-grouping layout and the current one.
///
/// Every field is optional so a config written by any version loads. Reading prefers the
/// nested keys; writing only ever emits them, so the flat keys disappear on first save.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncSettingsWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sync_provider: Option<String>,

    // Current nested layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credentials: Option<ProviderCredentials>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schedule: Option<SyncSchedule>,

    // Layout written before the grouping existed. Read, never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    webdav_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    webdav_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    webdav_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sync_on_open: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sync_on_change: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sync_interval_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_sync_time: Option<String>,
}

impl From<SyncSettingsWire> for SyncSettings {
    fn from(wire: SyncSettingsWire) -> Self {
        // Nested wins where present, so a config saved by this version is not overridden
        // by flat keys an older version may have left behind alongside it.
        let credentials = wire.credentials.unwrap_or(ProviderCredentials {
            webdav: WebdavCredentials {
                url: wire.webdav_url,
                username: wire.webdav_username,
                password: wire.webdav_password,
            },
        });

        let schedule = wire.schedule.unwrap_or(SyncSchedule {
            on_open: wire.sync_on_open.unwrap_or(false),
            on_change: wire.sync_on_change.unwrap_or(false),
            interval_minutes: wire.sync_interval_minutes.unwrap_or(0),
            last_sync_time: wire.last_sync_time,
        });

        Self {
            provider: wire.sync_provider,
            credentials,
            schedule,
        }
    }
}

impl From<SyncSettings> for SyncSettingsWire {
    fn from(settings: SyncSettings) -> Self {
        Self {
            sync_provider: settings.provider,
            credentials: Some(settings.credentials),
            schedule: Some(settings.schedule),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a vault-level settings blob the way serde will when it is flattened.
    fn parse(json: &str) -> SyncSettings {
        serde_json::from_str(json).expect("should parse")
    }

    #[test]
    fn settings_written_before_the_grouping_still_load() {
        // The whole point of the wire form: nobody has to reconfigure.
        let settings = parse(
            r#"{
                "sync_provider": "webdav",
                "webdav_url": "https://example.com/dav",
                "webdav_username": "nicolas",
                "webdav_password": "hunter2",
                "sync_on_open": true,
                "sync_on_change": false,
                "sync_interval_minutes": 30,
                "last_sync_time": "2026-08-24T10:00:00Z"
            }"#,
        );

        assert_eq!(settings.provider.as_deref(), Some("webdav"));
        assert_eq!(
            settings.credentials.webdav.url.as_deref(),
            Some("https://example.com/dav")
        );
        assert_eq!(
            settings.credentials.webdav.username.as_deref(),
            Some("nicolas")
        );
        assert_eq!(
            settings.credentials.webdav.password.as_deref(),
            Some("hunter2")
        );
        assert!(settings.schedule.on_open);
        assert!(!settings.schedule.on_change);
        assert_eq!(settings.schedule.interval_minutes, 30);
        assert_eq!(
            settings.schedule.last_sync_time.as_deref(),
            Some("2026-08-24T10:00:00Z")
        );
    }

    #[test]
    fn settings_survive_a_save_and_reload() {
        let original = parse(
            r#"{"sync_provider":"webdav","webdav_url":"https://example.com/dav",
                "webdav_username":"nicolas","webdav_password":"hunter2",
                "sync_on_open":true,"sync_interval_minutes":15}"#,
        );

        let saved = serde_json::to_string(&original).unwrap();
        let reloaded: SyncSettings = serde_json::from_str(&saved).unwrap();

        assert_eq!(
            original, reloaded,
            "a save must not change what was configured"
        );
    }

    #[test]
    fn saving_writes_the_grouped_form_and_drops_the_flat_keys() {
        let settings =
            parse(r#"{"sync_provider":"webdav","webdav_url":"https://example.com/dav"}"#);

        let saved = serde_json::to_string(&settings).unwrap();

        assert!(
            saved.contains("credentials"),
            "should write the grouped form"
        );
        assert!(
            !saved.contains("webdav_url"),
            "the flat keys should not be written back: {saved}"
        );
    }

    #[test]
    fn the_grouped_form_wins_over_leftover_flat_keys() {
        // An older version writing flat keys alongside must not override current settings.
        let settings = parse(
            r#"{
                "sync_provider": "webdav",
                "webdav_url": "https://stale.example.com",
                "credentials": {"webdav": {"url": "https://current.example.com"}}
            }"#,
        );

        assert_eq!(
            settings.credentials.webdav.url.as_deref(),
            Some("https://current.example.com")
        );
    }

    #[test]
    fn a_vault_that_never_configured_sync_loads_as_unconfigured() {
        let settings = parse("{}");

        assert!(!settings.is_configured());
        assert!(settings.provider.is_none());
        assert_eq!(settings.schedule.interval_minutes, 0);
    }

    #[test]
    fn a_username_without_a_url_is_not_configured() {
        // The URL is the part that cannot be defaulted, so a stray username is not setup.
        let settings = parse(r#"{"webdav_username":"nicolas"}"#);
        assert!(!settings.credentials.webdav.is_configured());
        assert!(!settings.is_configured());
    }

    #[test]
    fn a_blank_url_is_not_configured() {
        let settings = parse(r#"{"webdav_url":"   "}"#);
        assert!(!settings.credentials.webdav.is_configured());
    }

    #[test]
    fn a_whole_vault_entry_written_by_an_older_version_still_loads() {
        // The acceptance criterion end to end: a config saved before the grouping opens
        // with its sync settings intact, and nobody is asked to reconfigure.
        let vault: crate::types::VaultConfig = serde_json::from_str(
            r#"{
                "path": "/home/nicolas/brain",
                "name": "brain",
                "sync_provider": "webdav",
                "webdav_url": "https://example.com/dav",
                "webdav_username": "nicolas",
                "webdav_password": "hunter2",
                "sync_on_open": true,
                "sync_interval_minutes": 30
            }"#,
        )
        .expect("an older vault entry should still load");

        assert_eq!(vault.path, "/home/nicolas/brain");
        assert!(vault.sync.uses("webdav"));
        assert_eq!(
            vault.sync.credentials.webdav.url.as_deref(),
            Some("https://example.com/dav")
        );
        assert!(vault.sync.schedule.on_open);
        assert_eq!(vault.sync.schedule.interval_minutes, 30);
    }

    #[test]
    fn a_vault_entry_survives_being_saved_and_reopened() {
        let json = r#"{"path":"/p","name":"n","sync_provider":"webdav",
            "webdav_url":"https://example.com/dav","webdav_password":"hunter2"}"#;
        let original: crate::types::VaultConfig = serde_json::from_str(json).unwrap();

        let saved = serde_json::to_string(&original).unwrap();
        let reopened: crate::types::VaultConfig = serde_json::from_str(&saved).unwrap();

        assert_eq!(original.sync, reopened.sync);
        assert_eq!(
            reopened.sync.credentials.webdav.password.as_deref(),
            Some("hunter2"),
            "credentials must survive the round trip"
        );
    }

    #[test]
    fn uses_names_the_active_provider_only() {
        let settings = parse(r#"{"sync_provider":"webdav"}"#);
        assert!(settings.uses("webdav"));
        assert!(!settings.uses("notion"));

        let off = parse("{}");
        assert!(!off.uses("webdav"));
    }
}
