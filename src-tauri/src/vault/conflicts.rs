//! Recognition of Syncthing's conflict-copy filenames.
//!
//! This is the one vocabulary boundary shared by note enumeration, search, conflict UI, and
//! publishing. A filename is hidden as a conflict copy only when it matches Syncthing's complete
//! Markdown convention, never merely because a user wrote the marker text in a filename.

use std::path::{Path, PathBuf};

const MARKER: &str = ".sync-conflict-";

/// Return the original Markdown path for a valid Syncthing conflict copy.
pub(crate) fn original_for_conflict(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".md")?;
    let (original_stem, suffix) = stem.rsplit_once(MARKER)?;
    let mut parts = suffix.split('-');
    let (Some(date), Some(time), Some(device), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if original_stem.is_empty()
        || date.len() != 8
        || time.len() != 6
        || !date.bytes().all(|byte| byte.is_ascii_digit())
        || !time.bytes().all(|byte| byte.is_ascii_digit())
        || device.is_empty()
        || !device.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(path.with_file_name(format!("{original_stem}.md")))
}

/// Whether `path` is a valid Syncthing Markdown conflict copy.
pub(crate) fn is_conflict_copy(path: &Path) -> bool {
    original_for_conflict(path).is_some()
}

#[cfg(test)]
mod tests {
    use super::{is_conflict_copy, original_for_conflict};
    use std::path::Path;

    #[test]
    fn recognises_only_the_complete_syncthing_markdown_pattern() {
        let conflict = Path::new("Projects/Plan.sync-conflict-20260913-142233-ABCDEF.md");
        assert!(is_conflict_copy(conflict));
        assert_eq!(
            original_for_conflict(conflict),
            Some(Path::new("Projects/Plan.md").to_path_buf())
        );

        for ordinary_file in [
            "Projects/Plan.sync-conflict-draft.md",
            "Projects/Plan.sync-conflict-20260913-14223-ABCDEF.md",
            "Projects/Plan.sync-conflict-20260913-142233-.md",
            "Projects/Plan.sync-conflict-20260913-142233-ABC-DEF.md",
            "Projects/Plan.sync-conflict-20260913-142233-ABCDEF.txt",
        ] {
            assert!(
                !is_conflict_copy(Path::new(ordinary_file)),
                "{ordinary_file} must remain an ordinary file"
            );
        }
    }
}
