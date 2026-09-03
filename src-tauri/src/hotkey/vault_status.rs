//! Whether the vault a capture would file into is actually there, and what to plan when it
//! is not.
//!
//! Registration is gated on a vault having been *configured* — decided once, at startup, from
//! `config.active_vault.is_some()`. Whether that vault is still *present* is a different and
//! far more volatile fact: an external drive gets unplugged, a synced folder gets renamed, a
//! network mount drops. Caching that at registration time would go stale the moment it
//! happened, so it is asked fresh on every activation instead.
//!
//! Kept separate from `startup.rs`: the decision here needs a path and a way to check it, not
//! an `AppHandle`, so it is testable without a running app — same split as the rest of this
//! module (`mod.rs` decides, `portal.rs` and `startup.rs` are the untestable protocol layer).

use std::path::Path;

/// What an activation should do, once it knows what vault it would file into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationPlan {
    /// The vault is there. Proceed as normal.
    ShowCaptureWindow,
    /// A vault was configured, but is not reachable right now. Say so instead of opening an
    /// overlay that can only fail once the user tries to save into it.
    NotifyVaultUnavailable { path: String },
    /// No vault was ever configured. Unreachable in practice — registration itself requires
    /// one — but a real value here rather than a panic if that invariant is ever loosened.
    Nothing,
}

/// Decide what an activation should do. `exists` is injected so this stays a pure function
/// callable from a test without touching the filesystem; the real caller passes
/// [`vault_is_reachable`].
pub fn activation_plan(
    configured_vault: Option<String>,
    exists: impl FnOnce(&str) -> bool,
) -> ActivationPlan {
    match configured_vault {
        Some(path) if exists(&path) => ActivationPlan::ShowCaptureWindow,
        Some(path) => ActivationPlan::NotifyVaultUnavailable { path },
        None => ActivationPlan::Nothing,
    }
}

/// True when `path` names a directory that is actually there right now.
///
/// A plain existence-and-type check, not a deep validity check — permissions, the
/// `.helixnotes` scaffold, and so on are already handled by `open_vault` and `save_note`.
/// This only answers the one question an activation needs before it decides whether to open
/// the overlay at all: is there anywhere to file into.
pub fn vault_is_reachable(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// The title and body for the notification shown when a capture cannot open because its
/// vault is not there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultUnavailableNotice {
    pub title: String,
    pub body: String,
}

pub fn vault_unavailable_notice(path: &str) -> VaultUnavailableNotice {
    VaultUnavailableNotice {
        title: "Quick capture".to_string(),
        body: format!(
            "Your vault isn't available right now ({path}), so there's nowhere to file a \
             note. Open Second Brain to reconnect it, then try the hotkey again."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_available_vault_shows_the_capture_window() {
        let plan = activation_plan(Some("/vault".to_string()), |_| true);
        assert_eq!(plan, ActivationPlan::ShowCaptureWindow);
    }

    #[test]
    fn an_unreachable_configured_vault_is_reported_rather_than_silently_ignored() {
        let plan = activation_plan(Some("/vault".to_string()), |_| false);
        assert_eq!(
            plan,
            ActivationPlan::NotifyVaultUnavailable {
                path: "/vault".to_string()
            }
        );
    }

    #[test]
    fn no_configured_vault_plans_nothing() {
        // Unreachable through the real startup path, since registration itself requires a
        // configured vault, but this is the honest answer if that ever changes.
        assert_eq!(activation_plan(None, |_| true), ActivationPlan::Nothing);
    }

    #[test]
    fn a_real_directory_is_reachable_and_a_missing_path_is_not() {
        let existing = std::env::temp_dir();
        assert!(vault_is_reachable(existing.to_str().unwrap()));
        assert!(!vault_is_reachable("/this/path/should/not/exist/anywhere"));
    }

    #[test]
    fn a_file_is_not_a_reachable_vault() {
        // A vault is a directory. A path that resolves to a plain file (an unmounted network
        // share sometimes leaves a placeholder file, for instance) is not one.
        let file =
            std::env::temp_dir().join(format!("sb-vault-status-test-{}", std::process::id()));
        std::fs::write(&file, b"not a vault").expect("temp file writes");
        assert!(!vault_is_reachable(file.to_str().unwrap()));
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn the_notice_names_the_path_and_says_what_to_do() {
        let notice = vault_unavailable_notice("/mnt/vault");
        assert!(notice.body.contains("/mnt/vault"));
        assert!(notice.body.contains("Open Second Brain"));
    }
}
