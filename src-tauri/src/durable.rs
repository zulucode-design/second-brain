//! Crash-safe whole-file replacement for critical writes (audit D-11).
//!
//! `replace` writes the new bytes to a uniquely named temporary file in the destination's
//! own directory, flushes it with `sync_all`, then atomically replaces the destination:
//!
//! - Unix: `rename(2)` is atomic within one filesystem (hence the same-directory temporary),
//!   and the parent directory is `fsync`ed afterwards so the rename itself survives power loss.
//! - Windows: `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`, which does
//!   not return until the move is flushed. NTFS has no directory `fsync` to add.
//!
//! A reader therefore sees either the complete previous file or the complete new one, never a
//! truncated mix. On any failure the temporary is removed and the destination is untouched.
//! Unix permissions: `Private` files are created `0600`; `Shared` files keep the mode of the
//! file they replace (a new file gets the process umask default).

use std::io::Write;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Owner-only on Unix: app config, sync control state, anything holding secrets.
    Private,
    /// Ordinary vault data: notes, Notion maps, shared settings.
    Shared,
}

pub fn replace(path: &Path, data: &[u8], mode: Mode) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no valid filename", path.display()))?;
    let temporary = parent.join(format!(".{filename}.{}.tmp", uuid::Uuid::new_v4()));

    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    if mode == Mode::Private {
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
            let permissions = match mode {
                Mode::Private => Some(std::fs::Permissions::from_mode(0o600)),
                Mode::Shared => std::fs::metadata(path).ok().map(|meta| meta.permissions()),
            };
            if let Some(permissions) = permissions {
                file.set_permissions(permissions)
                    .map_err(|error| error.to_string())?;
            }
        }
        #[cfg(not(unix))]
        let _ = mode;
        file.write_all(data).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);

        #[cfg(test)]
        if tests::FAIL_BEFORE_REPLACE.with(|fail| fail.get()) {
            return Err("injected failure before replace".to_string());
        }

        atomic_replace(&temporary, path)?;
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

#[cfg(not(windows))]
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        pub(crate) static FAIL_BEFORE_REPLACE: Cell<bool> = const { Cell::new(false) };
    }

    /// Runs `body` with every `replace` on this thread failing after its temporary is
    /// fully written — the moment a crash would leave the most on disk.
    pub(crate) fn with_interrupted_replace<T>(body: impl FnOnce() -> T) -> T {
        FAIL_BEFORE_REPLACE.with(|fail| fail.set(true));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        FAIL_BEFORE_REPLACE.with(|fail| fail.set(false));
        result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }

    fn scratch() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-durable-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entries(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    #[test]
    fn replaces_existing_content() {
        let dir = scratch();
        let path = dir.join("note.md");
        std::fs::write(&path, "old").unwrap();
        replace(&path, b"new", Mode::Shared).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(entries(&dir), 1, "no temporary is left behind");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_interrupted_replace_keeps_the_previous_good_content() {
        let dir = scratch();
        let path = dir.join("config.json");
        std::fs::write(&path, "previous good").unwrap();
        let error = with_interrupted_replace(|| replace(&path, b"half", Mode::Private));
        assert!(error.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous good");
        assert_eq!(entries(&dir), 1, "the temporary is cleaned up");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_temporary_write_leaves_the_destination_untouched() {
        let dir = scratch();
        let path = dir.join("missing-parent").join("file.json");
        assert!(replace(&path, b"data", Mode::Shared).is_err());
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only_and_shared_files_keep_their_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        let private = dir.join("config.json");
        std::fs::write(&private, "old").unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o644)).unwrap();
        replace(&private, b"secret", Mode::Private).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&private), 0o600);

        let shared = dir.join("note.md");
        std::fs::write(&shared, "old").unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o640)).unwrap();
        replace(&shared, b"new", Mode::Shared).unwrap();
        assert_eq!(mode(&shared), 0o640);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
