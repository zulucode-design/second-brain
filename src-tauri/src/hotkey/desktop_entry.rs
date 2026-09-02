//! Making the app id resolvable when nothing installed a desktop entry for us.
//!
//! The portal will not accept an app id it cannot find an installed `.desktop` for. A deb or
//! rpm ships one (`bundle.linux.{deb,rpm}.files`), so those are fine. An **AppImage installs
//! nothing** — it is a single file the user runs from wherever they put it — so without help
//! the hotkey is permanently unavailable there, reporting `AppIdRejected`.
//!
//! Measured 2026-09-01 on GNOME/Wayland, with no entry present and then with one:
//!
//! ```text
//! Register    : FAILED -> Could not register app ID: App info not found for '...'
//! Register    : OK
//! ```
//!
//! So the fix is for the AppImage to write its own user-level entry, pointing `Exec` at
//! itself. This is the ordinary AppImage self-integration pattern, and a user-level entry in
//! `~/.local/share/applications/` is enough — no root, no install step.
//!
//! The entry is `NoDisplay=true`. It exists to make the app id resolvable, not to launch
//! anything: an AppImage the user runs directly needs no menu item, and a user running
//! AppImageLauncher or `appimaged` already gets one from that, under a mangled basename
//! (`appimagekit_<hash>_HelixNotes.desktop`) that cannot match the app id. Without
//! `NoDisplay` those users would see the app twice. Measured 2026-09-01: the portal accepts
//! an app id whose only entry is `NoDisplay=true`, so hiding it costs nothing.
//!
//! Two ways to write an entry GLib silently refuses to load, both of which the portal reports
//! only as "App info not found": an `Exec` whose argv[0] does not name an existing program,
//! and an unquoted path containing a space. An AppImage lives wherever the user dropped it,
//! which is very often a path with a space in it, so the quoting here is not optional.

use std::path::{Path, PathBuf};

/// Where an AppImage learns its own location. Set by the AppImage runtime, absent otherwise,
/// which is also how we tell we are running as one.
const APPIMAGE_ENV: &str = "APPIMAGE";

/// What was done, so the caller can say something true about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Integration {
    /// Not running from an AppImage. A packaged install ships its own entry.
    NotNeeded,
    /// An entry naming this app id already points at this exact AppImage.
    AlreadyCurrent,
    /// An entry was written, because none existed or it pointed somewhere else.
    Written { path: PathBuf },
}

/// The AppImage's own path, if this process is running from one.
pub fn running_as_appimage() -> Option<PathBuf> {
    std::env::var_os(APPIMAGE_ENV)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Where the entry belongs. Basename must be the app id, or the portal will not match it.
pub fn entry_path(data_home: &Path, app_id: &str) -> PathBuf {
    data_home
        .join("applications")
        .join(format!("{app_id}.desktop"))
}

/// The entry's contents.
///
/// `Exec` is quoted because GLib parses it with shell rules: an unquoted path containing a
/// space truncates argv[0], and GLib then refuses to load the file at all.
pub fn entry_contents(name: &str, exec: &Path, wm_class: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Exec=\"{exec}\"\n\
         Terminal=false\n\
         Categories=Office;\n\
         StartupWMClass={wm_class}\n\
         NoDisplay=true\n",
        exec = exec.display()
    )
}

/// Whether what is on disk already points at this AppImage.
///
/// An AppImage gets moved, renamed, and replaced by a newer download. When it does, the old
/// entry names a path that no longer exists, GLib drops it, and the hotkey stops working with
/// no visible cause — so a stale `Exec` has to be rewritten rather than left alone.
pub fn is_current(existing: Option<&str>, exec: &Path) -> bool {
    let Some(existing) = existing else {
        return false;
    };
    let wanted = format!("Exec=\"{}\"", exec.display());
    existing.lines().any(|line| line.trim_end() == wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_is_named_for_the_app_id_because_the_portal_matches_on_that() {
        let path = entry_path(Path::new("/home/u/.local/share"), "io.github.example.App");
        assert_eq!(
            path,
            PathBuf::from("/home/u/.local/share/applications/io.github.example.App.desktop")
        );
    }

    #[test]
    fn an_exec_path_containing_a_space_is_quoted() {
        // Unquoted, GLib truncates argv[0] at the space and refuses the entry, and the portal
        // then reports only "App info not found".
        let contents = entry_contents(
            "Second Brain",
            Path::new("/home/u/My Apps/SecondBrain.AppImage"),
            "helixnotes",
        );
        assert!(contents.contains("Exec=\"/home/u/My Apps/SecondBrain.AppImage\""));
    }

    #[test]
    fn the_entry_is_hidden_because_it_exists_only_to_name_the_app_id() {
        // AppImageLauncher writes its own visible entry under a mangled basename. Without
        // this the user sees the app twice. Measured: the portal accepts a hidden entry.
        let contents = entry_contents(
            "Second Brain",
            Path::new("/home/u/Apps/SecondBrain.AppImage"),
            "helixnotes",
        );
        assert!(contents.contains("NoDisplay=true"));
    }

    #[test]
    fn an_entry_pointing_at_this_appimage_is_left_alone() {
        let exec = Path::new("/home/u/Apps/SecondBrain.AppImage");
        let existing = entry_contents("Second Brain", exec, "helixnotes");
        assert!(is_current(Some(&existing), exec));
    }

    #[test]
    fn an_entry_pointing_somewhere_else_is_rewritten() {
        // The AppImage was moved or replaced by a new download. The old entry now names a
        // path that does not exist, GLib drops it, and the hotkey dies silently.
        let existing = entry_contents(
            "Second Brain",
            Path::new("/home/u/Downloads/SecondBrain.AppImage"),
            "helixnotes",
        );
        assert!(!is_current(
            Some(&existing),
            Path::new("/home/u/Apps/SecondBrain.AppImage")
        ));
    }

    #[test]
    fn a_missing_entry_is_not_current() {
        assert!(!is_current(
            None,
            Path::new("/home/u/Apps/SecondBrain.AppImage")
        ));
    }
}
