//! Starting the app at login: opt-in, off by default, current-user only.
//!
//! Linux uses the XDG autostart mechanism: a desktop entry in `$XDG_CONFIG_HOME/autostart/`.
//! It needs no admin privilege, and any desktop's own "Startup Applications" tool can see and
//! remove it too, so the app is never the only way to turn this off. Presence is the whole
//! signal — the file existing means "run me at login", removing it means don't — so this
//! module adds or removes the file rather than toggling a property inside it.
//!
//! The stored preference and the entry on disk are reconciled on every settings save and once
//! at startup, rather than trusted to have stayed in sync. A stale `Exec` pointing at a binary
//! that has moved or been replaced fails by doing nothing, forever, with nothing to see — the
//! same silent-failure shape this ticket spent three attempts closing for the hotkey's app id.
//! [`sync`] is idempotent, so paying that cost on every save and every launch is cheap.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::hotkey::ApplicationId;

/// Where the entry belongs. Basename is the app id, matching the convention the hotkey's own
/// desktop entries use, though this is a different mechanism entirely (XDG autostart, not the
/// `GlobalShortcuts` portal) and no portal ever reads this file.
pub fn entry_path(config_home: &Path, app_id: &ApplicationId) -> PathBuf {
    config_home
        .join("autostart")
        .join(format!("{app_id}.desktop"))
}

/// The entry's contents. `Exec` is quoted for the same reason every other entry this app
/// writes is: GLib parses it with shell rules, and an unquoted path containing a space
/// truncates argv[0] and the entry is silently dropped.
pub fn entry_contents(name: &str, exec: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Exec=\"{exec}\"\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec = exec.display()
    )
}

/// Whether what is on disk already points at `exec`.
///
/// The binary moves: an update installs to a new path, an AppImage gets re-downloaded. An
/// entry naming the old location does not error — it just silently starts nothing — so a
/// stale `Exec` has to be rewritten rather than left alone.
pub fn is_current(existing: Option<&str>, exec: &Path) -> bool {
    let Some(existing) = existing else {
        return false;
    };
    let wanted = format!("Exec=\"{}\"", exec.display());
    existing.lines().any(|line| line.trim_end() == wanted)
}

/// Make the entry on disk match `enabled`, for `app_id` starting `exec`.
///
/// Idempotent both ways: disabling when already absent, and enabling when already current,
/// touch nothing. That is what makes calling this on every settings save and every startup
/// cheap rather than a write nobody asked for.
pub fn sync(
    enabled: bool,
    config_home: &Path,
    app_id: &ApplicationId,
    exec: &Path,
) -> io::Result<()> {
    let path = entry_path(config_home, app_id);

    if !enabled {
        if path.exists() {
            fs::remove_file(&path)?;
        }
        return Ok(());
    }

    let existing = fs::read_to_string(&path).ok();
    if is_current(existing.as_deref(), exec) {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, entry_contents("Second Brain", exec))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_id() -> ApplicationId {
        ApplicationId::parse("io.github.example.App").expect("example id is valid")
    }

    fn temp_config_home() -> PathBuf {
        std::env::temp_dir().join(format!("sb-autostart-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn the_entry_is_named_for_the_app_id_under_the_autostart_directory() {
        let path = entry_path(Path::new("/home/u/.config"), &app_id());
        assert_eq!(
            path,
            PathBuf::from("/home/u/.config/autostart/io.github.example.App.desktop")
        );
    }

    #[test]
    fn an_exec_path_with_a_space_is_quoted() {
        let contents = entry_contents("Second Brain", Path::new("/home/u/My Apps/app"));
        assert!(contents.contains("Exec=\"/home/u/My Apps/app\""));
    }

    #[test]
    fn enabling_writes_an_entry_that_was_not_there() {
        let config_home = temp_config_home();
        let exec = Path::new("/opt/second-brain/helixnotes");

        sync(true, &config_home, &app_id(), exec).expect("sync writes the entry");

        let written =
            fs::read_to_string(entry_path(&config_home, &app_id())).expect("the entry now exists");
        assert!(written.contains("Exec=\"/opt/second-brain/helixnotes\""));

        fs::remove_dir_all(&config_home).ok();
    }

    #[test]
    fn enabling_again_with_the_same_exec_does_not_rewrite_the_file() {
        let config_home = temp_config_home();
        let exec = Path::new("/opt/second-brain/helixnotes");
        sync(true, &config_home, &app_id(), exec).expect("first sync writes");
        let path = entry_path(&config_home, &app_id());
        let written_at = fs::metadata(&path).expect("entry exists").modified().ok();

        std::thread::sleep(std::time::Duration::from_millis(10));
        sync(true, &config_home, &app_id(), exec).expect("second sync is a no-op");

        let still_at = fs::metadata(&path)
            .expect("entry still exists")
            .modified()
            .ok();
        assert_eq!(
            written_at, still_at,
            "an unchanged entry must not be rewritten"
        );

        fs::remove_dir_all(&config_home).ok();
    }

    #[test]
    fn a_moved_binary_refreshes_the_entry_rather_than_leaving_it_stale() {
        let config_home = temp_config_home();
        let old_exec = Path::new("/opt/second-brain-old/helixnotes");
        let new_exec = Path::new("/opt/second-brain/helixnotes");
        sync(true, &config_home, &app_id(), old_exec).expect("first sync writes");

        sync(true, &config_home, &app_id(), new_exec).expect("second sync refreshes");

        let written = fs::read_to_string(entry_path(&config_home, &app_id()))
            .expect("the entry still exists");
        assert!(written.contains("Exec=\"/opt/second-brain/helixnotes\""));
        assert!(!written.contains("second-brain-old"));

        fs::remove_dir_all(&config_home).ok();
    }

    #[test]
    fn disabling_removes_an_existing_entry() {
        let config_home = temp_config_home();
        let exec = Path::new("/opt/second-brain/helixnotes");
        sync(true, &config_home, &app_id(), exec).expect("first sync writes");

        sync(false, &config_home, &app_id(), exec).expect("disabling removes it");

        assert!(!entry_path(&config_home, &app_id()).exists());
        fs::remove_dir_all(&config_home).ok();
    }

    #[test]
    fn disabling_when_nothing_was_ever_written_is_not_an_error() {
        let config_home = temp_config_home();
        let exec = Path::new("/opt/second-brain/helixnotes");
        sync(false, &config_home, &app_id(), exec).expect("disabling with nothing there is fine");
        fs::remove_dir_all(&config_home).ok();
    }
}
