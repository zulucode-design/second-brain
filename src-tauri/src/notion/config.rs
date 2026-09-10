//! Where the Notion publisher's settings live, split across two homes for a reason.
//!
//! The split is not organisational tidiness. It falls out of ADR-0002: the vault folder is
//! synced wholesale between machines, so anything in it travels, and anything that must
//! *not* travel has to live somewhere else entirely.
//!
//! - **The token is machine-local.** It is a workspace-write credential, and a credential
//!   in a synced folder is a credential on every machine that folder ever reaches. It
//!   therefore rides on [`crate::types::VaultConfig`], which is stored in the per-machine
//!   `config.json`.
//! - **The database registry is vault-scoped.** It records which Notion databases hold
//!   *this vault's* notes. Keeping it machine-local would mean a second machine could not
//!   tell that the databases already exist, so it would create four more and every note
//!   would appear twice.
//!
//! Notion is deliberately **not** a value of [`crate::sync_config::SyncSettings::provider`].
//! That field names the one provider a vault syncs with, so adding Notion to it would mean
//! that turning on the Notion view turns off file sync. The two are unrelated: the machines
//! sync to each other, and Notion is a read-only view fed separately (ADR-0002).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::vault::para::ParaCategory;

/// The Notion API version this app speaks.
///
/// Pinned rather than tracking whatever is current, because the version is not cosmetic:
/// `2026-03-11` is where `POST /v1/pages/{id}/move` exists, and that endpoint is the only
/// way a note changing category keeps the same page. It is also the version in which a
/// database contains *data sources* and pages are parented to a data source rather than to
/// the database — which is why [`DatabaseLink`] stores both ids.
pub const API_VERSION: &str = "2026-03-11";

/// Written into every database this app creates, and never read by this version.
///
/// Reading it is #58's job: recognising databases created by a previous install so a
/// reinstall reclaims them instead of creating a second set of four. Writing it costs one
/// field in a request already being made, and skipping it would leave every database
/// created by v1 permanently unrecognisable — the future ticket could then only ever help
/// people who set up Notion after it shipped.
pub const MARKER: &str = "Managed by Second Brain. Do not rename this database.";

/// The property holding a note's local id on its Notion page.
///
/// This is the whole reason the mapping can be rebuilt: the local note id travels *to*
/// Notion as an ordinary property, so a vault that has lost its map can query it back.
/// Nothing is written in the other direction — the note's own frontmatter is read, never
/// touched.
pub const NOTE_ID_PROPERTY: &str = "Note ID";

/// How often the publisher looks for changed notes, when nothing says otherwise.
///
/// Five minutes rather than the ten the original design assumed. A poll that finds no
/// changed note makes no API call at all — detection is local — so the interval is paced
/// against how stale the view may be, not against the rate limit.
pub const DEFAULT_POLL_MINUTES: u32 = 5;

/// The machine-local half: the credential and whether this machine publishes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotionSettings {
    /// Whether this machine pushes to Notion.
    ///
    /// Machine-local by design, and load-bearing: exactly one machine may publish. Notion
    /// cannot read anything, so every page exists because some machine called the API, and
    /// two machines calling it can both observe "no page for this note" and both create
    /// one.
    #[serde(default)]
    pub enabled: bool,

    /// The internal integration token.
    ///
    /// Plaintext in a 0600 file today, matching the WebDAV password beside it. #56 moves
    /// every provider secret to the OS keyring at once; doing only this one would protect
    /// the newer credential and leave the older one exposed, which is theatre rather than
    /// security.
    #[serde(default)]
    pub token: Option<String>,

    #[serde(default)]
    pub poll_minutes: Option<u32>,

    /// When a push last completed, for the Settings panel.
    #[serde(default)]
    pub last_run: Option<String>,
}

impl NotionSettings {
    /// Whether this machine has everything it needs to publish.
    ///
    /// The token is the part that cannot be defaulted or recovered, so its absence means
    /// unconfigured however many other fields survived.
    pub fn is_configured(&self) -> bool {
        self.enabled
            && self
                .token
                .as_deref()
                .is_some_and(|token| !token.trim().is_empty())
    }

    pub fn poll_interval_minutes(&self) -> u32 {
        self.poll_minutes.unwrap_or(DEFAULT_POLL_MINUTES).max(1)
    }
}

/// One category's Notion database, and the data source inside it that holds the pages.
///
/// Both ids are kept because the API needs each for different calls: the database id
/// identifies the thing the user sees in their sidebar, and the data source id is what a
/// page is parented to and what a query runs against. Deriving one from the other means an
/// extra round trip on every run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseLink {
    pub database_id: String,
    pub data_source_id: String,
}

/// Which Notion databases hold this vault's notes.
///
/// Vault-scoped, so it travels with the vault and a second machine finds the databases
/// rather than creating its own. It holds no credential — a database id is not a secret,
/// and is useless without the token that lives machine-local.
///
/// Unlike the per-note map this is written at setup and essentially never again, so a
/// single file is safe here: the conflict-copy risk that shaped the map comes from a file
/// being rewritten on every run by two machines, which this one is not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseRegistry {
    /// The Notion page the four databases were created under.
    #[serde(default)]
    pub parent_page_id: Option<String>,

    /// One entry per category. A `BTreeMap` rather than four fields so the file reads in a
    /// stable order and a category missing from it is an ordinary absent key rather than a
    /// null needing interpretation.
    #[serde(default)]
    pub databases: BTreeMap<String, DatabaseLink>,
}

impl DatabaseRegistry {
    pub fn link(&self, category: ParaCategory) -> Option<&DatabaseLink> {
        self.databases.get(category.folder_name())
    }

    pub fn set_link(&mut self, category: ParaCategory, link: DatabaseLink) {
        self.databases
            .insert(category.folder_name().to_string(), link);
    }

    /// Whether every category has a database, which is what "setup finished" means.
    ///
    /// Partial setup is a real state, not a hypothetical: creating four databases is four
    /// API calls and the third can fail. Reporting that as configured would leave notes in
    /// one category silently unpublishable forever.
    pub fn is_complete(&self) -> bool {
        self.parent_page_id.is_some()
            && ParaCategory::ALL
                .iter()
                .all(|category| self.link(*category).is_some())
    }

    /// Categories with no database yet, so setup can resume where it stopped rather than
    /// starting again and creating duplicates of the ones that did succeed.
    pub fn missing(&self) -> Vec<ParaCategory> {
        ParaCategory::ALL
            .into_iter()
            .filter(|category| self.link(*category).is_none())
            .collect()
    }
}

/// The directory holding everything the publisher records about this vault.
///
/// Inside `.helixnotes/`, so it is synced (ADR-0002's table): both machines must agree
/// which note owns which page, or the second machine to publish duplicates every page.
pub fn notion_dir(vault_path: &Path) -> PathBuf {
    vault_path.join(".helixnotes").join("notion")
}

pub fn registry_path(vault_path: &Path) -> PathBuf {
    notion_dir(vault_path).join("databases.json")
}

pub fn load_registry(vault_path: &Path) -> DatabaseRegistry {
    // A registry that cannot be read is treated as absent rather than fatal, and absent
    // means "not set up yet" — which prompts setup instead of publishing. The dangerous
    // reading would be the opposite: treating a damaged file as "no databases exist" and
    // creating four more. Setup is gated on the user, so it cannot happen unattended.
    match std::fs::read_to_string(registry_path(vault_path)) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|error| {
            log::warn!("The Notion database registry is unreadable, so this vault reads as not set up: {error}");
            DatabaseRegistry::default()
        }),
        Err(_) => DatabaseRegistry::default(),
    }
}

pub fn save_registry(vault_path: &Path, registry: &DatabaseRegistry) -> Result<(), String> {
    let dir = notion_dir(vault_path);
    std::fs::create_dir_all(&dir).map_err(|error| format!("Cannot create {dir:?}: {error}"))?;
    let encoded = serde_json::to_string_pretty(registry).map_err(|error| error.to_string())?;
    std::fs::write(registry_path(vault_path), encoded)
        .map_err(|error| format!("Cannot write the Notion database registry: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(n: &str) -> DatabaseLink {
        DatabaseLink {
            database_id: format!("db-{n}"),
            data_source_id: format!("ds-{n}"),
        }
    }

    #[test]
    fn a_machine_without_a_token_is_not_configured() {
        let settings = NotionSettings {
            enabled: true,
            ..Default::default()
        };
        assert!(!settings.is_configured());
    }

    #[test]
    fn a_blank_token_is_not_configured() {
        let settings = NotionSettings {
            enabled: true,
            token: Some("   ".into()),
            ..Default::default()
        };
        assert!(!settings.is_configured());
    }

    #[test]
    fn a_configured_machine_that_is_switched_off_does_not_publish() {
        // Exactly one machine may publish, so the off switch has to beat a valid token.
        let settings = NotionSettings {
            enabled: false,
            token: Some("ntn_example".into()),
            ..Default::default()
        };
        assert!(!settings.is_configured());
    }

    #[test]
    fn settings_that_were_never_configured_write_nothing_surprising() {
        let saved = serde_json::to_value(NotionSettings::default()).unwrap();
        assert_eq!(saved["enabled"], serde_json::json!(false));
        assert_eq!(saved["token"], serde_json::json!(null));
    }

    #[test]
    fn the_poll_interval_never_collapses_to_zero() {
        // A zero interval is a busy loop against a rate-limited API, and it is one typo
        // away in a settings file a user may edit by hand.
        let settings = NotionSettings {
            poll_minutes: Some(0),
            ..Default::default()
        };
        assert_eq!(settings.poll_interval_minutes(), 1);
    }

    #[test]
    fn an_unset_interval_falls_back_to_the_default() {
        assert_eq!(
            NotionSettings::default().poll_interval_minutes(),
            DEFAULT_POLL_MINUTES
        );
    }

    #[test]
    fn a_registry_is_incomplete_until_every_category_has_a_database() {
        let mut registry = DatabaseRegistry {
            parent_page_id: Some("page".into()),
            ..Default::default()
        };
        assert!(!registry.is_complete());

        for category in ParaCategory::ALL {
            registry.set_link(category, link(category.folder_name()));
        }
        assert!(registry.is_complete());
        assert!(registry.missing().is_empty());
    }

    #[test]
    fn a_registry_without_a_parent_page_is_incomplete_even_with_four_databases() {
        // Without the parent there is nowhere to create a replacement, so setup is not
        // finished however many databases happen to exist.
        let mut registry = DatabaseRegistry::default();
        for category in ParaCategory::ALL {
            registry.set_link(category, link(category.folder_name()));
        }
        assert!(!registry.is_complete());
    }

    #[test]
    fn an_interrupted_setup_reports_only_what_is_missing() {
        // Four databases is four API calls; the third can fail. Resuming must not recreate
        // the two that succeeded.
        let mut registry = DatabaseRegistry {
            parent_page_id: Some("page".into()),
            ..Default::default()
        };
        registry.set_link(ParaCategory::Projects, link("p"));
        registry.set_link(ParaCategory::Areas, link("a"));

        assert_eq!(
            registry.missing(),
            vec![ParaCategory::Resources, ParaCategory::Archives]
        );
    }

    #[test]
    fn a_registry_survives_a_save_and_reload() {
        let vault = tempdir();
        let mut registry = DatabaseRegistry {
            parent_page_id: Some("parent".into()),
            ..Default::default()
        };
        registry.set_link(ParaCategory::Resources, link("r"));

        save_registry(&vault, &registry).unwrap();
        assert_eq!(load_registry(&vault), registry);
    }

    #[test]
    fn a_vault_that_never_configured_notion_loads_as_empty() {
        let vault = tempdir();
        let registry = load_registry(&vault);
        assert!(!registry.is_complete());
        assert_eq!(registry.missing().len(), 4);
    }

    #[test]
    fn a_damaged_registry_reads_as_not_set_up_rather_than_panicking() {
        let vault = tempdir();
        std::fs::create_dir_all(notion_dir(&vault)).unwrap();
        std::fs::write(registry_path(&vault), "{ this is not json").unwrap();

        assert!(!load_registry(&vault).is_complete());
    }

    #[test]
    fn the_registry_holds_no_credential() {
        // It lives in the synced vault, so anything secret in it reaches every machine the
        // vault ever touches. This asserts the shape rather than trusting the comment.
        let mut registry = DatabaseRegistry {
            parent_page_id: Some("parent".into()),
            ..Default::default()
        };
        registry.set_link(ParaCategory::Projects, link("p"));

        let saved = serde_json::to_string(&registry).unwrap();
        assert!(!saved.contains("token"), "registry must carry no token");
    }

    fn tempdir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("notion-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
