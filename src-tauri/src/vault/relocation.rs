use same_file::Handle;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

/// In-vault staging area for note bytes awaiting an atomic rename into place.
const STAGING_DIR: &str = "staging";

/// Marker naming the vault-relative path a staged file was claimed from. See `sweep_staging`.
const ORIGIN_MARKER: &str = "origin";

/// The rewritten copy a `rewrite_file` transaction stages before publishing it.
pub(crate) const STAGED_REPLACEMENT: &str = "replacement.md";

/// The manifest whose presence is what authorizes replaying a directory move.
pub(crate) const DIRECTORY_MANIFEST: &str = "directory-move.json";

#[derive(Clone)]
pub struct DirectoryRewrite {
    pub relative_path: PathBuf,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct DirectoryMoveManifest {
    source: PathBuf,
    destination: PathBuf,
    ownership_marker: String,
    ownership_token: String,
    rewrites: Vec<DirectoryMoveManifestRewrite>,
}

#[derive(Serialize, Deserialize)]
struct DirectoryMoveManifestRewrite {
    relative_path: PathBuf,
    before: Vec<u8>,
    after: Vec<u8>,
}

pub struct DirectoryRelocation {
    vault: PathBuf,
    source: PathBuf,
    destination: PathBuf,
    source_identity: Handle,
    rewrites: Vec<DirectoryRewrite>,
    transaction_dir: PathBuf,
    ownership_marker: String,
    ownership_token: String,
}

#[derive(Debug)]
pub struct DirectoryRecoveryFailure {
    pub message: String,
    pub paths: Vec<String>,
}

impl DirectoryRelocation {
    pub fn commit(self) -> Result<PathBuf, String> {
        remove_ownership_marker(
            &self.destination,
            &self.ownership_marker,
            &self.ownership_token,
        )
        .map_err(|error| {
            format!(
                "Repair required: notebook move completed, but ownership cleanup failed: {error}"
            )
        })?;
        cleanup_directory_manifest(&self.transaction_dir).map_err(|error| {
            format!(
                "Repair required: notebook move completed, but its recovery manifest remains: {error}"
            )
        })?;
        Ok(self.destination)
    }

    pub fn rollback(self) -> Result<(), String> {
        if let Err(error) = verify_directory_owner(
            &self.destination,
            &self.source_identity,
            &self.ownership_marker,
            &self.ownership_token,
        ) {
            return Err(format!(
                "Repair required: notebook move rollback preserved an unowned destination: {error}"
            ));
        }
        let mut failures = rollback_directory_rewrites(
            &self.vault,
            &self.destination,
            &self.source_identity,
            &self.ownership_marker,
            &self.ownership_token,
            self.rewrites.iter().rev(),
        );
        if let Err(error) = fs::rename(&self.destination, &self.source) {
            failures.push(format!("could not restore notebook directory: {error}"));
        }
        if failures.is_empty() {
            remove_ownership_marker(&self.source, &self.ownership_marker, &self.ownership_token)?;
            cleanup_directory_manifest(&self.transaction_dir)
        } else {
            Err(format!(
                "Repair required: notebook move rollback incomplete: {}",
                failures.join("; ")
            ))
        }
    }
}

/// Move a directory as one recoverable operation and durably rewrite its Markdown files.
///
/// A manifest is synced before the directory changes location. Until `commit` succeeds,
/// startup recovery can finish the destination rewrites without guessing which notes were
/// updated. Rollback only touches the directory whose identity was captured here.
pub fn relocate_directory(
    vault_root: &Path,
    source: &Path,
    destination: &Path,
    rewrites: Vec<DirectoryRewrite>,
) -> Result<DirectoryRelocation, String> {
    let vault = canonical_directory(vault_root, "vault path")?;
    let source = canonical_directory(source, "directory relocation source")?;
    if !source.starts_with(&vault) || source == vault {
        return Err("Directory relocation source must stay inside the active vault".to_string());
    }
    let destination_parent = destination
        .parent()
        .ok_or("Directory relocation destination must have a parent")?;
    let destination_parent = canonical_directory(
        destination_parent,
        "directory relocation destination parent",
    )?;
    if !destination_parent.starts_with(&vault) || destination_parent == vault {
        return Err(
            "Directory relocation destination must stay inside the active vault".to_string(),
        );
    }
    let destination_name = destination
        .file_name()
        .ok_or("Directory relocation destination must have a name")?;
    let destination = destination_parent.join(destination_name);
    if destination.exists() {
        return Err("Directory relocation destination already exists".to_string());
    }

    for rewrite in &rewrites {
        validate_directory_rewrite(&source, rewrite)?;
    }

    let source_identity = Handle::from_path(&source)
        .map_err(|error| format!("Could not identify directory relocation source: {error}"))?;
    let transaction_dir = create_manifest_dir(&vault)?;
    let ownership_token = transaction_dir
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or("Directory relocation transaction has no identity")?
        .to_string();
    let ownership_marker = format!(".helixnotes-directory-move-{ownership_token}");
    if let Err(error) = create_ownership_marker(&source, &ownership_marker, &ownership_token) {
        let _ = remove_ownership_marker(&source, &ownership_marker, &ownership_token);
        let _ = fs::remove_dir(&transaction_dir);
        return Err(error);
    }
    let manifest = DirectoryMoveManifest {
        source: source
            .strip_prefix(&vault)
            .map_err(|error| error.to_string())?
            .to_path_buf(),
        destination: destination
            .strip_prefix(&vault)
            .map_err(|error| error.to_string())?
            .to_path_buf(),
        ownership_marker: ownership_marker.clone(),
        ownership_token: ownership_token.clone(),
        rewrites: rewrites
            .iter()
            .map(|rewrite| DirectoryMoveManifestRewrite {
                relative_path: rewrite.relative_path.clone(),
                before: rewrite.before.clone(),
                after: rewrite.after.clone(),
            })
            .collect(),
    };
    if let Err(error) = write_directory_manifest(&transaction_dir, &manifest) {
        let _ = remove_ownership_marker(&source, &ownership_marker, &ownership_token);
        let _ = fs::remove_dir(&transaction_dir);
        return Err(error);
    }

    if let Err(error) = fs::rename(&source, &destination) {
        let _ = remove_ownership_marker(&source, &ownership_marker, &ownership_token);
        let _ = cleanup_directory_manifest(&transaction_dir);
        return Err(format!("Could not move notebook directory: {error}"));
    }
    let destination_identity = Handle::from_path(&destination).map_err(|error| {
        format!("Repair required: could not identify moved notebook directory: {error}")
    })?;
    if destination_identity != source_identity {
        return Err(
            "Repair required: the directory relocation destination is not the source directory"
                .to_string(),
        );
    }
    verify_directory_owner(
        &destination,
        &source_identity,
        &ownership_marker,
        &ownership_token,
    )?;

    let transaction = DirectoryRelocation {
        vault,
        source,
        destination,
        source_identity,
        rewrites,
        transaction_dir,
        ownership_marker,
        ownership_token,
    };
    for rewrite in &transaction.rewrites {
        let path = transaction.destination.join(&rewrite.relative_path);
        if let Err(error) = rewrite_file(&transaction.vault, &path, |current| {
            if current == rewrite.before || current == rewrite.after {
                Ok(rewrite.after.clone())
            } else {
                Err("The note changed while its notebook was moving".to_string())
            }
        }) {
            return match transaction.rollback() {
                Ok(()) => Err(format!("Could not rewrite moved notebook metadata: {error}")),
                Err(rollback) => Err(format!(
                    "Repair required: could not rewrite moved notebook metadata: {error}. {rollback}"
                )),
            };
        }
    }
    Ok(transaction)
}

/// Complete directory moves that were interrupted after their durable manifest was written.
pub fn recover_directory_relocations(vault_root: &Path) -> Vec<DirectoryRecoveryFailure> {
    let Ok(vault) = canonical_directory(vault_root, "vault path") else {
        return vec![DirectoryRecoveryFailure {
            message: "Could not open the vault for directory-move recovery".to_string(),
            paths: vec![vault_root.to_string_lossy().to_string()],
        }];
    };
    let recovery_root = match crate::machine_local::relocation_dir(&vault) {
        Ok(root) => root,
        Err(message) => {
            return vec![DirectoryRecoveryFailure {
                message,
                paths: vec![vault.to_string_lossy().to_string()],
            }]
        }
    };
    let Ok(entries) = fs::read_dir(&recovery_root) else {
        return Vec::new();
    };
    let mut failures = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                failures.push(DirectoryRecoveryFailure {
                    message: format!("could not read a recovery directory entry: {error}"),
                    paths: vec![recovery_root.to_string_lossy().to_string()],
                });
                continue;
            }
        };
        let transaction_dir = entry.path();
        let manifest_path = transaction_dir.join(DIRECTORY_MANIFEST);
        if !manifest_path.is_file() {
            // Killed after reserving the directory but before the manifest became durable,
            // so no move was ever authorized and there is nothing to replay. Discard it,
            // or these accumulate for the life of the vault. `remove_dir` refuses to touch
            // a non-empty directory, so a half-written transaction is left for inspection
            // rather than deleted.
            let _ = fs::remove_dir(&transaction_dir);
            continue;
        }
        let mut affected_paths = vec![manifest_path.to_string_lossy().to_string()];
        let result = (|| {
            let bytes = fs::read(&manifest_path)
                .map_err(|error| format!("could not read move manifest: {error}"))?;
            let manifest: DirectoryMoveManifest = serde_json::from_slice(&bytes)
                .map_err(|error| format!("could not parse move manifest: {error}"))?;
            validate_relative_path(&manifest.source)?;
            validate_relative_path(&manifest.destination)?;
            let source = vault.join(&manifest.source);
            let destination = vault.join(&manifest.destination);
            affected_paths.push(source.to_string_lossy().to_string());
            affected_paths.push(destination.to_string_lossy().to_string());
            for rewrite in &manifest.rewrites {
                affected_paths.push(
                    source
                        .join(&rewrite.relative_path)
                        .to_string_lossy()
                        .to_string(),
                );
                affected_paths.push(
                    destination
                        .join(&rewrite.relative_path)
                        .to_string_lossy()
                        .to_string(),
                );
            }
            let (root, use_after) = match (source.is_dir(), destination.is_dir()) {
                (true, false) => (source, false),
                (false, true) => (destination, true),
                _ => {
                    return Err(
                        "source and destination state is ambiguous; both were preserved"
                            .to_string(),
                    )
                }
            };
            let identity = Handle::from_path(&root)
                .map_err(|error| format!("could not identify recovery directory: {error}"))?;
            verify_directory_owner(
                &root,
                &identity,
                &manifest.ownership_marker,
                &manifest.ownership_token,
            )?;
            for rewrite in &manifest.rewrites {
                verify_directory_owner(
                    &root,
                    &identity,
                    &manifest.ownership_marker,
                    &manifest.ownership_token,
                )?;
                validate_relative_path(&rewrite.relative_path)?;
                let path = root.join(&rewrite.relative_path);
                rewrite_file(&vault, &path, |current| {
                    let target = if use_after {
                        &rewrite.after
                    } else {
                        &rewrite.before
                    };
                    if current == rewrite.before || current == rewrite.after {
                        Ok(target.clone())
                    } else {
                        Err("note changed after the recorded notebook move".to_string())
                    }
                })?;
            }
            remove_ownership_marker(&root, &manifest.ownership_marker, &manifest.ownership_token)?;
            cleanup_directory_manifest(&transaction_dir)
        })();
        if let Err(error) = result {
            affected_paths.sort();
            affected_paths.dedup();
            failures.push(DirectoryRecoveryFailure {
                message: format!("{}: {error}", manifest_path.display()),
                paths: affected_paths,
            });
        }
    }
    failures
}

/// The destructive filesystem seam for moving a Markdown note inside one vault.
///
/// The rewrite is derived from the source snapshot owned by this function. The
/// destination is create-new and synced before the unchanged source is claimed and
/// removed, so a concurrent autosave is never unlinked by path alone.
pub fn relocate_file<F>(
    vault_root: &Path,
    source: &Path,
    destination_dir: &Path,
    preferred_name: &OsStr,
    rewrite: F,
) -> Result<PathBuf, String>
where
    F: FnOnce(&[u8]) -> Result<Vec<u8>, String>,
{
    relocate_file_with_hook(
        vault_root,
        source,
        destination_dir,
        preferred_name,
        rewrite,
        |_| Ok(()),
    )
}

/// Durably rewrite one Markdown file without exposing a truncated or half-written note.
///
/// The replacement is synced before the source is claimed. The original is then kept in
/// the relocation recovery area until the replacement occupies the original path, which
/// also works on Windows where renaming over an existing file is not portable.
pub fn rewrite_file<F>(vault_root: &Path, source: &Path, rewrite: F) -> Result<(), String>
where
    F: FnOnce(&[u8]) -> Result<Vec<u8>, String>,
{
    let vault = canonical_directory(vault_root, "vault path")?;
    let source = validate_source(&vault, source)?;
    let mut source_file =
        File::open(&source).map_err(|error| format!("Could not open rewrite source: {error}"))?;
    let source_identity = Handle::from_file(
        source_file
            .try_clone()
            .map_err(|error| format!("Could not identify rewrite source: {error}"))?,
    )
    .map_err(|error| format!("Could not identify rewrite source: {error}"))?;
    let source_metadata = source_file
        .metadata()
        .map_err(|error| format!("Could not inspect rewrite source: {error}"))?;
    let mut source_bytes = Vec::new();
    source_file
        .read_to_end(&mut source_bytes)
        .map_err(|error| format!("Could not read rewrite source: {error}"))?;
    let source_hash = digest(&source_bytes);
    let output = rewrite(&source_bytes)?;
    let output_hash = digest(&output);

    let transaction_dir = create_staging_dir(&vault)?;
    let replacement = transaction_dir.join(STAGED_REPLACEMENT);
    let mut replacement_file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&replacement)
    {
        Ok(file) => file,
        Err(error) => {
            let _ = fs::remove_dir(&transaction_dir);
            return Err(format!("Could not reserve rewrite replacement: {error}"));
        }
    };
    let replacement_identity = match Handle::from_file(
        replacement_file
            .try_clone()
            .map_err(|error| format!("Could not identify rewrite replacement: {error}"))?,
    ) {
        Ok(identity) => identity,
        Err(error) => {
            drop(replacement_file);
            let _ = fs::remove_file(&replacement);
            let _ = fs::remove_dir(&transaction_dir);
            return Err(format!("Could not identify rewrite replacement: {error}"));
        }
    };
    if let Err(error) = replacement_file
        .write_all(&output)
        .and_then(|_| replacement_file.set_permissions(source_metadata.permissions()))
        .and_then(|_| replacement_file.sync_all())
    {
        drop(replacement_file);
        return cleanup_before_claim_error(
            &replacement,
            &replacement_identity,
            &transaction_dir,
            format!("Could not publish rewritten note: {error}"),
        );
    }
    drop(replacement_file);

    let recovery_source = transaction_dir.join(
        source
            .file_name()
            .unwrap_or_else(|| OsStr::new("recovery.md")),
    );
    // Record where the original came from *before* claiming it. Between the claim below
    // and the publish further down, these bytes exist nowhere else in the vault; if the
    // process dies in that window, this note is what lets `sweep_staging` put them back
    // rather than discard them.
    if let Err(error) = write_origin_marker(&transaction_dir, &vault, &source) {
        return cleanup_before_claim_error(
            &replacement,
            &replacement_identity,
            &transaction_dir,
            error,
        );
    }
    if let Err(error) = fs::rename(&source, &recovery_source) {
        return cleanup_before_claim_error(
            &replacement,
            &replacement_identity,
            &transaction_dir,
            format!("Could not claim rewrite source: {error}"),
        );
    }
    if let Some(parent) = source.parent() {
        if let Err(error) = sync_directory(parent) {
            let restore = restore_recovery_source(&recovery_source, &source);
            let cleanup = remove_owned_file(&replacement, &replacement_identity);
            let _ = fs::remove_dir(&transaction_dir);
            return Err(format!(
                "Could not sync claimed rewrite source ({error}). Source recovery: {}. Replacement cleanup: {}",
                result_summary(restore),
                result_summary(cleanup)
            ));
        }
    }
    if let Err(error) = sync_directory(&transaction_dir) {
        let restore = restore_recovery_source(&recovery_source, &source);
        let cleanup = remove_owned_file(&replacement, &replacement_identity);
        let _ = fs::remove_dir(&transaction_dir);
        return Err(format!(
            "Could not sync rewrite recovery ({error}). Source recovery: {}. Replacement cleanup: {}",
            result_summary(restore),
            result_summary(cleanup)
        ));
    }

    let claimed_identity = match Handle::from_path(&recovery_source) {
        Ok(identity) => identity,
        Err(error) => {
            let restore = restore_recovery_source(&recovery_source, &source);
            let cleanup = remove_owned_file(&replacement, &replacement_identity);
            let _ = fs::remove_dir(&transaction_dir);
            return Err(format!(
                "Could not identify claimed rewrite source ({error}). Source recovery: {}. Replacement cleanup: {}",
                result_summary(restore),
                result_summary(cleanup)
            ));
        }
    };
    let claimed_bytes = match fs::read(&recovery_source) {
        Ok(bytes) => bytes,
        Err(error) => {
            let restore = restore_recovery_source(&recovery_source, &source);
            let cleanup = remove_owned_file(&replacement, &replacement_identity);
            let _ = fs::remove_dir(&transaction_dir);
            return Err(format!(
                "Could not verify claimed rewrite source ({error}). Source recovery: {}. Replacement cleanup: {}",
                result_summary(restore),
                result_summary(cleanup)
            ));
        }
    };
    if claimed_identity != source_identity || digest(&claimed_bytes) != source_hash {
        let restore = restore_recovery_source(&recovery_source, &source);
        let cleanup = remove_owned_file(&replacement, &replacement_identity);
        let _ = fs::remove_dir(&transaction_dir);
        return Err(format!(
            "The note changed during rewrite. Source recovery: {}. Replacement cleanup: {}",
            result_summary(restore),
            result_summary(cleanup)
        ));
    }

    if let Err(error) = fs::rename(&replacement, &source) {
        let restore = restore_recovery_source(&recovery_source, &source);
        let cleanup = remove_owned_file(&replacement, &replacement_identity);
        let _ = fs::remove_dir(&transaction_dir);
        return Err(format!(
            "Could not install rewritten note ({error}). Source recovery: {}. Replacement cleanup: {}",
            result_summary(restore),
            result_summary(cleanup)
        ));
    }
    if let Some(parent) = source.parent() {
        sync_directory(parent)
            .map_err(|error| format!("Repair required: could not sync rewritten note: {error}"))?;
    }
    sync_directory(&transaction_dir).map_err(|error| {
        format!("Repair required: could not sync the rewrite recovery directory: {error}")
    })?;
    let mut installed = File::open(&source).map_err(|error| {
        format!("Repair required: could not open the installed rewrite: {error}")
    })?;
    let installed_identity = Handle::from_file(installed.try_clone().map_err(|error| {
        format!("Repair required: could not identify the installed rewrite: {error}")
    })?)
    .map_err(|error| {
        format!("Repair required: could not identify the installed rewrite: {error}")
    })?;
    let mut installed_bytes = Vec::new();
    installed
        .read_to_end(&mut installed_bytes)
        .map_err(|error| {
            format!("Repair required: could not verify the installed rewrite: {error}")
        })?;
    if installed_identity != replacement_identity || digest(&installed_bytes) != output_hash {
        return Err(
            "Repair required: the installed rewrite was replaced or changed; the original recovery copy was preserved"
                .to_string(),
        );
    }
    remove_redundant_file(&recovery_source).map_err(|error| {
        format!(
            "Repair required: rewritten note is durable, but its recovery copy remains: {error}"
        )
    })?;
    sync_directory(&transaction_dir).map_err(|error| {
        format!("Repair required: could not sync recovery cleanup after rewrite: {error}")
    })?;
    let _ = fs::remove_dir(&transaction_dir);
    Ok(())
}

fn relocate_file_with_hook<F, H>(
    vault_root: &Path,
    source: &Path,
    destination_dir: &Path,
    preferred_name: &OsStr,
    rewrite: F,
    before_source_claim: H,
) -> Result<PathBuf, String>
where
    F: FnOnce(&[u8]) -> Result<Vec<u8>, String>,
    H: FnOnce(&Path) -> Result<(), String>,
{
    let vault = canonical_directory(vault_root, "vault path")?;
    let source = validate_source(&vault, source)?;
    let destination_dir = validate_destination(&vault, destination_dir)?;
    validate_preferred_name(preferred_name)?;

    let mut source_file = File::open(&source)
        .map_err(|error| format!("Could not open relocation source: {error}"))?;
    let source_identity = Handle::from_file(
        source_file
            .try_clone()
            .map_err(|error| format!("Could not identify relocation source: {error}"))?,
    )
    .map_err(|error| format!("Could not identify relocation source: {error}"))?;
    let source_metadata = source_file
        .metadata()
        .map_err(|error| format!("Could not inspect relocation source: {error}"))?;
    let mut source_bytes = Vec::new();
    source_file
        .read_to_end(&mut source_bytes)
        .map_err(|error| format!("Could not read relocation source: {error}"))?;
    let source_hash = digest(&source_bytes);
    let output = rewrite(&source_bytes)?;

    let transaction_dir = create_staging_dir(&vault)?;
    let (destination, mut destination_file) =
        match reserve_destination(&destination_dir, preferred_name) {
            Ok(reservation) => reservation,
            Err(error) => {
                let _ = fs::remove_dir(&transaction_dir);
                return Err(error);
            }
        };
    let destination_identity = Handle::from_file(
        destination_file
            .try_clone()
            .map_err(|error| format!("Could not identify relocation destination: {error}"))?,
    )
    .map_err(|error| format!("Could not identify relocation destination: {error}"))?;
    let publish = destination_file
        .write_all(&output)
        .and_then(|_| destination_file.set_permissions(source_metadata.permissions()))
        .and_then(|_| destination_file.sync_all());
    drop(destination_file);
    if let Err(error) = publish {
        return cleanup_before_claim_error(
            &destination,
            &destination_identity,
            &transaction_dir,
            format!("Could not publish relocated note: {error}"),
        );
    }
    if let Err(error) = sync_directory(&destination_dir) {
        return cleanup_before_claim_error(
            &destination,
            &destination_identity,
            &transaction_dir,
            format!("Could not sync relocation destination: {error}"),
        );
    }

    if let Err(error) = before_source_claim(&source) {
        return cleanup_before_claim_error(
            &destination,
            &destination_identity,
            &transaction_dir,
            error,
        );
    }

    let recovery_source = transaction_dir.join(
        source
            .file_name()
            .unwrap_or_else(|| OsStr::new("recovery.md")),
    );
    if let Err(error) = fs::rename(&source, &recovery_source) {
        return cleanup_before_claim_error(
            &destination,
            &destination_identity,
            &transaction_dir,
            format!("Could not claim relocation source: {error}"),
        );
    }
    if let Some(parent) = source.parent() {
        sync_directory(parent)
            .map_err(|error| format!("Could not sync claimed source location: {error}"))?;
    }

    let claimed_identity = Handle::from_path(&recovery_source)
        .map_err(|error| format!("Could not identify claimed relocation source: {error}"))?;
    let claimed_bytes = fs::read(&recovery_source)
        .map_err(|error| format!("Could not verify claimed relocation source: {error}"))?;
    if claimed_identity != source_identity || digest(&claimed_bytes) != source_hash {
        let restore = restore_recovery_source(&recovery_source, &source);
        let cleanup = remove_owned_file(&destination, &destination_identity);
        let _ = fs::remove_dir(&transaction_dir);
        return match (restore, cleanup) {
            (Ok(()), Ok(())) => Err(
                "The note changed during relocation; the current source was preserved".to_string(),
            ),
            (restore, cleanup) => Err(format!(
                "The note changed during relocation. Source recovery: {}. Destination cleanup: {}. Recovery artifact: {}",
                result_summary(restore),
                result_summary(cleanup),
                recovery_source.display()
            )),
        };
    }

    if let Err(error) = remove_redundant_file(&recovery_source) {
        return Err(format!(
            "Relocation destination is durable at {}, but a recoverable source copy remains at {}: {error}",
            destination.display(),
            recovery_source.display()
        ));
    }
    let _ = fs::remove_dir(&transaction_dir);
    sync_directory(&destination_dir)
        .map_err(|error| format!("Could not finish syncing relocation: {error}"))?;
    Ok(destination)
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("Invalid {label}: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("{label} must be a real directory"));
    }
    crate::vault::path::canonicalize(path, label)
}

fn validate_source(vault: &Path, source: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("Invalid relocation source: {error}"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || source.extension().and_then(OsStr::to_str) != Some("md")
    {
        return Err("Relocation source must be a real Markdown file".to_string());
    }
    let source = crate::vault::path::canonicalize(source, "relocation source")?;
    if !source.starts_with(vault) || source == vault {
        return Err("Relocation source must stay inside the active vault".to_string());
    }
    Ok(source)
}

fn validate_destination(vault: &Path, destination: &Path) -> Result<PathBuf, String> {
    let destination = canonical_directory(destination, "relocation destination")?;
    let relative = destination
        .strip_prefix(vault)
        .map_err(|_| "Relocation destination must stay inside the active vault".to_string())?;
    let first = relative.components().next();
    let category = match first {
        Some(Component::Normal(name)) => crate::vault::para::ParaCategory::ALL
            .into_iter()
            .any(|candidate| name == OsStr::new(candidate.folder_name())),
        _ => false,
    };
    let holding = relative == Path::new(".helixnotes").join(crate::vault::para::UNFILED_DIR);
    let trash = relative == Path::new(".helixnotes").join("trash");
    if !category && !holding && !trash {
        return Err(
            "Relocation destination must be inside a PARA category, the Holding Area, or Trash"
                .to_string(),
        );
    }
    Ok(destination)
}

fn validate_preferred_name(name: &OsStr) -> Result<(), String> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || Path::new(name).extension().and_then(OsStr::to_str) != Some("md")
    {
        return Err("Relocation filename must be a single Markdown filename".to_string());
    }
    Ok(())
}

/// Claim a free name in the vault's holding area, returning the path to rename onto.
///
/// Shares `reserve_destination`'s policy so a rescued note collides the same way any other
/// note does, and claims the name by creating it rather than by testing and hoping.
pub(crate) fn reserve_holding_slot(holding: &Path, name: &OsStr) -> Result<PathBuf, String> {
    reserve_destination(holding, name).map(|(path, _file)| path)
}

/// Whether this transaction directory carries the manifest that authorizes replaying it.
pub(crate) fn is_manifest_transaction(transaction_dir: &Path) -> bool {
    transaction_dir.join(DIRECTORY_MANIFEST).is_file()
}

fn reserve_destination(
    destination_dir: &Path,
    preferred_name: &OsStr,
) -> Result<(PathBuf, File), String> {
    let preferred = Path::new(preferred_name);
    let stem = preferred.file_stem().unwrap_or_else(|| OsStr::new("note"));
    let extension = preferred.extension();
    for suffix in 0usize.. {
        let filename = if suffix == 0 {
            preferred_name.to_owned()
        } else {
            let mut value = OsString::from(stem);
            value.push(format!(" {suffix}"));
            if let Some(extension) = extension {
                value.push(".");
                value.push(extension);
            }
            value
        };
        let destination = destination_dir.join(filename);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
        {
            Ok(file) => return Ok((destination, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not reserve relocation destination: {error}")),
        }
    }
    unreachable!("the filename suffix space is effectively unbounded")
}

/// Staging area for note bytes on their way into the vault. Stays *inside* the vault
/// because publishing a rewritten note is `fs::rename(staged, note)`, which requires both
/// paths on one filesystem; a machine-local staging area would fail with `EXDEV` for any
/// vault on a separate partition or mount.
///
/// Leaving these bytes in a synced folder is safe because nothing replays them: recovery
/// acts only on a `directory-move.json` manifest, and those are machine-local. Whatever a
/// crash leaves behind here is inert, and `sweep_staging` clears it on the next open.
fn create_staging_dir(vault: &Path) -> Result<PathBuf, String> {
    let app_data = vault.join(".helixnotes");
    let metadata = fs::symlink_metadata(&app_data)
        .map_err(|error| format!("Invalid vault app-data directory: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Vault app-data directory must be a real directory".to_string());
    }
    let staging_root = app_data.join(STAGING_DIR);
    fs::create_dir_all(&staging_root)
        .map_err(|error| format!("Could not create note staging directory: {error}"))?;
    let staging_root = canonical_directory(&staging_root, "note staging directory")?;
    if !staging_root.starts_with(vault) {
        return Err("Note staging directory escaped the active vault".to_string());
    }
    reserve_transaction(&staging_root)
}

/// Directory for a single directory-move manifest, on this machine only.
///
/// Manifests are the one piece of relocation state that is ever *replayed*, which is
/// precisely why they must not travel with the vault: replaying another machine's
/// in-flight move is the data loss this machinery exists to prevent.
fn create_manifest_dir(vault: &Path) -> Result<PathBuf, String> {
    reserve_transaction(&crate::machine_local::relocation_dir(vault)?)
}

fn reserve_transaction(root: &Path) -> Result<PathBuf, String> {
    loop {
        let transaction = root.join(uuid::Uuid::new_v4().to_string());
        match fs::create_dir(&transaction) {
            Ok(()) => return Ok(transaction),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not reserve relocation recovery path: {error}"
                ))
            }
        }
    }
}

/// Records the claimed note's location *relative to the vault root*.
///
/// Relative, not absolute, because staging sits inside the vault and therefore travels
/// with it: an absolute path recorded on one machine names nothing on another, and
/// restoring to it would write outside that machine's vault entirely.
fn write_origin_marker(transaction_dir: &Path, vault: &Path, source: &Path) -> Result<(), String> {
    let relative = source
        .strip_prefix(vault)
        .map_err(|_| "The rewrite source escaped the active vault".to_string())?;
    validate_relative_path(relative)?;
    let path = transaction_dir.join(ORIGIN_MARKER);
    let mut file = File::create(&path)
        .map_err(|error| format!("Could not record the rewrite source location: {error}"))?;
    let recorded = relative
        .to_str()
        .ok_or("Vault paths must be valid UTF-8 to be recoverable")?;
    file.write_all(recorded.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Could not record the rewrite source location: {error}"))?;
    sync_directory(transaction_dir)
        .map_err(|error| format!("Could not sync the rewrite source location: {error}"))
}

/// Clear the staging area left behind by an interrupted note rewrite, restoring any note
/// whose only copy is the staged one.
///
/// Runs at vault open, before recovery, so directory-move recovery finds a complete tree.
///
/// A staged file is never replayed — only a `directory-move.json` manifest authorizes
/// that, and those are machine-local — but it may still be the sole copy of a note. The
/// `origin` marker says where it came from, and the origin path being *absent* is what
/// distinguishes the two cases: absent means the rewrite died between claiming the
/// original and publishing its replacement, so the staged bytes are the note; present
/// means the replacement was published and the staged copy is a stale duplicate.
pub fn sweep_staging(vault_root: &Path) -> usize {
    let mut restored = 0;
    let staging = vault_root.join(".helixnotes").join(STAGING_DIR);
    // Resolve the vault once so the per-transaction containment check below compares two
    // canonical paths; a relative or symlinked vault root would defeat it otherwise.
    let Ok(vault) = canonical_directory(vault_root, "vault path") else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(&staging) else {
        return 0;
    };
    for entry in entries.flatten() {
        let transaction_dir = entry.path();
        match restore_staged_note(&vault, &transaction_dir) {
            Ok(true) => restored += 1,
            Ok(false) => {}
            Err(error) => {
                // Leave the whole transaction alone: the bytes may still be the only copy,
                // and deleting them to tidy up is the one unacceptable outcome.
                log::error!("Could not restore a staged note from {transaction_dir:?}: {error}");
                continue;
            }
        }
        let _ = fs::remove_dir_all(&transaction_dir);
    }
    let _ = fs::remove_dir(&staging);
    restored
}

/// Put a staged note back if these bytes are its only copy. Returns whether it restored one.
///
/// The origin is resolved against *this* vault rather than trusted as written, so a
/// transaction that arrived from another machine can only ever write inside this vault.
fn restore_staged_note(vault_root: &Path, transaction_dir: &Path) -> Result<bool, String> {
    let Ok(recorded) = fs::read_to_string(transaction_dir.join(ORIGIN_MARKER)) else {
        return Ok(false);
    };
    let relative = PathBuf::from(recorded);
    validate_relative_path(&relative)?;
    let origin = vault_root.join(&relative);
    let Some(name) = relative.file_name() else {
        return Ok(false);
    };
    let staged = transaction_dir.join(name);
    // Origin present means the replacement was published and the staged copy is stale;
    // origin absent means the rewrite died holding the note's only copy.
    if origin.exists() || !staged.is_file() {
        return Ok(false);
    }
    let Some(parent) = origin.parent() else {
        return Ok(false);
    };
    if !parent.is_dir() {
        return Err(format!("{parent:?} no longer exists"));
    }
    // Rejecting `..` is not enough. Staging travels with the vault, so the notebook the
    // origin names may itself be a symlink pointing out of the vault on this machine, and
    // `join` would follow it. Resolve the parent and require it to land inside the vault,
    // the same check `create_staging_dir` makes for the staging root.
    let parent = canonical_directory(parent, "restored note directory")?;
    if !parent.starts_with(vault_root) {
        return Err(format!("{parent:?} is outside the vault"));
    }
    let origin = parent.join(name);
    if origin.exists() {
        return Ok(false);
    }
    fs::rename(&staged, &origin).map_err(|error| error.to_string())?;
    let _ = sync_directory(&parent);
    Ok(true)
}

fn validate_directory_rewrite(source: &Path, rewrite: &DirectoryRewrite) -> Result<(), String> {
    validate_relative_path(&rewrite.relative_path)?;
    if rewrite.relative_path.extension().and_then(OsStr::to_str) != Some("md") {
        return Err("Directory rewrites must target Markdown files".to_string());
    }
    let path = source.join(&rewrite.relative_path);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("Invalid directory rewrite source: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Directory rewrite source must be a real file".to_string());
    }
    Ok(())
}

fn create_ownership_marker(root: &Path, name: &str, token: &str) -> Result<(), String> {
    let path = root.join(name);
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not reserve directory ownership marker: {error}"))?;
    marker
        .write_all(token.as_bytes())
        .and_then(|_| marker.sync_all())
        .map_err(|error| format!("Could not persist directory ownership marker: {error}"))?;
    sync_directory(root)
        .map_err(|error| format!("Could not sync directory ownership marker: {error}"))
}

fn verify_directory_owner(
    root: &Path,
    expected_identity: &Handle,
    marker_name: &str,
    token: &str,
) -> Result<(), String> {
    let identity = Handle::from_path(root)
        .map_err(|error| format!("could not identify directory owner: {error}"))?;
    if &identity != expected_identity {
        return Err("directory identity changed".to_string());
    }
    let marker = root.join(marker_name);
    let metadata = fs::symlink_metadata(&marker)
        .map_err(|error| format!("ownership marker is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("ownership marker is not a real file".to_string());
    }
    let stored = fs::read_to_string(&marker)
        .map_err(|error| format!("could not read ownership marker: {error}"))?;
    if stored != token {
        return Err("ownership marker does not match the transaction".to_string());
    }
    Ok(())
}

fn remove_ownership_marker(root: &Path, name: &str, token: &str) -> Result<(), String> {
    let marker = root.join(name);
    let stored = fs::read_to_string(&marker)
        .map_err(|error| format!("could not read ownership marker for cleanup: {error}"))?;
    if stored != token {
        return Err("ownership marker was replaced; it was preserved".to_string());
    }
    remove_redundant_file(&marker)?;
    sync_directory(root).map_err(|error| format!("Could not sync ownership cleanup: {error}"))
}

fn validate_relative_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("Recovery paths must be relative and traversal-free".to_string());
    }
    Ok(())
}

fn write_directory_manifest(
    transaction_dir: &Path,
    manifest: &DirectoryMoveManifest,
) -> Result<(), String> {
    let path = transaction_dir.join(DIRECTORY_MANIFEST);
    let bytes = serde_json::to_vec(manifest).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not reserve directory-move manifest: {error}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Could not persist directory-move manifest: {error}"))?;
    sync_directory(transaction_dir)
        .map_err(|error| format!("Could not sync directory-move manifest: {error}"))
}

fn cleanup_directory_manifest(transaction_dir: &Path) -> Result<(), String> {
    let path = transaction_dir.join(DIRECTORY_MANIFEST);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Could not remove directory-move manifest: {error}")),
    }
    fs::remove_dir(transaction_dir)
        .map_err(|error| format!("Could not remove directory-move recovery directory: {error}"))?;
    if let Some(parent) = transaction_dir.parent() {
        sync_directory(parent)
            .map_err(|error| format!("Could not sync directory-move cleanup: {error}"))?;
    }
    Ok(())
}

fn rollback_directory_rewrites<'a>(
    vault: &Path,
    destination: &Path,
    destination_identity: &Handle,
    ownership_marker: &str,
    ownership_token: &str,
    rewrites: impl Iterator<Item = &'a DirectoryRewrite>,
) -> Vec<String> {
    let mut failures = Vec::new();
    for rewrite in rewrites {
        if let Err(error) = verify_directory_owner(
            destination,
            destination_identity,
            ownership_marker,
            ownership_token,
        ) {
            failures.push(format!("destination ownership changed: {error}"));
            break;
        }
        let path = destination.join(&rewrite.relative_path);
        if let Err(error) = rewrite_file(vault, &path, |current| {
            if current == rewrite.after {
                Ok(rewrite.before.clone())
            } else if current == rewrite.before {
                Ok(current.to_vec())
            } else {
                Err("note changed during notebook-move rollback; it was preserved".to_string())
            }
        }) {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
    failures
}

fn restore_recovery_source(recovery: &Path, source: &Path) -> Result<(), String> {
    fs::hard_link(recovery, source).map_err(|error| {
        format!("could not restore without overwriting another writer ({error})")
    })?;
    if let Some(parent) = source.parent() {
        sync_directory(parent)
            .map_err(|error| format!("Could not sync restored source: {error}"))?;
    }
    remove_redundant_file(recovery)
}

fn cleanup_destination_error<T>(
    destination: &Path,
    identity: &Handle,
    error: String,
) -> Result<T, String> {
    match remove_owned_file(destination, identity) {
        Ok(()) => Err(error),
        Err(cleanup) => Err(format!(
            "{error}; a recovery copy remains at {} because cleanup failed: {cleanup}",
            destination.display()
        )),
    }
}

fn cleanup_before_claim_error<T>(
    destination: &Path,
    identity: &Handle,
    transaction_dir: &Path,
    error: String,
) -> Result<T, String> {
    let result = cleanup_destination_error(destination, identity, error);
    let _ = fs::remove_dir(transaction_dir);
    result
}

fn remove_owned_file(path: &Path, identity: &Handle) -> Result<(), String> {
    let current = Handle::from_path(path)
        .map_err(|error| format!("could not identify cleanup target: {error}"))?;
    if &current != identity {
        return Err("cleanup target was replaced; it was preserved".to_string());
    }
    remove_redundant_file(path)
}

// Clearing the read-only attribute is the correct Windows operation. The Clippy lint
// guards against Unix mode-bit widening, but this branch is not compiled on Unix.
#[cfg_attr(windows, allow(clippy::permissions_set_readonly_false))]
fn remove_redundant_file(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let mut permissions = fs::metadata(path)
            .map_err(|error| error.to_string())?
            .permissions();
        if permissions.readonly() {
            permissions.set_readonly(false);
            fs::set_permissions(path, permissions).map_err(|error| error.to_string())?;
        }
    }
    fs::remove_file(path).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    // Stable Rust has no portable Windows directory-fsync. The destination file itself
    // is always sync_all'd before the source is claimed.
    Ok(())
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn result_summary(result: Result<(), String>) -> String {
    match result {
        Ok(()) => "completed".to_string(),
        Err(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier};

    fn vault(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("note-relocation-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join(".helixnotes")).unwrap();
        for category in crate::vault::para::ParaCategory::ALL {
            fs::create_dir(root.join(category.folder_name())).unwrap();
        }
        root
    }

    fn directory_rewrite(path: &str, before: &str, after: &str) -> DirectoryRewrite {
        DirectoryRewrite {
            relative_path: PathBuf::from(path),
            before: before.as_bytes().to_vec(),
            after: after.as_bytes().to_vec(),
        }
    }

    fn staged_transaction(root: &Path, origin: &str, name: &str, body: &str) -> PathBuf {
        let transaction = root
            .join(".helixnotes")
            .join(STAGING_DIR)
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&transaction).unwrap();
        fs::write(transaction.join(ORIGIN_MARKER), origin).unwrap();
        fs::write(transaction.join(name), body).unwrap();
        transaction
    }

    #[test]
    fn a_staged_note_is_restored_relative_to_this_vault() {
        let root = vault("staging-restore");
        staged_transaction(&root, "Projects/Plan.md", "Plan.md", "only copy");

        assert_eq!(sweep_staging(&root), 1);

        assert_eq!(
            fs::read_to_string(root.join("Projects/Plan.md")).unwrap(),
            "only copy"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Staging travels with the vault, so a marker can arrive from another machine. The
    /// origin is resolved against *this* vault and must never escape it.
    #[test]
    fn a_staged_note_from_another_machine_cannot_escape_this_vault() {
        let root = vault("staging-escape");
        let outside = std::env::temp_dir().join(format!("escape-{}.md", uuid::Uuid::new_v4()));
        let traversal = format!("../../../..{}", outside.to_string_lossy());
        staged_transaction(
            &root,
            &traversal,
            outside.file_name().unwrap().to_str().unwrap(),
            "stolen",
        );

        assert_eq!(sweep_staging(&root), 0);

        assert!(!outside.exists(), "the sweep wrote outside the vault");
        fs::remove_dir_all(root).unwrap();
    }

    /// Rejecting `..` is not enough: the notebook the origin names may itself be a symlink
    /// out of the vault on the machine that receives it.
    #[cfg(unix)]
    #[test]
    fn a_staged_note_cannot_escape_through_a_symlinked_notebook() {
        use std::os::unix::fs::symlink;
        let root = vault("staging-symlink");
        let outside = std::env::temp_dir().join(format!("outside-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&outside).unwrap();
        fs::remove_dir(root.join("Projects")).unwrap();
        symlink(&outside, root.join("Projects")).unwrap();
        staged_transaction(&root, "Projects/Plan.md", "Plan.md", "only copy");

        assert_eq!(sweep_staging(&root), 0);

        assert!(
            !outside.join("Plan.md").exists(),
            "the sweep followed a symlink out of the vault"
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn a_staged_note_whose_original_is_back_in_place_is_discarded_not_restored() {
        let root = vault("staging-stale");
        fs::write(root.join("Projects/Plan.md"), "published").unwrap();
        staged_transaction(&root, "Projects/Plan.md", "Plan.md", "stale copy");

        assert_eq!(sweep_staging(&root), 0);

        assert_eq!(
            fs::read_to_string(root.join("Projects/Plan.md")).unwrap(),
            "published"
        );
        assert!(!root.join(".helixnotes").join(STAGING_DIR).exists());
        fs::remove_dir_all(root).unwrap();
    }

    const CRASH_VAULT_VAR: &str = "HELIXNOTES_CRASH_TEST_VAULT";
    const CRASH_NOTES: usize = 400;

    fn crash_note_body(index: usize, moved: bool) -> String {
        format!("{} note {index}", if moved { "archive" } else { "project" })
    }

    /// Interrupt a real notebook move with a real process kill, then recover.
    ///
    /// A unit test cannot produce the state this exercises: unwinding a `DirectoryRelocation`
    /// runs its cleanup, so the crash-recovery path only ever sees a simulation. Here a child
    /// process performs the move against a real vault and the parent SIGKILLs it partway
    /// through, leaving whatever the kernel happened to have written. The parent then runs
    /// startup recovery — reading manifests from the machine-local directory they now live in.
    ///
    /// Where the kill lands is deliberately not controlled, so the assertions are the
    /// invariants that must hold at *every* interruption point rather than one expected
    /// outcome: every note survives exactly once, all of them on the same side of the move,
    /// and recovery leaves no manifest behind for a later open to replay.
    #[cfg(unix)]
    #[test]
    fn killing_the_app_mid_notebook_move_still_recovers() {
        if let Ok(vault) = std::env::var(CRASH_VAULT_VAR) {
            perform_crash_test_move(Path::new(&vault));
            unreachable!("the child is killed or exits inside the move");
        }

        // The child must resolve machine-local state to the same place this process does,
        // or the parent recovers against an empty directory and the test proves nothing.
        let machine_root = crate::machine_local::test_root();
        // Escalating delays walk the kill across the operation: the early ones land during
        // the rename and manifest write, the later ones during the per-note rewrites.
        let mut interrupted = 0;
        let mut restored = 0;
        for delay_ms in (0..40).map(|step| step * 2) {
            let root = vault("directory-crash-kill");
            let source = root.join("Projects/Launch");
            fs::create_dir(&source).unwrap();
            for index in 0..CRASH_NOTES {
                fs::write(
                    source.join(format!("Note{index}.md")),
                    crash_note_body(index, false),
                )
                .unwrap();
            }

            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "vault::relocation::tests::killing_the_app_mid_notebook_move_still_recovers",
                    "--exact",
                ])
                .env(CRASH_VAULT_VAR, &root)
                .env(crate::machine_local::TEST_ROOT_VAR, &machine_root)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            // Wait for the child to reach the move before timing the kill. Sleeping from
            // process spawn instead would measure test-binary startup, and every kill
            // would land before the operation began.
            let ready = root.join("child-ready");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !ready.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&ready);

            let manifests = crate::machine_local::relocation_dir(&root).unwrap();
            if fs::read_dir(&manifests).unwrap().next().is_some() {
                interrupted += 1;
            }
            // Exactly the startup sequence in `commands::open_vault_path`.
            restored += sweep_staging(&root);
            let failures = recover_directory_relocations(&root);
            assert!(failures.is_empty(), "delay {delay_ms}ms: {failures:?}");

            assert_crash_test_vault_is_whole(&root, delay_ms);

            assert!(
                fs::read_dir(&manifests).unwrap().next().is_none(),
                "delay {delay_ms}ms: a manifest survived recovery and would replay on the next open"
            );
            fs::remove_dir_all(root).unwrap();
        }
        // Without this the test would quietly pass while killing children that had not
        // begun the move, testing nothing.
        assert!(
            interrupted > 0,
            "no run was killed with a move in flight; the timings need widening"
        );
        // The narrowest and most dangerous window: killed between claiming a note's only
        // copy and publishing its replacement. If no run lands there, the restore path in
        // `sweep_staging` is untested and this test is weaker than it looks.
        assert!(
            restored > 0,
            "no run was killed while a note existed only in staging; the timings need widening"
        );
        eprintln!("interrupted {interrupted} moves; restored {restored} staged notes");
    }

    #[cfg(unix)]
    fn perform_crash_test_move(root: &Path) {
        let rewrites = (0..CRASH_NOTES)
            .map(|index| DirectoryRewrite {
                relative_path: PathBuf::from(format!("Note{index}.md")),
                before: crash_note_body(index, false).into_bytes(),
                after: crash_note_body(index, true).into_bytes(),
            })
            .collect();
        fs::write(root.join("child-ready"), b"go").unwrap();
        let transaction = relocate_directory(
            root,
            &root.join("Projects/Launch"),
            &root.join("Archives/Launch"),
            rewrites,
        )
        .expect("the move should start cleanly");
        // Nothing commits: if the kill has not landed yet, wait for it rather than tidying
        // up, so the parent always inspects an interrupted transaction.
        std::mem::forget(transaction);
        std::thread::sleep(std::time::Duration::from_secs(30));
        std::process::exit(0);
    }

    /// Every note is present exactly once, and the notebook is wholly on one side of the
    /// move. A note that exists in neither place, in both, or with torn content is data loss.
    #[cfg(unix)]
    fn assert_crash_test_vault_is_whole(root: &Path, delay_ms: u64) {
        let source = root.join("Projects/Launch");
        let destination = root.join("Archives/Launch");
        let moved = destination.is_dir();
        assert!(
            moved != source.is_dir(),
            "delay {delay_ms}ms: the notebook is in both places or neither"
        );
        let home = if moved { &destination } else { &source };
        for index in 0..CRASH_NOTES {
            let note = home.join(format!("Note{index}.md"));
            let body = fs::read_to_string(&note)
                .unwrap_or_else(|error| panic!("delay {delay_ms}ms: lost {note:?}: {error}"));
            assert_eq!(
                body,
                crash_note_body(index, moved),
                "delay {delay_ms}ms: {note:?} was left half-written"
            );
        }
    }

    #[test]
    fn directory_move_rollback_never_rewrites_a_recreated_source_file() {
        let root = vault("directory-rollback-ownership");
        let source = root.join("Projects/Launch");
        let destination = root.join("Archives/Launch");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Plan.md"), "project").unwrap();

        let transaction = relocate_directory(
            &root,
            &source,
            &destination,
            vec![directory_rewrite("Plan.md", "project", "archive")],
        )
        .unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Plan.md"), "unowned replacement").unwrap();

        let result = transaction.rollback();

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(source.join("Plan.md")).unwrap(),
            "unowned replacement"
        );
        assert_eq!(
            fs::read_to_string(destination.join("Plan.md")).unwrap(),
            "project"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_move_rollback_never_rewrites_a_replacement_destination() {
        let root = vault("directory-rollback-destination-ownership");
        let source = root.join("Projects/Launch");
        let destination = root.join("Archives/Launch");
        let displaced = root.join("Archives/MovedByAnotherWriter");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Plan.md"), "project").unwrap();
        let transaction = relocate_directory(
            &root,
            &source,
            &destination,
            vec![directory_rewrite("Plan.md", "project", "archive")],
        )
        .unwrap();
        fs::rename(&destination, &displaced).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("Plan.md"), "archive").unwrap();

        let result = transaction.rollback();

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(destination.join("Plan.md")).unwrap(),
            "archive"
        );
        assert_eq!(
            fs::read_to_string(displaced.join("Plan.md")).unwrap(),
            "archive"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_move_preserves_a_note_changed_after_preflight() {
        let root = vault("directory-concurrent-edit");
        let source = root.join("Projects/Launch");
        let destination = root.join("Archives/Launch");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Plan.md"), "project").unwrap();
        let rewrites = vec![directory_rewrite("Plan.md", "project", "archive")];
        fs::write(source.join("Plan.md"), "concurrent edit").unwrap();

        let result = relocate_directory(&root, &source, &destination, rewrites);

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(source.join("Plan.md")).unwrap(),
            "concurrent edit"
        );
        assert!(!destination.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_recovery_completes_every_rewrite_in_an_interrupted_directory_move() {
        let root = vault("directory-crash-recovery");
        let source = root.join("Projects/Launch");
        let destination = root.join("Archives/Launch");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Plan.md"), "project plan").unwrap();
        fs::write(source.join("Draft.md"), "project draft").unwrap();
        let transaction = relocate_directory(
            &root,
            &source,
            &destination,
            vec![
                directory_rewrite("Plan.md", "project plan", "archive plan"),
                directory_rewrite("Draft.md", "project draft", "archive draft"),
            ],
        )
        .unwrap();
        fs::write(destination.join("Draft.md"), "project draft").unwrap();
        drop(transaction);

        let failures = recover_directory_relocations(&root);

        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(
            fs::read_to_string(destination.join("Plan.md")).unwrap(),
            "archive plan"
        );
        assert_eq!(
            fs::read_to_string(destination.join("Draft.md")).unwrap(),
            "archive draft"
        );
        let manifests = crate::machine_local::relocation_dir(&root).unwrap();
        assert!(fs::read_dir(manifests).unwrap().next().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_startup_recovery_reports_every_path_and_preserves_the_notebook_tree() {
        let root = vault("directory-recovery-report");
        // Built component by component rather than from a "Projects/Launch" literal: the
        // reported paths are compared as strings, and `join` keeps a forward slash
        // verbatim on Windows while the recovery code produces backslashes.
        let source = root.join("Projects").join("Launch");
        let destination = root.join("Archives").join("Launch");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Plan.md"), "project").unwrap();
        let transaction = relocate_directory(
            &root,
            &source,
            &destination,
            vec![directory_rewrite("Plan.md", "project", "archive")],
        )
        .unwrap();
        fs::write(destination.join("Plan.md"), "concurrent edit").unwrap();
        drop(transaction);

        let failures = recover_directory_relocations(&root);

        assert_eq!(failures.len(), 1);
        let paths = &failures[0].paths;
        for expected in [
            source.clone(),
            destination.clone(),
            source.join("Plan.md"),
            destination.join("Plan.md"),
        ] {
            assert!(paths.contains(&expected.to_string_lossy().to_string()));
        }
        assert!(!source.exists());
        assert_eq!(
            fs::read_to_string(destination.join("Plan.md")).unwrap(),
            "concurrent edit"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_relocations_never_overwrite_each_other() {
        let root = vault("race");
        let destination = root.join("Resources");
        let barrier = Arc::new(Barrier::new(9));
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let source_dir = root.join("Projects").join(format!("source-{index}"));
                fs::create_dir(&source_dir).unwrap();
                let source = source_dir.join("Plan.md");
                fs::write(&source, format!("note-{index}")).unwrap();
                let root = root.clone();
                let destination = destination.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    relocate_file(&root, &source, &destination, OsStr::new("Plan.md"), |raw| {
                        Ok(raw.to_vec())
                    })
                    .unwrap()
                })
            })
            .collect();
        barrier.wait();
        let relocated: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        let mut contents: Vec<_> = relocated
            .iter()
            .map(|path| fs::read_to_string(path).unwrap())
            .collect();
        contents.sort();
        let expected: Vec<_> = (0..8).map(|index| format!("note-{index}")).collect();

        assert_eq!(relocated.iter().collect::<HashSet<_>>().len(), 8);
        assert_eq!(contents, expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_replacement_is_preserved_instead_of_unlinked() {
        let root = vault("replacement");
        let source = root.join("Projects/Plan.md");
        let displaced = root.join("Projects/original.md");
        fs::write(&source, "original").unwrap();

        let result = relocate_file_with_hook(
            &root,
            &source,
            &root.join("Resources"),
            OsStr::new("Plan.md"),
            |raw| Ok(raw.to_vec()),
            |path| {
                fs::rename(path, &displaced).map_err(|error| error.to_string())?;
                fs::write(path, "autosave replacement").map_err(|error| error.to_string())
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&source).unwrap(), "autosave replacement");
        assert_eq!(fs::read_to_string(&displaced).unwrap(), "original");
        assert!(!root.join("Resources/Plan.md").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_modification_is_preserved_instead_of_unlinked() {
        let root = vault("modified");
        let source = root.join("Projects/Plan.md");
        fs::write(&source, "original").unwrap();
        let result = relocate_file_with_hook(
            &root,
            &source,
            &root.join("Resources"),
            OsStr::new("Plan.md"),
            |raw| Ok(raw.to_vec()),
            |path| fs::write(path, "new autosave").map_err(|error| error.to_string()),
        );

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&source).unwrap(), "new autosave");
        assert!(!root.join("Resources/Plan.md").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn injected_failure_before_claim_leaves_source_unchanged() {
        let root = vault("fault");
        let source = root.join("Projects/Plan.md");
        fs::write(&source, "precious").unwrap();
        let result = relocate_file_with_hook(
            &root,
            &source,
            &root.join("Resources"),
            OsStr::new("Plan.md"),
            |raw| Ok(raw.to_vec()),
            |_| Err("injected failure".to_string()),
        );

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&source).unwrap(), "precious");
        assert!(!root.join("Resources/Plan.md").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn outside_destination_is_rejected_without_writes() {
        let root = vault("outside");
        let outside = std::env::temp_dir().join(format!("outside-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&outside).unwrap();
        let source = root.join("Projects/Plan.md");
        fs::write(&source, "precious").unwrap();
        let result = relocate_file(&root, &source, &outside, OsStr::new("Plan.md"), |raw| {
            Ok(raw.to_vec())
        });

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(source).unwrap(), "precious");
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_destination_escape_is_rejected_without_writes() {
        use std::os::unix::fs::symlink;
        let root = vault("symlink");
        let outside = std::env::temp_dir().join(format!("outside-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&outside).unwrap();
        let linked = root.join("Resources/linked");
        symlink(&outside, &linked).unwrap();
        let source = root.join("Projects/Plan.md");
        fs::write(&source, "precious").unwrap();
        let result = relocate_file(&root, &source, &linked, OsStr::new("Plan.md"), |raw| {
            Ok(raw.to_vec())
        });

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(source).unwrap(), "precious");
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn invalid_recovery_directory_leaves_no_published_duplicate() {
        use std::os::unix::fs::symlink;
        let root = vault("recovery-symlink");
        let outside = std::env::temp_dir().join(format!("outside-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join(".helixnotes/staging")).unwrap();
        let source = root.join("Projects/Plan.md");
        let destination = root.join("Resources/Plan.md");
        fs::write(&source, "precious").unwrap();

        let result = relocate_file(
            &root,
            &source,
            &root.join("Resources"),
            OsStr::new("Plan.md"),
            |raw| Ok(raw.to_vec()),
        );

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(source).unwrap(), "precious");
        assert!(!destination.exists());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
