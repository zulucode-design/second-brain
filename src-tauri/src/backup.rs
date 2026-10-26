use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::ZipArchive;

use crate::types::{BackupEntry, BackupKind};

const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_ARCHIVE_BYTES: u64 = 100 * 1024 * 1024 * 1024;

impl BackupKind {
    const ALL: [Self; 2] = [Self::Backup, Self::PreSync];

    /// Archives are named `<prefix>-YYYY-MM-DDTHH-MM-SS.zip`, in UTC.
    fn prefix(self) -> &'static str {
        match self {
            Self::Backup => "helixnotes-backup",
            Self::PreSync => "helixnotes-pre-sync",
        }
    }
}

#[derive(Debug)]
pub struct RestoreError {
    pub message: String,
    pub changed: bool,
}

impl From<String> for RestoreError {
    fn from(message: String) -> Self {
        Self {
            message,
            changed: false,
        }
    }
}

impl From<&str> for RestoreError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

/// Returns the default backup directory, `backups/` under the app's config root.
pub fn default_backup_dir() -> Result<PathBuf, String> {
    let dir = crate::app_dirs::config_root()?.join("backups");
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}

/// Returns the effective backup directory (custom or default)
pub fn get_backup_dir(custom: &Option<String>) -> Result<PathBuf, String> {
    match custom {
        Some(p) if !p.is_empty() => {
            let dir = PathBuf::from(p);
            if !dir.exists() {
                fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            }
            Ok(dir)
        }
        _ => default_backup_dir(),
    }
}

/// Create a backup zip of the vault
pub fn create_backup(
    vault_path: &str,
    backup_dir: &Path,
    include_attachments: bool,
) -> Result<BackupEntry, String> {
    create_backup_of_kind(
        vault_path,
        backup_dir,
        include_attachments,
        BackupKind::Backup,
    )
}

/// Create the identifiable, full-vault recovery point required before incoming sync.
pub fn create_pre_sync_backup(vault_path: &str, backup_dir: &Path) -> Result<BackupEntry, String> {
    create_backup_of_kind(vault_path, backup_dir, true, BackupKind::PreSync)
}

fn create_backup_of_kind(
    vault_path: &str,
    backup_dir: &Path,
    include_attachments: bool,
    kind: BackupKind,
) -> Result<BackupEntry, String> {
    let vault = Path::new(vault_path);
    if !vault.is_dir() {
        return Err("Vault directory does not exist".to_string());
    }

    if !backup_dir.exists() {
        fs::create_dir_all(backup_dir).map_err(|e| e.to_string())?;
    }

    let now = Utc::now();
    let timestamp = now.format("%Y-%m-%dT%H-%M-%S").to_string();
    let filename = format!("{}-{timestamp}.zip", kind.prefix());
    let backup_path = backup_dir.join(&filename);

    let file = fs::File::create(&backup_path)
        .map_err(|e| format!("Failed to create backup file: {}", e))?;
    let mut zip = zip::ZipWriter::new(file);
    // zip 2 marked every entry as made on Unix; zip 9 marks them DOS on Windows unless told.
    // Unix keeps new backups exactly as earlier releases wrote them.
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(6))
        .system(zip::System::Unix);

    let attachments_dir = vault.join(METADATA_DIR).join("attachments");
    // The archive holds exactly what a restore touches: the top-level entries it replaces and
    // the metadata directory, each subtree whole, nested dot entries included.
    // Attachments are the sole configurable exclusion because they can dominate backup size.
    let walker = WalkDir::new(vault).into_iter().filter_entry(|entry| {
        let name = entry.file_name().to_string_lossy();
        let in_restore = entry.depth() != 1 || name == METADATA_DIR || restore_replaces(&name);
        in_restore && (include_attachments || entry.path() != attachments_dir)
    });

    for entry in walker {
        let entry =
            entry.map_err(|error| format!("Failed to traverse vault for backup: {error}"))?;
        let path = entry.path();
        let relative = path.strip_prefix(vault).map_err(|e| e.to_string())?;

        if path.is_dir() {
            let dir_name = format!("{}/", crate::vault::path::to_portable_string(relative));
            if dir_name != "/" {
                zip.add_directory(&dir_name, options)
                    .map_err(|e| e.to_string())?;
            }
        } else {
            let name = crate::vault::path::to_portable_string(relative);
            zip.start_file(&name, options).map_err(|e| e.to_string())?;
            let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer).map_err(|e| e.to_string())?;
            zip.write_all(&buffer).map_err(|e| e.to_string())?;
        }
    }

    zip.finish().map_err(|e| e.to_string())?;

    let meta = fs::metadata(&backup_path).map_err(|e| e.to_string())?;

    Ok(BackupEntry {
        filename: filename.clone(),
        path: backup_path.to_string_lossy().to_string(),
        size: meta.len(),
        created: now.to_rfc3339(),
        kind,
    })
}

/// List all backups in the backup directory
pub fn list_backups(backup_dir: &Path) -> Result<Vec<BackupEntry>, String> {
    if !backup_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<BackupEntry> = Vec::new();

    for entry in fs::read_dir(backup_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "zip") {
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let meta = fs::metadata(&path).map_err(|e| e.to_string())?;

            let named = BackupKind::ALL
                .into_iter()
                .find_map(|kind| Some((kind, filename.strip_prefix(kind.prefix())?)));
            let kind = named.map_or(BackupKind::Backup, |(kind, _)| kind);
            let stamped = named
                .and_then(|(_, rest)| rest.strip_prefix('-')?.strip_suffix(".zip"))
                .and_then(|stamp| {
                    chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%dT%H-%M-%S").ok()
                });
            let created = match stamped {
                Some(time) => time.and_utc().to_rfc3339(),
                None => meta
                    .modified()
                    .map(|t| chrono::DateTime::<Utc>::from(t).to_rfc3339())
                    .unwrap_or_default(),
            };

            entries.push(BackupEntry {
                filename,
                path: path.to_string_lossy().to_string(),
                size: meta.len(),
                created,
                kind,
            });
        }
    }

    // Sort newest first
    entries.sort_by(|a, b| b.created.cmp(&a.created));
    Ok(entries)
}

fn validated_backup_file(backup_dir: &Path, backup_path: &str) -> Result<PathBuf, String> {
    let backup_dir = fs::canonicalize(backup_dir).map_err(|error| error.to_string())?;
    let backup = fs::canonicalize(backup_path).map_err(|error| error.to_string())?;
    if backup.parent() != Some(backup_dir.as_path())
        || backup.extension().and_then(|extension| extension.to_str()) != Some("zip")
        || !backup.is_file()
    {
        return Err(
            "Backup path must point to a ZIP file in the configured backup directory".to_string(),
        );
    }
    Ok(backup)
}

/// Every restore artifact next to a vault starts with this: `stage-`, `rollback-`, `journal-`.
const RESTORE_PREFIX: &str = ".second-brain-restore-";
const METADATA_DIR: &str = ".helixnotes";

/// A point in a restore where the process can stop. Production never stops; tests inject a
/// failure or a death before each filesystem step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreStep {
    Stage,
    Journal,
    Displace,
    Publish,
}

/// How an injected step ends a restore: `Failed` takes the normal error path; `Died` returns
/// at once with nothing cleaned up, exactly as if the process had been killed there.
#[derive(Debug)]
enum Interrupt {
    Failed(String),
    #[cfg_attr(not(test), allow(dead_code))]
    Died,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum RestorePhase {
    Unpacking,
    Committing,
    Published,
    /// The vault is whole again (pre-restore, or kept as restored) and only cleanup remains.
    /// Recorded before cleanup, so a later open never re-runs undo or publish on a vault the
    /// user may have edited since it opened.
    Undone,
    Kept,
}

/// The durable record of one restore, written next to the vault before anything is staged,
/// so a restore interrupted at any point can be finished or undone on the next open (#142).
/// The name lists let recovery tell a published backup entry from a live one without
/// guessing from directory contents alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreJournal {
    vault: PathBuf,
    stage: PathBuf,
    rollback: PathBuf,
    phase: RestorePhase,
    /// Every top-level vault entry before the commit, except the metadata directory.
    #[serde(default)]
    present: Vec<String>,
    /// The subset of `present` moved aside: everything not starting with a dot.
    #[serde(default)]
    displaced: Vec<String>,
    /// Top-level staged entries, except dot entries (the metadata directory among them).
    #[serde(default)]
    staged: Vec<String>,
    /// Entries of the staged metadata directory.
    #[serde(default)]
    staged_metadata: Vec<String>,
    /// The subset of `staged_metadata` that existed in the live metadata and was moved aside.
    #[serde(default)]
    displaced_metadata: Vec<String>,
}

/// Which whole state an interrupted restore was brought to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveredRestore {
    /// The vault is exactly as it was before the restore started.
    Undone,
    /// The restore had finished publishing; the vault is kept as restored.
    KeptRestored,
}

#[derive(Debug, Default)]
pub struct RestoreRecovery {
    pub outcomes: Vec<RecoveredRestore>,
    /// Restore directories next to the vault that no journal accounts for. They may hold the
    /// only copy of a vault's notes, so they are reported and never deleted.
    pub strays: Vec<PathBuf>,
}

/// Restore a backup through a journaled, validated sibling staging area and rollback commit.
pub fn restore_backup(
    vault_path: &str,
    backup_dir: &Path,
    backup_path: &str,
) -> Result<(), RestoreError> {
    restore_backup_with(
        Path::new(vault_path),
        backup_dir,
        Path::new(backup_path),
        |_| Ok(()),
    )
}

fn restore_backup_with(
    vault: &Path,
    backup_dir: &Path,
    backup_path: &Path,
    mut step: impl FnMut(RestoreStep) -> Result<(), Interrupt>,
) -> Result<(), RestoreError> {
    let backup = validated_backup_file(backup_dir, &backup_path.to_string_lossy())?;
    let vault = dunce::canonicalize(vault).map_err(|e| format!("Cannot resolve the vault: {e}"))?;
    let parent = vault.parent().ok_or("Vault must have a parent directory")?;
    // Two journals for one vault could undo each other's work, so an unfinished restore is
    // always recovered, by reopening the vault, before another starts.
    for path in restore_entries(parent)
        .into_iter()
        .filter(|path| is_journal(path))
    {
        match read_journal(&path) {
            Ok(journal) if journal.vault == vault => {
                return Err("An unfinished restore of this vault must be recovered first. Reopen the vault, then restore again."
                    .into());
            }
            Ok(_) => {}
            Err(_) => {
                return Err(format!(
                    "The restore record {} cannot be read, so it is unclear whether another restore is unfinished. Move that file out of the folder, then restore again.",
                    path.display()
                )
                .into());
            }
        }
    }
    let nonce = uuid::Uuid::new_v4();
    let journal_path = parent.join(format!("{RESTORE_PREFIX}journal-{nonce}.json"));
    let mut journal = RestoreJournal {
        stage: parent.join(format!("{RESTORE_PREFIX}stage-{nonce}")),
        rollback: parent.join(format!("{RESTORE_PREFIX}rollback-{nonce}")),
        vault,
        phase: RestorePhase::Unpacking,
        present: Vec::new(),
        displaced: Vec::new(),
        staged: Vec::new(),
        staged_metadata: Vec::new(),
        displaced_metadata: Vec::new(),
    };
    write_journal(&journal_path, &journal)?;
    if let Err(error) = fs::create_dir(&journal.stage) {
        let _ = fs::remove_file(&journal_path);
        return Err(format!("Failed to create restore staging area: {error}").into());
    }

    let mut died = false;
    let unpacked = extract_and_validate_with(&backup, &journal.stage, |_| {
        match step(RestoreStep::Stage) {
            Ok(()) => Ok(()),
            Err(Interrupt::Failed(error)) => Err(error),
            Err(Interrupt::Died) => {
                died = true;
                Err("the process stopped".to_string())
            }
        }
    })
    .and_then(|()| plan_commit(&mut journal));
    if died {
        return Err("Restore interrupted while unpacking".into());
    }
    if let Err(error) = unpacked {
        abandon(&journal_path, &journal);
        return Err(error.into());
    }
    if let Err(error) = fs::create_dir(&journal.rollback) {
        abandon(&journal_path, &journal);
        return Err(format!("Failed to create restore rollback area: {error}").into());
    }

    let committed = (|| -> Result<(), Interrupt> {
        step(RestoreStep::Journal)?;
        journal.phase = RestorePhase::Committing;
        write_journal(&journal_path, &journal).map_err(Interrupt::Failed)?;
        commit(&journal, &mut step)?;
        // Some filesystems (Btrfs, XFS) can persist the published record before the renames
        // it describes; flush the renames first. `finish` also repairs a lost publish.
        persist_renames(&journal).map_err(Interrupt::Failed)?;
        step(RestoreStep::Journal)?;
        journal.phase = RestorePhase::Published;
        write_journal(&journal_path, &journal).map_err(Interrupt::Failed)?;
        step(RestoreStep::Journal)
    })();
    match committed {
        Ok(()) => {
            if let Err(error) = finish(&journal_path, &journal) {
                log::warn!(
                    "Restore committed; leftover restore files could not be removed: {error}"
                );
            }
            Ok(())
        }
        Err(Interrupt::Died) => Err("Restore interrupted while committing".into()),
        // Failing after the published record is durable changes nothing: the restore stands.
        Err(Interrupt::Failed(_)) if journal.phase == RestorePhase::Published => {
            finish(&journal_path, &journal).map_err(|error| RestoreError {
                message: error,
                changed: true,
            })
        }
        // The same journal-driven undo that runs on the next open, so a failure here and a
        // crash here end in the same whole state. If it cannot finish, the journal stays for
        // the next open to retry (#142).
        Err(Interrupt::Failed(error)) => match undo(&journal_path, &journal) {
            Ok(()) => Err(format!("Restore commit failed and was rolled back: {error}").into()),
            Err(undo_error) => Err(RestoreError {
                message: format!(
                    "Restore commit failed ({error}) and rollback was incomplete: {undo_error}"
                ),
                changed: true,
            }),
        },
    }
}

/// Whether a restore replaces the top-level vault entry `name` wholesale. Dot entries
/// (`.stfolder`, `.git`, `.obsidian`) are never displaced or published; the metadata directory
/// is merged entry by entry instead.
fn restore_replaces(name: &str) -> bool {
    !name.starts_with('.')
}

/// Record every name the commit will move, before it moves anything.
fn plan_commit(journal: &mut RestoreJournal) -> Result<(), String> {
    for name in entry_names(&journal.vault)? {
        if name == METADATA_DIR {
            continue;
        }
        if restore_replaces(&name) {
            journal.displaced.push(name.clone());
        }
        journal.present.push(name);
    }
    // Archives made before the #14 fix carry the contents of `.stfolder` and other dot folders;
    // those stay in the stage and are removed with it.
    for name in entry_names(&journal.stage)? {
        if restore_replaces(&name) {
            journal.staged.push(name);
        }
    }
    let staged_metadata = journal.stage.join(METADATA_DIR);
    if staged_metadata.symlink_metadata().is_ok() {
        journal.staged_metadata = entry_names(&staged_metadata)
            .map_err(|e| format!("Invalid staged vault metadata: {e}"))?;
    }
    let live_metadata = journal.vault.join(METADATA_DIR);
    journal.displaced_metadata = journal
        .staged_metadata
        .iter()
        .filter(|name| exists(&live_metadata.join(name)))
        .cloned()
        .collect();
    Ok(())
}

fn commit(
    journal: &RestoreJournal,
    step: &mut impl FnMut(RestoreStep) -> Result<(), Interrupt>,
) -> Result<(), Interrupt> {
    let failed = Interrupt::Failed;
    let vault_metadata = journal.vault.join(METADATA_DIR);
    let rollback_metadata = journal.rollback.join(METADATA_DIR);
    for name in &journal.displaced {
        step(RestoreStep::Displace)?;
        move_entry(&journal.vault.join(name), &journal.rollback.join(name)).map_err(failed)?;
    }
    if !journal.displaced_metadata.is_empty() {
        fs::create_dir_all(&rollback_metadata).map_err(|e| failed(e.to_string()))?;
    }
    for name in &journal.displaced_metadata {
        step(RestoreStep::Displace)?;
        move_entry(&vault_metadata.join(name), &rollback_metadata.join(name)).map_err(failed)?;
    }
    for name in &journal.staged {
        step(RestoreStep::Publish)?;
        move_entry(&journal.stage.join(name), &journal.vault.join(name)).map_err(failed)?;
    }
    if !journal.staged_metadata.is_empty() {
        fs::create_dir_all(&vault_metadata).map_err(|e| failed(e.to_string()))?;
    }
    let staged_metadata = journal.stage.join(METADATA_DIR);
    for name in &journal.staged_metadata {
        step(RestoreStep::Publish)?;
        move_entry(&staged_metadata.join(name), &vault_metadata.join(name)).map_err(failed)?;
    }
    Ok(())
}

/// Bring every vault named by a restore journal next to `vault` back to one whole state:
/// before the restore, or fully restored if it had finished publishing. Runs before anything
/// reads or syncs the vault. An error means the vault must not be opened.
pub fn recover_interrupted_restore(vault: &Path) -> Result<RestoreRecovery, String> {
    recover_with(vault, &mut || Ok(()))
}

fn recover_with(
    vault: &Path,
    step: &mut impl FnMut() -> Result<(), Interrupt>,
) -> Result<RestoreRecovery, String> {
    let vault = dunce::canonicalize(vault).map_err(|e| format!("Cannot resolve the vault: {e}"))?;
    let Some(parent) = vault.parent() else {
        return Ok(RestoreRecovery::default());
    };
    let mut recovery = RestoreRecovery::default();
    let mut accounted = HashSet::new();
    let entries = restore_entries(parent);
    for path in &entries {
        if !is_journal(path) {
            continue;
        }
        let journal = read_journal(path).map_err(|error| {
            format!(
                "The restore record {} cannot be read, so the vault cannot be recovered safely: {error}. Nothing was changed; move that file out of the folder only after the vault looks right.",
                path.display()
            )
        })?;
        accounted.insert(journal.stage.clone());
        accounted.insert(journal.rollback.clone());
        if journal.vault != vault {
            continue;
        }
        let owned = |dir: &Path| {
            dir.parent() == Some(parent)
                && dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(RESTORE_PREFIX))
        };
        if !owned(&journal.stage) || !owned(&journal.rollback) {
            return Err(format!(
                "The restore record {} names folders outside {}, so it cannot be trusted. Nothing was changed; remove the record once the vault looks right.",
                path.display(),
                parent.display()
            ));
        }
        let (recovered, interrupted) = match journal.phase {
            RestorePhase::Published => (
                complete_publish(&journal).map(|()| RecoveredRestore::KeptRestored),
                true,
            ),
            RestorePhase::Unpacking | RestorePhase::Committing => (
                undo_with(&journal, step).map(|()| RecoveredRestore::Undone),
                true,
            ),
            // Settled by an earlier open; only its cleanup was left.
            RestorePhase::Kept => (Ok(RecoveredRestore::KeptRestored), false),
            RestorePhase::Undone => (Ok(RecoveredRestore::Undone), false),
        };
        let outcome = recovered.map_err(|error| {
            format!(
                "{error}. Nothing was deleted: until this is resolved, some or all of the vault's original entries may be in {} and the restored copies in {}. Move or rename anything in the way, then reopen the vault.",
                journal.rollback.display(),
                journal.stage.display()
            )
        })?;
        // Persist the whole state before allowing anything to open or sync the vault. Cleanup
        // alone may wait when a scanner holds a handle; the settled journal makes that safe.
        settle(path, &journal, outcome).map_err(|error| {
            format!(
                "The recovered vault could not be recorded durably: {error}. Nothing was deleted; the vault is at {}, restored copies may be in {}, and displaced originals may be in {}. Reopen the vault to retry.",
                journal.vault.display(),
                journal.stage.display(),
                journal.rollback.display()
            )
        })?;
        if let Err(error) = clean_up(path, &journal, outcome) {
            log::warn!("Recovered restore left files behind until the next open: {error}");
        }
        if !interrupted {
            continue;
        }
        log::warn!(
            "Recovered an interrupted restore of {}: {outcome:?}",
            vault.display()
        );
        recovery.outcomes.push(outcome);
    }
    for path in entries {
        if !is_journal(&path) && !accounted.contains(&path) && exists(&path) {
            recovery.strays.push(path);
        }
    }
    Ok(recovery)
}

/// Return the vault to its pre-restore state. Idempotent: every decision is made from the
/// journal's name lists plus where each entry is now, so a crash during `undo` is repaired by
/// running it again. The only recursive delete is the journal-named stage, which holds backup
/// copies alone: published entries are moved back into it first.
fn undo(journal_path: &Path, journal: &RestoreJournal) -> Result<(), String> {
    undo_with(journal, &mut || Ok(()))?;
    settle_and_clean_up(journal_path, journal, RecoveredRestore::Undone)
}

fn undo_with(
    journal: &RestoreJournal,
    step: &mut impl FnMut() -> Result<(), Interrupt>,
) -> Result<(), String> {
    let mut interruptible_move = |from: &Path, to: &Path| -> Result<(), String> {
        step().map_err(|_| "recovery interrupted".to_string())?;
        move_entry(from, to)
    };
    let vault_metadata = journal.vault.join(METADATA_DIR);
    let stage_metadata = journal.stage.join(METADATA_DIR);
    let rollback_metadata = journal.rollback.join(METADATA_DIR);
    for name in &journal.staged {
        let live = journal.vault.join(name);
        let published = !journal.present.contains(name) || exists(&journal.rollback.join(name));
        if exists(&live) && published {
            fs::create_dir_all(&journal.stage).map_err(|e| e.to_string())?;
            interruptible_move(&live, &journal.stage.join(name))?;
        }
    }
    for name in &journal.displaced {
        let aside = journal.rollback.join(name);
        if exists(&aside) {
            interruptible_move(&aside, &journal.vault.join(name))?;
        }
    }
    for name in &journal.staged_metadata {
        let live = vault_metadata.join(name);
        let published =
            !journal.displaced_metadata.contains(name) || exists(&rollback_metadata.join(name));
        if exists(&live) && published {
            fs::create_dir_all(&stage_metadata).map_err(|e| e.to_string())?;
            interruptible_move(&live, &stage_metadata.join(name))?;
        }
    }
    for name in &journal.displaced_metadata {
        let aside = rollback_metadata.join(name);
        if exists(&aside) {
            fs::create_dir_all(&vault_metadata).map_err(|e| e.to_string())?;
            interruptible_move(&aside, &vault_metadata.join(name))?;
        }
    }
    let missing: Vec<&String> = journal
        .displaced
        .iter()
        .filter(|name| !exists(&journal.vault.join(name)))
        .chain(
            journal
                .displaced_metadata
                .iter()
                .filter(|name| !exists(&vault_metadata.join(name))),
        )
        .collect();
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|name| name.as_str()).collect();
        return Err(format!(
            "{} could not be found in the vault or the rollback folder",
            names.join(", ")
        ));
    }
    Ok(())
}

/// Make sure every entry of a restore recorded as published is in the vault. A publish rename
/// a filesystem did not persist is finished from the stage rather than lost with it.
fn complete_publish(journal: &RestoreJournal) -> Result<(), String> {
    let vault_metadata = journal.vault.join(METADATA_DIR);
    let stage_metadata = journal.stage.join(METADATA_DIR);
    let pairs = journal
        .staged
        .iter()
        .map(|name| (journal.stage.join(name), journal.vault.join(name)))
        .chain(
            journal
                .staged_metadata
                .iter()
                .map(|name| (stage_metadata.join(name), vault_metadata.join(name))),
        );
    for (staged, live) in pairs {
        if !exists(&live) {
            if !exists(&staged) {
                return Err(format!("{} is missing from the restore", live.display()));
            }
            if let Some(parent) = live.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            move_entry(&staged, &live)?;
        }
    }
    Ok(())
}

/// Drop the stage, the rollback, and then the journal, once the vault is whole. For a kept
/// restore the rollback holds the replaced originals; for an undone one it is empty.
fn clean_up(
    journal_path: &Path,
    journal: &RestoreJournal,
    outcome: RecoveredRestore,
) -> Result<(), String> {
    let parent = journal.vault.parent();
    remove_restore_dir(&journal.stage, parent)?;
    match outcome {
        RecoveredRestore::KeptRestored => remove_restore_dir(&journal.rollback, parent)?,
        RecoveredRestore::Undone => {
            remove_empty_dir(&journal.rollback.join(METADATA_DIR))?;
            remove_empty_dir(&journal.rollback)?;
        }
    }
    remove_journal(journal_path)
}

/// After a successful commit, before the published record: the vault reaches that state only
/// through `complete_publish` and `clean_up`, like a recovered one.
fn finish(journal_path: &Path, journal: &RestoreJournal) -> Result<(), String> {
    complete_publish(journal)?;
    settle_and_clean_up(journal_path, journal, RecoveredRestore::KeptRestored)
}

/// Record that the vault is whole, then clean up. From here on a failed cleanup is retried on
/// the next open as cleanup alone.
fn settle_and_clean_up(
    journal_path: &Path,
    journal: &RestoreJournal,
    outcome: RecoveredRestore,
) -> Result<(), String> {
    settle(journal_path, journal, outcome)?;
    clean_up(journal_path, journal, outcome)
}

fn settle(
    journal_path: &Path,
    journal: &RestoreJournal,
    outcome: RecoveredRestore,
) -> Result<(), String> {
    let mut settled = journal.clone();
    settled.phase = match outcome {
        RecoveredRestore::Undone => RestorePhase::Undone,
        RecoveredRestore::KeptRestored => RestorePhase::Kept,
    };
    if settled.phase != journal.phase {
        // The settled record must never reach disk before the renames that made the vault
        // whole. Otherwise a power loss could leave recovery data to be cleaned up as stale.
        persist_renames(journal)?;
        write_journal(journal_path, &settled)?;
    }
    Ok(())
}

/// Clean up a restore that never reached the commit; the vault was not touched.
fn abandon(journal_path: &Path, journal: &RestoreJournal) {
    if let Err(error) = undo(journal_path, journal) {
        log::warn!("Could not clean up an abandoned restore: {error}");
    }
}

fn write_journal(path: &Path, journal: &RestoreJournal) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(journal).map_err(|e| e.to_string())?;
    crate::durable::replace(path, &bytes, crate::durable::Mode::Private)
        .map_err(|e| format!("Failed to record restore progress: {e}"))
}

fn read_journal(path: &Path) -> Result<RestoreJournal, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn remove_journal(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot remove {}: {error}", path.display())),
    }
}

/// Recursively delete a journal-named stage or rollback directory next to its vault, and
/// nothing else.
fn remove_restore_dir(path: &Path, vault_parent: Option<&Path>) -> Result<(), String> {
    let named = path.parent() == vault_parent
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(RESTORE_PREFIX));
    if !named {
        return Err(format!("Refusing to delete {}", path.display()));
    }
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot remove {}: {error}", path.display())),
    }
}

fn remove_empty_dir(path: &Path) -> Result<(), String> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot remove {}: {error}", path.display())),
    }
}

/// Restore artifacts next to a vault, sorted. An unreadable directory yields none: a restore
/// writes into this same directory, so none can have run there.
fn restore_entries(parent: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(parent)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(RESTORE_PREFIX))
        })
        .collect();
    entries.sort();
    entries
}

fn is_journal(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(&format!("{RESTORE_PREFIX}journal-")) && name.ends_with(".json")
        })
}

/// Flush the directory entries the commit renamed. Windows has no directory fsync; NTFS
/// orders these metadata writes ahead of the write-through journal replace.
fn persist_renames(journal: &RestoreJournal) -> Result<(), String> {
    #[cfg(unix)]
    for dir in [
        &journal.vault,
        &journal.vault.join(METADATA_DIR),
        &journal.rollback,
        &journal.rollback.join(METADATA_DIR),
        &journal.stage,
        &journal.stage.join(METADATA_DIR),
    ] {
        if dir.is_dir() {
            fs::File::open(dir)
                .and_then(|handle| handle.sync_all())
                .map_err(|e| format!("Cannot flush {}: {e}", dir.display()))?;
        }
    }
    #[cfg(not(unix))]
    let _ = journal;
    Ok(())
}

fn entry_names(dir: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        names.push(
            name.into_string()
                .map_err(|name| format!("Cannot restore a non-UTF-8 name: {name:?}"))?,
        );
    }
    names.sort();
    Ok(names)
}

fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// Rename without ever overwriting. Windows scanners (Defender, the search indexer,
/// Syncthing hashing) hold short-lived handles that make a rename fail, so it retries briefly.
fn move_entry(from: &Path, to: &Path) -> Result<(), String> {
    if exists(to) {
        return Err(format!("{} already exists", to.display()));
    }
    let mut retries = if cfg!(windows) { 5 } else { 0 };
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(_) if retries > 0 => {
                retries -= 1;
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
            Err(error) => {
                return Err(format!(
                    "Cannot move {} to {}: {error}",
                    from.display(),
                    to.display()
                ))
            }
        }
    }
}

/// An entry's name for messages. zip 9 decodes a name only when asked, and decoding can fail;
/// the raw bytes are then shown lossily, so reporting a refusal can never fail on the name.
fn entry_label<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> String {
    match entry.name() {
        Ok(name) => name.into_owned(),
        Err(_) => String::from_utf8_lossy(entry.name_raw()).into_owned(),
    }
}

/// Where an entry goes in the stage, read with this OS's own path rules, and whether it is a
/// directory. `None` when the name is not a plain relative path.
///
/// Backups store names as `to_portable_string` gives them. On Fedora a backslash is an ordinary
/// file name character, so `Resources/a\b.md`, and even `Resources/a\..\b.md`, is one file there
/// and must come back as one. On Windows the backslash separates components, so the second name
/// holds a `..` part there. zip 9 reads every name with Windows rules, so its paths are not used.
///
/// Before any component is read, this refuses on every OS a NUL and a name that Windows reads as
/// rooted or on a drive: `/abs.md`, `\abs.md`, `\\server\share\x.md`, `C:abs.md`. Then it refuses
/// any `..` component, read with this OS's own path rules.
fn native_entry(name: &str) -> Option<(PathBuf, bool)> {
    let windows_rooted = name.starts_with(['/', '\\'])
        || matches!(name.as_bytes(), [drive, b':', ..] if drive.is_ascii_alphabetic());
    if name.contains('\0') || windows_rooted {
        return None;
    }
    let mut path = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    // On Windows a part such as `c:x` reads as a drive when pushed, and replaces the path.
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }
    // A trailing separator marks a directory, so on Fedora a file named `x\` stays a file.
    Some((path, name.ends_with(std::path::is_separator)))
}

fn extract_and_validate_with(
    backup: &Path,
    stage: &Path,
    mut before_file_write: impl FnMut(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let file = fs::File::open(backup).map_err(|e| format!("Failed to open backup: {e}"))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("Invalid backup archive: {e}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "Backup contains too many entries (maximum {MAX_ARCHIVE_ENTRIES})"
        ));
    }
    let mut total_bytes = 0_u64;
    let mut paths = HashSet::new();
    // The second pass writes exactly the paths this pass accepted.
    let mut targets = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| format!("Unreadable backup entry: {e}"))?;
        let (relative, is_dir) = entry
            .name()
            .ok()
            .and_then(|name| native_entry(&name))
            .ok_or_else(|| format!("Unsafe path in backup: {}", entry_label(&entry)))?;
        if relative.as_os_str().is_empty() || !paths.insert(relative.clone()) {
            return Err(format!(
                "Duplicate or empty path in backup: {}",
                entry_label(&entry)
            ));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!(
                "Symbolic links are not allowed in backups: {}",
                entry_label(&entry)
            ));
        }
        total_bytes = total_bytes
            .checked_add(entry.size())
            .ok_or("Backup expanded size overflow")?;
        if total_bytes > MAX_ARCHIVE_BYTES {
            return Err(format!("Backup expands beyond {MAX_ARCHIVE_BYTES} bytes"));
        }
        targets.push((relative, is_dir));
    }

    // Extraction is deliberately a second pass: the complete archive shape and declared
    // expanded size are accepted before a single potentially large payload reaches disk.
    for (index, (relative, is_dir)) in targets.iter().enumerate() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("Unreadable backup entry: {e}"))?;
        let destination = stage.join(relative);
        if *is_dir {
            fs::create_dir_all(&destination)
                .map_err(|e| format!("Cannot stage backup directory: {e}"))?;
        } else {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("Cannot stage backup path: {e}"))?;
            }
            before_file_write(&destination)?;
            let mut output = fs::File::create(&destination)
                .map_err(|e| format!("Cannot stage backup file: {e}"))?;
            std::io::copy(&mut entry, &mut output)
                .map_err(|e| format!("Cannot extract backup entry {}: {e}", entry_label(&entry)))?;
            output
                .sync_all()
                .map_err(|e| format!("Cannot flush staged backup file: {e}"))?;
        }
    }
    Ok(())
}

/// Delete a single backup file
pub fn delete_backup(backup_dir: &Path, backup_path: &str) -> Result<(), String> {
    let path = validated_backup_file(backup_dir, backup_path)?;
    fs::remove_file(path).map_err(|error| error.to_string())
}

/// Remove old backups keeping only the newest `max_count`. Pre-sync backups can come every few
/// minutes, so the newest scheduled or manual backup is always kept, in place of the oldest one
/// that would otherwise stay: the pool never holds only minutes-old recovery points (#14).
pub fn cleanup_old_backups(backup_dir: &Path, max_count: u32) -> Result<(), String> {
    // backups are already sorted newest-first
    let mut kept = list_backups(backup_dir)?;
    if kept.len() as u32 <= max_count {
        return Ok(());
    }
    let mut to_remove = kept.split_off(max_count as usize);
    if !kept.iter().any(|entry| entry.kind == BackupKind::Backup) {
        let newest_backup = to_remove
            .iter()
            .position(|entry| entry.kind == BackupKind::Backup);
        if let (Some(index), Some(oldest_kept)) = (newest_backup, kept.pop()) {
            to_remove[index] = oldest_kept;
        }
    }
    for entry in to_remove {
        delete_backup(backup_dir, &entry.path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_old_backups, create_backup, create_pre_sync_backup, delete_backup,
        extract_and_validate_with, list_backups, recover_interrupted_restore, recover_with,
        restore_backup, restore_backup_with, Interrupt, RecoveredRestore, RestoreStep,
    };
    use crate::types::BackupKind;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use uuid::Uuid;
    use zip::{write::SimpleFileOptions, ZipArchive};

    /// Every file under `dir`, relative path to bytes: the whole observable state of a vault.
    fn tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        walkdir::WalkDir::new(dir)
            .into_iter()
            .map(Result::unwrap)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                let relative = entry.path().strip_prefix(dir).unwrap().to_path_buf();
                (relative, fs::read(entry.path()).unwrap())
            })
            .collect()
    }

    /// Restore bookkeeping left next to the vault: stage, rollback, and journal entries.
    fn restore_leftovers(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".second-brain-restore-"))
            .collect();
        names.sort();
        names
    }

    /// A live vault and a backup that differ in notes, folders, and metadata, so a torn
    /// mix of the two is distinguishable from either whole state.
    fn restore_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("second-brain-{label}-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        for (path, body) in [
            ("Projects/a.md", "live a"),
            ("Areas/b.md", "live b"),
            ("live-only.md", "only live"),
            (".helixnotes/vault_id", "live-id"),
            (".helixnotes/attachments/keep.bin", "attachment"),
        ] {
            let path = vault.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        fs::create_dir_all(&backups).unwrap();
        let backup = backups.join("snapshot.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&backup).unwrap());
        for (path, body) in [
            ("Projects/a.md", "restored a"),
            ("Resources/c.md", "restored c"),
            (".helixnotes/vault_id", "backup-id"),
            (".helixnotes/history/c/1.md", "history"),
        ] {
            writer
                .start_file(path, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        let root = dunce::canonicalize(root).unwrap();
        (
            root.clone(),
            root.join("vault"),
            root.join("backups"),
            root.join("backups/snapshot.zip"),
        )
    }

    #[test]
    fn a_restore_killed_at_any_step_recovers_to_one_whole_vault() {
        let (root, vault, backups, backup) = restore_fixture("kill-any-step");
        let before = tree(&vault);
        let mut steps = 0;
        restore_backup_with(&vault, &backups, &backup, |_| {
            steps += 1;
            Ok(())
        })
        .unwrap();
        let restored = tree(&vault);
        assert_ne!(before, restored);
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();

        let mut outcomes = Vec::new();
        for kill_at in 0..steps {
            let (root, vault, backups, backup) = restore_fixture("kill-any-step");
            let mut step = 0;
            let result = restore_backup_with(&vault, &backups, &backup, |_| {
                step += 1;
                if step - 1 == kill_at {
                    Err(Interrupt::Died)
                } else {
                    Ok(())
                }
            });
            assert!(
                result.is_err(),
                "kill at step {kill_at} must stop the restore"
            );

            let recovery = recover_interrupted_restore(&vault).unwrap();
            let after = tree(&vault);
            assert!(
                after == before || after == restored,
                "kill at step {kill_at} left a torn vault: {:?}",
                after.keys().collect::<Vec<_>>()
            );
            assert_eq!(recovery.outcomes.len(), 1, "kill at step {kill_at}");
            let expected = if after == restored {
                RecoveredRestore::KeptRestored
            } else {
                RecoveredRestore::Undone
            };
            assert_eq!(recovery.outcomes[0], expected, "kill at step {kill_at}");
            outcomes.push(expected);
            assert!(recovery.strays.is_empty(), "kill at step {kill_at}");
            assert_eq!(
                restore_leftovers(&root),
                Vec::<String>::new(),
                "kill at step {kill_at} left restore folders behind"
            );
            fs::remove_dir_all(root).unwrap();
        }
        assert!(outcomes.contains(&RecoveredRestore::Undone));
        assert!(
            outcomes.contains(&RecoveredRestore::KeptRestored),
            "some kill point must land after the restore is recorded as published"
        );
    }

    #[test]
    fn delete_backup_rejects_files_outside_backup_directory() {
        let root = std::env::temp_dir().join(format!("helixnotes-backup-test-{}", Uuid::new_v4()));
        let backup_dir = root.join("backups");
        let outside = root.join("outside.zip");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(&outside, "must survive").unwrap();

        let result = delete_backup(&backup_dir, &outside.to_string_lossy());

        assert!(result.is_err());
        assert!(outside.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_late_entry_leaves_the_live_vault_untouched() {
        let root = std::env::temp_dir().join(format!("helixnotes-restore-test-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backup_dir = root.join("backups");
        let backup = backup_dir.join("corrupt.zip");
        let live_note = vault.join("Projects/live.md");
        let live_attachment = vault.join(".helixnotes/attachments/live.bin");

        fs::create_dir_all(live_note.parent().unwrap()).unwrap();
        fs::create_dir_all(live_attachment.parent().unwrap()).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(&live_note, "original note").unwrap();
        fs::write(&live_attachment, b"original attachment").unwrap();

        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let mut writer = zip::ZipWriter::new(fs::File::create(&backup).unwrap());
        writer.start_file("Projects/restored.md", options).unwrap();
        writer.write_all(b"replacement note").unwrap();
        writer.start_file("Resources/corrupt.md", options).unwrap();
        writer
            .write_all(b"this entry will fail its CRC check")
            .unwrap();
        writer.finish().unwrap();

        let data_start = {
            let file = fs::File::open(&backup).unwrap();
            let mut archive = ZipArchive::new(file).unwrap();
            let start = archive
                .by_name("Resources/corrupt.md")
                .unwrap()
                .data_start()
                .expect("an opened entry knows where its data starts");
            start
        };
        let mut file = fs::OpenOptions::new().write(true).open(&backup).unwrap();
        file.seek(SeekFrom::Start(data_start)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();

        // The damaged byte must sit in the intended entry: it alone fails its CRC check.
        let mut archive = ZipArchive::new(fs::File::open(&backup).unwrap()).unwrap();
        let mut body = Vec::new();
        let error = archive
            .by_name("Resources/corrupt.md")
            .unwrap()
            .read_to_end(&mut body)
            .expect_err("the damaged entry must fail its CRC check");
        assert!(error.to_string().contains("checksum"), "{error}");
        let mut body = Vec::new();
        archive
            .by_name("Projects/restored.md")
            .unwrap()
            .read_to_end(&mut body)
            .unwrap();
        assert_eq!(body, b"replacement note");
        drop(archive);

        let result = restore_backup(
            vault.to_str().unwrap(),
            &backup_dir,
            backup.to_str().unwrap(),
        );

        assert!(result.is_err(), "the corrupt archive must be rejected");
        assert_eq!(fs::read_to_string(&live_note).unwrap(), "original note");
        assert_eq!(fs::read(&live_attachment).unwrap(), b"original attachment");
        assert!(!vault.join("Projects/restored.md").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_archive_leaves_the_live_vault_untouched() {
        let root =
            std::env::temp_dir().join(format!("second-brain-interrupted-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        let backup = backups.join("interrupted.zip");
        fs::create_dir_all(&vault).unwrap();
        fs::create_dir_all(&backups).unwrap();
        fs::write(vault.join("live.md"), "live").unwrap();
        let mut writer = zip::ZipWriter::new(fs::File::create(&backup).unwrap());
        writer
            .start_file("restored.md", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"restored").unwrap();
        writer.finish().unwrap();
        let length = fs::metadata(&backup).unwrap().len();
        fs::OpenOptions::new()
            .write(true)
            .open(&backup)
            .unwrap()
            .set_len(length / 2)
            .unwrap();

        assert!(
            restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap()).is_err()
        );
        assert_eq!(fs::read_to_string(vault.join("live.md")).unwrap(), "live");
        assert!(!vault.join("restored.md").exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// Write `backup`: one ordinary note first, then whatever `write` adds.
    fn write_archive(backup: &Path, write: impl FnOnce(&mut zip::ZipWriter<fs::File>)) {
        let mut writer = zip::ZipWriter::new(fs::File::create(backup).unwrap());
        writer
            .start_file("Projects/restored.md", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"restored").unwrap();
        write(&mut writer);
        writer.finish().unwrap();
    }

    /// Add each of `entries` to `writer`, as a file holding its bytes.
    fn write_entries(writer: &mut zip::ZipWriter<fs::File>, entries: &[(&str, &[u8])]) {
        for (name, body) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body).unwrap();
        }
    }

    /// A restore of the archive `write` builds must be refused for `reason`, report no change,
    /// and leave the live vault and everything beside it exactly as they were.
    fn assert_refused_unchanged(
        case: &str,
        reason: &str,
        write: impl FnOnce(&mut zip::ZipWriter<fs::File>),
    ) {
        assert_backup_refused_unchanged(case, reason, |backup| write_archive(backup, write));
    }

    /// As `assert_refused_unchanged`, for the archive `place` puts at the path it is given.
    fn assert_backup_refused_unchanged(case: &str, reason: &str, place: impl FnOnce(&Path)) {
        let (root, vault, backups, _) = restore_fixture("hostile-archive");
        let backup = backups.join("hostile.zip");
        place(&backup);
        let before = tree(&vault);
        let everything_before = tree(&root);

        let error = restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap())
            .expect_err(case);

        assert!(error.message.contains(reason), "{case}: {}", error.message);
        assert!(!error.changed, "{case}");
        assert_eq!(tree(&vault), before, "{case}");
        assert_eq!(tree(&root), everything_before, "{case}");
        assert_eq!(restore_leftovers(&root), Vec::<String>::new(), "{case}");
        fs::remove_dir_all(root).unwrap();
    }

    /// A restore of an archive holding `entries`, after one ordinary note, must succeed and give
    /// each entry back, byte for byte, at the path `PathBuf::from` reads from its name on this OS.
    fn assert_restored_at_native_paths(case: &str, entries: &[(&str, &[u8])]) {
        let (root, vault, backups, _) = restore_fixture("native-names");
        let backup = backups.join("native.zip");
        write_archive(&backup, |writer| write_entries(writer, entries));

        restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap())
            .unwrap_or_else(|error| panic!("{case}: {}", error.message));

        // The live metadata stays, since the archive has none.
        let expected: BTreeMap<PathBuf, Vec<u8>> = [
            ("Projects/restored.md", &b"restored"[..]),
            (".helixnotes/vault_id", b"live-id"),
            (".helixnotes/attachments/keep.bin", b"attachment"),
        ]
        .iter()
        .chain(entries)
        .map(|(path, body)| (PathBuf::from(path), body.to_vec()))
        .collect();
        assert_eq!(tree(&vault), expected, "{case}");
        assert_eq!(restore_leftovers(&root), Vec::<String>::new(), "{case}");
        fs::remove_dir_all(root).unwrap();
    }

    /// Forward-slash `..`, rooted, drive, UNC and NUL names are refused on every OS.
    #[test]
    fn rooted_drive_and_parent_names_are_refused_before_any_live_change() {
        for name in [
            "../escape.md",
            "a/../../escape.md",
            "a/../b.md",
            "/abs.md",
            "\\abs.md",
            "C:\\abs.md",
            "C:abs.md",
            "\\\\server\\share\\abs.md",
            "//server/share/abs.md",
            "nul\0.md",
        ] {
            assert_refused_unchanged(&format!("{name:?}"), "Unsafe path in backup", |writer| {
                writer
                    .start_file(name, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(b"hostile").unwrap();
            });
        }
    }

    /// `..` between backslashes is a parent on Windows, so each archive is refused there, whole.
    /// On Fedora each name is one ordinary file and comes back at that name, byte for byte.
    #[test]
    fn backslash_parents_are_refused_on_windows_and_kept_on_fedora() {
        for (name, body) in [
            (
                "Resources/a\\..\\b.md",
                &b"one Fedora file named a\\..\\b.md"[..],
            ),
            ("Resources/..\\b.md", b"one Fedora file named ..\\b.md"),
            (
                "a\\..\\..\\escape.md",
                b"one Fedora file, an escape on Windows",
            ),
        ] {
            if cfg!(windows) {
                assert_refused_unchanged(name, "Unsafe path in backup", |writer| {
                    write_entries(writer, &[(name, body)])
                });
            } else {
                assert_restored_at_native_paths(name, &[(name, body)]);
            }
        }
        // A restore never publishes a top-level name that starts with a dot, so on Fedora this
        // one would not reach the vault; on Windows it escapes, and is refused.
        if cfg!(windows) {
            assert_refused_unchanged("..\\escape.md", "Unsafe path in backup", |writer| {
                write_entries(writer, &[("..\\escape.md", b"escape")])
            });
        }
    }

    /// On Windows `c:x.md` names a drive even inside a path, and pushing it would replace the
    /// stage path. On Fedora it is an ordinary file name.
    #[cfg(windows)]
    #[test]
    fn a_drive_inside_a_name_is_refused_on_windows() {
        assert_refused_unchanged("Projects/c:x.md", "Unsafe path in backup", |writer| {
            writer
                .start_file("Projects/c:x.md", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"hostile").unwrap();
        });
    }

    /// On Fedora `a\b.md` is one file, so it comes back as that file, byte for byte. On Windows
    /// the backslash separates `a` from `b.md`, as zip 2.6.1 restored it there.
    #[test]
    fn a_backslash_name_restores_at_its_native_path() {
        let (root, vault, backups, _) = restore_fixture("backslash-name");
        let backup = backups.join("backslash.zip");
        write_archive(&backup, |writer| {
            for (name, body) in [("a\\b.md", "top level"), ("Resources/c\\d.md", "nested")] {
                writer
                    .start_file(name, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(body.as_bytes()).unwrap();
            }
        });

        restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap()).unwrap();

        let (top, nested) = if cfg!(windows) {
            (
                vault.join("a").join("b.md"),
                vault.join("Resources").join("c").join("d.md"),
            )
        } else {
            (
                vault.join("a\\b.md"),
                vault.join("Resources").join("c\\d.md"),
            )
        };
        assert_eq!(fs::read(top).unwrap(), b"top level");
        assert_eq!(fs::read(nested).unwrap(), b"nested");
        // `PathBuf::from` reads each name as this OS does: one file on Fedora, two parts on
        // Windows. The live metadata stays, since the archive has none.
        let expected: BTreeMap<PathBuf, Vec<u8>> = [
            ("Projects/restored.md", "restored"),
            ("a\\b.md", "top level"),
            ("Resources/c\\d.md", "nested"),
            (".helixnotes/vault_id", "live-id"),
            (".helixnotes/attachments/keep.bin", "attachment"),
        ]
        .into_iter()
        .map(|(path, body)| (PathBuf::from(path), body.as_bytes().to_vec()))
        .collect();
        assert_eq!(tree(&vault), expected);
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinks_and_two_entries_for_one_path_are_refused_before_any_live_change() {
        assert_refused_unchanged("symlink", "Symbolic links are not allowed", |writer| {
            writer
                .add_symlink(
                    "Projects/link.md",
                    "../../outside.md",
                    SimpleFileOptions::default(),
                )
                .unwrap();
        });
        assert_refused_unchanged("b.md and b.md/", "Duplicate or empty path", |writer| {
            writer
                .start_file("b.md", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"file").unwrap();
            writer
                .add_directory("b.md/", SimpleFileOptions::default())
                .unwrap();
        });
    }

    #[test]
    fn backup_includes_vault_metadata_but_can_exclude_attachments() {
        let root =
            std::env::temp_dir().join(format!("second-brain-backup-scope-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        fs::create_dir_all(vault.join(".helixnotes/history/note-id")).unwrap();
        fs::create_dir_all(vault.join(".helixnotes/attachments")).unwrap();
        fs::write(vault.join("note.md"), "note").unwrap();
        fs::write(vault.join(".helixnotes/history/note-id/old.md"), "old").unwrap();
        fs::write(vault.join(".helixnotes/vault_id"), "vault-id").unwrap();
        fs::write(vault.join(".helixnotes/attachments/image.png"), "image").unwrap();

        let entry = create_backup(vault.to_str().unwrap(), &backups, false).unwrap();
        let mut archive = ZipArchive::new(fs::File::open(entry.path).unwrap()).unwrap();
        assert!(archive.by_name("note.md").is_ok());
        assert!(archive
            .by_name(".helixnotes/history/note-id/old.md")
            .is_ok());
        assert!(archive.by_name(".helixnotes/vault_id").is_ok());
        assert!(archive
            .by_name(".helixnotes/attachments/image.png")
            .is_err());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pre_sync_backup_is_identifiable_and_always_contains_attachments() {
        let root = std::env::temp_dir().join(format!("second-brain-pre-sync-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        fs::create_dir_all(vault.join(".helixnotes/attachments")).unwrap();
        fs::write(vault.join("note.md"), "safe point").unwrap();
        fs::write(vault.join(".helixnotes/attachments/file.bin"), "binary").unwrap();
        let entry = create_pre_sync_backup(vault.to_str().unwrap(), &backups).unwrap();
        assert!(entry.filename.starts_with("helixnotes-pre-sync-"));
        assert_eq!(entry.kind, BackupKind::PreSync);
        let mut archive = ZipArchive::new(fs::File::open(entry.path).unwrap()).unwrap();
        assert!(archive.by_name("note.md").is_ok());
        assert!(archive.by_name(".helixnotes/attachments/file.bin").is_ok());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_preserves_attachments_when_the_backup_omits_them() {
        let root =
            std::env::temp_dir().join(format!("second-brain-restore-scope-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        fs::create_dir_all(vault.join(".helixnotes/attachments")).unwrap();
        fs::create_dir_all(&backups).unwrap();
        fs::write(vault.join("old.md"), "old").unwrap();
        fs::write(vault.join(".helixnotes/attachments/kept.bin"), "kept").unwrap();
        let backup = backups.join("without-attachments.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&backup).unwrap());
        writer
            .start_file("new.md", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"new").unwrap();
        writer.finish().unwrap();

        restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap()).unwrap();

        assert!(!vault.join("old.md").exists());
        assert_eq!(fs::read_to_string(vault.join("new.md")).unwrap(), "new");
        assert_eq!(
            fs::read(vault.join(".helixnotes/attachments/kept.bin")).unwrap(),
            b"kept"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staging_write_failure_never_reaches_the_live_vault() {
        let root =
            std::env::temp_dir().join(format!("second-brain-stage-failure-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        fs::create_dir_all(&vault).unwrap();
        fs::create_dir_all(&backups).unwrap();
        fs::write(vault.join("live.md"), "live").unwrap();
        let backup = backups.join("collision.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&backup).unwrap());
        writer
            .start_file("collision", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"file").unwrap();
        writer
            .start_file("collision/child.md", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"cannot have a file parent").unwrap();
        writer.finish().unwrap();

        assert!(
            restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap()).is_err()
        );
        assert_eq!(fs::read_to_string(vault.join("live.md")).unwrap(), "live");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_injected_disk_write_failure_stops_staging() {
        let root =
            std::env::temp_dir().join(format!("second-brain-disk-failure-{}", Uuid::new_v4()));
        let stage = root.join("stage");
        let backup = root.join("backup.zip");
        fs::create_dir_all(&stage).unwrap();
        let mut writer = zip::ZipWriter::new(fs::File::create(&backup).unwrap());
        writer
            .start_file("first.md", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"first").unwrap();
        writer
            .start_file("second.md", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"second").unwrap();
        writer.finish().unwrap();
        let mut writes = 0;

        let error = extract_and_validate_with(&backup, &stage, |_| {
            writes += 1;
            if writes == 2 {
                Err("injected disk-full write failure".to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert!(error.contains("disk-full"));
        assert_eq!(fs::read_to_string(stage.join("first.md")).unwrap(), "first");
        assert!(!stage.join("second.md").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_commit_shape_is_rejected_before_live_paths_are_displaced() {
        let root = std::env::temp_dir().join(format!("second-brain-rollback-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        fs::create_dir_all(&vault).unwrap();
        fs::create_dir_all(&backups).unwrap();
        fs::write(vault.join("live.md"), "live").unwrap();
        let backup = backups.join("commit-failure.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&backup).unwrap());
        writer
            .start_file("replacement.md", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"replacement").unwrap();
        // A regular file at the metadata directory path passes archive extraction but
        // cannot be enumerated during commit, after the live note has been displaced.
        writer
            .start_file(".helixnotes", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"not a directory").unwrap();
        writer.finish().unwrap();

        assert!(
            restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap()).is_err()
        );
        assert_eq!(fs::read_to_string(vault.join("live.md")).unwrap(), "live");
        assert!(!vault.join("replacement.md").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_mid_commit_failure_rolls_back_every_live_path() {
        let (root, vault, backups, backup) = restore_fixture("mid-commit");
        let before = tree(&vault);
        let mut publications = 0;

        let error = restore_backup_with(&vault, &backups, &backup, |step| {
            if step == RestoreStep::Publish {
                publications += 1;
                if publications == 2 {
                    return Err(Interrupt::Failed("injected publish failure".to_string()));
                }
            }
            Ok(())
        })
        .unwrap_err();

        assert!(!error.changed);
        assert!(error.message.contains("rolled back"));
        assert_eq!(tree(&vault), before);
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_incomplete_rollback_keeps_its_journal_for_the_next_open() {
        let (root, vault, backups, backup) = restore_fixture("incomplete-rollback");
        let mut publications = 0;
        let damage_root = root.clone();

        let error = restore_backup_with(&vault, &backups, &backup, |step| {
            if step == RestoreStep::Publish {
                publications += 1;
                if publications == 2 {
                    let rollback = restore_leftovers(&damage_root)
                        .into_iter()
                        .find(|name| name.contains("rollback-"))
                        .unwrap();
                    fs::remove_file(damage_root.join(rollback).join("live-only.md")).unwrap();
                    return Err(Interrupt::Failed("injected publish failure".to_string()));
                }
            }
            Ok(())
        })
        .unwrap_err();

        assert!(error.changed);
        assert!(error.message.contains("rollback was incomplete"));
        assert!(
            restore_leftovers(&root)
                .iter()
                .any(|name| name.contains("journal-")),
            "the journal must survive so the next open can retry"
        );
        assert!(
            recover_interrupted_restore(&vault).is_err(),
            "a lost original must keep the vault from opening"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_touches_only_its_own_vault_and_reports_unowned_restore_folders() {
        let (root, vault, backups, backup) = restore_fixture("foreign-journal");
        let other = root.join("other-vault");
        fs::create_dir_all(other.join("Projects")).unwrap();
        fs::write(other.join("Projects/other.md"), "other").unwrap();
        let other_before = tree(&other);
        let vault_before = tree(&vault);
        let mut displacements = 0;
        restore_backup_with(&other, &backups, &backup, |step| {
            if step == RestoreStep::Displace {
                displacements += 1;
                if displacements == 1 {
                    return Err(Interrupt::Died);
                }
            }
            Ok(())
        })
        .unwrap_err();
        let torn_other = tree(&other);
        let stray = root.join(".second-brain-restore-rollback-left-by-an-old-build");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join("only-copy.md"), "precious").unwrap();
        let recovery = recover_interrupted_restore(&vault).unwrap();

        assert!(recovery.outcomes.is_empty());
        assert_eq!(tree(&vault), vault_before);
        assert_eq!(
            tree(&other),
            torn_other,
            "another vault's restore is not ours to touch"
        );
        assert_eq!(recovery.strays, vec![stray.clone()]);
        assert_eq!(
            fs::read_to_string(stray.join("only-copy.md")).unwrap(),
            "precious"
        );
        let other_recovery = recover_interrupted_restore(&other).unwrap();
        assert_eq!(other_recovery.outcomes, vec![RecoveredRestore::Undone]);
        assert_eq!(tree(&other), other_before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_journal_blocks_recovery_without_touching_the_vault() {
        let (root, vault, _, _) = restore_fixture("unreadable-recovery-journal");
        let before = tree(&vault);
        let damaged = root.join(".second-brain-restore-journal-damaged.json");
        fs::write(&damaged, "not a journal").unwrap();

        let error = recover_interrupted_restore(&vault).unwrap_err();

        assert!(error.contains(&damaged.display().to_string()), "{error}");
        assert_eq!(tree(&vault), before);
        assert!(damaged.exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// The #14 failure: every synced vault has a `.stfolder` with a marker file in it, and
    /// archives made before #14 carried that marker. The restore must go through and leave the
    /// live dot folders exactly as they are.
    #[test]
    fn dot_folders_in_an_old_archive_leave_the_live_ones_untouched() {
        let (root, vault, backups, _) = restore_fixture("dot-folder");
        fs::create_dir_all(vault.join(".stfolder")).unwrap();
        fs::write(
            vault.join(".stfolder/syncthing-folder-live.txt"),
            "live marker",
        )
        .unwrap();
        fs::write(vault.join(".stignore"), "live ignores").unwrap();
        let backup = backups.join("helixnotes-pre-sync-2026-09-29T03-25-00.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&backup).unwrap());
        for (path, body) in [
            ("note.md", "restored"),
            (".stfolder/syncthing-folder-old.txt", "old marker"),
            (".obsidian/app.json", "backup settings"),
        ] {
            writer
                .start_file(path, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap();

        restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap()).unwrap();

        assert_eq!(
            fs::read_to_string(vault.join("note.md")).unwrap(),
            "restored"
        );
        assert!(!vault.join("Projects").exists());
        let dot_entries: Vec<_> = tree(&vault)
            .into_iter()
            .filter(|(path, _)| {
                path.to_string_lossy().starts_with('.') && !path.starts_with(".helixnotes")
            })
            .collect();
        assert_eq!(
            dot_entries,
            vec![
                (
                    PathBuf::from(".stfolder/syncthing-folder-live.txt"),
                    b"live marker".to_vec()
                ),
                (PathBuf::from(".stignore"), b"live ignores".to_vec()),
            ]
        );
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_backup_holds_exactly_what_a_restore_replaces() {
        let root =
            std::env::temp_dir().join(format!("second-brain-backup-shape-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        for (path, body) in [
            ("Projects/note.md", "before"),
            ("Projects/.obsidian/app.json", "nested settings"),
            ("Projects/.draft.md", "nested hidden note"),
            (".helixnotes/vault_id", "vault-id"),
            (".stfolder/syncthing-folder-live.txt", "marker"),
            (".stignore", "ignores"),
            (".git/HEAD", "ref"),
        ] {
            let path = vault.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }

        let entry = create_pre_sync_backup(vault.to_str().unwrap(), &backups).unwrap();
        let archive = ZipArchive::new(fs::File::open(&entry.path).unwrap()).unwrap();
        let mut names: Vec<String> = archive
            .file_names()
            .map(|name| name.unwrap().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                ".helixnotes/",
                ".helixnotes/vault_id",
                "Projects/",
                "Projects/.draft.md",
                "Projects/.obsidian/",
                "Projects/.obsidian/app.json",
                "Projects/note.md",
            ]
        );
        drop(archive);

        let before = tree(&vault);
        fs::write(vault.join("Projects/note.md"), "after").unwrap();
        fs::remove_file(vault.join("Projects/.draft.md")).unwrap();
        restore_backup(vault.to_str().unwrap(), &backups, &entry.path).unwrap();
        assert_eq!(tree(&vault), before);
        fs::remove_dir_all(root).unwrap();
    }

    /// The entries v0.1.0-alpha.1, built with zip 2.4.2, reads back: portable names with `/`
    /// separators, directory entries ending in `/`, files deflated, all marked as made on Unix.
    #[test]
    fn a_new_backup_is_written_as_earlier_releases_read_it() {
        let root =
            std::env::temp_dir().join(format!("second-brain-backup-format-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        let every_byte: Vec<u8> = (0..=u8::MAX).collect();
        let files: [(&str, &[u8]); 6] = [
            (
                "Projects/Café ☕/Plan für 2027.md",
                "# Plan für 2027\r\nCafé ☕\n".as_bytes(),
            ),
            ("Resources/日本語/メモ.md", "メモ".as_bytes()),
            ("Archives/empty.md", b""),
            ("Areas/.drafts/.deeper/hidden.md", b"in a nested dot folder"),
            (
                ".helixnotes/attachments/every-byte.bin",
                every_byte.as_slice(),
            ),
            (".helixnotes/vault_id", b"vault-id"),
        ];
        for (path, body) in files {
            let path = vault.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }

        let entry = create_backup(vault.to_str().unwrap(), &backups, true).unwrap();

        let mut archive = ZipArchive::new(fs::File::open(&entry.path).unwrap()).unwrap();
        let mut names: Vec<String> = archive
            .file_names()
            .map(|name| name.unwrap().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                ".helixnotes/",
                ".helixnotes/attachments/",
                ".helixnotes/attachments/every-byte.bin",
                ".helixnotes/vault_id",
                "Archives/",
                "Archives/empty.md",
                "Areas/",
                "Areas/.drafts/",
                "Areas/.drafts/.deeper/",
                "Areas/.drafts/.deeper/hidden.md",
                "Projects/",
                "Projects/Café ☕/",
                "Projects/Café ☕/Plan für 2027.md",
                "Resources/",
                "Resources/日本語/",
                "Resources/日本語/メモ.md",
            ]
        );
        for index in 0..archive.len() {
            let entry = archive.by_index(index).unwrap();
            assert_eq!(entry.system(), zip::System::Unix, "{:?}", entry.name());
        }
        for (path, body) in files {
            let mut entry = archive.by_name(path).unwrap();
            if !body.is_empty() {
                assert_eq!(
                    entry.compression(),
                    zip::CompressionMethod::Deflated,
                    "{path}"
                );
            }
            let mut read = Vec::new();
            entry.read_to_end(&mut read).unwrap();
            assert_eq!(read, body, "{path}");
        }
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    /// Fedora file and folder names may hold a backslash anywhere, hand-added attachments
    /// included. A backup stores them unchanged and restores them to the same native files.
    #[cfg(unix)]
    #[test]
    fn fedora_backslash_names_survive_a_backup_and_restore() {
        let root =
            std::env::temp_dir().join(format!("second-brain-backslash-trip-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        let files: [(&str, &[u8]); 5] = [
            ("Resources/a\\b.md", b"# a\\b\r\nliteral backslash\n"),
            ("Areas/x\\y/note.md", b"in a folder named x\\y"),
            (
                "Projects/ends in\\",
                b"a file whose name ends in a backslash",
            ),
            (
                ".helixnotes/attachments/scan\\1.png",
                b"\x89PNG\r\n\x1a\n\x00\xff",
            ),
            ("Projects/plain.md", b"plain"),
        ];
        for (path, body) in files {
            let path = vault.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }

        let entry = create_backup(vault.to_str().unwrap(), &backups, true).unwrap();

        let archive = ZipArchive::new(fs::File::open(&entry.path).unwrap()).unwrap();
        let mut names: Vec<String> = archive
            .file_names()
            .map(|name| name.unwrap().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                ".helixnotes/",
                ".helixnotes/attachments/",
                ".helixnotes/attachments/scan\\1.png",
                "Areas/",
                "Areas/x\\y/",
                "Areas/x\\y/note.md",
                "Projects/",
                "Projects/ends in\\",
                "Projects/plain.md",
                "Resources/",
                "Resources/a\\b.md",
            ]
        );
        drop(archive);

        let before = tree(&vault);
        fs::write(vault.join("Resources/a\\b.md"), "edited after the backup").unwrap();
        fs::remove_dir_all(vault.join("Areas")).unwrap();
        fs::remove_file(vault.join("Projects/ends in\\")).unwrap();

        restore_backup(vault.to_str().unwrap(), &backups, &entry.path).unwrap();

        assert_eq!(tree(&vault), before);
        assert!(vault.join("Areas/x\\y").is_dir());
        assert!(vault.join("Projects/ends in\\").is_file());
        assert!(!vault.join("Resources/a").exists());
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();
    }

    /// The vault zip 2.6.1 wrote `test-fixtures/deps-zip9/old-backup.zip` from, as byte strings
    /// so no checkout setting can change them. The README beside the archive says how.
    const OLD_BACKUP_VAULT: &[(&str, &[u8])] = &[
        (
            "Projects/Café ☕/Plan für 2027.md",
            b"# Plan f\xc3\xbcr 2027\r\n\r\nCaf\xc3\xa9 \xe2\x98\x95: CRLF and LF line endings stay as written.\n",
        ),
        ("Resources/日本語/メモ.md", b"# \xe3\x83\xa1\xe3\x83\xa2\n"),
        ("Archives/empty.md", b""),
        (".helixnotes/vault_id", b"00000000-0000-4000-8000-000000000237"),
        (
            ".helixnotes/history/00000000-0000-4000-8000-000000000001/2026-10-01T09-30-00.md",
            b"# Plan\n\nAn older version.\n",
        ),
        (
            ".helixnotes/attachments/diagram.png",
            b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x7f\x80\xfe\xff",
        ),
    ];

    #[test]
    fn a_backup_written_with_zip_2_restores_in_this_build() {
        let root = std::env::temp_dir().join(format!("second-brain-old-backup-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let backups = root.join("backups");
        for (path, body) in [
            ("live.md", "live"),
            (".helixnotes/vault_id", "live-id"),
            (".helixnotes/attachments/live.bin", "live attachment"),
        ] {
            let path = vault.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        fs::create_dir_all(&backups).unwrap();
        let backup = backups.join("helixnotes-backup-2026-10-01T09-30-00.zip");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/deps-zip9/old-backup.zip"),
            &backup,
        )
        .unwrap();

        // The fixture must hold both methods a restore reads.
        let mut archive = ZipArchive::new(fs::File::open(&backup).unwrap()).unwrap();
        let mut methods = BTreeMap::new();
        for index in 0..archive.len() {
            let entry = archive.by_index(index).unwrap();
            if !entry.is_dir() {
                methods.insert(entry.name().unwrap().into_owned(), entry.compression());
            }
        }
        drop(archive);
        let expected_methods: BTreeMap<String, zip::CompressionMethod> = OLD_BACKUP_VAULT
            .iter()
            .map(|(path, _)| {
                let method = if *path == ".helixnotes/attachments/diagram.png" {
                    zip::CompressionMethod::Stored
                } else {
                    zip::CompressionMethod::Deflated
                };
                (path.to_string(), method)
            })
            .collect();
        assert_eq!(methods, expected_methods);

        restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap()).unwrap();

        let expected: BTreeMap<PathBuf, Vec<u8>> = OLD_BACKUP_VAULT
            .iter()
            .map(|(path, body)| (PathBuf::from(*path), body.to_vec()))
            .collect();
        assert_eq!(tree(&vault), expected);
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();
    }

    /// The vault the earlier `create_backup`, on zip 2.4.2, wrote
    /// `test-fixtures/deps-zip9/old-backslash-names.zip` from. Every backslash is part of a Fedora
    /// file or folder name. The README beside the archive says how it was made.
    const BACKSLASH_VAULT: &[(&str, &[u8])] = &[
        (
            "Resources/a\\b.md",
            b"# a\\b\r\nA note whose name holds a backslash.\n",
        ),
        ("Areas/x\\y/note.md", b"In a folder named x\\y.\n"),
        (
            ".helixnotes/attachments/scan\\1.png",
            b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x5c\xff",
        ),
        ("Projects/plain.md", b"An ordinary note.\n"),
        (
            ".helixnotes/vault_id",
            b"00000000-0000-4000-8000-000000000238",
        ),
    ];

    /// Fedora reads each backslash as part of a name and Windows as a separator, as the earlier
    /// release restored them.
    #[test]
    fn backslash_names_in_a_zip_2_backup_restore_at_their_native_paths() {
        let root = std::env::temp_dir().join(format!(
            "second-brain-old-backslash-names-{}",
            Uuid::new_v4()
        ));
        let vault = root.join("vault");
        let backups = root.join("backups");
        for (path, body) in [("live.md", "live"), (".helixnotes/vault_id", "live-id")] {
            let path = vault.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        fs::create_dir_all(&backups).unwrap();
        let backup = backups.join("helixnotes-backup-2026-10-01T09-30-00.zip");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("test-fixtures/deps-zip9/old-backslash-names.zip"),
            &backup,
        )
        .unwrap();

        // The fixture stores the backslashes as written.
        let archive = ZipArchive::new(fs::File::open(&backup).unwrap()).unwrap();
        let mut files: Vec<String> = archive
            .file_names()
            .map(|name| name.unwrap().into_owned())
            .filter(|name| !name.ends_with('/'))
            .collect();
        files.sort();
        let mut expected_files: Vec<&str> = BACKSLASH_VAULT.iter().map(|(path, _)| *path).collect();
        expected_files.sort();
        assert_eq!(files, expected_files);
        drop(archive);

        restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap()).unwrap();

        let expected: BTreeMap<PathBuf, Vec<u8>> = BACKSLASH_VAULT
            .iter()
            .map(|(path, body)| (PathBuf::from(*path), body.to_vec()))
            .collect();
        assert_eq!(tree(&vault), expected);
        let note = if cfg!(windows) {
            vault.join("Resources").join("a").join("b.md")
        } else {
            vault.join("Resources").join("a\\b.md")
        };
        assert_eq!(fs::read(note).unwrap(), BACKSLASH_VAULT[0].1);
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pruning_keeps_the_newest_scheduled_backup_among_frequent_pre_sync_ones() {
        let root = std::env::temp_dir().join(format!("second-brain-retention-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let names = [
            "helixnotes-backup-2026-09-27T10-00-00.zip",
            "helixnotes-backup-2026-09-28T10-00-00.zip",
            "helixnotes-pre-sync-2026-09-29T10-00-00.zip",
            "helixnotes-pre-sync-2026-09-29T10-05-00.zip",
            "helixnotes-pre-sync-2026-09-29T10-10-00.zip",
            "helixnotes-pre-sync-2026-09-29T10-15-00.zip",
            "helixnotes-pre-sync-2026-09-29T10-20-00.zip",
            "helixnotes-pre-sync-2026-09-29T10-25-00.zip",
        ];
        for name in names {
            fs::write(root.join(name), "").unwrap();
        }
        let remaining = |root: &Path| {
            let mut names: Vec<String> = list_backups(root)
                .unwrap()
                .into_iter()
                .map(|entry| entry.filename)
                .collect();
            names.sort();
            names
        };

        cleanup_old_backups(&root, 5).unwrap();
        assert_eq!(
            remaining(&root),
            vec![
                "helixnotes-backup-2026-09-28T10-00-00.zip",
                "helixnotes-pre-sync-2026-09-29T10-10-00.zip",
                "helixnotes-pre-sync-2026-09-29T10-15-00.zip",
                "helixnotes-pre-sync-2026-09-29T10-20-00.zip",
                "helixnotes-pre-sync-2026-09-29T10-25-00.zip",
            ]
        );

        // With a scheduled backup already among the newest, pruning is plain newest-first.
        fs::write(root.join("helixnotes-backup-2026-09-29T10-30-00.zip"), "").unwrap();
        cleanup_old_backups(&root, 5).unwrap();
        assert_eq!(
            remaining(&root),
            vec![
                "helixnotes-backup-2026-09-29T10-30-00.zip",
                "helixnotes-pre-sync-2026-09-29T10-10-00.zip",
                "helixnotes-pre-sync-2026-09-29T10-15-00.zip",
                "helixnotes-pre-sync-2026-09-29T10-20-00.zip",
                "helixnotes-pre-sync-2026-09-29T10-25-00.zip",
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn both_backup_kinds_are_dated_from_their_names() {
        let root =
            std::env::temp_dir().join(format!("second-brain-backup-list-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for name in [
            "helixnotes-backup-2026-09-28T10-00-00.zip",
            "helixnotes-pre-sync-2026-09-29T03-25-00.zip",
        ] {
            fs::write(root.join(name), "").unwrap();
        }

        let dated: Vec<(String, String, BackupKind)> = list_backups(&root)
            .unwrap()
            .into_iter()
            .map(|entry| (entry.filename, entry.created, entry.kind))
            .collect();

        assert_eq!(
            dated,
            vec![
                (
                    "helixnotes-pre-sync-2026-09-29T03-25-00.zip".to_string(),
                    "2026-09-29T03:25:00+00:00".to_string(),
                    BackupKind::PreSync
                ),
                (
                    "helixnotes-backup-2026-09-28T10-00-00.zip".to_string(),
                    "2026-09-28T10:00:00+00:00".to_string(),
                    BackupKind::Backup
                ),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_recovery_killed_at_any_step_finishes_when_run_again() {
        let (root, vault, backups, backup) = restore_fixture("kill-recovery");
        let before = tree(&vault);
        let mut restore_steps = 0;
        restore_backup_with(&vault, &backups, &backup, |_| {
            restore_steps += 1;
            Ok(())
        })
        .unwrap();
        let restored = tree(&vault);
        fs::remove_dir_all(root).unwrap();

        let mut recovery_kills = 0;
        for restore_kill in 0..restore_steps {
            for recovery_kill in 0.. {
                let (root, vault, backups, backup) = restore_fixture("kill-recovery");
                let mut step = 0;
                let _ = restore_backup_with(&vault, &backups, &backup, |_| {
                    step += 1;
                    if step - 1 == restore_kill {
                        Err(Interrupt::Died)
                    } else {
                        Ok(())
                    }
                });
                let mut recovery_step = 0;
                let first = recover_with(&vault, &mut || {
                    recovery_step += 1;
                    if recovery_step - 1 == recovery_kill {
                        Err(Interrupt::Died)
                    } else {
                        Ok(())
                    }
                });
                if first.is_ok() {
                    fs::remove_dir_all(root).unwrap();
                    break;
                }
                recovery_kills += 1;

                recover_interrupted_restore(&vault).unwrap();
                let after = tree(&vault);
                assert!(
                    after == before || after == restored,
                    "restore killed at {restore_kill}, recovery killed at {recovery_kill}: torn vault"
                );
                assert_eq!(
                    restore_leftovers(&root),
                    Vec::<String>::new(),
                    "restore killed at {restore_kill}, recovery killed at {recovery_kill}"
                );
                fs::remove_dir_all(root).unwrap();
            }
        }
        assert!(recovery_kills > 0, "no kill point landed inside a recovery");
    }

    /// Stop a restore at the `nth` step of `kind`, as if the process died there.
    fn kill_restore_at(vault: &Path, backups: &Path, backup: &Path, kind: RestoreStep, nth: usize) {
        let mut seen = 0;
        let result = restore_backup_with(vault, backups, backup, |step| {
            if step == kind {
                seen += 1;
                if seen == nth {
                    return Err(Interrupt::Died);
                }
            }
            Ok(())
        });
        assert!(result.is_err(), "the {kind:?} #{nth} kill point must exist");
    }

    fn leftover(root: &Path, kind: &str) -> PathBuf {
        root.join(
            restore_leftovers(root)
                .into_iter()
                .find(|name| name.contains(kind))
                .unwrap_or_else(|| panic!("no {kind} left next to the vault")),
        )
    }

    #[test]
    fn a_second_restore_waits_until_the_first_is_recovered() {
        let (root, vault, backups, backup) = restore_fixture("second-restore");
        kill_restore_at(&vault, &backups, &backup, RestoreStep::Displace, 2);
        let torn = tree(&vault);

        let error = restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap())
            .unwrap_err();

        assert!(!error.changed);
        assert!(
            error.message.contains("unfinished restore"),
            "{}",
            error.message
        );
        assert_eq!(
            tree(&vault),
            torn,
            "the refused restore must not touch the vault"
        );
        assert_eq!(
            restore_leftovers(&root)
                .iter()
                .filter(|name| name.contains("journal-"))
                .count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_publish_lost_to_power_failure_is_completed_not_deleted() {
        let (root, vault, backups, backup) = restore_fixture("lost-publish");
        let reference = restore_fixture("lost-publish-reference");
        restore_backup(
            reference.1.to_str().unwrap(),
            &reference.2,
            reference.3.to_str().unwrap(),
        )
        .unwrap();
        let restored = tree(&reference.1);
        fs::remove_dir_all(&reference.0).unwrap();
        // Die after the published record is durable, then undo one publish rename the way a
        // filesystem that did not persist it would.
        kill_restore_at(&vault, &backups, &backup, RestoreStep::Journal, 3);
        let stage = leftover(&root, "stage-");
        fs::rename(vault.join("Resources"), stage.join("Resources")).unwrap();

        let recovery = recover_interrupted_restore(&vault).unwrap();

        assert_eq!(recovery.outcomes, vec![RecoveredRestore::KeptRestored]);
        assert_eq!(tree(&vault), restored);
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_corrupt_journal_cannot_delete_outside_the_vaults_directory() {
        let (root, vault, _, _) = restore_fixture("corrupt-journal");
        let elsewhere = root.join("elsewhere");
        let victim = elsewhere.join(".second-brain-restore-stage-victim");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep.md"), "not ours").unwrap();
        let vault_path = dunce::canonicalize(&vault).unwrap();
        let journal = serde_json::json!({
            "vault": vault_path,
            "stage": victim,
            "rollback": elsewhere.join(".second-brain-restore-rollback-victim"),
            "phase": "unpacking",
        });
        fs::write(
            root.join(".second-brain-restore-journal-corrupt.json"),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();

        assert!(recover_interrupted_restore(&vault).is_err());
        assert_eq!(
            fs::read_to_string(victim.join("keep.md")).unwrap(),
            "not ours"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_blocked_recovery_refuses_and_says_where_the_notes_are() {
        let (root, vault, backups, backup) = restore_fixture("blocked-recovery");
        kill_restore_at(&vault, &backups, &backup, RestoreStep::Displace, 2);
        let rollback = leftover(&root, "rollback-");
        // The user recreated a folder while the app was down, so the original cannot return.
        fs::create_dir_all(vault.join("Areas")).unwrap();
        fs::write(vault.join("Areas/new.md"), "made while the app was down").unwrap();

        let error = recover_interrupted_restore(&vault).unwrap_err();

        assert!(error.contains(&rollback.display().to_string()), "{error}");
        assert!(error.contains("Nothing was deleted"), "{error}");
        assert_eq!(
            fs::read_to_string(rollback.join("Areas/b.md")).unwrap(),
            "live b"
        );
        assert!(leftover(&root, "journal-").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_settlement_failure_refuses_to_open_and_keeps_recovery_data() {
        let (root, vault, backups, backup) = restore_fixture("settlement-failure");
        kill_restore_at(&vault, &backups, &backup, RestoreStep::Publish, 2);
        let journal = leftover(&root, "journal-");
        let mut damaged = false;

        let error = recover_with(&vault, &mut || {
            if !damaged {
                fs::remove_file(&journal).unwrap();
                fs::create_dir(&journal).unwrap();
                damaged = true;
            }
            Ok(())
        })
        .unwrap_err();

        assert!(error.contains("could not be recorded durably"), "{error}");
        assert!(error.contains(&vault.display().to_string()), "{error}");
        assert!(
            error.contains(&leftover(&root, "stage-").display().to_string()),
            "{error}"
        );
        assert!(
            error.contains(&leftover(&root, "rollback-").display().to_string()),
            "{error}"
        );
        assert!(leftover(&root, "stage-").exists());
        assert!(leftover(&root, "rollback-").exists());
        assert!(journal.is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_whole_vault_opens_even_when_cleanup_cannot_finish() {
        use std::os::unix::fs::PermissionsExt;
        let (root, vault, backups, backup) = restore_fixture("cleanup-blocked");
        let before = tree(&vault);
        kill_restore_at(&vault, &backups, &backup, RestoreStep::Stage, 2);
        let stage = leftover(&root, "stage-");
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o500)).unwrap();

        let recovery = recover_interrupted_restore(&vault).unwrap();

        assert_eq!(recovery.outcomes, vec![RecoveredRestore::Undone]);
        assert_eq!(tree(&vault), before);
        assert!(
            leftover(&root, "journal-").exists(),
            "cleanup retries on the next open"
        );
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            recover_interrupted_restore(&vault)
                .unwrap()
                .outcomes
                .is_empty(),
            "the later open only finishes cleanup; the recovery was reported once"
        );
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_locked_entry_exhausts_the_retries_and_refuses_to_open() {
        use std::os::windows::fs::OpenOptionsExt;
        let (root, vault, backups, backup) = restore_fixture("locked-entry");
        kill_restore_at(&vault, &backups, &backup, RestoreStep::Displace, 2);
        let rollback = leftover(&root, "rollback-");
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(rollback.join("Areas/b.md"))
            .unwrap();
        let started = std::time::Instant::now();

        let error = recover_interrupted_restore(&vault).unwrap_err();

        assert!(
            started.elapsed() >= std::time::Duration::from_millis(1500),
            "{error}"
        );
        assert!(error.contains(&rollback.display().to_string()), "{error}");
        drop(lock);
        recover_interrupted_restore(&vault).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// Recover once with the stage undeletable, so cleanup has to wait for a later open.
    #[cfg(unix)]
    fn recover_with_cleanup_blocked(root: &Path, vault: &Path) -> Vec<RecoveredRestore> {
        use std::os::unix::fs::PermissionsExt;
        let stage = leftover(root, "stage-");
        fs::create_dir_all(stage.join("pin")).unwrap();
        fs::write(stage.join("pin/pin.md"), "pin").unwrap();
        fs::set_permissions(stage.join("pin"), fs::Permissions::from_mode(0o500)).unwrap();
        let outcomes = recover_interrupted_restore(vault).unwrap().outcomes;
        assert!(
            leftover(root, "journal-").exists(),
            "cleanup must still be pending"
        );
        fs::set_permissions(stage.join("pin"), fs::Permissions::from_mode(0o700)).unwrap();
        outcomes
    }

    #[cfg(unix)]
    #[test]
    fn edits_made_after_a_kept_restore_survive_the_delayed_cleanup() {
        let (root, vault, backups, backup) = restore_fixture("late-cleanup-kept");
        kill_restore_at(&vault, &backups, &backup, RestoreStep::Journal, 3);
        assert_eq!(
            recover_with_cleanup_blocked(&root, &vault),
            vec![RecoveredRestore::KeptRestored]
        );
        // The vault opened; the user deletes a restored folder before the next open.
        fs::remove_dir_all(vault.join("Resources")).unwrap();
        let edited = tree(&vault);

        let later = recover_interrupted_restore(&vault).unwrap();

        assert!(
            later.outcomes.is_empty(),
            "nothing was interrupted this time"
        );
        assert_eq!(
            tree(&vault),
            edited,
            "the deleted folder must not come back"
        );
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn edits_made_after_an_undone_restore_survive_the_delayed_cleanup() {
        let (root, vault, backups, backup) = restore_fixture("late-cleanup-undone");
        kill_restore_at(&vault, &backups, &backup, RestoreStep::Publish, 2);
        assert_eq!(
            recover_with_cleanup_blocked(&root, &vault),
            vec![RecoveredRestore::Undone]
        );
        // The user creates a folder with the name of an entry the restore would have added.
        fs::create_dir_all(vault.join("Resources")).unwrap();
        fs::write(vault.join("Resources/mine.md"), "written after the restore").unwrap();
        let edited = tree(&vault);

        let later = recover_interrupted_restore(&vault).unwrap();

        assert!(
            later.outcomes.is_empty(),
            "nothing was interrupted this time"
        );
        assert_eq!(
            tree(&vault),
            edited,
            "the user's new folder must not be taken"
        );
        assert_eq!(restore_leftovers(&root), Vec::<String>::new());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_journal_blocks_restore_and_says_which_file_to_move() {
        let (root, vault, backups, backup) = restore_fixture("unreadable-journal");
        let damaged = root.join(".second-brain-restore-journal-damaged.json");
        fs::write(&damaged, "not a journal").unwrap();

        let error = restore_backup(vault.to_str().unwrap(), &backups, backup.to_str().unwrap())
            .unwrap_err();

        assert!(
            error.message.contains(&damaged.display().to_string()),
            "{}",
            error.message
        );
        fs::remove_dir_all(root).unwrap();
    }
}
