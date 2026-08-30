use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Relocate a regular file without ever replacing an existing destination.
///
/// The destination name is reserved with `create_new`, so concurrent relocations cannot
/// choose the same path. The source is removed only after the destination is complete;
/// on failure, cleanup is attempted and the source remains the recoverable copy.
pub fn relocate_file(
    source: &Path,
    destination_dir: &Path,
    preferred_name: &OsStr,
    replacement_contents: Option<&[u8]>,
) -> Result<PathBuf, String> {
    let source_metadata = fs::metadata(source).map_err(|error| error.to_string())?;
    if !source_metadata.is_file() {
        return Err("Relocation source must be a regular file".to_string());
    }

    fs::create_dir_all(destination_dir).map_err(|error| error.to_string())?;
    let preferred_path = Path::new(preferred_name);
    let stem = preferred_path
        .file_stem()
        .unwrap_or_else(|| OsStr::new("note"))
        .to_string_lossy();
    let extension = preferred_path.extension().map(|value| value.to_owned());

    for suffix in 0usize.. {
        let filename = if suffix == 0 {
            preferred_name.to_owned()
        } else if let Some(extension) = &extension {
            OsStr::new(&format!(
                "{} {}.{}",
                stem,
                suffix,
                extension.to_string_lossy()
            ))
            .to_owned()
        } else {
            OsStr::new(&format!("{} {}", stem, suffix)).to_owned()
        };
        let destination = destination_dir.join(filename);

        let mut destination_file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        };

        let write_result = match replacement_contents {
            Some(contents) => destination_file.write_all(contents),
            None => File::open(source)
                .and_then(|mut source_file| io::copy(&mut source_file, &mut destination_file))
                .map(|_| ()),
        }
        .and_then(|_| destination_file.flush());
        drop(destination_file);

        if let Err(error) = write_result {
            let _ = fs::remove_file(&destination);
            return Err(format!("Could not write relocated note: {error}"));
        }

        if let Err(error) = fs::set_permissions(&destination, source_metadata.permissions()) {
            let _ = fs::remove_file(&destination);
            return Err(format!(
                "Could not preserve relocated note permissions: {error}"
            ));
        }

        if let Err(error) = fs::remove_file(source) {
            let cleanup = fs::remove_file(&destination);
            return match cleanup {
                Ok(()) => Err(format!("Could not remove relocation source: {error}")),
                Err(cleanup_error) => Err(format!(
                    "Could not remove relocation source ({error}); a recoverable duplicate remains at {} because cleanup also failed: {cleanup_error}",
                    destination.display()
                )),
            };
        }

        return Ok(destination);
    }

    unreachable!("the filename suffix space is effectively unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use uuid::Uuid;

    #[test]
    fn concurrent_relocations_never_overwrite_each_other() {
        let root = std::env::temp_dir().join(format!("note-relocation-race-{}", Uuid::new_v4()));
        let first_dir = root.join("first");
        let second_dir = root.join("second");
        let destination = root.join("destination");
        fs::create_dir_all(&first_dir).unwrap();
        fs::create_dir_all(&second_dir).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let first = first_dir.join("Plan.md");
        let second = second_dir.join("Plan.md");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();

        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = [first, second]
            .into_iter()
            .map(|source| {
                let destination = destination.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    relocate_file(&source, &destination, OsStr::new("Plan.md"), None).unwrap()
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

        assert_ne!(relocated[0], relocated[1]);
        assert_eq!(contents, ["first", "second"]);
        assert!(destination.join("Plan.md").is_file());
        assert!(destination.join("Plan 1.md").is_file());
        fs::remove_dir_all(root).unwrap();
    }
}
