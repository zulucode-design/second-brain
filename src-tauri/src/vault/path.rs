use std::path::{Path, PathBuf};

pub(crate) fn canonicalize(path: &Path, label: &str) -> Result<PathBuf, String> {
    dunce::canonicalize(path).map_err(|error| format!("Invalid {label}: {error}"))
}

/// Serialize a vault-relative path for persisted state and Markdown references.
/// Backslashes are separators only on Windows; on Unix they may be filename content.
pub(crate) fn to_portable_string(path: &Path) -> String {
    let native = path.to_string_lossy();
    #[cfg(windows)]
    {
        native.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        native.into_owned()
    }
}
