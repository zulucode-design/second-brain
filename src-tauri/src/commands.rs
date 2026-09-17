use crate::asset_scope;
use crate::bulk_mutation::BulkMutationTerminal;
use crate::hotkey;
use crate::search::SearchIndex;
use crate::state::AppState;
use crate::types::*;
use crate::vault::{operations, repair, watcher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_fs::FsExt;

fn publish_repair_status(
    state: &State<'_, AppState>,
    status: repair::RepairStatus,
) -> Result<(), String> {
    *state
        .repair_status
        .lock()
        .map_err(|error| error.to_string())? = status.clone();
    if let Some(app) = state
        .app_handle
        .lock()
        .map_err(|error| error.to_string())?
        .clone()
    {
        let _ = app.emit("repair-status-changed", status);
    }
    Ok(())
}

fn record_repair_issue(
    state: &State<'_, AppState>,
    vault_path: &str,
    issue: repair::RepairIssue,
) -> Result<(), String> {
    let mut status = repair::load(vault_path).unwrap_or_else(|error| {
        let mut status = state
            .repair_status
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        status.record(repair::RepairIssue {
            key: "reconciliation:ledger".to_string(),
            stage: repair::RepairStage::Reconciliation,
            message: format!("The repair ledger was unreadable and has been replaced: {error}"),
            paths: vec![repair::ledger_location(vault_path)],
        });
        status
    });
    status.record(issue);
    if let Err(error) = repair::save(vault_path, &status) {
        status.record(repair::RepairIssue {
            key: "reconciliation:ledger-write".to_string(),
            stage: repair::RepairStage::Reconciliation,
            message: format!(
                "The repair warning could not be saved to disk. Keep the app open and retry: {error}"
            ),
            paths: vec![repair::ledger_location(vault_path)],
        });
        log::error!("Could not persist vault repair status: {error}");
    }
    publish_repair_status(state, status)
}

fn record_transaction_repair_if_needed(
    state: &State<'_, AppState>,
    vault_path: &str,
    operation: &str,
    paths: Vec<String>,
    error: &str,
) {
    if !error.starts_with("Repair required: ") {
        return;
    }
    let affected = paths.join("|");
    let _ = record_repair_issue(
        state,
        vault_path,
        repair::RepairIssue {
            key: format!("reconciliation:transaction:{operation}:{affected}"),
            stage: repair::RepairStage::Reconciliation,
            message: error.to_string(),
            paths,
        },
    );
}

/// The recovery policy every incremental index update shares: a failed incremental update
/// is recoverable by rebuilding, so only a failed *rebuild* is worth telling the user
/// about. `attempt` names the operation that failed, for the repair message.
///
/// Shared with the external-change indexer, which runs on its own thread and holds its own
/// handle to the index rather than reading it back out of `AppState`.
pub(crate) fn rebuild_after_failed_index_update(
    state: &State<'_, AppState>,
    vault_path: &str,
    search: &SearchIndex,
    attempt: &str,
    incremental_error: &str,
    paths: Vec<String>,
) -> Result<(), String> {
    let Err(rebuild_error) = search.rebuild(vault_path) else {
        return Ok(());
    };
    record_repair_issue(
        state,
        vault_path,
        repair::RepairIssue {
            key: "search:index".to_string(),
            stage: repair::RepairStage::Search,
            message: format!(
                "{attempt} failed ({incremental_error}); full rebuild also failed: {rebuild_error}"
            ),
            paths,
        },
    )
}

fn record_projection_warning(
    warnings: &mut Vec<String>,
    projection: &str,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        let warning = format!("{projection} is pending repair: {error}");
        log::error!("{warning}");
        warnings.push(warning);
    }
}

/// The canonical Markdown commit is authoritative. Projection work is deliberately
/// downgraded to a repair warning after that boundary so callers always adopt the new CAS.
fn committed_save_outcome(
    revision: String,
    projections: impl IntoIterator<Item = (&'static str, Result<(), String>)>,
) -> SaveCommitOutcome {
    let mut warnings = Vec::new();
    for (name, result) in projections {
        record_projection_warning(&mut warnings, name, result);
    }
    SaveCommitOutcome {
        revision,
        warnings,
        entry: None,
    }
}

fn index_note_now(state: &State<'_, AppState>, vault_path: &str, path: &str) -> Result<(), String> {
    let Some(search) = state.search_index.lock().ok().and_then(|g| g.clone()) else {
        return Ok(());
    };
    let Err(incremental_error) = search.index_note(path) else {
        return Ok(());
    };
    rebuild_after_failed_index_update(
        state,
        vault_path,
        &search,
        "Search update",
        &incremental_error,
        vec![path.to_string()],
    )
}

/// Record semantic work after the Markdown mutation has succeeded. Inference never runs
/// here: `note_changed` is a local SQLite enqueue and the semantic worker owns the network.
fn queue_semantic_note_now(state: &State<'_, AppState>, path: &str) {
    let semantic = state
        .semantic_index
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    if let Some(semantic) = semantic {
        if let Err(error) = semantic.note_changed(Path::new(path)) {
            log::error!("Could not queue semantic indexing for {path}: {error}");
        }
    }
}

fn remove_semantic_note_now(state: &State<'_, AppState>, path: &str) {
    let semantic = state
        .semantic_index
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    if let Some(semantic) = semantic {
        if let Err(error) = semantic.note_removed(Path::new(path)) {
            log::error!("Could not remove semantic indexing for {path}: {error}");
        }
    }
}

fn queue_moved_semantic_notes_now(
    state: &State<'_, AppState>,
    old_path: &str,
    new_path: &str,
    rewritten_paths: &[String],
) {
    remove_semantic_note_now(state, old_path);
    queue_semantic_note_now(state, new_path);
    for path in rewritten_paths {
        queue_semantic_note_now(state, path);
    }
}

/// Bulk notebook operations can change many paths without emitting one event per note on
/// every filesystem. Reconciliation is local-only: it removes vanished paths and queues the
/// current Markdown files, leaving inference to the background worker.
fn reconcile_semantic_now(state: &State<'_, AppState>, vault_path: &str) {
    let semantic = state
        .semantic_index
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    if let Some(semantic) = semantic {
        if let Err(error) = semantic.reconcile_from_notes(Path::new(vault_path)) {
            log::error!("Could not reconcile semantic search after a notebook change: {error}");
        }
    }
}

fn remove_note_now(
    state: &State<'_, AppState>,
    vault_path: &str,
    path: &str,
) -> Result<(), String> {
    let Some(search) = state.search_index.lock().ok().and_then(|g| g.clone()) else {
        return Ok(());
    };
    let Err(incremental_error) = search.remove_note(path) else {
        return Ok(());
    };
    rebuild_after_failed_index_update(
        state,
        vault_path,
        &search,
        "Search removal",
        &incremental_error,
        vec![path.to_string()],
    )
}

fn reindex_moved_note_now(
    state: &State<'_, AppState>,
    vault_path: &str,
    old_path: &str,
    new_path: &str,
    rewritten_paths: &[String],
) -> Result<(), String> {
    let Some(search) = state.search_index.lock().ok().and_then(|g| g.clone()) else {
        return Ok(());
    };
    let mut upserts = Vec::with_capacity(rewritten_paths.len() + 1);
    upserts.push(new_path.to_string());
    upserts.extend_from_slice(rewritten_paths);
    let Err(incremental_error) = search.apply_note_changes(&[old_path.to_string()], &upserts)
    else {
        return Ok(());
    };
    let mut paths = vec![old_path.to_string(), new_path.to_string()];
    paths.extend_from_slice(rewritten_paths);
    rebuild_after_failed_index_update(
        state,
        vault_path,
        &search,
        "Search move update",
        &incremental_error,
        paths,
    )
}

fn rebuild_search_now(
    state: &State<'_, AppState>,
    vault_path: &str,
    paths: Vec<String>,
) -> Result<(), String> {
    let search = state
        .search_index
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    if let Some(search) = search {
        if let Err(error) = search.rebuild(vault_path) {
            record_repair_issue(
                state,
                vault_path,
                repair::RepairIssue {
                    key: "search:index".to_string(),
                    stage: repair::RepairStage::Search,
                    message: format!("Full search rebuild failed: {error}"),
                    paths,
                },
            )?;
        }
    }
    Ok(())
}

fn clear_vault_runtime(state: &State<'_, AppState>) -> Result<(), String> {
    *state.watcher.lock().map_err(|error| error.to_string())? = None;
    *state
        .search_index
        .lock()
        .map_err(|error| error.to_string())? = None;
    *state
        .semantic_index
        .lock()
        .map_err(|error| error.to_string())? = None;
    Ok(())
}

// ── Vault Management ──

#[cfg(target_os = "ios")]
use tauri_plugin_ios_vault_access::{FolderSelection, IosVaultAccessExt};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalVaultResult {
    pub bookmark_id: String,
    pub path: String,
    pub name: String,
}

#[cfg(target_os = "ios")]
impl From<FolderSelection> for ExternalVaultResult {
    fn from(selection: FolderSelection) -> Self {
        Self {
            bookmark_id: selection.bookmark_id,
            path: selection.path,
            name: selection.name,
        }
    }
}

fn open_vault_path(
    app: AppHandle,
    state: &State<'_, AppState>,
    path: String,
    external: Option<(String, String)>,
) -> Result<(), String> {
    let _note_mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    operations::ensure_vault_structure(&path)?;
    // Before anything reads state: pull machine-local state out of the vault if this is
    // an older vault, and drop staging left behind by an interrupted note rewrite.
    crate::machine_local::migrate(Path::new(&path))?;
    let legacy_max_versions = state
        .config
        .lock()
        .map_err(|error| error.to_string())?
        .max_versions_per_note;
    let shared_settings =
        crate::vault_settings::load_or_create(Path::new(&path), legacy_max_versions)?;
    let restored = crate::vault::relocation::sweep_staging(Path::new(&path));
    if restored > 0 {
        log::warn!("Restored {restored} note(s) left in staging by an interrupted rewrite");
    }
    let (mut repair_status, ledger_error) = match repair::load(&path) {
        Ok(status) => (status, None),
        Err(error) => (repair::RepairStatus::default(), Some(error)),
    };
    repair_status.clear_stage(repair::RepairStage::Reconciliation);
    if let Some(error) = ledger_error {
        repair_status.record(repair::RepairIssue {
            key: "reconciliation:ledger".to_string(),
            stage: repair::RepairStage::Reconciliation,
            message: format!("The repair ledger was unreadable and has been replaced: {error}"),
            paths: vec![repair::ledger_location(&path)],
        });
    }

    let directory_recovery_failures =
        crate::vault::relocation::recover_directory_relocations(Path::new(&path));
    let directory_recovery_blocked = !directory_recovery_failures.is_empty();
    for (index, failure) in directory_recovery_failures.into_iter().enumerate() {
        repair_status.record(repair::RepairIssue {
            key: format!("reconciliation:notebook-move:{index}"),
            stage: repair::RepairStage::Reconciliation,
            message: format!(
                "Could not recover an interrupted notebook move: {}",
                failure.message
            ),
            paths: failure.paths,
        });
    }

    // Put any note an external program misplaced back under its own category before the
    // search index is built, so the index never records a note at a path it is about to
    // move away from.
    if directory_recovery_blocked {
        log::warn!(
            "Skipped category reconciliation because an interrupted notebook move needs repair"
        );
    } else {
        match operations::reconcile_categories(&path) {
            Ok(report) => {
                if report.relocated > 0 {
                    log::info!("Moved {} notes back under their category", report.relocated);
                }
                if report.needs_attention() {
                    log::warn!(
                        "Vault reconciliation needs attention: {} unfiled notes, {} failures",
                        report.unfiled.len(),
                        report.failures.len()
                    );
                }
                for failure in report.failures {
                    repair_status.record(repair::RepairIssue {
                        key: format!("reconciliation:{}", failure.path),
                        stage: repair::RepairStage::Reconciliation,
                        message: failure.message,
                        paths: vec![failure.path],
                    });
                }
            }
            Err(error) => repair_status.record(repair::RepairIssue {
                key: "reconciliation:vault".to_string(),
                stage: repair::RepairStage::Reconciliation,
                message: format!("Could not reconcile note categories: {error}"),
                paths: vec![path.clone()],
            }),
        }
    }

    // Stage both derived search stores before replacing the active runtime. Semantic
    // inference is always Ollama-backed in v1, independently of the provider selected for
    // writing tools, so switching those tools never makes the local knowledge index vanish.
    let (ollama_url, ollama_token) = {
        let config = state.config.lock().map_err(|error| error.to_string())?;
        (
            config
                .ollama_base_url
                .clone()
                .unwrap_or_else(|| crate::ai_provider::DEFAULT_OLLAMA_URL.to_string()),
            config.ollama_api_key.clone(),
        )
    };
    let search = std::sync::Arc::new(SearchIndex::new(&path)?);
    let semantic = std::sync::Arc::new(crate::semantic_search::SemanticIndex::open(
        Path::new(&path),
        &ollama_url,
        ollama_token,
    )?);
    semantic.start_background();
    #[cfg(target_os = "ios")]
    {
        if external.is_some() {
            if let Err(error) = search.rebuild(&path) {
                repair_status.record(repair::RepairIssue {
                    key: "search:index".to_string(),
                    stage: repair::RepairStage::Search,
                    message: format!("Search rebuild while opening the vault failed: {error}"),
                    paths: vec![path.clone()],
                });
            } else {
                repair_status.clear_stage(repair::RepairStage::Search);
            }
        } else {
            let search_bg = search.clone();
            let vault = path.clone();
            let app_handle = app.clone();
            std::thread::spawn(move || match search_bg.rebuild(&vault) {
                Ok(()) => log::info!("mobile: search index rebuild complete"),
                Err(error) => {
                    let state = app_handle.state::<AppState>();
                    let _ = record_repair_issue(
                        &state,
                        &vault,
                        repair::RepairIssue {
                            key: "search:index".to_string(),
                            stage: repair::RepairStage::Search,
                            message: format!("Background search rebuild failed: {error}"),
                            paths: vec![vault.clone()],
                        },
                    );
                }
            });
        }
    }
    #[cfg(all(mobile, not(target_os = "ios")))]
    {
        let search_bg = search.clone();
        let vault = path.clone();
        let app_handle = app.clone();
        std::thread::spawn(move || match search_bg.rebuild(&vault) {
            Ok(()) => log::info!("mobile: search index rebuild complete"),
            Err(error) => {
                let state = app_handle.state::<AppState>();
                let _ = record_repair_issue(
                    &state,
                    &vault,
                    repair::RepairIssue {
                        key: "search:index".to_string(),
                        stage: repair::RepairStage::Search,
                        message: format!("Background search rebuild failed: {error}"),
                        paths: vec![vault.clone()],
                    },
                );
            }
        });
    }
    #[cfg(desktop)]
    {
        // Reconcile rather than rebuild: on an ordinary open almost nothing changed, and
        // reconcile touches only what did. It falls back to a full rebuild internally if
        // reconciliation itself fails, so this is never worse than what ran here before.
        if let Err(error) = search.reconcile(&path) {
            repair_status.record(repair::RepairIssue {
                key: "search:index".to_string(),
                stage: repair::RepairStage::Search,
                message: format!("Search index update while opening the vault failed: {error}"),
                paths: vec![path.clone()],
            });
        } else {
            repair_status.clear_stage(repair::RepairStage::Search);
        }
    }

    repair::save(&path, &repair_status)?;
    publish_repair_status(state, repair_status)?;

    let new_watcher = watcher::start_watcher(app.clone(), path.clone(), search.clone())?;
    asset_scope::allow_vault_assets(&app, Path::new(&path))?;
    let stable_vault_id = crate::machine_local::vault_id(Path::new(&path))?;

    // Update config. External vaults use the bookmark as their stable identity;
    // the resolved path is refreshed whenever the vault opens.
    let mut search_slot = state.search_index.lock().map_err(|e| e.to_string())?;
    let mut semantic_slot = state.semantic_index.lock().map_err(|e| e.to_string())?;
    let mut watcher_slot = state.watcher.lock().map_err(|e| e.to_string())?;
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    let mut next = config.clone();
    let active_bookmark_id = external
        .as_ref()
        .map(|(bookmark_id, _)| bookmark_id.clone());
    if let Some((bookmark_id, name)) = external {
        if let Some(vault) = next
            .vaults
            .iter_mut()
            .find(|vault| vault.bookmark_id.as_deref() == Some(bookmark_id.as_str()))
        {
            vault.path.clone_from(&path);
            vault.name = name;
            vault.vault_id = Some(stable_vault_id.clone());
        } else {
            next.vaults.push(VaultConfig {
                path: path.clone(),
                name,
                bookmark_id: Some(bookmark_id),
                vault_id: Some(stable_vault_id.clone()),
                ..Default::default()
            });
        }
    } else {
        let name = std::path::Path::new(&path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Some(vault) = next.vaults.iter_mut().find(|vault| {
            vault.bookmark_id.is_none()
                && (vault.path == path
                    || vault.vault_id.as_deref() == Some(stable_vault_id.as_str()))
        }) {
            vault.path.clone_from(&path);
            vault.name = name;
            vault.vault_id = Some(stable_vault_id.clone());
        } else {
            next.vaults.push(VaultConfig {
                path: path.clone(),
                name,
                vault_id: Some(stable_vault_id.clone()),
                ..Default::default()
            });
        }
    }
    next.active_vault = Some(path.clone());
    next.active_bookmark_id = active_bookmark_id;
    // Compatibility projection for existing callers; the in-vault file is authoritative.
    next.max_versions_per_note = shared_settings.max_versions_per_note;
    save_app_config(&next)?;
    *search_slot = Some(search);
    *semantic_slot = Some(semantic.clone());
    *watcher_slot = Some(new_watcher);
    *config = next;
    // The lock has to be gone before the hotkey is claimed: registration reads this same
    // config to find the trigger, and would deadlock against the guard still held here.
    drop(config);

    let semantic_vault = path.clone();
    std::thread::spawn(move || {
        if let Err(error) = semantic.reconcile_from_notes(Path::new(&semantic_vault)) {
            log::error!("Could not reconcile the semantic index: {error}");
        }
    });

    register_hotkey_now_a_vault_exists(&app);
    crate::sync_sidecar::activate_vault(app.clone(), PathBuf::from(&path));

    Ok(())
}

/// Claim the quick-capture hotkey now that there is somewhere to capture *to*.
///
/// Startup refuses to register when no vault is configured, which is right — a hotkey that
/// opens an overlay with nowhere to save is worse than none. But that check ran only at
/// startup, and nothing repeated it, so a first run went: pick a vault, and quick capture
/// stays dead until the app is restarted. The settings panel even kept saying "Not
/// registered yet — open a vault to enable quick capture" to someone who just had.
///
/// Both backends need this, for the same reason and with the same startup-only check
/// (`hotkey::startup::spawn` on Linux, `hotkey::windows::spawn` here). Re-running spawn
/// rather than calling `apply_trigger` directly, because everything else spawn does —
/// preparing the capture window, resyncing autostart, publishing the status — is equally
/// unfinished for a launch that had no vault.
#[allow(unused_variables)]
fn register_hotkey_now_a_vault_exists(app: &AppHandle) {
    // Only when there is genuinely nothing bound. Opening a second vault, or reopening the
    // same one, must not re-register a hotkey this app already holds: on Windows that asks
    // the OS for a combination we are ourselves already holding, which comes back as
    // `AlreadyRegistered` and would be reported to the user as "already used by another
    // application" — a conflict with nobody.
    let already_registered = app
        .state::<AppState>()
        .hotkey_status
        .lock()
        .map(|status| status.availability == hotkey::Availability::Available)
        .unwrap_or(false);
    if already_registered {
        return;
    }

    #[cfg(target_os = "linux")]
    hotkey::startup::spawn(app.clone());
    #[cfg(target_os = "windows")]
    hotkey::windows::spawn(app.clone());
}

#[tauri::command]
pub async fn open_vault(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let _transition = state.vault_transition.lock().await;
    open_vault_path(app.clone(), &state, path, None)?;
    #[cfg(target_os = "ios")]
    app.ios_vault_access().release_active()?;
    Ok(())
}

#[tauri::command]
pub async fn choose_external_vault(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<ExternalVaultResult>, String> {
    let _transition = state.vault_transition.lock().await;
    #[cfg(target_os = "ios")]
    {
        let picker_app = app.clone();
        let selection = tauri::async_runtime::spawn_blocking(move || {
            picker_app.ios_vault_access().choose_folder()
        })
        .await
        .map_err(|error| error.to_string())??;
        if selection.cancelled {
            return Ok(None);
        }
        let result = ExternalVaultResult::from(selection);
        let bookmark_was_registered = state
            .config
            .lock()
            .map_err(|error| error.to_string())?
            .vaults
            .iter()
            .any(|vault| vault.bookmark_id.as_deref() == Some(result.bookmark_id.as_str()));
        if let Err(error) = open_vault_path(
            app.clone(),
            &state,
            result.path.clone(),
            Some((result.bookmark_id.clone(), result.name.clone())),
        ) {
            if bookmark_was_registered {
                let _ = app.ios_vault_access().rollback_staged();
            } else {
                let _ = app.ios_vault_access().forget_bookmark(&result.bookmark_id);
            }
            return Err(error);
        }
        app.ios_vault_access().commit_staged()?;
        Ok(Some(result))
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (&app, &state);
        Err("Files folders are only available on iOS.".to_string())
    }
}

#[tauri::command]
pub async fn restore_external_vault(
    app: AppHandle,
    state: State<'_, AppState>,
    bookmark_id: String,
) -> Result<ExternalVaultResult, String> {
    let _transition = state.vault_transition.lock().await;
    #[cfg(target_os = "ios")]
    {
        let resolver_app = app.clone();
        let selection = tauri::async_runtime::spawn_blocking(move || {
            resolver_app.ios_vault_access().resolve_folder(&bookmark_id)
        })
        .await
        .map_err(|error| error.to_string())??;
        let result = ExternalVaultResult::from(selection);
        if let Err(error) = open_vault_path(
            app.clone(),
            &state,
            result.path.clone(),
            Some((result.bookmark_id.clone(), result.name.clone())),
        ) {
            let _ = app.ios_vault_access().rollback_staged();
            return Err(error);
        }
        app.ios_vault_access().commit_staged()?;
        Ok(result)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (&app, &state, &bookmark_id);
        Err("Files folders are only available on iOS.".to_string())
    }
}

#[tauri::command]
pub async fn remove_vault(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    bookmark_id: Option<String>,
) -> Result<(), String> {
    let _transition = state.vault_transition.lock().await;
    let mut config = state.config.lock().map_err(|error| error.to_string())?;
    let target = if let Some(bookmark_id) = bookmark_id.as_deref() {
        config
            .vaults
            .iter()
            .find(|vault| vault.bookmark_id.as_deref() == Some(bookmark_id))
    } else {
        config
            .vaults
            .iter()
            .find(|vault| vault.bookmark_id.is_none() && vault.path == path)
    }
    .map(|vault| {
        let is_active = if let Some(bookmark_id) = vault.bookmark_id.as_deref() {
            config.active_bookmark_id.as_deref() == Some(bookmark_id)
        } else {
            config.active_bookmark_id.is_none()
                && config.active_vault.as_deref() == Some(vault.path.as_str())
        };
        (
            vault.path.clone(),
            vault.bookmark_id.clone(),
            vault.vault_id.clone(),
            is_active,
        )
    });

    let Some((target_path, target_bookmark, target_vault_id, is_active)) = target else {
        return Ok(());
    };

    let old = config.clone();
    let mut next = old.clone();
    if let Some(id) = target_bookmark.as_deref() {
        next.vaults
            .retain(|vault| vault.bookmark_id.as_deref() != Some(id));
    } else {
        next.vaults
            .retain(|vault| vault.bookmark_id.is_some() || vault.path != target_path);
    }
    if is_active {
        next.active_vault = None;
        next.active_bookmark_id = None;
    }
    if let Some(vault_id) = target_vault_id.as_deref() {
        let config_path = app_config_path()?;
        crate::sync_config::reserve_webdav_retirement(&config_path, vault_id)?;
    }
    save_app_config(&next)?;

    #[cfg(target_os = "ios")]
    if let Some(id) = target_bookmark.as_deref() {
        if let Err(error) = app.ios_vault_access().forget_bookmark(id) {
            return match save_app_config(&old) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "{error} Configuration rollback also failed: {rollback_error}"
                )),
            };
        }
    }
    #[cfg(not(target_os = "ios"))]
    let _ = (&app, &target_bookmark);

    if is_active {
        clear_vault_runtime(&state)?;
    }
    *config = next;

    Ok(())
}

#[tauri::command]
pub fn get_app_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    Ok(config.clone())
}

#[tauri::command]
pub fn get_pending_open_file(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let mut pending = state.pending_open_file.lock().map_err(|e| e.to_string())?;
    Ok(pending.take())
}

#[tauri::command]
pub fn set_theme(state: State<'_, AppState>, theme: String) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.theme = theme;
    save_app_config(&config)?;
    Ok(())
}

#[tauri::command]
pub fn set_system_themes(
    state: State<'_, AppState>,
    light: String,
    dark: String,
) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.system_light_theme = light;
    config.system_dark_theme = dark;
    save_app_config(&config)?;
    Ok(())
}

#[tauri::command]
pub fn set_accent_color(state: State<'_, AppState>, color: String) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.accent_color = Some(color);
    save_app_config(&config)?;
    Ok(())
}

#[tauri::command]
pub fn save_custom_theme(
    state: State<'_, AppState>,
    theme: crate::types::CustomTheme,
) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    if let Some(pos) = config.custom_themes.iter().position(|t| t.id == theme.id) {
        config.custom_themes[pos] = theme;
    } else {
        config.custom_themes.push(theme);
    }
    save_app_config(&config)?;
    Ok(())
}

#[tauri::command]
pub fn delete_custom_theme(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    clear_custom_theme_references(&mut config, &id);
    save_app_config(&config)?;
    Ok(())
}

fn clear_custom_theme_references(config: &mut AppConfig, id: &str) {
    config.custom_themes.retain(|t| t.id != id);
    if config.theme == id {
        config.theme = "system".to_string();
    }
    if config.system_light_theme == id {
        config.system_light_theme = "light".to_string();
    }
    if config.system_dark_theme == id {
        config.system_dark_theme = "dark".to_string();
    }
}

#[cfg(test)]
mod custom_theme_reference_tests {
    use super::*;

    #[test]
    fn deleting_custom_theme_resets_system_pair_references() {
        let mut config = AppConfig {
            theme: "custom-work".to_string(),
            system_light_theme: "custom-work".to_string(),
            system_dark_theme: "custom-work".to_string(),
            ..Default::default()
        };

        clear_custom_theme_references(&mut config, "custom-work");

        assert_eq!(config.theme, "system");
        assert_eq!(config.system_light_theme, "light");
        assert_eq!(config.system_dark_theme, "dark");
    }
}

#[tauri::command]
pub fn export_custom_theme(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> Result<(), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let theme = config
        .custom_themes
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| "Theme not found".to_string())?;
    let export = serde_json::json!({ "version": 1, "themes": [theme] });
    let data = serde_json::to_string_pretty(&export).map_err(|e| e.to_string())?;
    ensure_scoped_path(&app, Path::new(&path))?;
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn import_custom_themes(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<crate::types::CustomTheme>, String> {
    ensure_scoped_path(&app, Path::new(&path))?;
    let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let parsed: serde_json::Value = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    let themes: Vec<crate::types::CustomTheme> =
        serde_json::from_value(parsed["themes"].clone())
            .map_err(|e| format!("Invalid theme file: {}", e))?;
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    for theme in &themes {
        if let Some(pos) = config.custom_themes.iter().position(|t| t.id == theme.id) {
            config.custom_themes[pos] = theme.clone();
        } else {
            config.custom_themes.push(theme.clone());
        }
    }
    save_app_config(&config)?;
    Ok(themes)
}

#[tauri::command]
pub fn set_font_size(app: AppHandle, state: State<'_, AppState>, size: u32) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.font_size = Some(size);
    save_app_config(&config)?;
    drop(config);

    use tauri::Emitter;
    app.emit("editor-font-size-changed", size)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_font_family(state: State<'_, AppState>, family: String) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.font_family = Some(family);
    save_app_config(&config)?;
    Ok(())
}

#[tauri::command]
pub fn set_line_height(state: State<'_, AppState>, height: f64) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.line_height = Some(height);
    save_app_config(&config)?;
    Ok(())
}

#[tauri::command]
pub fn set_ui_scale(app: AppHandle, state: State<'_, AppState>, scale: f64) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.ui_scale = Some(scale);
    save_app_config(&config)?;
    drop(config);

    use tauri::Emitter;
    app.emit("ui-scale-changed", scale)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_content_width(state: State<'_, AppState>, width: Option<u32>) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.content_width = width;
    save_app_config(&config)?;
    Ok(())
}

// ── Notebooks ──

#[tauri::command]
pub fn get_notebooks(state: State<'_, AppState>) -> Result<Vec<NotebookEntry>, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::scan_notebooks(vault_path)
}

#[tauri::command]
pub fn count_root_notes(state: State<'_, AppState>) -> Result<usize, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::count_root_notes(vault_path)
}

#[tauri::command]
pub fn create_notebook(
    state: State<'_, AppState>,
    parent_relative: Option<String>,
    name: String,
) -> Result<NotebookEntry, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::create_notebook(vault_path, parent_relative.as_deref(), &name)
}

#[tauri::command]
pub fn rename_notebook(
    state: State<'_, AppState>,
    path: String,
    new_name: String,
    active_note_path: Option<String>,
) -> Result<RelocationOutcome, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let config = state.config.lock().map_err(|error| error.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    let active_note = match active_note_path.as_ref() {
        Some(active) => {
            let suffix = active
                .strip_prefix(&path)
                .ok_or("Active note is not inside the notebook being renamed")?
                .to_string();
            Some((suffix, operations::read_note(vault_path, active)?))
        }
        None => None,
    };
    let renamed = operations::rename_notebook(vault_path, &path, &new_name)?;
    reconcile_semantic_now(&state, vault_path);
    let note = active_note.map(|(suffix, mut note)| {
        note.path = format!("{renamed}{suffix}");
        note
    });
    let authoritative_path = note
        .as_ref()
        .map(|value| value.path.clone())
        .unwrap_or(renamed);
    Ok(RelocationOutcome {
        path: authoritative_path,
        note,
        warnings: Vec::new(),
    })
}

#[tauri::command]
pub fn move_notebook(
    state: State<'_, AppState>,
    notebook_path: String,
    dest_parent: String,
    active_note_path: Option<String>,
) -> Result<RelocationOutcome, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let vault_path = {
        let config = state.config.lock().map_err(|e| e.to_string())?;
        config
            .active_vault
            .as_ref()
            .ok_or("No active vault")?
            .clone()
    };

    let moved = match operations::move_notebook_with_active(
        &vault_path,
        &notebook_path,
        &dest_parent,
        active_note_path.as_deref(),
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            record_transaction_repair_if_needed(
                &state,
                &vault_path,
                "move-notebook",
                vec![notebook_path.clone(), dest_parent],
                &error,
            );
            return Err(error);
        }
    };
    let new_full_path = moved.path;
    let note = moved.active_note;
    let mut warnings = Vec::new();
    record_projection_warning(
        &mut warnings,
        "Search projection",
        rebuild_search_now(
            &state,
            &vault_path,
            vec![notebook_path, new_full_path.clone()],
        ),
    );
    reconcile_semantic_now(&state, &vault_path);
    let authoritative_path = note
        .as_ref()
        .map(|value| value.path.clone())
        .unwrap_or(new_full_path);

    Ok(RelocationOutcome {
        path: authoritative_path,
        note,
        warnings,
    })
}

#[tauri::command]
pub fn delete_notebook(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let _notion_deletions = state.notion_deletions.try_lock().map_err(|_| {
        "Notion is processing deletions; try the notebook deletion again in a moment".to_string()
    })?;
    let vault_path = {
        let config = state.config.lock().map_err(|e| e.to_string())?;
        config
            .active_vault
            .as_ref()
            .ok_or("No active vault")?
            .clone()
    };
    let note_paths = operations::notebook_note_paths(&vault_path, &path)?;
    crate::notion::commands::with_notes_deleted(
        std::path::Path::new(&vault_path),
        &note_paths,
        || operations::delete_notebook(&vault_path, &path),
    )?;
    reconcile_semantic_now(&state, &vault_path);
    Ok(())
}

// ── Notes ──

#[tauri::command]
pub fn get_notes(
    state: State<'_, AppState>,
    notebook_path: Option<String>,
) -> Result<Vec<NoteEntry>, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::scan_notes(vault_path, notebook_path.as_deref())
}

#[tauri::command]
pub fn read_note(state: State<'_, AppState>, path: String) -> Result<NoteContent, String> {
    let config = state.config.lock().map_err(|error| error.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::read_note(vault_path, &path)
}

/// Read-only view of a Markdown file the user opened from outside the vault (file association,
/// CLI argument, or open dialog).
///
/// The fs scope alone is the authorization: the OS-open paths in `lib.rs` add the file to it
/// before the frontend ever learns the path, so an unscoped IPC caller cannot read anything
/// through this. Deliberately narrower than `ensure_readable_path`, which would also admit
/// every `.md` inside the active vault — including the history, holding-area, and trash files
/// that `read_note` and `read_unfiled_note` gate on purpose. In-vault paths are routed to
/// `read_note` by the frontend and never need this command.
#[tauri::command]
pub fn read_external_note<R: Runtime>(
    app: AppHandle<R>,
    path: String,
) -> Result<NoteContent, String> {
    ensure_scoped_path(&app, Path::new(&path))?;
    operations::read_external_note(&path)
}

/// Read-only preview for a regular Markdown note directly inside the Holding Area.
#[tauri::command]
pub fn read_unfiled_note(state: State<'_, AppState>, path: String) -> Result<NoteContent, String> {
    let config = state.config.lock().map_err(|error| error.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::read_unfiled_note(vault_path, &path)
}

#[tauri::command]
pub fn save_note(
    state: State<'_, AppState>,
    path: String,
    meta: NoteMeta,
    body: String,
    expected_revision: String,
) -> Result<SaveCommitOutcome, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let config = state.config.lock().map_err(|error| error.to_string())?;
    let vault_path = config
        .active_vault
        .as_ref()
        .ok_or("No active vault")?
        .clone();
    let max_versions = config.max_versions_per_note;
    drop(config);

    let outcome = operations::save_note_if_revision(
        &vault_path,
        &path,
        &meta,
        &body,
        &expected_revision,
        |written, revision| state.own_writes.record(written, revision),
    )?;
    let entry = operations::saved_note_entry(&vault_path, &outcome.note);
    let note_id = meta.id.clone();
    let snapshot_vault = vault_path.clone();
    let old_raw = outcome.old_raw;
    std::thread::spawn(move || {
        crate::history::maybe_snapshot(&snapshot_vault, &note_id, &old_raw, max_versions);
    });

    let index_result = index_note_now(&state, &vault_path, &path);
    queue_semantic_note_now(&state, &path);

    Ok(SaveCommitOutcome {
        entry: Some(entry),
        ..committed_save_outcome(outcome.revision, [("Search projection", index_result)])
    })
}

#[tauri::command]
pub fn duplicate_note(
    state: State<'_, AppState>,
    path: String,
) -> Result<crate::types::NoteEntry, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault = config
        .active_vault
        .as_ref()
        .ok_or("No active vault")?
        .clone();
    drop(config);

    let entry = operations::duplicate_note(&path, &vault)?;
    index_note_now(&state, &vault, &entry.path)?;
    queue_semantic_note_now(&state, &entry.path);
    Ok(entry)
}

#[tauri::command]
pub fn create_note(
    state: State<'_, AppState>,
    notebook_relative: Option<String>,
    title: String,
) -> Result<NoteEntry, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    let entry = operations::create_note(vault_path, notebook_relative.as_deref(), &title)?;

    index_note_now(&state, vault_path, &entry.path)?;
    queue_semantic_note_now(&state, &entry.path);

    Ok(entry)
}

/// File a quick capture as a note under the chosen PARA category.
///
/// Deliberately built on `create_note` and `save_note` rather than writing a file directly:
/// those already refuse an uncategorised note, snapshot history, and index synchronously, so
/// a captured note is findable the moment the overlay closes. A private path here would be a
/// second way to create a note, and the two would drift.
#[tauri::command]
pub fn quick_capture_note(
    state: State<'_, AppState>,
    category: String,
    text: String,
) -> Result<NoteEntry, String> {
    let capture = hotkey::capture::split(&text).map_err(|error| error.message().to_string())?;
    let category = crate::vault::para::ParaCategory::from_name(&category)
        .ok_or("Choose a category: Projects, Areas, Resources, or Archives")?;

    let entry = create_note(
        state.clone(),
        Some(category.folder_name().to_string()),
        capture.title,
    )?;

    // A title-only capture is already complete; writing an empty body would take a second
    // lock and a second index pass to change nothing.
    if capture.body.is_empty() {
        return Ok(entry);
    }

    let current_revision = operations::read_vault_note(
        &state
            .config
            .lock()
            .map_err(|error| error.to_string())?
            .active_vault
            .clone()
            .ok_or("No active vault")?,
        &entry.path,
    )?
    .revision;
    save_note(
        state,
        entry.path.clone(),
        entry.meta.clone(),
        capture.body.clone(),
        current_revision,
    )?;

    Ok(NoteEntry {
        preview: capture.body,
        ..entry
    })
}

/// Fetch and distil a web page away from Tauri's UI thread, then file it only after the
/// complete note is ready. This order prevents a failed network request from creating a draft.
#[tauri::command]
pub async fn clip_web_page(
    state: State<'_, AppState>,
    notebook_relative: String,
    source_url: String,
) -> Result<NoteEntry, String> {
    let clipping = tokio::task::spawn_blocking(move || crate::web_clipping::clip_url(&source_url))
        .await
        .map_err(|error| format!("Web clipping task failed: {error}"))?
        .map_err(|error| error.message().to_string())?;
    let (article, canonical_url) = clipping;

    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let config = state.config.lock().map_err(|error| error.to_string())?;
    let vault_path = config
        .active_vault
        .as_ref()
        .ok_or("No active vault")?
        .clone();
    drop(config);

    let entry = operations::create_web_clipping(
        &vault_path,
        &notebook_relative,
        &article.title,
        &article.markdown,
        &canonical_url,
    )?;
    if let Err(error) = index_note_now(&state, &vault_path, &entry.path) {
        let _ = std::fs::remove_file(&entry.path);
        return Err(error);
    }
    queue_semantic_note_now(&state, &entry.path);
    Ok(entry)
}

/// The AI backend's last known reachability.
///
/// Read from stored state rather than probing, so asking is instant and the UI can call
/// it freely while rendering.
#[tauri::command]
pub fn get_ai_status(state: State<'_, AppState>) -> Result<crate::ai_health::AiStatus, String> {
    state
        .ai_health
        .lock()
        .map(|health| health.status.clone())
        .map_err(|e| e.to_string())
}

/// Probe the AI backend now instead of waiting for the next scheduled check.
///
/// For the moment after the user changes the endpoint or wakes the other machine, when
/// waiting out the poller would feel broken.
#[tauri::command]
pub async fn refresh_ai_status(app: AppHandle) -> Result<crate::ai_health::AiStatus, String> {
    Ok(crate::ai_health::check_now(&app).await.0)
}

/// Whether the global capture hotkey is registered right now, and why not when it is not.
///
/// Read from stored state rather than re-run: the answer was decided once at startup by a
/// portal handshake that can prompt, so this can be polled freely without spending that again.
#[tauri::command]
pub fn get_hotkey_status(state: State<'_, AppState>) -> Result<hotkey::HotkeyStatus, String> {
    state
        .hotkey_status
        .lock()
        .map(|status| status.clone())
        .map_err(|error| error.to_string())
}

/// Open the desktop's own shortcut-configuration UI for the quick-capture hotkey.
///
/// The app cannot change the binding itself (ADR-0001), so this is the entirety of "change
/// the hotkey": hand the user to the compositor. A fresh, short-lived handshake gets the
/// session `ConfigureShortcuts` needs — the shortcut is already bound, so this only reaches
/// `ListShortcuts` and skips `BindShortcuts`, the call that would otherwise prompt.
#[tauri::command]
pub async fn open_hotkey_settings() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let app_id = hotkey::configured_application_id()?;
        let registration = hotkey::portal::register(&app_id)
            .await
            .map_err(|cause| cause.reason())?;
        let result = registration.configure().await.map_err(|error| match error {
            // Below GlobalShortcuts version 2 there is no ConfigureShortcuts at all. The UI
            // hides the button in that case, so reaching here means something offered it
            // anyway — say what to do rather than surfacing "requires version 2".
            ashpd::Error::RequiresVersion(_, _) => {
                "Your desktop's shortcuts portal cannot open its editor from an app. Change the \
                 shortcut in your desktop's own Keyboard settings instead."
                    .to_string()
            }
            other => other.to_string(),
        });
        // Tidy the temporary session rather than leaving it for the process lifetime; the
        // portal only cleans these up on bus disconnect otherwise. Best-effort: a failure to
        // close costs nothing the user can act on, so it is logged, not surfaced.
        if let Err(error) = registration.close().await {
            log::warn!("Could not close the temporary configuration session: {error}");
        }
        result
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("Hotkey settings are only available on Linux right now.".to_string())
    }
}

/// Notes that carry no category and so cannot be filed.
///
/// Non-empty means the user has something to resolve: until each one is given a
/// category, the app cannot say where it belongs.
#[tauri::command]
pub fn list_unfiled_notes(state: State<'_, AppState>) -> Result<Vec<NoteEntry>, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::list_unfiled_notes(vault_path)
}

/// Give an unfiled note a category, which moves it out of the holding area and into that
/// category's folder.
#[tauri::command]
pub fn file_unfiled_note(
    state: State<'_, AppState>,
    note_path: String,
    category: String,
) -> Result<String, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    let new_path = operations::file_unfiled_note(vault_path, &note_path, &category)?;

    index_note_now(&state, vault_path, &new_path)?;
    queue_moved_semantic_notes_now(&state, &note_path, &new_path, &[]);

    Ok(new_path)
}

#[tauri::command]
pub fn rename_note(
    state: State<'_, AppState>,
    path: String,
    new_title: String,
) -> Result<RelocationOutcome, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config
        .active_vault
        .as_ref()
        .ok_or("No active vault")?
        .clone();
    drop(config);
    let outcome = match operations::rename_note_with_outcome(&path, &new_title, &vault_path) {
        Ok(outcome) => outcome,
        Err(error) => {
            record_transaction_repair_if_needed(
                &state,
                &vault_path,
                "rename-note",
                vec![path],
                &error,
            );
            return Err(error);
        }
    };
    let mut warnings = Vec::new();
    record_projection_warning(
        &mut warnings,
        "Search projection",
        reindex_moved_note_now(
            &state,
            &vault_path,
            &path,
            &outcome.path,
            &outcome.rewritten_paths,
        ),
    );
    queue_moved_semantic_notes_now(&state, &path, &outcome.path, &outcome.rewritten_paths);
    let note = outcome.note;
    Ok(RelocationOutcome {
        path: outcome.path,
        note: Some(note),
        warnings,
    })
}

#[tauri::command]
pub fn delete_note(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let _notion_deletions = state.notion_deletions.try_lock().map_err(|_| {
        "Notion is processing deletions; try the note deletion again in a moment".to_string()
    })?;
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;

    // Before the file goes: the note id that identifies its Notion page lives inside it,
    // and once it is deleted there is nothing left to read. Deleting the map entry here
    // instead would leak the page — it would sit in Notion with no way left to find it.
    if let Err(error) = crate::notion::commands::with_notes_deleted(
        std::path::Path::new(vault_path),
        std::slice::from_ref(&path),
        || operations::delete_note(vault_path, &path),
    ) {
        record_transaction_repair_if_needed(
            &state,
            vault_path,
            "delete-note",
            vec![path.clone()],
            &error,
        );
        return Err(error);
    }

    remove_note_now(&state, vault_path, &path)?;
    remove_semantic_note_now(&state, &path);

    Ok(())
}

#[tauri::command]
pub fn move_note(
    state: State<'_, AppState>,
    note_path: String,
    dest_notebook: String,
) -> Result<RelocationOutcome, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;

    let outcome = operations::move_note_with_outcome(vault_path, &note_path, &dest_notebook)?;

    let mut warnings = Vec::new();
    record_projection_warning(
        &mut warnings,
        "Search projection",
        reindex_moved_note_now(
            &state,
            vault_path,
            &note_path,
            &outcome.path,
            &outcome.rewritten_paths,
        ),
    );
    queue_moved_semantic_notes_now(&state, &note_path, &outcome.path, &outcome.rewritten_paths);
    let note = outcome.note;

    Ok(RelocationOutcome {
        path: outcome.path,
        note: Some(note),
        warnings,
    })
}

// ── Tags ──

#[tauri::command]
pub fn get_all_tags(state: State<'_, AppState>) -> Result<Vec<(String, usize)>, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::get_all_tags(vault_path)
}

// ── Wiki-links ──

#[tauri::command]
pub fn get_all_note_titles(state: State<'_, AppState>) -> Result<Vec<NoteTitleEntry>, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    let vault = std::path::Path::new(vault_path);
    let hn_dir = operations::helixnotes_dir(vault_path);
    let mut entries = Vec::new();

    for entry in walkdir::WalkDir::new(vault)
        .into_iter()
        // Cross-platform exclusion (string "/.helixnotes/" checks miss Windows
        // backslash paths, which leaked .helixnotes/history snapshots in as dupes).
        .filter_entry(|e| !operations::is_hidden(e.path()) && !e.path().starts_with(&hn_dir))
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        // Read frontmatter title, fallback to filename
        let title = if let Ok(raw) = std::fs::read_to_string(path) {
            crate::vault::frontmatter::extract_title(&raw).unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
        } else {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        };

        // Store vault-relative path with forward slashes for cross-platform portability.
        // Absolute paths (e.g. C:\Users\...) break when the vault is synced to another device.
        let rel = path
            .strip_prefix(vault)
            .map(|r| r.to_string_lossy().replace('\\', "/").to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());

        entries.push(NoteTitleEntry { title, path: rel });
    }

    Ok(entries)
}

#[tauri::command]
pub async fn get_note_switcher_titles(
    state: State<'_, AppState>,
    recent_paths: Vec<String>,
) -> Result<Vec<NoteTitleEntry>, String> {
    let vault_path = state
        .config
        .lock()
        .map_err(|error| error.to_string())?
        .active_vault
        .clone()
        .ok_or_else(|| "No active vault".to_string())?;

    tauri::async_runtime::spawn_blocking(move || {
        operations::get_note_switcher_titles(&vault_path, &recent_paths)
    })
    .await
    .map_err(|error| error.to_string())?
}

// ── Graph ──

#[tauri::command]
pub fn get_graph_data(state: State<'_, AppState>) -> Result<crate::types::GraphData, String> {
    use std::collections::{HashMap, HashSet};

    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    let vault = std::path::Path::new(vault_path);

    // Pass 1: collect ALL notes as nodes (no title deduplication - every note must appear)
    let mut graph_nodes = Vec::new();
    // title → list of node indices (one title can map to multiple notes in different folders)
    let mut title_to_idxs: HashMap<String, Vec<usize>> = HashMap::new();
    // vault-relative path without extension → idx (for [[subfolder/note]] style links)
    let mut relpath_to_idx: HashMap<String, usize> = HashMap::new();
    // absolute path → idx (for fast active-note lookup)
    let mut path_to_idx: HashMap<String, usize> = HashMap::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut contents: Vec<String> = Vec::new();

    let hn_dir = operations::helixnotes_dir(vault_path);
    for entry in walkdir::WalkDir::new(vault)
        .into_iter()
        // Cross-platform exclusion (string "/.helixnotes/" checks miss Windows
        // backslash paths; is_hidden also covers .stversions/.stfolder/.trash/.git).
        .filter_entry(|e| !operations::is_hidden(e.path()) && !e.path().starts_with(&hn_dir))
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let path_str = path.to_string_lossy().to_string();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        // Deduplicate by canonical path (handles symlinks)
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let canonical_str = canonical.to_string_lossy().to_string();
        if !seen_paths.insert(canonical_str) {
            continue;
        }

        let raw = std::fs::read_to_string(path).unwrap_or_default();

        // Fast title extraction: scan for "title: " line in frontmatter without full YAML parse
        let title = extract_title_fast(&raw).unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });

        let idx = graph_nodes.len();
        let title_lower = title.to_lowercase();
        title_to_idxs.entry(title_lower).or_default().push(idx);
        path_to_idx.insert(path_str.clone(), idx);

        // Also index by vault-relative path without extension (e.g. "subfolder/note name")
        if let Ok(rel) = path.strip_prefix(vault) {
            let rel_no_ext = rel.with_extension("");
            // Normalize Windows backslashes so [[folder/note]] links resolve cross-platform.
            let rel_lower = rel_no_ext
                .to_string_lossy()
                .replace('\\', "/")
                .to_lowercase();
            relpath_to_idx.entry(rel_lower).or_insert(idx);
        }

        graph_nodes.push(crate::types::GraphNode {
            title,
            path: path_str,
        });
        contents.push(raw);
    }

    // Pass 2: extract edges from wiki-links (inline scan, no regex)
    // edge_map: (src, tgt) → index in edges vec, used to detect reverse links
    let mut edges: Vec<crate::types::GraphEdge> = Vec::new();
    let mut edge_map: HashMap<(usize, usize), usize> = HashMap::new();

    let add_edge = |edges: &mut Vec<crate::types::GraphEdge>,
                    edge_map: &mut HashMap<(usize, usize), usize>,
                    src: usize,
                    tgt: usize| {
        if src == tgt {
            return;
        }
        if edge_map.contains_key(&(src, tgt)) {
            return;
        } // exact duplicate
        if let Some(&rev_idx) = edge_map.get(&(tgt, src)) {
            // Reverse direction already exists - mark it as bidirectional
            edges[rev_idx].bidirectional = true;
        } else {
            let idx = edges.len();
            edge_map.insert((src, tgt), idx);
            edges.push(crate::types::GraphEdge {
                source: src,
                target: tgt,
                bidirectional: false,
            });
        }
    };

    for (source_idx, body) in contents.iter().enumerate() {
        let bytes = body.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i + 1 < len {
            if bytes[i] == b'[' && bytes[i + 1] == b'[' {
                i += 2;
                let start = i;
                while i + 1 < len && !(bytes[i] == b']' && bytes[i + 1] == b']') {
                    i += 1;
                }
                if i + 1 < len {
                    let link_raw = &body[start..i];
                    // Strip |alias, #heading, ^block
                    let link = link_raw.split('|').next().unwrap_or(link_raw);
                    let link = link.split('#').next().unwrap_or(link);
                    let link = link.split('^').next().unwrap_or(link);
                    let link = link.trim().to_lowercase();

                    // 1. Try vault-relative path match (e.g. "arkhost/note name")
                    if let Some(&target_idx) = relpath_to_idx.get(&link) {
                        add_edge(&mut edges, &mut edge_map, source_idx, target_idx);
                    } else if let Some(targets) = title_to_idxs.get(&link) {
                        // 2. Title match - connect to all notes with this title
                        for &target_idx in targets {
                            add_edge(&mut edges, &mut edge_map, source_idx, target_idx);
                        }
                    } else if link.contains('/') {
                        // 3. Path-based ref: try last segment as title fallback
                        if let Some(seg) = link.rsplit('/').next() {
                            if let Some(targets) = title_to_idxs.get(seg) {
                                for &target_idx in targets {
                                    add_edge(&mut edges, &mut edge_map, source_idx, target_idx);
                                }
                            }
                        }
                    }
                    i += 2;
                }
            } else {
                i += 1;
            }
        }
    }

    Ok(crate::types::GraphData {
        nodes: graph_nodes,
        edges,
    })
}

/// Fast title extraction from frontmatter without full YAML parsing.
/// Scans for `title: ...` line within `---` fences.
fn extract_title_fast(raw: &str) -> Option<String> {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    // Find the closing ---
    let after_open = &trimmed[3..];
    let end = after_open.find("\n---")?;
    let frontmatter = &after_open[..end];
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(title) = line.strip_prefix("title:") {
            let val = title.trim();
            // Strip surrounding quotes
            if (val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\''))
            {
                return Some(val[1..val.len() - 1].to_string());
            }
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

// ── Tasks ──

#[tauri::command]
pub fn get_tasks(state: State<'_, AppState>) -> Result<Vec<crate::types::TaskItem>, String> {
    use rayon::prelude::*;
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    let vault = std::path::Path::new(vault_path);
    let hn_dir = operations::helixnotes_dir(vault_path);

    let task_re = regex::Regex::new(r"^\s*[-*]\s\[([ xX])\]\s+(.+?)\s*$").unwrap();
    let due_re = regex::Regex::new(r"\bdue:(\d{4}-\d{2}-\d{2})\b").unwrap();
    let prio_re = regex::Regex::new(r"(?i)(?:^|\s)!(high|medium|med|low)\b").unwrap();

    let paths: Vec<std::path::PathBuf> = walkdir::WalkDir::new(vault)
        .into_iter()
        .filter_entry(|e| !operations::is_hidden(e.path()) && !e.path().starts_with(&hn_dir))
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file() && e.path().extension().and_then(|x| x.to_str()) == Some("md")
        })
        .map(|e| e.into_path())
        .collect();

    // Read and parse each note in parallel; on Android every read_to_string is a FUSE round-trip.
    let tasks: Vec<crate::types::TaskItem> = paths
        .par_iter()
        .flat_map(|path| {
            let mut out: Vec<crate::types::TaskItem> = Vec::new();
            let raw = match std::fs::read_to_string(path) {
                Ok(r) => r,
                Err(_) => return out,
            };
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let (meta, body) = crate::vault::frontmatter::parse_note(&raw, &filename);
            let note_path = path.to_string_lossy().to_string();
            for (i, line) in body.lines().enumerate() {
                let caps = match task_re.captures(line) {
                    Some(c) => c,
                    None => continue,
                };
                let completed = !caps[1].trim().is_empty();
                let content = caps[2].to_string();
                let due = due_re.captures(&content).map(|c| c[1].to_string());
                let priority = prio_re.captures(&content).map(|c| {
                    let p = c[1].to_lowercase();
                    if p == "medium" {
                        "med".to_string()
                    } else {
                        p
                    }
                });
                let mut text = due_re.replace_all(&content, "").to_string();
                text = prio_re.replace_all(&text, " ").to_string();
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                out.push(crate::types::TaskItem {
                    note_path: note_path.clone(),
                    note_title: meta.title.clone(),
                    line: i,
                    raw_line: line.to_string(),
                    text,
                    completed,
                    due,
                    priority,
                });
            }
            out
        })
        .collect();
    Ok(tasks)
}

fn read_task_note(
    state: &State<'_, AppState>,
    note_path: &str,
) -> Result<(String, NoteMeta, String), String> {
    let config = state.config.lock().map_err(|error| error.to_string())?;
    let vault_path = config
        .active_vault
        .as_ref()
        .ok_or("No active vault")?
        .clone();
    drop(config);
    let note = operations::read_vault_note(&vault_path, note_path)?;
    Ok((vault_path, note.meta, note.content))
}

fn toggle_checkbox_line(line: &str, done: bool) -> String {
    let mut s = line.to_string();
    if done {
        if let Some(pos) = s.find("[ ]") {
            s.replace_range(pos..pos + 3, "[x]");
        }
    } else {
        for marker in ["[x]", "[X]"] {
            if let Some(pos) = s.find(marker) {
                s.replace_range(pos..pos + 3, "[ ]");
                break;
            }
        }
    }
    s
}

#[tauri::command]
pub fn set_task_done(
    state: State<'_, AppState>,
    note_path: String,
    line: usize,
    raw_line: String,
    done: bool,
) -> Result<NoteContent, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let (vault_path, meta, body) = read_task_note(&state, &note_path)?;
    let mut lines: Vec<String> = body.lines().map(|l| l.to_string()).collect();
    // Verify the expected line; if the note drifted, fall back to the first exact match.
    let idx = if lines.get(line).map(|l| *l == raw_line).unwrap_or(false) {
        line
    } else {
        lines
            .iter()
            .position(|l| *l == raw_line)
            .ok_or("Task line not found (note changed)")?
    };

    let toggled = toggle_checkbox_line(&lines[idx], done);
    if toggled == lines[idx] {
        return operations::read_note(&vault_path, &note_path); // already in the desired state
    }
    lines[idx] = toggled;
    let mut new_body = lines.join("\n");
    if body.ends_with('\n') && !new_body.ends_with('\n') {
        new_body.push('\n');
    }
    let save_outcome = operations::save_note(&vault_path, &note_path, &meta, &new_body)?;

    if let Err(error) = index_note_now(&state, &vault_path, &note_path) {
        log::error!("Task Markdown committed but search projection needs repair: {error}");
    }
    queue_semantic_note_now(&state, &note_path);
    Ok(save_outcome.note)
}

fn set_priority_on_line(line: &str, priority: Option<&str>) -> String {
    let prio_re = regex::Regex::new(r"(?i)(?:^|\s)!(?:high|medium|med|low)\b").unwrap();
    let stripped = prio_re.replace_all(line, "").to_string();
    let stripped = stripped.trim_end();
    match priority {
        Some(p) => format!("{} !{}", stripped, p),
        None => stripped.to_string(),
    }
}

#[tauri::command]
pub fn set_task_priority(
    state: State<'_, AppState>,
    note_path: String,
    line: usize,
    raw_line: String,
    priority: Option<String>,
) -> Result<NoteContent, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    // Normalize/validate priority ("medium" -> "med"); None clears it.
    let prio = match priority.as_deref() {
        None | Some("") | Some("none") => None,
        Some("high") => Some("high"),
        Some("med") | Some("medium") => Some("med"),
        Some("low") => Some("low"),
        Some(_) => return Err("Invalid priority".to_string()),
    };

    let (vault_path, meta, body) = read_task_note(&state, &note_path)?;
    let mut lines: Vec<String> = body.lines().map(|l| l.to_string()).collect();
    let idx = if lines.get(line).map(|l| *l == raw_line).unwrap_or(false) {
        line
    } else {
        lines
            .iter()
            .position(|l| *l == raw_line)
            .ok_or("Task line not found (note changed)")?
    };

    let updated = set_priority_on_line(&lines[idx], prio);
    if updated == lines[idx] {
        return operations::read_note(&vault_path, &note_path);
    }
    lines[idx] = updated;
    let mut new_body = lines.join("\n");
    if body.ends_with('\n') && !new_body.ends_with('\n') {
        new_body.push('\n');
    }
    let save_outcome = operations::save_note(&vault_path, &note_path, &meta, &new_body)?;

    if let Err(error) = index_note_now(&state, &vault_path, &note_path) {
        log::error!("Task Markdown committed but search projection needs repair: {error}");
    }
    queue_semantic_note_now(&state, &note_path);
    Ok(save_outcome.note)
}

fn set_due_on_line(line: &str, due: Option<&str>) -> String {
    let due_re = regex::Regex::new(r"(?:^|\s)due:\d{4}-\d{2}-\d{2}\b").unwrap();
    let stripped = due_re.replace_all(line, "").to_string();
    let stripped = stripped.trim_end();
    match due {
        Some(d) => format!("{} due:{}", stripped, d),
        None => stripped.to_string(),
    }
}

/// Interpret the due date a task edit requests: `None` or empty clears it.
///
/// A task due date is written into the Markdown line, so it has to be a date a calendar
/// can actually produce. Shape alone is not enough: `2026-02-31` and `2026-13-01` match
/// `YYYY-MM-DD` but name no day, and once stored they sort and compare as ordinary dates
/// that no calendar view can ever land on.
fn parse_task_due_date(due: Option<&str>) -> Result<Option<&str>, String> {
    let value = match due {
        None | Some("") => return Ok(None),
        Some(value) => value,
    };
    // Shape first, so the stored text is always exactly `YYYY-MM-DD`: chrono itself accepts
    // unpadded input like `2026-9-4`, which would then sort as a string against padded dates.
    let date_re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
    if !date_re.is_match(value) {
        return Err(format!("“{value}” is not a date. Use YYYY-MM-DD."));
    }
    // Then the calendar, which is what rejects a well-shaped day that does not exist.
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| format!("“{value}” is not a real calendar date."))?;
    Ok(Some(value))
}

#[cfg(test)]
mod task_due_date_tests {
    use super::parse_task_due_date;

    #[test]
    fn a_real_calendar_date_is_accepted() {
        assert_eq!(
            parse_task_due_date(Some("2026-09-14")),
            Ok(Some("2026-09-14"))
        );
        assert_eq!(
            parse_task_due_date(Some("2026-12-31")),
            Ok(Some("2026-12-31"))
        );
        assert_eq!(
            parse_task_due_date(Some("2026-01-01")),
            Ok(Some("2026-01-01"))
        );
    }

    #[test]
    fn a_leap_day_is_accepted_only_in_a_leap_year() {
        assert_eq!(
            parse_task_due_date(Some("2028-02-29")),
            Ok(Some("2028-02-29"))
        );
        assert!(parse_task_due_date(Some("2026-02-29")).is_err());
        // 1900 is divisible by 4 but is not a leap year: only real calendar rules reject it.
        assert!(parse_task_due_date(Some("1900-02-29")).is_err());
    }

    #[test]
    fn an_impossible_date_is_rejected_with_a_message_naming_the_value() {
        for impossible in [
            "2026-02-31", // February never has 31 days
            "2026-13-01", // there is no thirteenth month
            "2026-00-10", // there is no zeroth month
            "2026-04-31", // April has 30 days
            "2026-06-00", // there is no zeroth day
            "2026-12-32",
        ] {
            let error = parse_task_due_date(Some(impossible))
                .expect_err(&format!("{impossible} is not a real calendar date"));
            assert!(
                error.contains(impossible),
                "the message must name the rejected value so the user can see what was wrong, got: {error}"
            );
        }
    }

    #[test]
    fn a_malformed_value_is_rejected_and_never_reaches_the_markdown() {
        for malformed in [
            "2026-9-14",
            "14-09-2026",
            "tomorrow",
            "2026-09",
            "2026-09-14T00:00",
            " ",
        ] {
            assert!(
                parse_task_due_date(Some(malformed)).is_err(),
                "{malformed} must not be written into a task line"
            );
        }
    }

    #[test]
    fn an_absent_or_empty_value_clears_the_due_date() {
        assert_eq!(parse_task_due_date(None), Ok(None));
        assert_eq!(parse_task_due_date(Some("")), Ok(None));
    }
}

#[tauri::command]
pub fn set_task_due(
    state: State<'_, AppState>,
    note_path: String,
    line: usize,
    raw_line: String,
    due: Option<String>,
) -> Result<NoteContent, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let due_val: Option<&str> = parse_task_due_date(due.as_deref())?;

    let (vault_path, meta, body) = read_task_note(&state, &note_path)?;
    let mut lines: Vec<String> = body.lines().map(|l| l.to_string()).collect();
    let idx = if lines.get(line).map(|l| *l == raw_line).unwrap_or(false) {
        line
    } else {
        lines
            .iter()
            .position(|l| *l == raw_line)
            .ok_or("Task line not found (note changed)")?
    };

    let updated = set_due_on_line(&lines[idx], due_val);
    if updated == lines[idx] {
        return operations::read_note(&vault_path, &note_path);
    }
    lines[idx] = updated;
    let mut new_body = lines.join("\n");
    if body.ends_with('\n') && !new_body.ends_with('\n') {
        new_body.push('\n');
    }
    let save_outcome = operations::save_note(&vault_path, &note_path, &meta, &new_body)?;

    if let Err(error) = index_note_now(&state, &vault_path, &note_path) {
        log::error!("Task Markdown committed but search projection needs repair: {error}");
    }
    queue_semantic_note_now(&state, &note_path);
    Ok(save_outcome.note)
}

// ── Search ──

#[tauri::command]
pub fn search_notes(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<SearchResult>, String> {
    let search_guard = state.search_index.lock().map_err(|e| e.to_string())?;
    let search = search_guard
        .as_ref()
        .ok_or("Search index not initialized")?;
    search.search(&query, limit.unwrap_or(20))
}

#[tauri::command]
pub async fn semantic_search(
    state: State<'_, AppState>,
    query: String,
    category: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let category = category
        .map(|value| {
            crate::vault::para::ParaCategory::from_name(&value)
                .ok_or_else(|| format!("Unknown PARA category: {value}"))
        })
        .transpose()?;
    let semantic = state
        .semantic_index
        .lock()
        .map_err(|error| error.to_string())?
        .clone()
        .ok_or("Semantic search is not initialized")?;
    tokio::task::spawn_blocking(move || semantic.search(&query, category, limit.unwrap_or(20)))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn get_semantic_status(
    state: State<'_, AppState>,
) -> Result<crate::semantic_search::SemanticStatus, String> {
    state
        .semantic_index
        .lock()
        .map_err(|error| error.to_string())?
        .as_ref()
        .ok_or("Semantic search is not initialized")?
        .status()
}

#[tauri::command]
pub async fn rebuild_semantic_index(state: State<'_, AppState>) -> Result<(), String> {
    let vault = {
        let config = state.config.lock().map_err(|error| error.to_string())?;
        config.active_vault.clone().ok_or("No active vault")?
    };
    let semantic = state
        .semantic_index
        .lock()
        .map_err(|error| error.to_string())?
        .clone()
        .ok_or("Semantic search is not initialized")?;
    tokio::task::spawn_blocking(move || semantic.rebuild_from_notes(Path::new(&vault)))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn reindex(state: State<'_, AppState>) -> Result<(), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    let search_guard = state.search_index.lock().map_err(|e| e.to_string())?;
    let search = search_guard
        .as_ref()
        .ok_or("Search index not initialized")?;
    search.rebuild(vault_path)
}

#[tauri::command]
pub fn get_repair_status(state: State<'_, AppState>) -> Result<repair::RepairStatus, String> {
    state
        .repair_status
        .lock()
        .map(|status| status.clone())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn retry_repairs(state: State<'_, AppState>) -> Result<repair::RepairStatus, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let vault_path = state
        .config
        .lock()
        .map_err(|error| error.to_string())?
        .active_vault
        .clone()
        .ok_or("No active vault")?;
    let mut status = repair::load(&vault_path)?;

    if status.has_stage(repair::RepairStage::Search) {
        let search = state
            .search_index
            .lock()
            .map_err(|error| error.to_string())?
            .clone()
            .ok_or("Search index not initialized")?;
        if search.rebuild(&vault_path).is_ok() {
            status.clear_stage(repair::RepairStage::Search);
        }
    }

    status.clear_stage(repair::RepairStage::Reconciliation);
    let mut reconciliation_moved_notes = false;
    match operations::reconcile_categories(&vault_path) {
        Ok(report) => {
            reconciliation_moved_notes = report.relocated > 0 || report.moved_to_holding > 0;
            for failure in report.failures {
                status.record(repair::RepairIssue {
                    key: format!("reconciliation:{}", failure.path),
                    stage: repair::RepairStage::Reconciliation,
                    message: failure.message,
                    paths: vec![failure.path],
                });
            }
        }
        Err(error) => status.record(repair::RepairIssue {
            key: "reconciliation:vault".to_string(),
            stage: repair::RepairStage::Reconciliation,
            message: error,
            paths: vec![vault_path.clone()],
        }),
    }

    if reconciliation_moved_notes {
        let search = state
            .search_index
            .lock()
            .map_err(|error| error.to_string())?
            .clone()
            .ok_or("Search index not initialized")?;
        if let Err(error) = search.rebuild(&vault_path) {
            status.record(repair::RepairIssue {
                key: "search:index".to_string(),
                stage: repair::RepairStage::Search,
                message: format!("Search rebuild after reconciliation failed: {error}"),
                paths: vec![vault_path.clone()],
            });
        } else {
            status.clear_stage(repair::RepairStage::Search);
        }
    }

    repair::save(&vault_path, &status)?;
    publish_repair_status(&state, status.clone())?;
    Ok(status)
}

// ── Trash ──

#[tauri::command]
pub fn get_trash(state: State<'_, AppState>) -> Result<TrashContents, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::get_trash_contents(vault_path)
}

#[tauri::command]
pub fn restore_note(state: State<'_, AppState>, trash_path: String) -> Result<String, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let _notion_deletions = state.notion_deletions.try_lock().map_err(|_| {
        "Notion is processing deletions; try restoring the note again in a moment".to_string()
    })?;
    let vault_path = {
        let config = state.config.lock().map_err(|e| e.to_string())?;
        config
            .active_vault
            .as_ref()
            .ok_or("No active vault")?
            .clone()
    };
    let restored = match crate::notion::commands::with_notes_restored(
        std::path::Path::new(&vault_path),
        std::slice::from_ref(&trash_path),
        || operations::restore_note(&vault_path, &trash_path),
    ) {
        Ok(restored) => restored,
        Err(error) => {
            record_transaction_repair_if_needed(
                &state,
                &vault_path,
                "restore-note",
                vec![trash_path],
                &error,
            );
            return Err(error);
        }
    };
    index_note_now(&state, &vault_path, &restored)?;
    queue_semantic_note_now(&state, &restored);
    Ok(restored)
}

#[tauri::command]
pub fn restore_notebook(state: State<'_, AppState>, trash_path: String) -> Result<String, String> {
    let _mutation = state
        .note_mutation
        .lock()
        .map_err(|error| error.to_string())?;
    let _notion_deletions = state.notion_deletions.try_lock().map_err(|_| {
        "Notion is processing deletions; try restoring the notebook again in a moment".to_string()
    })?;
    let vault_path = {
        let config = state.config.lock().map_err(|e| e.to_string())?;
        config
            .active_vault
            .as_ref()
            .ok_or("No active vault")?
            .clone()
    };
    let note_paths = operations::trash_notebook_note_paths(&vault_path, &trash_path)?;
    let restored = crate::notion::commands::with_notes_restored(
        std::path::Path::new(&vault_path),
        &note_paths,
        || operations::restore_notebook(&vault_path, &trash_path),
    )?;
    reconcile_semantic_now(&state, &vault_path);
    Ok(restored)
}

#[tauri::command]
pub fn permanent_delete(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let vault_path = {
        let config = state.config.lock().map_err(|e| e.to_string())?;
        config
            .active_vault
            .as_ref()
            .ok_or("No active vault")?
            .clone()
    };
    operations::permanent_delete(&vault_path, &path)
}

#[tauri::command]
pub fn empty_trash(state: State<'_, AppState>) -> Result<(), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::empty_trash(vault_path)
}

// ── Vault State ──

#[tauri::command]
pub fn load_vault_state(state: State<'_, AppState>) -> Result<VaultState, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::load_vault_state(vault_path)
}

#[tauri::command]
pub fn save_vault_state(state: State<'_, AppState>, vault_state: VaultState) -> Result<(), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::save_vault_state(vault_path, &vault_state)
}

// ── Clipboard ──

/// Copy text to the system clipboard through the native backend.
#[cfg(desktop)]
#[tauri::command]
pub fn copy_text_to_clipboard(text: String) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Clipboard init failed: {}", e))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("Failed to copy text: {}", e))
}

#[cfg(mobile)]
#[tauri::command]
pub fn copy_text_to_clipboard(_text: String) -> Result<(), String> {
    Err("Text clipboard copy is only supported on desktop".to_string())
}

/// Read image from system clipboard (bypasses WebKitGTK clipboard bug).
/// Returns PNG bytes as Vec<u8>, or error if no image on clipboard.
#[cfg(desktop)]
#[tauri::command]
pub fn read_clipboard_image() -> Result<Vec<u8>, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Clipboard init failed: {}", e))?;
    let img = clipboard
        .get_image()
        .map_err(|_| "No image on clipboard".to_string())?;
    // Encode RGBA data to PNG
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(
            std::io::Cursor::new(&mut buf),
            img.width as u32,
            img.height as u32,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("PNG header failed: {}", e))?;
        writer
            .write_image_data(&img.bytes)
            .map_err(|e| format!("PNG encode failed: {}", e))?;
    }
    Ok(buf)
}

#[cfg(mobile)]
#[tauri::command]
pub fn read_clipboard_image() -> Result<Vec<u8>, String> {
    Err("Clipboard image reading not supported on Android".to_string())
}

/// Copy an image file to the system clipboard.
#[cfg(desktop)]
#[tauri::command]
pub fn copy_image_to_clipboard(app: AppHandle, path: String) -> Result<(), String> {
    ensure_readable_path(&app, Path::new(&path))?;
    let data = std::fs::read(&path).map_err(|e| format!("Failed to read image: {}", e))?;
    let img =
        image::load_from_memory(&data).map_err(|e| format!("Failed to decode image: {}", e))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let img_data = arboard::ImageData {
        width: w as usize,
        height: h as usize,
        bytes: std::borrow::Cow::Owned(rgba.into_raw()),
    };
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Clipboard init failed: {}", e))?;
    clipboard
        .set_image(img_data)
        .map_err(|e| format!("Failed to set clipboard image: {}", e))?;
    Ok(())
}

#[cfg(mobile)]
#[tauri::command]
pub fn copy_image_to_clipboard(_app: AppHandle, _path: String) -> Result<(), String> {
    Err("Clipboard image copy not supported on Android".to_string())
}

/// Copy PNG bytes directly to the system clipboard.
#[cfg(desktop)]
#[tauri::command]
pub fn copy_png_to_clipboard(data: Vec<u8>) -> Result<(), String> {
    let img =
        image::load_from_memory(&data).map_err(|e| format!("Failed to decode image: {}", e))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let img_data = arboard::ImageData {
        width: w as usize,
        height: h as usize,
        bytes: std::borrow::Cow::Owned(rgba.into_raw()),
    };
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Clipboard init failed: {}", e))?;
    clipboard
        .set_image(img_data)
        .map_err(|e| format!("Failed to set clipboard image: {}", e))?;
    Ok(())
}

#[cfg(mobile)]
#[tauri::command]
pub fn copy_png_to_clipboard(_data: Vec<u8>) -> Result<(), String> {
    Err("Clipboard image copy not supported on Android".to_string())
}

// ── Attachments ──

#[tauri::command]
pub fn save_image(
    state: State<'_, AppState>,
    name: String,
    data: Vec<u8>,
) -> Result<String, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::save_image(vault_path, &name, &data)
}

#[tauri::command]
pub fn save_attachment(
    state: State<'_, AppState>,
    name: String,
    data: Vec<u8>,
) -> Result<String, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::save_attachment(vault_path, &name, &data)
}

// ── Notebook Icons ──

#[tauri::command]
pub fn get_notebook_icons(
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::load_notebook_icons(vault_path)
}

#[tauri::command]
pub fn set_notebook_icon(
    state: State<'_, AppState>,
    notebook_relative: String,
    icon_relative: Option<String>,
) -> Result<(), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::set_notebook_icon(vault_path, &notebook_relative, icon_relative.as_deref())
}

// ── General Settings ──

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn set_general_settings(
    state: State<'_, AppState>,
    compact_notes: bool,
    time_format: String,
    week_start: String,
    gpu_acceleration: bool,
    autostart: bool,
    pdf_preview: bool,
    pdf_height: u32,
    title_mode: String,
    hide_title_in_body: bool,
    show_line_numbers: bool,
    show_link_arrows: bool,
    default_view_mode: bool,
    new_notes_in_source_mode: bool,
    show_tray_icon: bool,
    close_to_tray: bool,
    enable_wiki_links: bool,
    show_note_dates: bool,
    show_note_switcher: bool,
    startup_view: StartupView,
    restore_last_session: bool,
    show_all_notes: bool,
    show_quick_access: bool,
    show_tasks: bool,
    show_trash: bool,
) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.compact_notes = compact_notes;
    config.show_note_dates = show_note_dates;
    config.show_note_switcher = show_note_switcher;
    config.startup_view = startup_view;
    config.restore_last_session = restore_last_session;
    config.show_all_notes = show_all_notes;
    config.show_quick_access = show_quick_access;
    config.show_tasks = show_tasks;
    config.show_trash = show_trash;
    config.time_format = time_format;
    config.week_start = week_start;
    config.gpu_acceleration = gpu_acceleration;
    config.autostart = autostart;
    config.pdf_preview = pdf_preview;
    config.pdf_height = pdf_height;
    config.title_mode = title_mode;
    config.hide_title_in_body = hide_title_in_body;
    config.show_line_numbers = show_line_numbers;
    config.show_link_arrows = show_link_arrows;
    config.default_view_mode = default_view_mode;
    config.new_notes_in_source_mode = new_notes_in_source_mode;
    config.show_tray_icon = show_tray_icon;
    config.close_to_tray = close_to_tray;
    config.enable_wiki_links = enable_wiki_links;
    save_app_config(&config)?;

    // The stored preference is the source of truth; the OS-level entry is a projection of
    // it, kept in sync every time it could have changed rather than left to drift.
    #[cfg(target_os = "linux")]
    if let (Some(config_home), Some(exec), Ok(app_id)) = (
        dirs::config_dir(),
        hotkey::desktop_entry::current_executable(),
        hotkey::configured_application_id(),
    ) {
        if let Err(error) = crate::autostart::sync(config.autostart, &config_home, &app_id, &exec) {
            log::warn!("Could not update the autostart entry: {error}");
        }
    }
    #[cfg(target_os = "windows")]
    if let (Ok(exec), Ok(app_id)) = (std::env::current_exe(), hotkey::configured_application_id()) {
        if let Err(error) = crate::autostart::sync(config.autostart, &app_id, &exec) {
            log::warn!("Could not update the autostart entry: {error}");
        }
    }

    Ok(())
}

// ── Quick Access ──

#[tauri::command]
pub async fn get_quick_access(state: State<'_, AppState>) -> Result<Vec<NoteEntry>, String> {
    let vault_path = state
        .config
        .lock()
        .map_err(|error| error.to_string())?
        .active_vault
        .clone()
        .ok_or_else(|| "No active vault".to_string())?;

    tauri::async_runtime::spawn_blocking(move || operations::get_quick_access_notes(&vault_path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn add_quick_access(state: State<'_, AppState>, note_relative: String) -> Result<(), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::add_quick_access(vault_path, &note_relative)
}

#[tauri::command]
pub fn remove_quick_access(
    state: State<'_, AppState>,
    note_relative: String,
) -> Result<(), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::remove_quick_access(vault_path, &note_relative)
}

#[tauri::command]
pub fn reorder_quick_access(state: State<'_, AppState>, paths: Vec<String>) -> Result<(), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    operations::save_quick_access(vault_path, &paths)
}

fn is_counted_vault_file(vault: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(vault).unwrap_or(path);
    let components: Vec<_> = relative.components().map(|part| part.as_os_str()).collect();

    if components
        .iter()
        .any(|part| *part == std::ffi::OsStr::new(".trash"))
    {
        return false;
    }

    match components
        .iter()
        .position(|part| *part == std::ffi::OsStr::new(".helixnotes"))
    {
        Some(0) => components.get(1) == Some(&std::ffi::OsStr::new("attachments")),
        Some(_) => false,
        None => true,
    }
}

#[cfg(test)]
mod vault_stats_path_tests {
    use super::is_counted_vault_file;
    use std::path::Path;

    #[test]
    fn stats_count_notes_and_attachments_but_not_private_or_trash_files() {
        let vault = Path::new("vault");
        assert!(is_counted_vault_file(
            vault,
            &vault.join("Projects/note.md")
        ));
        assert!(is_counted_vault_file(
            vault,
            &vault.join(".helixnotes/attachments/image.png")
        ));
        assert!(!is_counted_vault_file(
            vault,
            &vault.join(".helixnotes/state.json")
        ));
        assert!(!is_counted_vault_file(vault, &vault.join(".trash/note.md")));
        assert!(!is_counted_vault_file(
            vault,
            &vault.join("Projects/.trash/note.md")
        ));
    }
}

#[tauri::command]
pub fn get_vault_stats(state: State<'_, AppState>) -> Result<VaultStats, String> {
    use rayon::prelude::*;
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;

    let paths: Vec<std::path::PathBuf> = walkdir::WalkDir::new(vault_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        // Attachments under .helixnotes count; no other private metadata or Trash does.
        .filter(|path| is_counted_vault_file(Path::new(vault_path), path))
        .collect();

    // Stat files in parallel; each metadata() is a FUSE round-trip on Android.
    let (total_notes, total_attachments, notes_size, attachments_size) = paths
        .par_iter()
        .map(|path| {
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                (1u64, 0u64, size, 0u64)
            } else {
                (0u64, 1u64, 0u64, size)
            }
        })
        .reduce(
            || (0u64, 0u64, 0u64, 0u64),
            |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2, a.3 + b.3),
        );

    Ok(VaultStats {
        total_notes,
        total_attachments,
        notes_size,
        attachments_size,
        total_size: notes_size + attachments_size,
    })
}

#[tauri::command]
pub fn import_obsidian(app: AppHandle) -> Result<(), String> {
    let vault_path = {
        let state = app.state::<AppState>();
        let config = state.config.lock().map_err(|e| e.to_string())?;
        config
            .active_vault
            .clone()
            .ok_or("No active vault".to_string())?
    };
    // Fire-and-forget: return immediately, do work in background thread
    std::thread::spawn(move || {
        use tauri::Emitter;
        let (result, terminal) = do_import_obsidian(&app, &vault_path);
        let _ = app.emit(
            "import-done",
            import_done_payload(result.as_ref(), &terminal),
        );
    });
    Ok(())
}

/// Run the import and classify how it ended.
///
/// Reconciliation runs even when the conversion reported failures, because by then the vault on
/// disk has already changed and the projections would otherwise describe the vault as it was.
fn do_import_obsidian(
    app: &AppHandle,
    vault_path: &str,
) -> (Option<ImportResult>, BulkMutationTerminal) {
    let state = app.state::<AppState>();
    let _lease = match state
        .bulk_mutation
        .acquire(&state.note_mutation, &state.vault_activity)
    {
        Ok(lease) => lease,
        // Refused before the vault was touched: nothing changed.
        Err(error) => return (None, BulkMutationTerminal::failure(error)),
    };
    let result = match crate::vault::import::import(vault_path) {
        Ok(result) => result,
        // `import` only fails outright before it writes anything; everything that can fail
        // after the first write is recorded in `failed` instead.
        Err(error) => return (None, BulkMutationTerminal::failure(error)),
    };

    let reconciled = reconcile_bulk_projections(&state, vault_path);
    let terminal = classify_import(&result, reconciled);
    (Some(result), terminal)
}

/// How an import that got as far as running ended:
///
/// | conversion | reconciliation | vault changed | outcome            |
/// |------------|----------------|---------------|--------------------|
/// | clean      | ok             | either        | success            |
/// | failures   | ok             | no            | failure            |
/// | failures   | ok             | yes           | changed-incomplete |
/// | either     | failed         | no            | failure            |
/// | either     | failed         | yes           | changed-incomplete |
///
/// A reconciliation failure after the vault changed is changed-incomplete rather than a
/// failure: the notes were converted, but the views of them no longer match.
fn classify_import(result: &ImportResult, reconciled: Result<(), String>) -> BulkMutationTerminal {
    match reconciled {
        Err(error) if result.mutated() => BulkMutationTerminal::changed_incomplete(error),
        Err(error) => BulkMutationTerminal::failure(error),
        Ok(()) if result.failed.is_empty() => BulkMutationTerminal::success(),
        Ok(()) if result.mutated() => {
            BulkMutationTerminal::changed_incomplete(describe_import_failures(&result.failed))
        }
        Ok(()) => BulkMutationTerminal::failure(describe_import_failures(&result.failed)),
    }
}

/// Name what failed without pasting an unbounded list of paths into a notification.
fn describe_import_failures(failed: &[String]) -> String {
    const SHOWN: usize = 3;
    let shown = failed
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("; ");
    match failed.len().saturating_sub(SHOWN) {
        0 => format!("{} file(s) could not be converted: {shown}", failed.len()),
        remaining => format!(
            "{} file(s) could not be converted: {shown}; and {remaining} more",
            failed.len()
        ),
    }
}

/// Carry the shared terminal contract (`success`, `outcome`, `error`) and the counts the
/// settings panel shows in one payload, so a partial import still reports what it converted.
fn import_done_payload(
    result: Option<&ImportResult>,
    terminal: &BulkMutationTerminal,
) -> serde_json::Value {
    /// Flattened so every `ImportResult` field reaches the panel without being listed by hand.
    #[derive(serde::Serialize)]
    struct Payload<'a> {
        #[serde(flatten)]
        terminal: &'a BulkMutationTerminal,
        #[serde(flatten)]
        result: Option<&'a ImportResult>,
    }
    serde_json::json!(Payload { terminal, result })
}

#[cfg(test)]
mod import_outcome_tests {
    use super::{classify_import, import_done_payload};
    use crate::bulk_mutation::BulkMutationTerminal;
    use crate::types::ImportResult;

    fn result(files_converted: u64, failed: &[&str]) -> ImportResult {
        ImportResult {
            files_converted,
            links_converted: 2,
            frontmatter_normalized: 1,
            syntax_converted: 0,
            attachments_moved: 0,
            failed: failed.iter().map(|entry| entry.to_string()).collect(),
        }
    }

    fn outcome(result: &ImportResult, reconciled: Result<(), String>) -> String {
        serde_json::to_value(classify_import(result, reconciled)).unwrap()["outcome"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn an_import_that_changed_nothing_and_failed_is_a_failure_not_a_partial_change() {
        assert_eq!(outcome(&result(0, &["a.md: denied"]), Ok(())), "failure");
        assert_eq!(outcome(&result(0, &[]), Err("index".into())), "failure");
    }

    #[test]
    fn an_import_that_changed_the_vault_and_then_failed_is_changed_incomplete() {
        assert_eq!(
            outcome(&result(3, &["a.md: denied"]), Ok(())),
            "changed-incomplete"
        );
        // Converted cleanly, but the projections could not be rebuilt over the new notes.
        assert_eq!(
            outcome(&result(3, &[]), Err("index rebuild failed".into())),
            "changed-incomplete"
        );
    }

    #[test]
    fn a_clean_import_is_a_success() {
        assert_eq!(outcome(&result(3, &[]), Ok(())), "success");
        assert_eq!(outcome(&result(0, &[]), Ok(())), "success");
    }

    /// The panel needs the terminal contract and the counts in one payload: a partial import
    /// must still be able to report what it converted.
    #[test]
    fn a_partial_import_reports_its_counts_alongside_the_reason_it_stopped() {
        let result = result(3, &["a.md: denied"]);
        let payload = import_done_payload(
            Some(&result),
            &BulkMutationTerminal::changed_incomplete("a.md: denied"),
        );

        assert_eq!(payload["success"], false);
        assert_eq!(payload["outcome"], "changed-incomplete");
        assert_eq!(payload["error"], "a.md: denied");
        assert_eq!(payload["files_converted"], 3);
        assert_eq!(payload["links_converted"], 2);
        assert_eq!(payload["frontmatter_normalized"], 1);
        assert_eq!(payload["failed"][0], "a.md: denied");
    }

    /// A refusal or a pre-write failure has no result to report; the payload must still be a
    /// well-formed terminal rather than a payload with missing counters.
    #[test]
    fn a_refused_import_still_emits_a_well_formed_terminal() {
        let payload = import_done_payload(None, &BulkMutationTerminal::failure("already running"));

        assert_eq!(payload["success"], false);
        assert_eq!(payload["outcome"], "failure");
        assert_eq!(payload["error"], "already running");
        assert!(payload.get("files_converted").is_none());
    }
}

pub(crate) fn reconcile_bulk_projections(state: &AppState, vault_path: &str) -> Result<(), String> {
    let legacy_max_versions = state
        .config
        .lock()
        .map_err(|error| error.to_string())?
        .max_versions_per_note;
    let shared = crate::vault_settings::load_or_create(Path::new(vault_path), legacy_max_versions)?;
    {
        let mut config = state.config.lock().map_err(|error| error.to_string())?;
        if config.max_versions_per_note != shared.max_versions_per_note {
            config.max_versions_per_note = shared.max_versions_per_note;
            save_app_config(&config)?;
        }
    }
    if let Some(search) = state
        .search_index
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
    {
        search.reconcile(vault_path)?;
    }
    if let Some(semantic) = state
        .semantic_index
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
    {
        semantic.reconcile_from_notes(Path::new(vault_path))?;
    }
    Ok(())
}

// ── Orphaned attachment cleanup ──

#[derive(serde::Serialize)]
pub struct OrphanAttachment {
    pub name: String,
    pub size: u64,
}

// Conservatively find files in .helixnotes/attachments not referenced by ANY note. Scans every
// .md in the vault (including .helixnotes/trash, so a restorable trashed note keeps its files)
// plus notebook_icons.json, and matches each filename against both the raw text and a
// percent-decoded copy (so a URL-encoded path like `my%20file.png` still counts as a reference).
// When in doubt a file is KEPT: a leftover orphan is harmless, a wrong deletion is not.
fn scan_orphaned_attachments(vault: &str) -> Result<Vec<(String, u64)>, String> {
    let attachments_dir = operations::helixnotes_dir(vault).join("attachments");
    if !attachments_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<(String, u64)> = Vec::new();
    for entry in std::fs::read_dir(&attachments_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        if p.is_file() {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            files.push((name, size));
        }
    }
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut haystack = String::new();
    for entry in walkdir::WalkDir::new(vault)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("md") {
            if let Ok(content) = std::fs::read_to_string(p) {
                haystack.push_str(&content);
                haystack.push('\n');
            }
        }
    }
    // Folder icons live in attachments but are referenced here, not in notes. (#157)
    let icons_path = operations::helixnotes_dir(vault).join("notebook_icons.json");
    if let Ok(content) = std::fs::read_to_string(&icons_path) {
        haystack.push_str(&content);
        haystack.push('\n');
    }
    let decoded = percent_decode(&haystack);
    let orphans = files
        .into_iter()
        .filter(|(name, _)| !haystack.contains(name.as_str()) && !decoded.contains(name.as_str()))
        .collect();
    Ok(orphans)
}

#[tauri::command]
pub fn find_orphaned_attachments(
    state: State<'_, AppState>,
) -> Result<Vec<OrphanAttachment>, String> {
    let vault = {
        let config = state.config.lock().map_err(|e| e.to_string())?;
        config.active_vault.clone().ok_or("No active vault")?
    };
    let orphans = scan_orphaned_attachments(&vault)?;
    Ok(orphans
        .into_iter()
        .map(|(name, size)| OrphanAttachment { name, size })
        .collect())
}

#[tauri::command]
pub fn trash_orphaned_attachments(
    state: State<'_, AppState>,
    names: Vec<String>,
) -> Result<u32, String> {
    let vault = {
        let config = state.config.lock().map_err(|e| e.to_string())?;
        config.active_vault.clone().ok_or("No active vault")?
    };
    // Re-scan now and only move files that are STILL orphaned (guards against a reference added
    // between scan and confirm), intersected with the caller's selection.
    let current: std::collections::HashSet<String> = scan_orphaned_attachments(&vault)?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let attachments_dir = operations::helixnotes_dir(&vault).join("attachments");
    let trash_dir = operations::helixnotes_dir(&vault).join("trash");
    std::fs::create_dir_all(&trash_dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f").to_string();
    let mut moved = 0u32;
    for (i, name) in names.iter().enumerate() {
        if !current.contains(name) {
            continue;
        }
        // Path-traversal guard: only act on a bare filename inside the attachments dir.
        if std::path::Path::new(name)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .as_deref()
            != Some(name.as_str())
        {
            continue;
        }
        let src = attachments_dir.join(name);
        if !src.is_file() {
            continue;
        }
        let dest = trash_dir.join(format!("{}_{}_attachment_{}", stamp, i, name));
        if std::fs::rename(&src, &dest).is_ok() {
            moved += 1;
        }
    }
    Ok(moved)
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(h), Some(l)) = (hi, lo) {
                if let Ok(byte) = u8::from_str_radix(&format!("{}{}", h as char, l as char), 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
        } else {
            result.push(b as char);
        }
    }
    result
}

// ── Open files and URLs with the system default handler ──

fn active_vault_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let state = app.state::<AppState>();
    let config = state.config.lock().map_err(|error| error.to_string())?;
    config
        .active_vault
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| "No active vault".to_string())
}

fn path_is_in_active_vault<R: Runtime>(app: &AppHandle<R>, path: &Path) -> bool {
    let Ok(vault) = active_vault_path(app)
        .and_then(|path| std::fs::canonicalize(path).map_err(|error| error.to_string()))
    else {
        return false;
    };
    let canonical = if path.exists() {
        std::fs::canonicalize(path).ok()
    } else {
        path.parent()
            .and_then(|parent| std::fs::canonicalize(parent).ok())
    };
    canonical.is_some_and(|path| path.starts_with(vault))
}

fn ensure_scoped_path<R: Runtime>(app: &AppHandle<R>, path: &Path) -> Result<(), String> {
    if app.fs_scope().is_allowed(path) {
        Ok(())
    } else {
        Err("Path was not selected by the user".to_string())
    }
}

fn ensure_readable_path<R: Runtime>(app: &AppHandle<R>, path: &Path) -> Result<(), String> {
    if path.is_file() && (path_is_in_active_vault(app, path) || app.fs_scope().is_allowed(path)) {
        Ok(())
    } else {
        Err("File must be inside the active vault or selected by the user".to_string())
    }
}

fn ensure_writable_path<R: Runtime>(app: &AppHandle<R>, path: &Path) -> Result<(), String> {
    if path_is_in_active_vault(app, path) || app.fs_scope().is_allowed(path) {
        Ok(())
    } else {
        Err("Destination must be inside the active vault or selected by the user".to_string())
    }
}

fn validate_external_url(url: &str) -> Result<(), String> {
    if url.chars().any(char::is_whitespace) {
        return Err("URL must not contain whitespace".to_string());
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| "Invalid URL".to_string())?;
    match parsed.scheme() {
        "http" | "https" | "mailto" | "tel" | "sms" => Ok(()),
        _ => Err("Unsupported URL scheme".to_string()),
    }
}

#[cfg(target_os = "linux")]
fn open_linux(argument: &std::ffi::OsStr) -> Result<(), String> {
    let mut command = std::process::Command::new("xdg-open");
    command.arg(argument);
    if std::env::var("APPIMAGE").is_ok() {
        command
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("LD_PRELOAD")
            .env_remove("GIO_LAUNCHED_DESKTOP_FILE")
            .env_remove("GIO_LAUNCHED_DESKTOP_FILE_PID");
        if let Ok(original_path) = std::env::var("PATH_ORIG") {
            command.env("PATH", original_path);
        }
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Failed to open item: {error}"))
}

fn open_path_with_system(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    return open_linux(path.as_os_str());

    #[cfg(not(target_os = "linux"))]
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|error| error.to_string())
}

fn open_url_with_system(url: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    return open_linux(std::ffi::OsStr::new(url));

    #[cfg(not(target_os = "linux"))]
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|error| error.to_string())
}

#[cfg(test)]
mod external_access_tests {
    use super::{ensure_readable_path, ensure_writable_path, validate_external_url};
    use crate::state::AppState;
    use crate::types::AppConfig;
    use std::fs;
    use tauri_plugin_fs::FsExt;

    #[test]
    fn external_urls_allow_supported_schemes() {
        for url in [
            "https://helixnotes.com",
            "http://example.com",
            "mailto:hello@example.com",
            "tel:+123456789",
            "sms:+123456789",
        ] {
            assert!(validate_external_url(url).is_ok(), "{url}");
        }
    }

    #[test]
    fn external_urls_reject_commands_and_unsupported_schemes() {
        for url in [
            "https://example.com & calc.exe",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "not a URL",
        ] {
            assert!(validate_external_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn file_access_requires_the_active_vault_or_an_explicit_user_scope() {
        let root = std::env::temp_dir().join(format!(
            "helixnotes-external-access-{}",
            uuid::Uuid::new_v4()
        ));
        let vault = root.join("vault");
        let vault_file = vault.join("note.md");
        let outside_file = root.join("outside.txt");
        let outside_destination = root.join("export.txt");
        fs::create_dir_all(&vault).unwrap();
        fs::write(&vault_file, b"note").unwrap();
        fs::write(&outside_file, b"outside").unwrap();

        let config = AppConfig {
            active_vault: Some(vault.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_fs::init())
            .manage(AppState::new(config))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();

        assert!(ensure_readable_path(app.handle(), &vault_file).is_ok());
        assert!(ensure_readable_path(app.handle(), &outside_file).is_err());
        assert!(ensure_writable_path(app.handle(), &outside_destination).is_err());

        app.fs_scope().allow_file(&outside_file).unwrap();
        app.fs_scope().allow_file(&outside_destination).unwrap();
        assert!(ensure_readable_path(app.handle(), &outside_file).is_ok());
        assert!(ensure_writable_path(app.handle(), &outside_destination).is_ok());

        fs::remove_dir_all(root).unwrap();
    }

    /// The external viewer must read an OS-authorized file from outside the vault without
    /// becoming a way to read anything else. This drives the command itself, not its parts:
    /// deleting the scope check from `read_external_note` has to fail here.
    #[test]
    fn the_external_viewer_reads_only_authorized_markdown_and_never_relaxes_mutation() {
        let root = std::env::temp_dir().join(format!(
            "helixnotes-external-viewer-{}",
            uuid::Uuid::new_v4()
        ));
        let vault = root.join("vault");
        let vault_note = vault.join("note.md");
        let outside_note = root.join("outside.md");
        let outside_text = root.join("outside.txt");
        fs::create_dir_all(&vault).unwrap();
        fs::write(&vault_note, b"inside").unwrap();
        fs::write(&outside_note, b"outside").unwrap();
        fs::write(&outside_text, b"not markdown").unwrap();

        let vault_path = vault.to_string_lossy().into_owned();
        let config = AppConfig {
            active_vault: Some(vault_path.clone()),
            ..Default::default()
        };
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_fs::init())
            .manage(AppState::new(config))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let view = |path: &std::path::Path| {
            super::read_external_note(app.handle().clone(), path.to_string_lossy().into_owned())
        };

        // Unauthorized: outside the vault and never selected by the user.
        assert!(view(&outside_note).is_err());
        // Unauthorized even inside the vault: this command is not a way around `read_note`'s
        // validator, which gates history, holding-area, and trash files.
        assert!(view(&vault_note).is_err());

        app.fs_scope().allow_file(&outside_note).unwrap();
        app.fs_scope().allow_file(&outside_text).unwrap();

        // Authorized: readable, and reported under the path that was asked for.
        let viewed = view(&outside_note).expect("an authorized external Markdown file is readable");
        assert_eq!(viewed.path, outside_note.to_str().unwrap());
        assert!(viewed.raw.contains("outside"));

        // Scoped but not Markdown: refused, because the viewer cannot render it.
        assert!(view(&outside_text).is_err());

        // The mutation guards are untouched: an authorized external file is still not a note
        // this vault will read through the vault validator, save, or delete.
        let outside_arg = outside_note.to_str().unwrap();
        let meta = crate::vault::frontmatter::parse_note("outside", "outside.md").0;
        assert!(crate::vault::operations::read_note(&vault_path, outside_arg).is_err());
        assert!(
            crate::vault::operations::save_note(&vault_path, outside_arg, &meta, "hacked").is_err()
        );
        assert!(crate::vault::operations::delete_note(&vault_path, outside_arg).is_err());
        assert_eq!(fs::read_to_string(&outside_note).unwrap(), "outside");

        fs::remove_dir_all(root).unwrap();
    }
}

#[tauri::command]
pub fn open_file(app: AppHandle, path: String) -> Result<(), String> {
    ensure_readable_path(&app, Path::new(&path))?;
    open_path_with_system(Path::new(&path))
}

#[tauri::command]
pub fn reveal_file(app: AppHandle, path: String) -> Result<(), String> {
    ensure_readable_path(&app, Path::new(&path))?;
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    validate_external_url(&url)?;
    open_url_with_system(&url)
}

#[tauri::command]
pub fn copy_file_to(app: AppHandle, source: String, destination: String) -> Result<(), String> {
    ensure_readable_path(&app, Path::new(&source))?;
    ensure_writable_path(&app, Path::new(&destination))?;
    std::fs::copy(&source, &destination).map_err(|e| format!("Failed to copy file: {}", e))?;
    Ok(())
}

#[tauri::command]
pub fn write_bytes_to(app: AppHandle, destination: String, data: Vec<u8>) -> Result<(), String> {
    ensure_writable_path(&app, Path::new(&destination))?;
    std::fs::write(&destination, &data).map_err(|e| format!("Failed to write file: {}", e))?;
    Ok(())
}

// ── Backup ──

#[tauri::command]
pub fn create_backup(app: AppHandle) -> Result<(), String> {
    let (vault_path, backup_dir, include_attachments, max_count) = {
        let state = app.state::<AppState>();
        let config = state.config.lock().map_err(|e| e.to_string())?;
        let vault_path = config.active_vault.clone().ok_or("No active vault")?;
        let backup_dir = crate::backup::get_backup_dir(&config.backup_location)?;
        (
            vault_path,
            backup_dir,
            config.backup_include_attachments,
            config.backup_max_count,
        )
    };

    std::thread::spawn(move || {
        use tauri::Emitter;
        match crate::backup::create_backup(&vault_path, &backup_dir, include_attachments) {
            Ok(entry) => {
                // Update last backup time
                let state = app.state::<AppState>();
                if let Ok(mut config) = state.config.lock() {
                    config.last_backup_time = Some(entry.created.clone());
                    let _ = save_app_config(&config);
                }
                // Cleanup old backups
                let _ = crate::backup::cleanup_old_backups(&backup_dir, max_count);
                let _ = app.emit(
                    "backup-done",
                    serde_json::json!({
                        "success": true,
                        "entry": entry,
                    }),
                );
            }
            Err(e) => {
                let _ = app.emit(
                    "backup-done",
                    serde_json::json!({
                        "success": false,
                        "error": e,
                    }),
                );
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub fn list_backups(state: State<'_, AppState>) -> Result<Vec<BackupEntry>, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let backup_dir = crate::backup::get_backup_dir(&config.backup_location)?;
    crate::backup::list_backups(&backup_dir)
}

#[tauri::command]
pub fn restore_backup(app: AppHandle, backup_path: String) -> Result<(), String> {
    let (vault_path, backup_dir) = {
        let state = app.state::<AppState>();
        let config = state.config.lock().map_err(|e| e.to_string())?;
        (
            config.active_vault.clone().ok_or("No active vault")?,
            crate::backup::get_backup_dir(&config.backup_location)?,
        )
    };

    std::thread::spawn(move || {
        use tauri::Emitter;
        let state = app.state::<AppState>();
        let terminal = match state
            .bulk_mutation
            .acquire(&state.note_mutation, &state.vault_activity)
        {
            Err(error) => BulkMutationTerminal::failure(error),
            Ok(_lease) => {
                match crate::backup::restore_backup(&vault_path, &backup_dir, &backup_path) {
                    Ok(()) => match reconcile_bulk_projections(&state, &vault_path) {
                        Ok(()) => crate::bulk_mutation::BulkMutationTerminal::success(),
                        Err(error) => BulkMutationTerminal::changed_incomplete(format!(
                            "Vault restored, but derived views could not be reconciled: {error}"
                        )),
                    },
                    Err(error) if error.changed => {
                        let reconciliation = reconcile_bulk_projections(&state, &vault_path).err();
                        let message = reconciliation.map_or(error.message.clone(), |reconcile| {
                            format!(
                                "{} Derived views also could not be reconciled: {reconcile}",
                                error.message
                            )
                        });
                        crate::bulk_mutation::BulkMutationTerminal::changed_incomplete(message)
                    }
                    Err(error) => BulkMutationTerminal::failure(error.message),
                }
            }
        };
        let _ = app.emit(
            "restore-done",
            serde_json::json!({
                "success": terminal.success,
                "outcome": terminal.outcome,
                "error": terminal.error,
            }),
        );
    });
    Ok(())
}

#[tauri::command]
pub fn delete_backup(state: State<'_, AppState>, backup_path: String) -> Result<(), String> {
    let config = state.config.lock().map_err(|error| error.to_string())?;
    let backup_dir = crate::backup::get_backup_dir(&config.backup_location)?;
    crate::backup::delete_backup(&backup_dir, &backup_path)
}

#[tauri::command]
pub fn set_backup_settings(
    state: State<'_, AppState>,
    enabled: bool,
    frequency: String,
    max_count: u32,
    location: Option<String>,
    include_attachments: bool,
) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.backup_enabled = enabled;
    config.backup_frequency = frequency;
    config.backup_max_count = max_count;
    config.backup_location = location;
    config.backup_include_attachments = include_attachments;
    save_app_config(&config)?;
    Ok(())
}

// ── Version History ──

#[tauri::command]
pub fn get_note_versions(
    state: State<'_, AppState>,
    note_id: String,
) -> Result<Vec<crate::types::VersionEntry>, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    crate::history::list_versions(vault_path, &note_id)
}

#[tauri::command]
pub fn create_version(
    state: State<'_, AppState>,
    path: String,
    note_id: String,
) -> Result<(), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    let max_versions = config.max_versions_per_note;
    let raw = operations::read_vault_note(vault_path, &path)?.raw;
    crate::history::force_snapshot(vault_path, &note_id, &raw, max_versions)
}

#[tauri::command]
pub fn get_note_version_content(
    state: State<'_, AppState>,
    note_id: String,
    timestamp: String,
) -> Result<String, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let vault_path = config.active_vault.as_ref().ok_or("No active vault")?;
    crate::history::get_version(vault_path, &note_id, &timestamp)
}

// ── AI ──

fn restart_semantic_index(app: &AppHandle) -> Result<(), String> {
    let (vault, base_url, token) = {
        let state = app.state::<AppState>();
        let config = state.config.lock().map_err(|error| error.to_string())?;
        let Some(vault) = config.active_vault.clone() else {
            return Ok(());
        };
        (
            vault,
            config
                .ollama_base_url
                .clone()
                .unwrap_or_else(|| crate::ai_provider::DEFAULT_OLLAMA_URL.to_string()),
            config.ollama_api_key.clone(),
        )
    };
    let semantic = Arc::new(crate::semantic_search::SemanticIndex::open(
        Path::new(&vault),
        &base_url,
        token,
    )?);
    semantic.start_background();
    *app.state::<AppState>()
        .semantic_index
        .lock()
        .map_err(|error| error.to_string())? = Some(semantic.clone());
    std::thread::spawn(move || {
        if let Err(error) = semantic.reconcile_from_notes(Path::new(&vault)) {
            log::error!("Could not reconcile semantic search after its settings changed: {error}");
        }
    });
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn set_ai_settings(
    app: AppHandle,
    provider: Option<AiProvider>,
    api_key: Option<String>,
    model: String,
    writing_style: Option<String>,
    base_url: Option<String>,
    ollama_api_key: Option<String>,
    openai_compatible_base_url: Option<String>,
    openai_compatible_api_key: Option<String>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    let generation = state
        .ai_health
        .lock()
        .map_err(|e| e.to_string())?
        .generation;
    let previous_target = crate::ai_health::health_target(&config, generation);
    let mut candidate = config.clone();
    let key = api_key.filter(|k| !k.trim().is_empty());
    match provider.as_ref() {
        Some(AiProvider::OpenAi) => candidate.openai_api_key = key,
        Some(AiProvider::Ollama) => {
            candidate.ollama_base_url = base_url.filter(|u| !u.trim().is_empty());
            candidate.ollama_api_key = ollama_api_key.filter(|k| !k.trim().is_empty());
        }
        Some(AiProvider::OpenAiCompatible) => {
            candidate.openai_compatible_base_url =
                openai_compatible_base_url.filter(|u| !u.trim().is_empty());
            candidate.openai_compatible_api_key =
                openai_compatible_api_key.filter(|k| !k.trim().is_empty());
        }
        Some(AiProvider::Anthropic | AiProvider::Unknown(_)) | None => candidate.ai_api_key = key,
    }
    candidate.ai_provider = provider;
    candidate.ai_model = model;
    candidate.ai_writing_style = writing_style.filter(|s| !s.trim().is_empty());
    let next_target = crate::ai_health::health_target(&candidate, generation);
    let semantic_settings_changed = candidate.ollama_base_url != config.ollama_base_url
        || candidate.ollama_api_key != config.ollama_api_key;
    let health_settings_changed =
        !crate::ai_health::same_probe_settings(previous_target.as_ref(), next_target.as_ref());
    // Persist both halves first: a failed keyring or config-file write must not change the
    // live provider or health identity for the rest of this process.
    commit_secret_config(&mut config, candidate)?;
    let invalidated_status = if health_settings_changed {
        let mut health = state.ai_health.lock().map_err(|e| e.to_string())?;
        health.generation += 1;
        let target = crate::ai_health::health_target(&config, health.generation);
        let status = target
            .as_ref()
            .map(crate::ai_health::AiStatus::unknown_for)
            .unwrap_or_else(crate::ai_health::AiStatus::unknown);
        health.status = status.clone();
        Some(status)
    } else {
        None
    };
    drop(config);

    if let Some(status) = invalidated_status {
        let _ = app.emit("ai-status-changed", status);
        let refresh_app = app.clone();
        tauri::async_runtime::spawn(async move {
            crate::ai_health::check_now(&refresh_app).await;
        });
    }
    if semantic_settings_changed {
        restart_semantic_index(&app)?;
    }
    Ok(())
}

#[tauri::command]
pub fn test_ai_connection(app: AppHandle) -> Result<(), String> {
    let (settings, health_target) = {
        let state = app.state::<AppState>();
        let config = state.config.lock().map_err(|e| e.to_string())?;
        let configured = crate::ai_provider::ConfiguredAiProvider::from_config(&config);
        let settings = configured
            .request_settings()
            .map_err(|error| error.to_string())?;
        let generation = state
            .ai_health
            .lock()
            .map_err(|error| error.to_string())?
            .generation;
        (settings, configured.health_target(generation))
    };

    std::thread::spawn(move || {
        use tauri::Emitter;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(crate::ai::test_connection(
            &settings,
            health_target.as_ref(),
        ));
        match result {
            Ok(msg) => {
                let _ = app.emit(
                    "ai-test-result",
                    serde_json::json!({ "success": true, "message": msg }),
                );
            }
            Err(e) => {
                let _ = app.emit(
                    "ai-test-result",
                    serde_json::json!({ "success": false, "error": e }),
                );
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub fn ai_ask(
    app: AppHandle,
    action: String,
    text: String,
    custom_prompt: Option<String>,
    request_id: String,
) -> Result<(), String> {
    let (settings, writing_style, health_target) = {
        let state = app.state::<AppState>();
        let config = state.config.lock().map_err(|e| e.to_string())?;
        let configured = crate::ai_provider::ConfiguredAiProvider::from_config(&config);
        let settings = configured
            .request_settings()
            .map_err(|error| error.to_string())?;
        let style = config.ai_writing_style.clone();
        let generation = state
            .ai_health
            .lock()
            .map_err(|e| e.to_string())?
            .generation;
        let health_target = configured
            .health_target(generation)
            .map(|target| target.id().clone());
        (settings, style, health_target)
    };

    // Refuse immediately when the backend is known to be down, with the reason the poller
    // already worked out. Starting the request instead would leave the user waiting on a
    // machine that is asleep.
    let status = app
        .state::<AppState>()
        .ai_health
        .lock()
        .map(|health| health.status.clone())
        .map_err(|e| e.to_string())?;
    if let Some(reason) = crate::ai_health::known_unavailability(&status, health_target.as_ref()) {
        return Err(reason);
    }

    let mut system_prompt = "You are a helpful writing assistant inside a note-taking app called Second Brain. \
        You help users improve, rewrite, summarize, and transform their text. \
        Return ONLY the resulting text - no explanations, no markdown code fences, no preamble. \
        Preserve the original language of the text unless specifically asked to translate. \
        Preserve any markdown formatting (bold, italic, links, images, tables, etc.) unless the user asks to change it. \
        If the text contains placeholders like __MEDIA_0__, __MEDIA_1__, etc., keep them exactly as they are in their original positions - they represent images and embedded files.".to_string();

    if let Some(ref style) = writing_style {
        system_prompt.push_str(&format!(
            "\n\nThe user's preferred writing style: {}",
            style
        ));
    }

    let user_message = match action.as_str() {
        "improve" => format!("Improve the writing quality of this text while keeping the same meaning and tone:\n\n{}", text),
        "fix_grammar" => format!("Fix all grammar, spelling, and punctuation errors in this text. Keep the original style:\n\n{}", text),
        "shorter" => format!("Make this text more concise while keeping the key points:\n\n{}", text),
        "longer" => format!("Expand this text with more detail while keeping the same style and tone:\n\n{}", text),
        "professional" => format!("Rewrite this text in a professional, formal tone:\n\n{}", text),
        "friendly" => format!("Rewrite this text in a casual, friendly tone:\n\n{}", text),
        "summarize" => format!("Write a brief summary of this text:\n\n{}", text),
        "explain" => format!("Explain this text in simpler terms:\n\n{}", text),
        "translate_en" => format!("Translate this text to English:\n\n{}", text),
        "translate_nl" => format!("Translate this text to Dutch:\n\n{}", text),
        "translate_de" => format!("Translate this text to German:\n\n{}", text),
        "translate_fr" => format!("Translate this text to French:\n\n{}", text),
        "translate_es" => format!("Translate this text to Spanish:\n\n{}", text),
        "custom" => {
            let prompt = custom_prompt.unwrap_or_else(|| "Improve this text".to_string());
            format!("{}\n\n{}", prompt, text)
        }
        _ => format!("Improve this text:\n\n{}", text),
    };

    crate::ai::ai_request(app, settings, system_prompt, user_message, request_id);
    Ok(())
}

// ── Helpers ──

// On mobile (Android + iOS) the OS config dir is injected at startup via the Tauri
// path resolver, since dirs::config_dir() is not reliable in the app sandbox.
static MOBILE_CONFIG_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

#[cfg(mobile)]
pub fn set_mobile_config_dir(path: std::path::PathBuf) {
    let _ = MOBILE_CONFIG_DIR.set(path);
}

/// The injected mobile directory, or `None` on desktop where it is never set.
// Unused under `cfg(test)`, where machine-local state is rooted in a temp directory.
#[cfg_attr(test, allow(dead_code))]
pub fn mobile_config_dir() -> Option<std::path::PathBuf> {
    MOBILE_CONFIG_DIR.get().cloned()
}

fn app_config_path() -> Result<std::path::PathBuf, String> {
    // Prefer the injected mobile dir when present (set only on mobile); fall back to
    // the platform config dir on desktop.
    let app_dir = if let Some(mobile_dir) = MOBILE_CONFIG_DIR.get() {
        mobile_dir.join("helixnotes")
    } else if let Some(config_dir) = dirs::config_dir() {
        config_dir.join("helixnotes")
    } else {
        return Err("Config directory not available yet".to_string());
    };
    std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
    Ok(app_dir.join("config.json"))
}

pub fn load_app_config() -> AppConfig {
    app_config_path()
        .map(|path| load_app_config_from(&path, &crate::secret_store::OsSecretStore))
        .unwrap_or_default()
}

fn load_app_config_from(
    path: &std::path::Path,
    store: &dyn crate::secret_store::SecretStore,
) -> AppConfig {
    let mut config = match read_app_config(path) {
        ConfigRead::Loaded(contents) => {
            let contents = match crate::sync_config::retire_webdav(path, &contents, store) {
                Ok(cleaned) => cleaned,
                Err(error) => {
                    log::warn!(
                        "WebDAV retirement is incomplete and will retry at startup: {error}"
                    );
                    // Current AppConfig has no WebDAV fields, so even the historical JSON
                    // cannot restore a production execution path while cleanup waits to retry.
                    contents
                }
            };
            match parse_app_config(&contents) {
                Ok(config) => config,
                Err(damage) => damaged_config(path, damage),
            }
        }
        ConfigRead::Absent => AppConfig::default(),
        ConfigRead::Unreadable(error) => damaged_config(
            path,
            ConfigDamage {
                reason: format!("config.json could not be read: {error}"),
                recovered_vaults: Vec::new(),
            },
        ),
    };
    if config.config_error.is_some() {
        // Leave the damaged file alone until it is preserved (done in `damaged_config`), and
        // skip migrations that would rewrite it.
        if let Err(error) = crate::secret_store::hydrate_config(&mut config, store) {
            config.secret_store_error = Some(error);
        }
        return config;
    }
    let identity_migration = populate_vault_ids(&mut config);

    match crate::secret_store::hydrate_config(&mut config, store) {
        Ok(outcome) => {
            config.secret_store_error = None;
            if identity_migration || outcome.migrated_plaintext {
                if let Err(save_error) = save_app_config_to(path, &config) {
                    if outcome.migrated_plaintext {
                        let rollback_error =
                            crate::secret_store::rollback_changes(store, &outcome.changes).err();
                        config = crate::secret_store::redacted_config(&config);
                        config.secret_store_error = Some(match rollback_error {
                            Some(rollback_error) => format!(
                                "Credentials could not be migrated because config.json could not be redacted ({save_error}); restoring the previous OS credentials also failed: {rollback_error}"
                            ),
                            None => format!(
                                "Credentials could not be migrated because config.json could not be redacted: {save_error}"
                            ),
                        });
                    }
                }
            }
        }
        Err(error) => {
            // hydrate_config clears all credential fields before touching the store, so an
            // unavailable/locked keyring can never turn the plaintext file into a runtime
            // fallback. Keep an existing plaintext file intact for a later migration retry.
            config.secret_store_error = Some(error);
        }
    }
    config
}

enum ConfigRead {
    Absent,
    Unreadable(std::io::Error),
    Loaded(String),
}

fn read_app_config(path: &std::path::Path) -> ConfigRead {
    match std::fs::read_to_string(path) {
        Ok(contents) => ConfigRead::Loaded(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ConfigRead::Absent,
        Err(error) => ConfigRead::Unreadable(error),
    }
}

struct ConfigDamage {
    reason: String,
    recovered_vaults: Vec<VaultConfig>,
}

/// Malformed (not JSON) and incompatible (JSON, but not an `AppConfig`) are reported
/// differently. Either way, every vault entry that still parses is returned for recovery.
fn parse_app_config(contents: &str) -> Result<AppConfig, ConfigDamage> {
    let document: serde_json::Value =
        serde_json::from_str(contents).map_err(|error| ConfigDamage {
            reason: format!("config.json is malformed: {error}"),
            recovered_vaults: Vec::new(),
        })?;
    serde_json::from_value(document.clone()).map_err(|error| ConfigDamage {
        reason: format!("config.json is incompatible with this version: {error}"),
        recovered_vaults: document
            .get("vaults")
            .and_then(serde_json::Value::as_array)
            .map(|vaults| {
                vaults
                    .iter()
                    .filter_map(|vault| serde_json::from_value(vault.clone()).ok())
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Startup config for a damaged `config.json`: defaults plus the recovered vault list, no
/// active vault, and an error the vault picker shows. The damaged file is copied aside
/// (owner-only, since it may hold credentials) first; if that fails, saving stays blocked.
fn damaged_config(path: &std::path::Path, damage: ConfigDamage) -> AppConfig {
    let preserved = path.with_extension(format!(
        "json.damaged-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    let copied = std::fs::copy(path, &preserved).and_then(|_| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&preserved, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    });
    let reason = damage.reason;
    let (message, config_save_blocked) = match copied {
        Ok(()) => (
            format!(
                "{reason}. Settings were reset; the damaged file was kept at {}.",
                preserved.display()
            ),
            false,
        ),
        Err(error) => (
            format!(
                "{reason}. The file could not be preserved ({error}), so settings will not be saved until it is fixed or removed: {}",
                path.display()
            ),
            true,
        ),
    };
    log::error!("{message}");
    AppConfig {
        vaults: damage.recovered_vaults,
        config_error: Some(message),
        config_save_blocked,
        ..AppConfig::default()
    }
}

fn populate_vault_ids(config: &mut AppConfig) -> bool {
    let mut changed = false;
    for vault in &mut config.vaults {
        if vault.vault_id.is_some() {
            continue;
        }
        if let Ok(identity) = crate::machine_local::vault_id(std::path::Path::new(&vault.path)) {
            vault.vault_id = Some(identity);
            changed = true;
        }
    }
    changed
}

fn write_private_file(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    crate::durable::replace(path, data, crate::durable::Mode::Private)
}

#[cfg(test)]
mod config_startup_tests {
    use super::*;

    struct EmptyStore;
    impl crate::secret_store::SecretStore for EmptyStore {
        fn get(&self, _id: &crate::secret_store::SecretId) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn set(&self, _id: &crate::secret_store::SecretId, _value: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _id: &crate::secret_store::SecretId) -> Result<(), String> {
            Ok(())
        }
    }

    fn scratch() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-config-startup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn preserved_copies(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.starts_with("config.json.damaged-"))
            .collect()
    }

    #[test]
    fn absent_config_is_a_clean_first_launch() {
        let dir = scratch();
        let loaded = load_app_config_from(&dir.join("config.json"), &EmptyStore);
        assert!(loaded.config_error.is_none());
        assert!(loaded.vaults.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_config_is_reported_and_preserved_not_treated_as_first_launch() {
        let dir = scratch();
        let path = dir.join("config.json");
        std::fs::write(&path, "{\"vaults\": [ truncated").unwrap();

        let loaded = load_app_config_from(&path, &EmptyStore);

        let error = loaded.config_error.as_deref().unwrap();
        assert!(error.contains("malformed"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"vaults\": [ truncated"
        );
        let copies = preserved_copies(&dir);
        assert_eq!(copies.len(), 1);
        assert_eq!(
            std::fs::read_to_string(dir.join(&copies[0])).unwrap(),
            "{\"vaults\": [ truncated"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn incompatible_config_keeps_its_discoverable_vaults_without_opening_one() {
        let dir = scratch();
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"vaults":[{"path":"/vaults/work","name":"Work"},{"bogus":true}],"active_vault":"/vaults/work","theme":42}"#,
        )
        .unwrap();

        let loaded = load_app_config_from(&path, &EmptyStore);

        assert!(loaded
            .config_error
            .as_deref()
            .unwrap()
            .contains("incompatible"));
        assert_eq!(loaded.vaults.len(), 1);
        assert_eq!(loaded.vaults[0].path, "/vaults/work");
        assert!(
            loaded.active_vault.is_none(),
            "a damaged config must not auto-open a vault"
        );
        assert_eq!(preserved_copies(&dir).len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_config_is_its_own_outcome() {
        let dir = scratch();
        // A directory where the file should be reads as an I/O error, not NotFound, on
        // every platform and regardless of privileges.
        let path = dir.join("config.json");
        std::fs::create_dir(&path).unwrap();

        let loaded = load_app_config_from(&path, &EmptyStore);

        let error = loaded.config_error.as_deref().unwrap();
        assert!(error.contains("could not be read"), "{error}");
        assert!(
            ensure_config_writable(&loaded).is_err(),
            "unpreserved config must not be overwritten"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_interrupted_config_save_keeps_the_previous_config() {
        let dir = scratch();
        let path = dir.join("config.json");
        let previous = AppConfig {
            theme: "dark".to_string(),
            ..Default::default()
        };
        save_app_config_to(&path, &previous).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        let next = AppConfig {
            theme: "light".to_string(),
            ..Default::default()
        };
        crate::durable::tests::with_interrupted_replace(|| {
            assert!(save_app_config_to(&path, &next).is_err());
        });

        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert_eq!(load_app_config_from(&path, &EmptyStore).theme, "dark");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_runtime_config_error_is_never_persisted() {
        let config = AppConfig {
            config_error: Some("damaged".to_string()),
            ..Default::default()
        };
        let persisted =
            serde_json::to_string(&crate::secret_store::redacted_config(&config)).unwrap();
        assert!(!persisted.contains("config_error"));
    }
}

#[cfg(all(test, unix))]
mod config_permission_tests {
    use super::write_private_file;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn config_files_are_owner_only() {
        let path = std::env::temp_dir().join(format!(
            "helixnotes-config-permissions-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_private_file(&path, b"secret").unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"secret");
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod secret_config_tests {
    use super::*;
    use crate::secret_store::test_support::MemoryStore;
    use crate::secret_store::{SecretId, SecretStore};

    struct UnavailableStore;

    impl SecretStore for UnavailableStore {
        fn get(&self, _id: &SecretId) -> Result<Option<String>, String> {
            Err("The OS secret store is unavailable or locked".to_string())
        }
        fn set(&self, _id: &SecretId, _value: &str) -> Result<(), String> {
            unreachable!()
        }
        fn delete(&self, _id: &SecretId) -> Result<(), String> {
            unreachable!()
        }
    }

    #[test]
    fn unavailable_store_starts_fail_closed_without_erasing_plaintext_recovery_copy() {
        let path = std::env::temp_dir().join(format!(
            "helixnotes-locked-secret-store-{}.json",
            uuid::Uuid::new_v4()
        ));
        let original = serde_json::to_string(&AppConfig {
            ai_api_key: Some("only-recovery-copy".to_string()),
            ..Default::default()
        })
        .unwrap();
        std::fs::write(&path, &original).unwrap();

        let loaded = load_app_config_from(&path, &UnavailableStore);

        assert!(
            loaded.ai_api_key.is_none(),
            "runtime must never use plaintext fallback"
        );
        assert_eq!(
            loaded.secret_store_error.as_deref(),
            Some("The OS secret store is unavailable or locked")
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "a failed migration must not erase the only recoverable credential"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn successful_startup_migration_redacts_the_config_file() {
        let path = std::env::temp_dir().join(format!(
            "helixnotes-secret-migration-{}.json",
            uuid::Uuid::new_v4()
        ));
        let original = AppConfig {
            ai_api_key: Some("move-me-to-the-keyring".to_string()),
            ..Default::default()
        };
        std::fs::write(&path, serde_json::to_string(&original).unwrap()).unwrap();
        let store = MemoryStore::default();

        let loaded = load_app_config_from(&path, &store);

        assert_eq!(loaded.ai_api_key, original.ai_api_key);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("move-me-to-the-keyring"));
        assert_eq!(
            store.0.borrow().get(&SecretId::AnthropicApiKey).cloned(),
            original.ai_api_key
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_invalid_retirement_marker_does_not_block_other_secret_hydration_or_updates() {
        let dir = std::env::temp_dir().join(format!(
            "helixnotes-invalid-webdav-retirement-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, serde_json::to_string(&AppConfig::default()).unwrap()).unwrap();
        std::fs::write(dir.join("webdav-retirement-v1.json"), "not valid json").unwrap();
        let store = MemoryStore::default();
        store
            .set(&SecretId::AnthropicApiKey, "existing-anthropic-key")
            .unwrap();

        let loaded = load_app_config_from(&path, &store);

        assert_eq!(loaded.ai_api_key.as_deref(), Some("existing-anthropic-key"));
        assert!(loaded.secret_store_error.is_none());

        let mut updated = loaded.clone();
        updated.ai_api_key = Some("rotated-anthropic-key".to_string());
        crate::secret_store::apply_config_changes(&loaded, &updated, &store).unwrap();
        assert_eq!(
            store
                .0
                .borrow()
                .get(&SecretId::AnthropicApiKey)
                .map(String::as_str),
            Some("rotated-anthropic-key")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unavailable_store_still_allows_non_secret_settings_to_persist() {
        let path = std::env::temp_dir().join(format!(
            "helixnotes-headless-settings-{}.json",
            uuid::Uuid::new_v4()
        ));
        let recovery = AppConfig {
            ai_api_key: Some("unmigrated-recovery-copy".to_string()),
            ..Default::default()
        };
        std::fs::write(&path, serde_json::to_string(&recovery).unwrap()).unwrap();
        let mut loaded = load_app_config_from(&path, &UnavailableStore);
        loaded.theme = "dark".to_string();

        save_app_config_with_recovery(&path, &loaded, Some(&recovery)).unwrap();

        let persisted: AppConfig =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(persisted.theme, "dark");
        assert_eq!(
            persisted.ai_api_key.as_deref(),
            Some("unmigrated-recovery-copy")
        );
        assert!(
            loaded.ai_api_key.is_none(),
            "the recovery copy must not become a runtime fallback"
        );
        std::fs::remove_file(path).unwrap();
    }
}

/// A damaged config.json that could not be copied aside must not be replaced by a save.
fn ensure_config_writable(config: &AppConfig) -> Result<(), String> {
    match (&config.config_error, config.config_save_blocked) {
        (Some(error), true) => Err(error.clone()),
        _ => Ok(()),
    }
}

fn save_app_config(config: &AppConfig) -> Result<(), String> {
    let path = app_config_path()?;
    ensure_config_writable(config)?;
    if config.secret_store_error.is_some() {
        let recovery: Option<AppConfig> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok());
        return save_app_config_with_recovery(&path, config, recovery.as_ref());
    }
    save_app_config_to(&path, config)
}

pub(crate) fn commit_secret_config(
    config: &mut AppConfig,
    mut candidate: AppConfig,
) -> Result<(), String> {
    if let Some(error) = &config.secret_store_error {
        return Err(format!(
            "Credentials cannot be changed until the OS secret store is available; restart the app after unlocking it. {error}"
        ));
    }

    let store = crate::secret_store::OsSecretStore;
    let changes = match crate::secret_store::apply_config_changes(config, &candidate, &store) {
        Ok(changes) => changes,
        Err(error) => {
            config.secret_store_error = Some(error.clone());
            return Err(error);
        }
    };
    candidate.secret_store_error = None;
    if let Err(save_error) = save_app_config(&candidate) {
        return match crate::secret_store::rollback_changes(&store, &changes) {
            Ok(()) => Err(save_error),
            Err(rollback_error) => Err(format!(
                "{save_error}; restoring the previous credentials also failed: {rollback_error}"
            )),
        };
    }
    *config = candidate;
    Ok(())
}

fn save_app_config_to(path: &std::path::Path, config: &AppConfig) -> Result<(), String> {
    let redacted = crate::secret_store::redacted_config(config);
    let data = serde_json::to_string_pretty(&redacted).map_err(|e| e.to_string())?;
    write_private_file(path, data.as_bytes())?;
    Ok(())
}

fn save_app_config_with_recovery(
    path: &std::path::Path,
    config: &AppConfig,
    recovery: Option<&AppConfig>,
) -> Result<(), String> {
    let mut persisted = crate::secret_store::redacted_config(config);
    if let Some(recovery) = recovery {
        crate::secret_store::copy_plaintext_recovery(recovery, &mut persisted);
    }
    let data = serde_json::to_string_pretty(&persisted).map_err(|error| error.to_string())?;
    write_private_file(path, data.as_bytes())
}

// ── Install Type Detection ──

#[tauri::command]
pub fn is_mobile_platform() -> bool {
    // Compile-time platform: true only for the Android/iOS builds. Authoritative, unlike the
    // webview user-agent, which some desktop WebKitGTK builds report mobile-looking (issue #63).
    cfg!(mobile)
}

#[cfg(test)]
mod commit_policy_tests {
    use super::committed_save_outcome;

    #[test]
    fn canonical_revision_survives_every_projection_failure() {
        let outcome = committed_save_outcome(
            "new-revision".to_string(),
            [
                ("Search projection", Err("index unavailable".to_string())),
                ("Repair ledger", Err("ledger unavailable".to_string())),
            ],
        );
        assert_eq!(outcome.revision, "new-revision");
        assert_eq!(outcome.warnings.len(), 2);
    }
}
