# ADR-0001: Linux global shortcuts go through the XDG portal, not an X11 grab

- **Status**: Accepted
- **Date**: 2026-08-30
- **Context**: Ticket #4 (global hotkey quick-capture)

## Context

Spec §5 makes a system-wide hotkey the entry point to capture, and §11 carried
"Linux support varies across X11/Wayland" forward as an unresolved decision. This ADR
resolves it.

The obvious route is `tauri-plugin-global-shortcut`. It wraps the `global-hotkey` crate,
which documents its Linux support as **X11 only**. The Wayland issue upstream
(tauri-apps/global-hotkey#28) has been open since March 2022 and is unresolved, because
Wayland deliberately has no protocol letting a client grab keys globally — only the
compositor can.

The primary Linux machine for this project runs **GNOME on Wayland** (Fedora 44). So the
plugin's one supported Linux path is the one path that machine does not use.

Running the X11 grab anyway, under XWayland, is worse than not shipping it. An X11 grab
"will only intercept keys sent to XWayland clients, not the whole desktop". Under a
Wayland session the hotkey would fire when an X11 window happened to hold focus and do
nothing otherwise, with no error either way. Ticket #4 explicitly forbids this: "fail
loudly with an explanation rather than appearing to work."

The alternative is `org.freedesktop.portal.GlobalShortcuts`. Verified present and
implemented on the target machine on 2026-08-30:

| Component | Version |
| --- | --- |
| xdg-desktop-portal | 1.22.1 |
| xdg-desktop-portal-gnome | 50.0 |
| GNOME Shell | 50.4 |
| `org.freedesktop.portal.GlobalShortcuts` | interface version 1 |

The GNOME backend implements `org.freedesktop.impl.portal.GlobalShortcuts`, and the
frontend exposes `CreateSession`, `BindShortcuts`, `ListShortcuts`, `ConfigureShortcuts`,
and the `Activated` / `Deactivated` signals. `ashpd` wraps this from Rust.

## Decision

**On Linux, register global shortcuts only through the XDG GlobalShortcuts portal.**
Never fall back to an X11 grab. Where the portal is unavailable, register nothing and
report why.

Three consequences are load-bearing and are part of this decision rather than
implementation detail.

### The app does not own the keybinding

`preferred_trigger` is a hint. The portal "typically result[s] in the portal presenting a
dialog showing the shortcuts and allowing users to configure the shortcuts", and returns a
`trigger_description` string for the app to display. After binding, the app cannot change
the key programmatically; it can only call `ConfigureShortcuts` to open the system UI.

This is why the settings UI is deliberately platform-divergent: on Linux it shows a
read-only trigger and a button into system settings, on Windows a key-capture field. A
uniform in-app capture field was rejected because on Linux it would display a key the
compositor may not have assigned — reintroducing the same "appears to work" failure this
ADR exists to avoid.

### The app id must be reverse-DNS and backed by an installed `.desktop`

Since xdg-desktop-portal 1.20 a non-sandboxed app must call
`org.freedesktop.host.portal.Registry.Register(app_id)` **before any other portal call**,
and since 1.21 `CreateSession` rejects an empty app id. GNOME's backend rejects
non-reverse-DNS identifiers and validates against installed desktop entries.

The bundle identifier therefore changes from `com.helixnotes.app` — upstream's domain,
which would collide with a real HelixNotes install in the shared portal permission store —
to **`io.github.zulucode_design.SecondBrain`**. Four components, lowercase, hyphen
converted to underscore, per Flatpak naming rules for a GitHub-hosted project.

This is safe: config, backups, and the search index all key off a hardcoded `"helixnotes"`
string (`commands.rs`, `backup.rs`, `search/mod.rs`), not the bundle identifier. Renaming
the identifier orphans nothing. Renaming that directory string *would*, and is out of
scope.

A dev build therefore needs a desktop entry before the portal will talk to it — but a
**user-level** entry in `~/.local/share/applications/` is enough, so no packaged install is
required to develop against this. Verified 2026-08-30: without an entry, `Register` fails
with "App info not found" and `CreateSession` with "An app id is required"; with one, both
succeed. `scripts/dev-desktop-entry.sh` installs it.

Two ways to write an entry that GLib silently refuses to load, both reported by the portal
only as "App info not found", and both passed as valid by `desktop-file-validate`: an `Exec`
whose argv[0] does not name an existing program, and an unquoted `Exec` path containing a
space, which truncates argv[0] to the same effect. This repo's own path contains a space.

### Denial is sticky and silent

If the user dismisses the one-time permission dialog, the decision persists in the portal
permission store and later attempts fail silently. Recovery requires
`flatpak permission-reset <app-id>`. The status surface must detect this case specifically
and name the command; a generic "unavailable" strands the user.

Bindings persist per app id across restarts while sessions do not, so startup calls
`CreateSession`, then `ListShortcuts`, and only calls `BindShortcuts` for a shortcut that
is not already bound. Binding unconditionally would prompt on every launch.

## Consequences

- Linux support is defined by portal availability, not session type. An X11 session gets
  it too when the portal answers — a side effect, not a promise. X11 is marked untested.
- Windows keeps `tauri-plugin-global-shortcut`, where the app does own the key and a real
  registration conflict exists to report. That is a separate backend and a separate ticket.
- Failure reporting reuses the `Availability` + `reason` shape established by `ai_health.rs`,
  so an unregistered hotkey reports a specific cause rather than silence.
- The `.desktop` file and app id become a shipping requirement of the feature, not
  packaging polish that can follow later.

## Rejected

| Option | Why rejected |
| --- | --- |
| `tauri-plugin-global-shortcut` on Linux | X11 only; the primary Linux machine is Wayland |
| X11 grab under XWayland as a fallback | Fires only when an X11 window has focus, silently otherwise — the exact failure #4 forbids |
| Portal on Wayland, X11 grab on X11 sessions | Two Linux paths to maintain for a session type this project does not use |
| Uniform in-app key capture on both platforms | On Linux the field would report a binding the compositor never made |
| Keeping `com.helixnotes.app` as the app id | Not a domain this project controls; shares a permission store with upstream |

## References

- `global-hotkey` platform support: https://docs.rs/global-hotkey/latest/global_hotkey/
- Upstream Wayland issue: https://github.com/tauri-apps/global-hotkey/issues/28
- GlobalShortcuts portal: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html
- `ashpd`: https://docs.rs/ashpd/latest/ashpd/desktop/global_shortcuts/index.html
- XWayland grab limitation: https://wayland.freedesktop.org/docs/book/Xwayland.html
- Portal Registry handshake in practice: https://github.com/electron/electron/issues/51875
- App id naming rules: https://docs.flatpak.org/en/latest/conventions.html
