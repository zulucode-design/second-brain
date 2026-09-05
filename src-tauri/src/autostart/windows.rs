//! Windows autostart: one value under `HKEY_CURRENT_USER\...\Run`.
//!
//! This key runs its values once, non-interactively, at login — it is not a service and
//! needs no elevation, since `HKEY_CURRENT_USER` is always writable by the user who owns
//! it. Presence is the whole signal, matching the Linux `.desktop` entry: the value
//! existing means "run me at login", removing it means don't. [`sync`] is idempotent for
//! the same reason the Linux version is — it runs on every settings save and every
//! startup, so paying its cost has to be cheap when nothing changed.

use std::io;
use std::path::Path;

use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

use crate::hotkey::ApplicationId;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The value name under the Run key. The app id, matching the convention the Linux entry's
/// filename already uses, though — like that entry — nothing else ever reads this name; it
/// only has to be stable so a repeated `sync` finds and replaces its own value rather than
/// leaving an old one beside a new one.
fn value_name(app_id: &ApplicationId) -> String {
    app_id.to_string()
}

/// Whether what the registry already holds points at `exec`.
///
/// The binary moves — an installer upgrades to a new path — and a value naming the old one
/// does not error, it just silently starts nothing, so a stale value has to be rewritten
/// rather than left alone. Compared as the value Windows will actually execute, quoted the
/// same way [`entry_value`] writes it, not as a bare path: an unquoted comparison would
/// call a path with a space "stale" forever even when it is not.
fn is_current(existing: Option<&str>, exec: &Path) -> bool {
    existing.is_some_and(|value| value == entry_value(exec))
}

/// The command Windows runs at login. Quoted for the same reason the Linux entry's `Exec`
/// is: an unquoted path containing a space would run only the text before the first space
/// as the program, silently failing on any install location with one in it.
fn entry_value(exec: &Path) -> String {
    format!("\"{}\"", exec.display())
}

/// Make the registry match `enabled`, for `app_id` starting `exec`.
///
/// Idempotent both ways: disabling when already absent, and enabling when already current,
/// touch nothing, which is what makes calling this on every settings save and every
/// startup cheap rather than a write nobody asked for.
pub fn sync(enabled: bool, app_id: &ApplicationId, exec: &Path) -> io::Result<()> {
    let run_key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        RUN_KEY,
        winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
    )?;
    let name = value_name(app_id);

    if !enabled {
        match run_key.delete_value(&name) {
            Ok(()) => Ok(()),
            // Disabling when there was never anything to disable is not a failure — the
            // Linux sync treats a missing entry the same way.
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    } else {
        let existing: Option<String> = run_key.get_value(&name).ok();
        if is_current(existing.as_deref(), exec) {
            return Ok(());
        }
        run_key.set_value(&name, &entry_value(exec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These exercise the real per-user registry hive rather than a fake one: HKCU is
    // always writable without elevation, which is the property this module exists to rely
    // on, so a test that could only pass by mocking that away would prove less than one
    // that actually writes and cleans up under a throwaway value name.
    fn scoped_app_id(label: &str) -> ApplicationId {
        ApplicationId::parse(format!("io.github.example.SbAutostartTest{label}"))
            .expect("generated test id is valid")
    }

    fn cleanup(app_id: &ApplicationId) {
        if let Ok(run_key) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_WRITE)
        {
            let _ = run_key.delete_value(value_name(app_id));
        }
    }

    #[test]
    fn an_exec_path_with_a_space_is_quoted() {
        assert_eq!(
            entry_value(Path::new(r"C:\Users\u\My Apps\app.exe")),
            "\"C:\\Users\\u\\My Apps\\app.exe\""
        );
    }

    #[test]
    fn enabling_writes_a_value_that_was_not_there() {
        let app_id = scoped_app_id("Enable");
        cleanup(&app_id);
        let exec = Path::new(r"C:\Program Files\Second Brain\helixnotes.exe");

        sync(true, &app_id, exec).expect("sync writes the value");

        let run_key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .expect("Run key exists");
        let written: String = run_key
            .get_value(value_name(&app_id))
            .expect("the value now exists");
        assert_eq!(written, entry_value(exec));
        cleanup(&app_id);
    }

    #[test]
    fn a_moved_binary_refreshes_the_value_rather_than_leaving_it_stale() {
        let app_id = scoped_app_id("Moved");
        cleanup(&app_id);
        let old_exec = Path::new(r"C:\Program Files\Second Brain Old\helixnotes.exe");
        let new_exec = Path::new(r"C:\Program Files\Second Brain\helixnotes.exe");
        sync(true, &app_id, old_exec).expect("first sync writes");

        sync(true, &app_id, new_exec).expect("second sync refreshes");

        let run_key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .expect("Run key exists");
        let written: String = run_key
            .get_value(value_name(&app_id))
            .expect("the value still exists");
        assert_eq!(written, entry_value(new_exec));
        cleanup(&app_id);
    }

    #[test]
    fn disabling_removes_an_existing_value() {
        let app_id = scoped_app_id("Disable");
        cleanup(&app_id);
        let exec = Path::new(r"C:\Program Files\Second Brain\helixnotes.exe");
        sync(true, &app_id, exec).expect("first sync writes");

        sync(false, &app_id, exec).expect("disabling removes it");

        let run_key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .expect("Run key exists");
        let result: Result<String, _> = run_key.get_value(value_name(&app_id));
        assert!(result.is_err(), "the value should no longer exist");
    }

    #[test]
    fn disabling_when_nothing_was_ever_written_is_not_an_error() {
        let app_id = scoped_app_id("NeverWritten");
        cleanup(&app_id);
        let exec = Path::new(r"C:\Program Files\Second Brain\helixnotes.exe");
        sync(false, &app_id, exec).expect("disabling with nothing there is fine");
    }
}
