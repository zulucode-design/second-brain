use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::ZipArchive;

use crate::types::BackupEntry;

const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_ARCHIVE_BYTES: u64 = 100 * 1024 * 1024 * 1024;

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

/// Returns the default backup directory (~/.config/helixnotes/backups/)
pub fn default_backup_dir() -> Result<PathBuf, String> {
    let config_dir = dirs::config_dir().ok_or("Cannot find config directory")?;
    let dir = config_dir.join("helixnotes").join("backups");
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
    create_backup_with_prefix(
        vault_path,
        backup_dir,
        include_attachments,
        "helixnotes-backup",
    )
}

/// Create the identifiable, full-vault recovery point required before incoming sync.
pub fn create_pre_sync_backup(vault_path: &str, backup_dir: &Path) -> Result<BackupEntry, String> {
    create_backup_with_prefix(vault_path, backup_dir, true, "helixnotes-pre-sync")
}

fn create_backup_with_prefix(
    vault_path: &str,
    backup_dir: &Path,
    include_attachments: bool,
    prefix: &str,
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
    let filename = format!("{prefix}-{timestamp}.zip");
    let backup_path = backup_dir.join(&filename);

    let file = fs::File::create(&backup_path)
        .map_err(|e| format!("Failed to create backup file: {}", e))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(6));

    let helixnotes_dir = vault.join(".helixnotes");
    let attachments_dir = helixnotes_dir.join("attachments");

    for entry in WalkDir::new(vault) {
        let entry =
            entry.map_err(|error| format!("Failed to traverse vault for backup: {error}"))?;
        let path = entry.path();

        // `.helixnotes` contains vault-scoped recoverable data. Attachments are the
        // sole configurable exclusion because they can dominate backup size.
        if path.starts_with(&helixnotes_dir)
            && !include_attachments
            && path.starts_with(&attachments_dir)
        {
            continue;
        }

        // Skip hidden files/directories (except .helixnotes which we handle above)
        if path != vault {
            if let Some(name) = path.file_name() {
                let name_str = name.to_string_lossy();
                if name_str.starts_with('.') && name_str != ".helixnotes" {
                    continue;
                }
            }
        }

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

            // Parse timestamp from filename: helixnotes-backup-YYYY-MM-DDTHH-MM-SS.zip
            let created = if let Some(ts) = filename
                .strip_prefix("helixnotes-backup-")
                .and_then(|s| s.strip_suffix(".zip"))
            {
                // ts = "2026-02-08T18-09-15" → "2026-02-08T18:09:15Z"
                if let Some(t_pos) = ts.find('T') {
                    let date_part = &ts[..t_pos];
                    let time_part = &ts[t_pos + 1..];
                    let time_colons = time_part.replace('-', ":");
                    format!("{}T{}Z", date_part, time_colons)
                } else {
                    meta.modified()
                        .map(|t| {
                            let dt: chrono::DateTime<Utc> = t.into();
                            dt.to_rfc3339()
                        })
                        .unwrap_or_default()
                }
            } else {
                meta.modified()
                    .map(|t| {
                        let dt: chrono::DateTime<Utc> = t.into();
                        dt.to_rfc3339()
                    })
                    .unwrap_or_default()
            };

            entries.push(BackupEntry {
                filename,
                path: path.to_string_lossy().to_string(),
                size: meta.len(),
                created,
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

/// Restore a backup using a fully validated sibling staging area and rollback commit.
pub fn restore_backup(
    vault_path: &str,
    backup_dir: &Path,
    backup_path: &str,
) -> Result<(), RestoreError> {
    let vault = Path::new(vault_path);
    let backup = validated_backup_file(backup_dir, backup_path)?;

    let parent = vault.parent().ok_or("Vault must have a parent directory")?;
    let nonce = uuid::Uuid::new_v4();
    let stage = parent.join(format!(".second-brain-restore-stage-{nonce}"));
    let rollback = parent.join(format!(".second-brain-restore-rollback-{nonce}"));
    fs::create_dir(&stage).map_err(|e| format!("Failed to create restore staging area: {e}"))?;
    if let Err(error) = extract_and_validate(&backup, &stage) {
        let _ = fs::remove_dir_all(&stage);
        return Err(error.into());
    }
    fs::create_dir(&rollback).map_err(|e| {
        let _ = fs::remove_dir_all(&stage);
        format!("Failed to create restore rollback area: {e}")
    })?;

    let result = commit_staged_restore(vault, &stage, &rollback);
    let _ = fs::remove_dir_all(&stage);
    if result.is_ok() {
        if let Err(error) = fs::remove_dir_all(&rollback) {
            log::warn!(
                "Restore committed; stale rollback directory {} could not be removed: {error}",
                rollback.display()
            );
        }
    }
    result
}

fn extract_and_validate(backup: &Path, stage: &Path) -> Result<(), String> {
    extract_and_validate_with(backup, stage, |_| Ok(()))
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
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| format!("Unreadable backup entry: {e}"))?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| format!("Unsafe path in backup: {}", entry.name()))?
            .to_path_buf();
        if relative.as_os_str().is_empty() || !paths.insert(relative.clone()) {
            return Err(format!(
                "Duplicate or empty path in backup: {}",
                entry.name()
            ));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!(
                "Symbolic links are not allowed in backups: {}",
                entry.name()
            ));
        }
        total_bytes = total_bytes
            .checked_add(entry.size())
            .ok_or("Backup expanded size overflow")?;
        if total_bytes > MAX_ARCHIVE_BYTES {
            return Err(format!("Backup expands beyond {MAX_ARCHIVE_BYTES} bytes"));
        }
    }

    // Extraction is deliberately a second pass: the complete archive shape and declared
    // expanded size are accepted before a single potentially large payload reaches disk.
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("Unreadable backup entry: {e}"))?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| format!("Unsafe path in backup: {}", entry.name()))?
            .to_path_buf();
        let destination = stage.join(&relative);
        if entry.is_dir() {
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
                .map_err(|e| format!("Cannot extract backup entry {}: {e}", entry.name()))?;
            output
                .sync_all()
                .map_err(|e| format!("Cannot flush staged backup file: {e}"))?;
        }
    }
    Ok(())
}

fn commit_staged_restore(vault: &Path, stage: &Path, rollback: &Path) -> Result<(), RestoreError> {
    commit_staged_restore_with(vault, stage, rollback, |_| Ok(()))
}

fn commit_staged_restore_with(
    vault: &Path,
    stage: &Path,
    rollback: &Path,
    mut before_publish: impl FnMut(&Path) -> Result<(), String>,
) -> Result<(), RestoreError> {
    let staged_metadata = stage.join(".helixnotes");
    let metadata_names = if staged_metadata.exists() {
        fs::read_dir(&staged_metadata)
            .map_err(|e| format!("Invalid staged vault metadata: {e}"))?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let mut displaced = Vec::new();
    for entry in fs::read_dir(vault).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if let Err(error) = fs::rename(entry.path(), rollback.join(&name)) {
            let rollback_error = rollback_displaced(vault, rollback, &displaced).err();
            let changed = rollback_error.is_some();
            return Err(RestoreError {
                message: rollback_error.map_or_else(
                    || format!("Failed to preserve live vault entry: {error}"),
                    |rollback| format!("Failed to preserve live vault entry ({error}) and rollback was incomplete: {rollback}"),
                ),
                changed,
            });
        }
        displaced.push(name);
    }

    let mut displaced_metadata = Vec::new();
    if !metadata_names.is_empty() {
        fs::create_dir_all(rollback.join(".helixnotes")).map_err(|e| e.to_string())?;
        for name in metadata_names {
            let live = vault.join(".helixnotes").join(&name);
            if live.exists() {
                if let Err(error) = fs::rename(&live, rollback.join(".helixnotes").join(&name)) {
                    let metadata_rollback =
                        rollback_metadata(vault, rollback, &displaced_metadata).err();
                    let path_rollback = rollback_displaced(vault, rollback, &displaced).err();
                    let rollback_error = metadata_rollback.or(path_rollback);
                    let changed = rollback_error.is_some();
                    return Err(RestoreError {
                        message: rollback_error.map_or_else(
                            || format!("Failed to preserve live vault metadata: {error}"),
                            |rollback| format!("Failed to preserve live vault metadata ({error}) and rollback was incomplete: {rollback}"),
                        ),
                        changed,
                    });
                }
                displaced_metadata.push(name);
            }
        }
    }

    let mut published = Vec::new();
    let publish_result = (|| -> Result<(), String> {
        for entry in fs::read_dir(stage).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let target = vault.join(entry.file_name());
            before_publish(&target)?;
            if entry.file_name() == ".helixnotes" {
                publish_metadata(entry.path(), &target, &mut published)?;
            } else {
                fs::rename(entry.path(), &target)
                    .map_err(|e| format!("Failed to publish restored entry: {e}"))?;
                published.push(target);
            }
        }
        Ok(())
    })();
    if let Err(error) = publish_result {
        let publish_cleanup = published
            .iter()
            .rev()
            .find_map(|path| remove_path(path).err());
        let path_rollback = rollback_displaced(vault, rollback, &displaced).err();
        let metadata_rollback = rollback_metadata(vault, rollback, &displaced_metadata).err();
        let rollback_error = publish_cleanup.or(path_rollback).or(metadata_rollback);
        let changed = rollback_error.is_some();
        return Err(RestoreError {
            message: rollback_error.map_or_else(
                || format!("Restore commit failed and was rolled back: {error}"),
                |rollback| {
                    format!(
                        "Restore commit failed ({error}) and rollback was incomplete: {rollback}"
                    )
                },
            ),
            changed,
        });
    }
    Ok(())
}

fn publish_metadata(
    source: PathBuf,
    destination: &Path,
    published: &mut Vec<PathBuf>,
) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = destination.join(entry.file_name());
        if target.exists() {
            return Err(format!(
                "Backup metadata would overwrite live metadata: {}",
                target.display()
            ));
        }
        fs::rename(entry.path(), &target).map_err(|e| e.to_string())?;
        published.push(target);
    }
    Ok(())
}

fn rollback_displaced(
    vault: &Path,
    rollback: &Path,
    names: &[std::ffi::OsString],
) -> Result<(), String> {
    for name in names.iter().rev() {
        fs::rename(rollback.join(name), vault.join(name)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn rollback_metadata(
    vault: &Path,
    rollback: &Path,
    names: &[std::ffi::OsString],
) -> Result<(), String> {
    let live = vault.join(".helixnotes");
    let old = rollback.join(".helixnotes");
    fs::create_dir_all(&live).map_err(|e| e.to_string())?;
    for name in names.iter().rev() {
        fs::rename(old.join(name), live.join(name)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|e| e.to_string())
    } else {
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}

/// Delete a single backup file
pub fn delete_backup(backup_dir: &Path, backup_path: &str) -> Result<(), String> {
    let path = validated_backup_file(backup_dir, backup_path)?;
    fs::remove_file(path).map_err(|error| error.to_string())
}

/// Remove old backups keeping only the newest `max_count`
pub fn cleanup_old_backups(backup_dir: &Path, max_count: u32) -> Result<(), String> {
    let mut backups = list_backups(backup_dir)?;

    // backups are already sorted newest-first
    if backups.len() as u32 > max_count {
        let to_remove = backups.split_off(max_count as usize);
        for entry in to_remove {
            delete_backup(backup_dir, &entry.path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        commit_staged_restore_with, create_backup, create_pre_sync_backup, delete_backup,
        extract_and_validate_with, restore_backup,
    };
    use std::fs;
    use std::io::{Seek, SeekFrom, Write};
    use uuid::Uuid;
    use zip::{write::SimpleFileOptions, ZipArchive};

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
                .data_start();
            start
        };
        let mut file = fs::OpenOptions::new().write(true).open(&backup).unwrap();
        file.seek(SeekFrom::Start(data_start)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();

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
        let root = std::env::temp_dir().join(format!("second-brain-mid-commit-{}", Uuid::new_v4()));
        let vault = root.join("vault");
        let stage = root.join("stage");
        let rollback = root.join("rollback");
        fs::create_dir_all(&vault).unwrap();
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&rollback).unwrap();
        fs::write(vault.join("live.md"), "live").unwrap();
        fs::write(stage.join("first.md"), "first").unwrap();
        fs::write(stage.join("second.md"), "second").unwrap();
        let mut publications = 0;

        let result = commit_staged_restore_with(&vault, &stage, &rollback, |_| {
            publications += 1;
            if publications == 2 {
                Err("injected publish failure".to_string())
            } else {
                Ok(())
            }
        });

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(vault.join("live.md")).unwrap(), "live");
        assert!(!vault.join("first.md").exists());
        assert!(!vault.join("second.md").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_incomplete_rollback_is_reported_as_changed() {
        let root = std::env::temp_dir().join(format!(
            "second-brain-incomplete-rollback-{}",
            Uuid::new_v4()
        ));
        let vault = root.join("vault");
        let stage = root.join("stage");
        let rollback = root.join("rollback");
        fs::create_dir_all(&vault).unwrap();
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&rollback).unwrap();
        fs::write(vault.join("live.md"), "live").unwrap();
        fs::write(stage.join("first.md"), "first").unwrap();
        fs::write(stage.join("second.md"), "second").unwrap();
        let mut publications = 0;
        let rollback_to_damage = rollback.clone();

        let error = commit_staged_restore_with(&vault, &stage, &rollback, |_| {
            publications += 1;
            if publications == 2 {
                fs::remove_file(rollback_to_damage.join("live.md")).unwrap();
                Err("injected publish failure".to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert!(error.changed);
        assert!(error.message.contains("rollback was incomplete"));
        fs::remove_dir_all(root).unwrap();
    }
}
