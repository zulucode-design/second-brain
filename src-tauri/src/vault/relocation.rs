use same_file::Handle;
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

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

    let transaction_dir = create_recovery_dir(&vault)?;
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
    fs::canonicalize(path).map_err(|error| format!("Invalid {label}: {error}"))
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
    let source =
        fs::canonicalize(source).map_err(|error| format!("Invalid relocation source: {error}"))?;
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
    if !category && !holding {
        return Err(
            "Relocation destination must be inside a PARA category or the Holding Area".to_string(),
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

fn create_recovery_dir(vault: &Path) -> Result<PathBuf, String> {
    let app_data = vault.join(".helixnotes");
    let metadata = fs::symlink_metadata(&app_data)
        .map_err(|error| format!("Invalid vault app-data directory: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Vault app-data directory must be a real directory".to_string());
    }
    let recovery_root = app_data.join("relocation-recovery");
    fs::create_dir_all(&recovery_root)
        .map_err(|error| format!("Could not create relocation recovery directory: {error}"))?;
    let recovery_root = canonical_directory(&recovery_root, "relocation recovery directory")?;
    if !recovery_root.starts_with(vault) {
        return Err("Relocation recovery directory escaped the active vault".to_string());
    }
    loop {
        let transaction = recovery_root.join(uuid::Uuid::new_v4().to_string());
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
        symlink(&outside, root.join(".helixnotes/relocation-recovery")).unwrap();
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
