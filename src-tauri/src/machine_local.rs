//! Machine-local vault state: everything that must NOT travel with a synced vault.
//!
//! The vault folder is meant to be synced wholesale between machines (ADR-0002), so the
//! boundary here is physical rather than an ignore list: state that is unsafe or useless
//! to share does not live in the vault at all. It lives under the per-machine data
//! directory, keyed by the vault's identity so it survives the vault folder being moved
//! or renamed.
//!
//! What lives here:
//!
//! - `search/` — the Tantivy index. Derived from the notes; each machine builds its own.
//! - `relocation/` — directory-move manifests. Replaying another machine's in-flight
//!   manifest is exactly the data loss the manifests exist to prevent.
//! - `sync_state.json` and `repair_issues.json` — each a record of what *this* machine saw.
//!
//! What deliberately does NOT live here: the staging directory used to publish rewritten
//! notes (`.helixnotes/staging/`). Publishing is an `fs::rename` onto the note's own path,
//! which requires the staged bytes to sit on the vault's filesystem — machine-local
//! staging would fail with `EXDEV` for any vault on a separate partition or mount. Staged
//! bytes are safe to leave in the vault because nothing ever replays them: only a
//! `directory-move.json` manifest authorizes recovery, and those are machine-local. A
//! leftover staging entry is inert garbage, swept on the next vault open.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Filename of the vault's identity marker, inside `.helixnotes/`.
///
/// This is intentionally part of the synced set: it names *the vault*, not the machine,
/// so every machine agrees which machine-local directory belongs to which vault. The
/// per-machine state is already separated by living on a different machine.
const VAULT_ID_FILE: &str = "vault_id";

fn helixnotes_dir(vault_path: &Path) -> PathBuf {
    vault_path.join(".helixnotes")
}

/// Overrides the machine-local root in test builds. See `machine_root`.
#[cfg(all(test, unix))]
pub const TEST_ROOT_VAR: &str = "HELIXNOTES_TEST_MACHINE_ROOT";

/// This process's machine-local root, for a test that must share one with a child process.
///
/// The test adopts whatever root is already in force rather than pinning one of its own:
/// pinning would work only if it happened to run before any other test initialised the
/// root, and fail silently — parent and child disagreeing — if it did not. The child
/// receives this path through `TEST_ROOT_VAR`, which it only ever reads, so no test
/// mutates the environment of a multi-threaded binary.
#[cfg(all(test, unix))]
pub fn test_root() -> PathBuf {
    machine_root().expect("the test machine root is always available")
}

/// Root of all machine-local state, shared by every vault on this machine.
///
/// Tests get a throwaway root per process, so a temp vault's machine-local state never
/// lands in (or leaks out of) the real per-machine directory. `TEST_ROOT_VAR` overrides
/// it so a test can hand the same state to a child process — the crash-recovery test
/// needs the child that dies and the parent that recovers to agree on where manifests live.
#[cfg(test)]
fn machine_root() -> Result<PathBuf, String> {
    #[cfg(unix)]
    if let Ok(root) = std::env::var(TEST_ROOT_VAR) {
        return Ok(PathBuf::from(root));
    }
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    Ok(ROOT
        .get_or_init(|| {
            std::env::temp_dir().join(format!("helixnotes-machine-state-{}", uuid::Uuid::new_v4()))
        })
        .clone())
}

/// Root of all machine-local state, shared by every vault on this machine.
///
/// Mobile injects its sandbox directory at startup because `dirs::data_local_dir()` is
/// not reliable there. There is deliberately no in-vault fallback: failing to open a
/// vault is better than silently writing recovery manifests into a synced folder.
#[cfg(not(test))]
fn machine_root() -> Result<PathBuf, String> {
    if let Some(mobile) = crate::commands::mobile_config_dir() {
        return Ok(mobile.join("helixnotes"));
    }
    dirs::data_local_dir()
        .map(|dir| dir.join("helixnotes"))
        .ok_or_else(|| "No per-machine data directory is available on this system".to_string())
}

/// How long to wait for another process to finish claiming the identity.
///
/// Only a live writer can hold the lock, and writing 36 bytes is instant, so this is a
/// generous allowance for a slow mount rather than a real expectation.
const CLAIM_LOCK_ATTEMPTS: u32 = 50;
const CLAIM_LOCK_WAIT: std::time::Duration = std::time::Duration::from_millis(10);

/// Read the vault's identity, creating it on first open.
///
/// The claim is made under an exclusive file lock rather than by `create_new`. Creating a
/// file and writing it are two steps, so a `create_new` claim is briefly an empty file, and
/// a process killed inside that window left it empty permanently: every later open then
/// waited for a writer that no longer existed, gave up, and failed. The vault became
/// unopenable for good.
///
/// A lock answers the question that waiting could not — is the writer alive? The kernel
/// releases it when its holder dies, so an empty file under a *free* lock is abandoned
/// rather than in flight, and can be claimed. Nothing here trusts the file's contents
/// unless it holds the lock, which is what makes two processes opening the same vault at
/// once still agree on one id.
fn vault_id(vault_path: &Path) -> Result<String, String> {
    let metadata_dir = helixnotes_dir(vault_path);
    std::fs::create_dir_all(&metadata_dir)
        .map_err(|error| format!("Could not create the vault metadata directory: {error}"))?;
    let path = metadata_dir.join(VAULT_ID_FILE);

    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(file) => file,
        // Only *claiming* an identity needs to write. A vault on a read-only mount, or one
        // whose marker is read-only, can still be opened and read, so an id already recorded
        // there is answer enough.
        Err(error) => {
            return std::fs::read_to_string(&path)
                .ok()
                .map(|contents| contents.trim().to_string())
                .filter(|id| is_well_formed_id(id))
                .ok_or_else(|| format!("Could not open the vault identity: {error}"));
        }
    };

    let Some(_claim) = ClaimLock::acquire(&file)? else {
        // The filesystem does not support locking. Fall back to what this did before locks,
        // which is conservative rather than correct: wait out an empty file and fail if it
        // never fills, since without a lock a stalled writer cannot be told from a dead one.
        return unlocked_vault_id(&path);
    };

    if let Some(id) = read_well_formed_id(&file)? {
        return Ok(id);
    }

    // Holding the lock, an empty file is an abandoned claim and a malformed one is damage.
    // Both are replaced, and both are safe to replace: what the id keys is either derived
    // (the index, rebuilt) or inert (staging).
    let replacement = uuid::Uuid::new_v4().to_string();
    write_identity(&file, &replacement)?;
    Ok(replacement)
}

/// The id currently in the file, if it is one.
fn read_well_formed_id(file: &std::fs::File) -> Result<Option<String>, String> {
    use std::io::{Read, Seek};

    let mut handle = file;
    handle
        .rewind()
        .map_err(|error| format!("Could not read the vault identity: {error}"))?;
    let mut contents = String::new();
    handle
        .read_to_string(&mut contents)
        .map_err(|error| format!("Could not read the vault identity: {error}"))?;
    let id = contents.trim().to_string();
    Ok(is_well_formed_id(&id).then_some(id))
}

fn write_identity(file: &std::fs::File, id: &str) -> Result<(), String> {
    use std::io::{Seek, Write};

    let mut handle = file;
    handle
        .rewind()
        .and_then(|()| handle.set_len(0))
        .and_then(|()| handle.write_all(id.as_bytes()))
        .and_then(|()| handle.sync_all())
        .map_err(|error| format!("Could not write the vault identity: {error}"))
}

/// An exclusive claim on the identity file.
///
/// Released on drop, and — the property the design rests on — by the kernel if the process
/// holding it dies without dropping anything.
struct ClaimLock<'a>(&'a std::fs::File);

impl<'a> ClaimLock<'a> {
    /// `Ok(None)` means this filesystem cannot lock, not that the lock was refused.
    fn acquire(file: &'a std::fs::File) -> Result<Option<ClaimLock<'a>>, String> {
        use fs4::FileExt;

        for attempt in 0..CLAIM_LOCK_ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(CLAIM_LOCK_WAIT);
            }
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Some(ClaimLock(file))),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                // Locking is unsupported here. Refusing to open the vault over that would be
                // a worse failure than the one locks are here to fix.
                Err(error) => {
                    log::warn!("Vault identity locking is unavailable: {error}");
                    return Ok(None);
                }
            }
        }
        Err(
            "The vault identity is held by another process that never finished writing it"
                .to_string(),
        )
    }
}

impl Drop for ClaimLock<'_> {
    fn drop(&mut self) {
        use fs4::FileExt;

        let _ = FileExt::unlock(self.0);
    }
}

/// The pre-lock behaviour, kept for filesystems that cannot lock.
fn unlocked_vault_id(path: &Path) -> Result<String, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("Could not read the vault identity: {error}"))?;
    let id = contents.trim().to_string();
    if is_well_formed_id(&id) {
        return Ok(id);
    }
    if id.is_empty() {
        return read_claimed_vault_id(path);
    }
    let replacement = uuid::Uuid::new_v4().to_string();
    std::fs::write(path, &replacement)
        .map_err(|error| format!("Could not repair the vault identity: {error}"))?;
    Ok(replacement)
}

fn is_well_formed_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Read an id another process is in the middle of claiming, without a lock to ask.
///
/// Reachable only on a filesystem that cannot lock. A writer stalled on a slow mount is
/// indistinguishable from a dead one here, so replacing the file on a timeout could let two
/// processes key the same vault to two directories. Failing the open is the safe answer;
/// where locking works, `vault_id` no longer has to make that trade.
fn read_claimed_vault_id(path: &Path) -> Result<String, String> {
    for attempt in 0..CLAIM_LOCK_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(CLAIM_LOCK_WAIT);
        }
        if let Ok(contents) = std::fs::read_to_string(path) {
            let id = contents.trim().to_string();
            if is_well_formed_id(&id) {
                return Ok(id);
            }
        }
    }
    Err("The vault identity was never finished being written".to_string())
}

fn cache() -> &'static Mutex<HashMap<PathBuf, PathBuf>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// This machine's state directory for `vault_path`, created if absent.
///
/// Recreating on demand is what makes deleting the directory outside the app survivable:
/// every caller goes through here, so a missing directory is rebuilt rather than fatal.
pub fn vault_dir(vault_path: &Path) -> Result<PathBuf, String> {
    let cached = cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(vault_path).cloned());
    if let Some(dir) = cached {
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("Could not create machine-local state: {error}"))?;
        return Ok(dir);
    }
    let dir = machine_root()?.join("vaults").join(vault_id(vault_path)?);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Could not create machine-local state: {error}"))?;
    if let Ok(mut cache) = cache().lock() {
        cache.insert(vault_path.to_path_buf(), dir.clone());
    }
    Ok(dir)
}

// Every accessor below shares one contract: the directory it names — or the parent of the
// file it names — exists by the time it returns. That is what lets callers treat a vault
// whose machine-local state was deleted outside the app as an ordinary open.

fn machine_local_dir(vault_path: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = vault_dir(vault_path)?.join(name);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Could not create machine-local {name}: {error}"))?;
    Ok(dir)
}

/// Directory holding in-flight directory-move manifests for this vault, on this machine.
pub fn relocation_dir(vault_path: &Path) -> Result<PathBuf, String> {
    machine_local_dir(vault_path, "relocation")
}

/// Directory holding this machine's Tantivy index for the vault.
pub fn search_dir(vault_path: &Path) -> Result<PathBuf, String> {
    machine_local_dir(vault_path, "search")
}

/// File recording what this machine last saw on the sync remote.
pub fn sync_state_path(vault_path: &Path) -> Result<PathBuf, String> {
    vault_dir(vault_path).map(|dir| dir.join("sync_state.json"))
}

/// File recording repairs this machine's vault needs.
pub fn repair_ledger_path(vault_path: &Path) -> Result<PathBuf, String> {
    vault_dir(vault_path).map(|dir| dir.join("repair_issues.json"))
}

/// Move state that used to live in the vault out to this machine's directory.
///
/// Runs on every vault open and is a no-op once there is nothing left in the vault to
/// move, which is what makes a second open free. Follows the same shape as the WebDAV
/// settings migration in `commands::migrate_global_sync_to_vault`: relocate what exists,
/// leave everything else alone.
pub fn migrate(vault_path: &Path) -> Result<(), String> {
    let metadata_dir = helixnotes_dir(vault_path);
    let destination = vault_dir(vault_path)?;

    // Manifests first: they are the only migrated state whose loss actually costs
    // something, so they move before anything else can fail.
    let legacy_relocation = metadata_dir.join("relocation-recovery");
    if legacy_relocation.is_dir() {
        rescue_legacy_staged_notes(&legacy_relocation, &metadata_dir)?;
        merge_directory(&legacy_relocation, &destination.join("relocation"))?;
    }

    // Derived from the accessors rather than spelled again, so adding machine-local state
    // cannot leave a file behind in the vault because one list was updated and one was not.
    let ledger = repair_ledger_path(vault_path)?;
    for target in [
        sync_state_path(vault_path)?,
        ledger.clone(),
        ledger.with_extension("json.backup"),
    ] {
        let Some(name) = target.file_name() else {
            continue;
        };
        let legacy = metadata_dir.join(name);
        if legacy.is_file() {
            move_path(&legacy, &target)?;
        }
    }

    // Derived state is deleted rather than moved: the index rebuilds from the notes, and
    // carrying a stale copy across would only delay the rebuild.
    let _ = std::fs::remove_dir_all(metadata_dir.join("search_index"));
    let _ = std::fs::remove_file(metadata_dir.join("search_index.version"));
    let _ = std::fs::remove_file(metadata_dir.join("repair_issues.json.tmp"));

    Ok(())
}

/// Return notes stranded in the pre-split recovery directory to the vault's holding area.
///
/// Before #27 one directory served both directory-move manifests and note staging, so a
/// vault upgraded while a note rewrite was in flight can hold that note's *only* copy in a
/// transaction directory. Migrating those bytes to the machine-local directory would strand
/// them: `sweep_staging` only looks in `.helixnotes/staging/`, and recovery only acts on
/// entries carrying a manifest.
///
/// The original path is unknowable — the `origin` marker did not exist yet — so the note
/// goes to the holding area for the user to refile rather than to a guessed location. A
/// transaction from `relocate_file` also published its destination before claiming, so its
/// claimed copy may be a duplicate; surfacing a duplicate in the holding area is the
/// tolerable error here and losing the note is not.
///
/// Every file here is rescued, including one named `replacement.md` — not just whatever
/// isn't. Ordinarily `replacement.md` is the rewritten copy, redundant because the real
/// note is safely still in the tree; but a note genuinely named `replacement.md` collided
/// with that exact file (#31), and *this* ancient layout wrote no origin marker, so there
/// is no way to tell the redundant case from the collided one apart by name or by content.
/// Skipping the redundant case saves nothing worth the risk of silently burying the
/// collided one — this is the same tolerable-duplicate-over-lost-note trade the paragraph
/// above already makes for `relocate_file`'s transactions.
fn rescue_legacy_staged_notes(legacy_relocation: &Path, metadata_dir: &Path) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(legacy_relocation) else {
        return Ok(());
    };
    let holding = metadata_dir.join(crate::vault::para::UNFILED_DIR);
    for entry in entries.flatten() {
        let transaction_dir = entry.path();
        if !transaction_dir.is_dir()
            || crate::vault::relocation::is_manifest_transaction(&transaction_dir)
        {
            continue;
        }
        let Ok(staged) = std::fs::read_dir(&transaction_dir) else {
            continue;
        };
        for file in staged.flatten() {
            let path = file.path();
            let name = file.file_name();
            if !path.is_file() {
                continue;
            }
            std::fs::create_dir_all(&holding)
                .map_err(|error| format!("Could not create the holding area: {error}"))?;
            // Reserve through relocation so a rescued `Plan.md` lands beside an existing one
            // as `Plan 1.md` — the vault's own collision policy — and so the name is claimed
            // atomically rather than tested and then raced for.
            let reserved = crate::vault::relocation::reserve_holding_slot(&holding, &name)?;
            move_onto(&path, &reserved)?;
        }
    }
    Ok(())
}

/// Move every entry of `source` into `destination`, then remove `source`.
///
/// Entries move individually rather than renaming the directory wholesale so a migration
/// interrupted partway through resumes cleanly on the next open: whatever already arrived
/// stays put, and whatever did not is still in the vault to be retried.
fn merge_directory(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("Could not create machine-local state: {error}"))?;
    let entries = std::fs::read_dir(source)
        .map_err(|error| format!("Could not read vault state being migrated: {error}"))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Could not read vault state being migrated: {error}"))?;
        move_path(&entry.path(), &destination.join(entry.file_name()))?;
    }
    let _ = std::fs::remove_dir(source);
    Ok(())
}

/// Move `source` onto `destination`, falling back to copy-and-delete across filesystems.
///
/// The vault and the machine-local directory are routinely on different mounts (an
/// external drive, a NAS), where `rename` fails with `EXDEV`.
fn move_path(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        // Already migrated by an earlier open that was interrupted after the move but
        // before the source was removed. The destination is authoritative.
        let _ = remove_any(source);
        return Ok(());
    }
    move_onto(source, destination)
}

/// Move `source` to `destination`, replacing whatever is there.
///
/// Unlike `move_path` this does not treat an existing destination as a reason to skip: the
/// caller has already reserved that path and owns it. Reusing `move_path` here would see
/// the reservation, decide the move had already happened, and delete the source.
fn move_onto(source: &Path, destination: &Path) -> Result<(), String> {
    if std::fs::rename(source, destination).is_ok() {
        return Ok(());
    }
    copy_recursively(source, destination)
        .map_err(|error| format!("Could not migrate vault state out of the vault: {error}"))?;
    remove_any(source).map_err(|error| format!("Could not clear migrated vault state: {error}"))
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn copy_recursively(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_recursively(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(source, destination).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vault directory with the legacy layout: machine-local state still inside
    /// `.helixnotes/`, exactly as an app built before this change would leave it.
    fn legacy_vault(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("machine-local-{label}-{}", uuid::Uuid::new_v4()));
        let metadata = root.join(".helixnotes");
        std::fs::create_dir_all(metadata.join("relocation-recovery/abc")).unwrap();
        std::fs::write(
            metadata.join("relocation-recovery/abc/directory-move.json"),
            b"{}",
        )
        .unwrap();
        std::fs::write(metadata.join("sync_state.json"), b"{\"files\":{}}").unwrap();
        std::fs::write(metadata.join("repair_issues.json"), b"{\"issues\":[]}").unwrap();
        std::fs::create_dir_all(metadata.join("search_index")).unwrap();
        root
    }

    fn bare_vault(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("machine-local-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".helixnotes")).unwrap();
        root
    }

    /// The bug this file's locking exists for: a process killed between creating the
    /// identity file and writing it leaves an empty file, and before locks that made the
    /// vault unopenable for good rather than for one run.
    #[test]
    fn adopts_an_identity_a_crashed_process_never_finished_writing() {
        let vault = bare_vault("abandoned-claim");
        let path = helixnotes_dir(&vault).join(VAULT_ID_FILE);
        std::fs::write(&path, b"").unwrap();

        let id = vault_id(&vault).expect("an abandoned claim is claimable");

        assert!(is_well_formed_id(&id));
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), id);
    }

    #[test]
    fn keeps_an_identity_that_is_already_written() {
        let vault = bare_vault("existing-id");
        let path = helixnotes_dir(&vault).join(VAULT_ID_FILE);
        std::fs::write(&path, b"11111111-2222-3333-4444-555555555555\n").unwrap();

        assert_eq!(
            vault_id(&vault).unwrap(),
            "11111111-2222-3333-4444-555555555555"
        );
    }

    #[test]
    fn replaces_an_identity_that_is_damaged_rather_than_empty() {
        let vault = bare_vault("damaged-id");
        let path = helixnotes_dir(&vault).join(VAULT_ID_FILE);
        std::fs::write(&path, b"not a vault id -- \x00 garbage").unwrap();

        let id = vault_id(&vault).unwrap();

        assert!(is_well_formed_id(&id));
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), id);
    }

    /// Replacing a damaged id must not leave any of the old bytes behind: the file is
    /// rewritten in place, so a shorter id written over a longer one would otherwise trail
    /// the remainder of what was there.
    #[test]
    fn leaves_no_remnant_of_a_longer_previous_value() {
        let vault = bare_vault("truncation");
        let path = helixnotes_dir(&vault).join(VAULT_ID_FILE);
        std::fs::write(&path, "!".repeat(4096).as_bytes()).unwrap();

        let id = vault_id(&vault).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), id);
    }

    /// Opening the marker for writing is how a claim is made, but reading a vault that
    /// already has one must not depend on being able to.
    #[test]
    #[cfg(unix)]
    fn reads_an_existing_identity_even_when_it_cannot_be_written() {
        use std::os::unix::fs::PermissionsExt;

        let vault = bare_vault("read-only-id");
        let path = helixnotes_dir(&vault).join(VAULT_ID_FILE);
        std::fs::write(&path, b"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();

        let id = vault_id(&vault);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(id.unwrap(), "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    }

    /// What the lock buys, stated as a contrast: the lockless path still cannot tell a dead
    /// writer from a slow one, so it waits and fails — which is what every open of a vault
    /// with an abandoned claim used to do, forever.
    #[test]
    fn without_a_lock_an_abandoned_claim_is_still_indistinguishable_from_a_live_one() {
        let vault = bare_vault("unlocked-fallback");
        let path = helixnotes_dir(&vault).join(VAULT_ID_FILE);
        std::fs::write(&path, b"").unwrap();

        let error = unlocked_vault_id(&path).expect_err("an empty file cannot be adopted");

        assert!(error.contains("never finished being written"));
    }

    /// The guarantee `create_new` used to provide, which the lock now has to carry: many
    /// processes opening one vault at once still settle on a single identity.
    #[test]
    fn concurrent_opens_agree_on_one_identity() {
        let vault = bare_vault("concurrent");

        let ids: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| vault_id(&vault).unwrap()))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });

        let first = &ids[0];
        assert!(
            ids.iter().all(|id| id == first),
            "processes disagreed on the vault identity: {ids:?}"
        );
    }

    #[test]
    fn migration_moves_machine_local_state_out_and_leaves_the_vault_syncable() {
        let vault = legacy_vault("migrate");
        let metadata = helixnotes_dir(&vault);

        migrate(&vault).unwrap();

        let local = vault_dir(&vault).unwrap();
        assert_eq!(
            std::fs::read(local.join("relocation/abc/directory-move.json")).unwrap(),
            b"{}"
        );
        assert!(local.join("sync_state.json").is_file());
        assert!(local.join("repair_issues.json").is_file());

        // Nothing unsafe to sync is left behind.
        assert!(!metadata.join("relocation-recovery").exists());
        assert!(!metadata.join("sync_state.json").exists());
        assert!(!metadata.join("repair_issues.json").exists());
        assert!(!metadata.join("search_index").exists());
        std::fs::remove_dir_all(vault).unwrap();
    }

    /// Before #27 one directory served both manifests and note staging, so an upgrade can
    /// find a note whose only copy is in a transaction directory.
    #[test]
    fn upgrading_mid_rewrite_returns_the_stranded_note_instead_of_burying_it() {
        let vault = legacy_vault("legacy-staging");
        let metadata = helixnotes_dir(&vault);
        let stranded = metadata.join("relocation-recovery/def");
        std::fs::create_dir_all(&stranded).unwrap();
        std::fs::write(stranded.join("replacement.md"), b"derived").unwrap();
        std::fs::write(stranded.join("Plan.md"), b"only copy").unwrap();

        migrate(&vault).unwrap();

        let rescued = metadata
            .join(crate::vault::para::UNFILED_DIR)
            .join("Plan.md");
        assert_eq!(std::fs::read(rescued).unwrap(), b"only copy");
        // The manifest-bearing transaction still migrated normally.
        assert!(vault_dir(&vault)
            .unwrap()
            .join("relocation/abc/directory-move.json")
            .is_file());
        std::fs::remove_dir_all(vault).unwrap();
    }

    /// Two stranded notes sharing a name must both survive, using the vault's own collision
    /// policy rather than a scheme invented for the rescue.
    #[test]
    fn two_stranded_notes_with_the_same_name_both_survive() {
        let vault = legacy_vault("legacy-collision");
        let metadata = helixnotes_dir(&vault);
        for transaction in ["one", "two"] {
            let stranded = metadata.join("relocation-recovery").join(transaction);
            std::fs::create_dir_all(&stranded).unwrap();
            std::fs::write(stranded.join("Plan.md"), transaction.as_bytes()).unwrap();
        }

        migrate(&vault).unwrap();

        let holding = metadata.join(crate::vault::para::UNFILED_DIR);
        let mut rescued: Vec<String> = std::fs::read_dir(&holding)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        rescued.sort();
        assert_eq!(
            rescued,
            vec!["Plan 1.md".to_string(), "Plan.md".to_string()]
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    /// #31: the ancient pre-#27 layout has no origin marker, so there is no way to tell
    /// "replacement.md is the redundant rewritten copy" apart from "replacement.md is the
    /// collided claimed original, wearing the redundant copy's name" — the crash could have
    /// landed on either side of the clobbering rename. A note whose only surviving copy is
    /// named exactly `replacement.md` must still be rescued, not silently folded into
    /// machine-local state on the theory that it was surely the redundant case.
    #[test]
    fn a_stranded_note_literally_named_replacement_md_is_still_rescued() {
        let vault = legacy_vault("legacy-replacement-collision");
        let metadata = helixnotes_dir(&vault);
        let stranded = metadata.join("relocation-recovery").join("collided");
        std::fs::create_dir_all(&stranded).unwrap();
        // Only one file: whichever side of the clobbering rename the crash landed on, the
        // ancient layout leaves exactly one file at this name, and it may be either bytes.
        std::fs::write(stranded.join("replacement.md"), b"only surviving copy").unwrap();

        migrate(&vault).unwrap();

        let rescued = metadata
            .join(crate::vault::para::UNFILED_DIR)
            .join("replacement.md");
        assert_eq!(
            std::fs::read(rescued).unwrap(),
            b"only surviving copy",
            "a note literally named replacement.md must not be assumed redundant and skipped"
        );
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_second_open_migrates_nothing_and_disturbs_nothing() {
        let vault = legacy_vault("idempotent");
        migrate(&vault).unwrap();
        let local = vault_dir(&vault).unwrap();
        // Stand in for state written after the first open; a re-run must not clobber it.
        std::fs::write(local.join("sync_state.json"), b"fresh").unwrap();

        migrate(&vault).unwrap();

        assert_eq!(
            std::fs::read(local.join("sync_state.json")).unwrap(),
            b"fresh"
        );
        assert!(local.join("relocation/abc/directory-move.json").is_file());
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn deleting_the_machine_local_directory_rebuilds_it_instead_of_failing() {
        let vault = legacy_vault("rebuild");
        let local = vault_dir(&vault).unwrap();
        std::fs::remove_dir_all(&local).unwrap();

        // Every accessor recreates what it needs, so the vault still opens.
        assert_eq!(vault_dir(&vault).unwrap(), local);
        assert!(relocation_dir(&vault).unwrap().is_dir());
        assert!(local.is_dir());
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn renaming_the_vault_folder_keeps_the_same_machine_local_state() {
        let vault = legacy_vault("rename");
        let before = vault_dir(&vault).unwrap();

        let moved = vault.with_file_name(format!(
            "{}-moved",
            vault.file_name().unwrap().to_string_lossy()
        ));
        std::fs::rename(&vault, &moved).unwrap();

        // Keyed by the vault's identity, not its path: the state follows the folder.
        assert_eq!(vault_dir(&moved).unwrap(), before);
        std::fs::remove_dir_all(moved).unwrap();
    }
}
