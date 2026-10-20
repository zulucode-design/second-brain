//! The capture-time similarity check (#9): after a note is saved, find existing notes that say
//! the same thing, so the user can fold the new one in instead of keeping a near-duplicate.
//!
//! The check only ever runs after the note is safely on disk, and every failure ends it quietly:
//! a capture must never wait on it or be lost to it. A check that could not run is kept, and
//! runs again once the semantic index has caught up (#216).

use crate::semantic_search::SemanticIndex;
use crate::state::AppState;
use crate::vault::operations;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
use tauri::{AppHandle, Emitter, Manager};

/// The score a note must reach to count as saying the same thing. Calibrated against live
/// embeddinggemma on scripts/similarity-fixture.json (#9). With captures as a title and a body
/// (#213), its 8 reworded duplicates scored 0.778 to 0.917 against their notes, and no other
/// pair, same-topic notes included, passed 0.571. The bar sits in that gap. Recalibrate when the
/// embedding model changes.
const SIMILAR_SCORE: f32 = 0.65;
/// How many similar notes a card shows; the rest are only counted.
const SHOWN_PER_CARD: usize = 3;
/// Shorter text embeds too loosely to call anything a duplicate.
const MIN_WORDS: usize = 3;

/// A just-written note and the existing notes most like it, each with the revision it had when
/// checked, so nothing is appended to a note that changed since the user saw it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarityCheck {
    /// The vault the check ran in, so a result that lands after a vault switch is dropped.
    pub vault: String,
    pub capture: CheckedNote,
    pub matches: Vec<SimilarMatch>,
    /// Every note that passed the bar, of which `matches` holds the best few.
    pub total: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckedNote {
    pub path: String,
    pub title: String,
    pub revision: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarMatch {
    #[serde(flatten)]
    pub note: CheckedNote,
    /// The passage that matched best: why the note was picked.
    pub excerpt: String,
}

/// Why [`check`] judged nothing.
#[derive(Debug)]
pub enum CheckError {
    /// The note at the path has another id, or none: the note checked for is elsewhere, and
    /// nothing says the backend is away.
    AnotherNote,
    /// The note could not be read, or the index or the embedding backend failed. Without the
    /// cause, which nothing logs: an embedding failure carries the backend's response body.
    Failed,
}

/// The existing notes most like the note at `path`, or `None` when there are none or the note
/// is too short to judge. With `id` not empty, the note at `path` must have that frontmatter id:
/// another note there is [`CheckError::AnotherNote`], never a note judged in its place.
pub fn check(
    index: &SemanticIndex,
    vault: &str,
    path: &str,
    id: &str,
) -> Result<Option<SimilarityCheck>, CheckError> {
    let note = operations::read_note(vault, path).map_err(|_| CheckError::Failed)?;
    // Compared on the read that is judged, so a note that takes `path` after this one is renamed
    // never stands in for it.
    if !id.is_empty() && note.meta.id != id {
        return Err(CheckError::AnotherNote);
    }
    let words =
        note.meta.title.split_whitespace().count() + note.content.split_whitespace().count();
    if words < MIN_WORDS {
        return Ok(None);
    }
    let found = index
        .similar(
            &note.meta.title,
            &note.content,
            &note.meta.id,
            SIMILAR_SCORE,
        )
        .map_err(|_| CheckError::Failed)?;
    #[cfg(test)]
    AFTER_SCORING.with_borrow_mut(|after| after.as_mut().map(|after| after()));
    let mut matches: Vec<SimilarMatch> = found
        .into_iter()
        // A note without an id is indexed by its path, which `similar` cannot exclude.
        .filter(|similar| !same_path(&similar.found.path, path))
        // Deleted or unreadable since it was indexed: nothing to append to. Changed since: the
        // passage may no longer be in it, and the card would vouch for text the user never saw.
        .filter_map(|similar| {
            let existing = operations::read_note(vault, &similar.found.path).ok()?;
            (existing.revision == similar.indexed_revision).then_some(SimilarMatch {
                note: CheckedNote {
                    path: similar.found.path,
                    title: similar.found.title,
                    revision: existing.revision,
                },
                excerpt: similar.found.snippet,
            })
        })
        .collect();
    if matches.is_empty() {
        return Ok(None);
    }
    // Counted after the drops, so the card never counts a note it cannot show.
    let total = matches.len();
    matches.truncate(SHOWN_PER_CARD);
    Ok(Some(SimilarityCheck {
        vault: vault.to_string(),
        capture: CheckedNote {
            path: note.path,
            title: note.meta.title,
            revision: note.revision,
        },
        total,
        matches,
    }))
}

/// [`check`] for the note at `path` in `vault`, the vault it was saved in, whose frontmatter id
/// is `id` (empty when it has none), with every failure treated as "nothing found": no index, an
/// unreachable embedding backend, or a note that vanished. The check is kept in that vault's list
/// while it waits its turn or runs, and stays kept when [`check`] itself fails, as it does while
/// the backend is unreachable or another note has taken the note's path, or when another vault
/// is open by then, to run once that vault's index has caught up (#216). Reruns leave it to this
/// first attempt, whose card a quick capture's notification follows, until the attempt ends.
pub fn check_now(state: &AppState, vault: &str, path: &str, id: &str) -> Option<SimilarityCheck> {
    // Without a semantic index nothing could run it, now or later.
    if state.semantic_index.lock().ok()?.is_none() {
        return None;
    }
    // The id the caller had when the note was saved or opened, not one read now: the note may
    // have been renamed or moved since, and the id still finds it.
    let kept = DeferredCheck {
        id: id.to_string(),
        path: path.to_string(),
    };
    // Marked before it is kept, so no rerun takes it from this attempt.
    let first = FirstAttempt::start(vault, &kept);
    // Kept before it waits its turn or runs, so quitting while it waits behind another check,
    // or on the backend's timeout, still leaves it to run later.
    with_kept(state, vault, |checks| checks.keep(kept.clone()));
    #[cfg(test)]
    BEFORE_TURN.with_borrow_mut(|before| before.as_mut().map(|before| before()));
    // Dropped in this order: a check left kept is the reruns' before the next turn starts.
    let _turn = (
        first,
        CHECKING.lock().unwrap_or_else(PoisonError::into_inner),
    );
    // A rerun already checking the note when this was kept, for an earlier check of it, ran it
    // and showed its card.
    let mut waiting = false;
    with_kept(state, vault, |checks| waiting = checks.holds(&kept));
    if !waiting {
        return None;
    }
    // Another vault's index cannot judge the note; its own vault's list keeps the check. Run
    // under the open vault's path, which its card is shown for.
    let (index, open) = open_index(state)?;
    if !same_path(&open, vault) {
        return None;
    }
    // Judged only while it is still the note at `path`: another note there leaves the check kept,
    // for a rerun to find this one by its id.
    match check(&index, &open, path, id) {
        Ok(found) => {
            with_kept(state, vault, |checks| checks.forget(&[&kept]));
            found
        }
        Err(_) => {
            // Without the error: an embedding failure carries the backend's response body,
            // which docs/log-privacy.md keeps out of the log.
            log::info!("Skipped the similarity check for a new note");
            None
        }
    }
}

/// Whether two paths name the same file or folder, as a vault opened through a link and
/// through its target do, and their notes.
fn same_path(one: &str, other: &str) -> bool {
    one == other
        || matches!(
            (dunce::canonicalize(one), dunce::canonicalize(other)),
            (Ok(one), Ok(other)) if one == other
        )
}

/// Whether the note at `path` is in `vault`, compared as resolved paths, since a vault opened
/// through a link keeps the link in its path while a note can carry the resolved one, and
/// whether the app has opened that vault: keeping a check creates the vault's machine-local
/// state, and nothing should appear in a folder that is not a vault. A note no longer at `path`,
/// renamed or moved since, alone or with the folders above it, is placed by the nearest folder
/// above it that is still there: its check is kept by its id.
pub(crate) fn note_in_opened_vault(vault: &str, path: &str) -> bool {
    let (Ok(root), Some(note)) = (
        dunce::canonicalize(vault),
        resolve_what_is_there(Path::new(path)),
    ) else {
        return false;
    };
    note.starts_with(&root) && operations::helixnotes_dir(vault).is_dir()
}

/// `path` with the nearest of it and the folders above it that is still there resolved, and the
/// names below that one, which are gone, after it. `None` when what is gone could lead anywhere
/// once resolved: a `..` below a folder that is gone, which nothing can resolve yet, or something
/// there that does not resolve, such as a link to nothing.
fn resolve_what_is_there(path: &Path) -> Option<PathBuf> {
    let mut gone = Vec::new();
    let mut at = path;
    loop {
        if let Ok(mut resolved) = dunce::canonicalize(at) {
            resolved.extend(gone.iter().rev());
            return Some(resolved);
        }
        match std::fs::symlink_metadata(at) {
            // Gone: the folder above it places it.
            Err(error) if is_gone(&error) => {}
            // There, yet it does not resolve: a link to nothing, which could point anywhere once
            // its target appears, or an entry that cannot be read, which could be one.
            _ => return None,
        }
        // A `..` has no name: below a folder that is gone, it could climb anywhere.
        gone.push(at.file_name()?);
        at = at.parent()?;
    }
}

/// The file in the vault's machine-local folder that keeps deferred checks across a restart.
const DEFERRED_FILE: &str = "deferred_similarity.json";

/// A note whose similarity check is running, or failed and is waiting to run (#216).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DeferredCheck {
    /// The note's frontmatter id, which finds it again after a rename or a move. Empty when it
    /// has none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) id: String,
    /// Where the note was when its check was kept: how a note without an id is found.
    pub(crate) path: String,
}

impl DeferredCheck {
    fn same_note(&self, other: &Self) -> bool {
        if self.id.is_empty() || other.id.is_empty() {
            self.path == other.path
        } else {
            self.id == other.id
        }
    }
}

/// The open vault's deferred checks, oldest first (#216). Every change rewrites a file in the
/// vault's machine-local folder, never inside the vault, so a restart keeps them and sync never
/// carries them.
#[derive(Debug, Default)]
pub struct DeferredChecks {
    /// The vault they belong to. `None` until a vault opens.
    vault: Option<String>,
    checks: Vec<DeferredCheck>,
    /// The file was there but could not be read: it is read again before anything is written
    /// over it, so an error that passes never costs the checks it holds.
    unread: bool,
    /// Notes whose checks ran or were dropped while the file could not be read, so its older
    /// checks for them do not come back.
    forgotten: Vec<DeferredCheck>,
}

/// The file's shape: `{"checks": [...]}`.
#[derive(Deserialize, Serialize)]
struct DeferredFile<C> {
    checks: C,
}

fn deferred_file(vault: &str) -> Result<PathBuf, String> {
    crate::machine_local::vault_dir(Path::new(vault)).map(|dir| dir.join(DEFERRED_FILE))
}

/// The checks in `vault`'s file. A missing file holds none, and so does a damaged one, which
/// the next save replaces. `None` when the file is there but cannot be read just now.
fn read_kept(vault: &str) -> Option<Vec<DeferredCheck>> {
    match std::fs::read(deferred_file(vault).ok()?) {
        Ok(bytes) => Some(
            serde_json::from_slice::<DeferredFile<Vec<DeferredCheck>>>(&bytes)
                .map(|file| file.checks)
                .unwrap_or_else(|_| {
                    log::warn!("The deferred similarity checks were damaged, so none are waiting");
                    Vec::new()
                }),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
        Err(_) => None,
    }
}

impl DeferredChecks {
    /// The checks kept for `vault`. A missing or damaged file is an empty list, and so is one
    /// that cannot be read, with one log line: a lost deferred check never stops a vault from
    /// opening.
    pub fn load(vault: &str) -> Self {
        let read = read_kept(vault);
        if read.is_none() {
            // The outcome only: the error can name the file.
            log::warn!("The deferred similarity checks could not be read, so none are waiting");
        }
        Self {
            vault: Some(vault.to_string()),
            unread: read.is_none(),
            checks: read.unwrap_or_default(),
            forgotten: Vec::new(),
        }
    }

    /// Makes `vault`'s checks the open ones, read from its file. Reopening the open vault keeps
    /// the list it has: a check kept since the file was read is only there.
    pub fn open(&mut self, vault: &str) {
        if !self
            .vault
            .as_deref()
            .is_some_and(|open| same_path(open, vault))
        {
            *self = Self::load(vault);
        }
    }

    /// Reads the file again when it could not be read before, its checks ahead of any made
    /// since, which replace their note's, and without those that ran or were dropped since.
    /// False while it still cannot be read.
    fn reread(&mut self) -> bool {
        if !self.unread {
            return true;
        }
        let Some(older) = self.vault.as_deref().and_then(read_kept) else {
            return false;
        };
        let mut checks: Vec<DeferredCheck> = older
            .into_iter()
            .filter(|old| {
                !self.holds(old) && !self.forgotten.iter().any(|gone| gone.same_note(old))
            })
            .collect();
        checks.append(&mut self.checks);
        self.checks = checks;
        self.unread = false;
        self.forgotten.clear();
        true
    }

    /// Reads a file that could not be read before and writes it back at once, without the checks
    /// that ran or were dropped since, so a restart does not bring them back.
    fn recover(&mut self) {
        if self.unread && self.reread() {
            self.save();
        }
    }

    /// Rewrites the file. When that fails, the checks still wait in memory until the app quits.
    fn save(&mut self) {
        let Some(vault) = self.vault.clone() else {
            return;
        };
        // Still unreadable, the file keeps what it holds, and these wait in memory.
        if !self.reread() {
            log::warn!("Could not save the deferred similarity checks");
            return;
        }
        let saved = deferred_file(&vault).and_then(|file| {
            let bytes = serde_json::to_vec_pretty(&DeferredFile {
                checks: &self.checks,
            })
            .map_err(|error| error.to_string())?;
            crate::durable::replace(&file, &bytes, crate::durable::Mode::Private)
        });
        if saved.is_err() {
            // Without the error, which can name the file.
            log::warn!("Could not save the deferred similarity checks");
        }
    }

    /// Keeps `check` as the newest, in place of an earlier one for the same note.
    fn keep(&mut self, check: DeferredCheck) {
        self.checks.retain(|kept| !kept.same_note(&check));
        self.checks.push(check);
        self.save();
    }

    /// Whether a check is kept for `check`'s note.
    fn holds(&self, check: &DeferredCheck) -> bool {
        self.checks.iter().any(|kept| kept.same_note(check))
    }

    /// Drops the checks kept for these notes: they ran, or the note is gone.
    fn forget(&mut self, done: &[&DeferredCheck]) {
        self.checks
            .retain(|kept| !done.iter().any(|check| kept.same_note(check)));
        if self.unread {
            self.forgotten
                .extend(done.iter().map(|check| (*check).clone()));
        }
        self.save();
    }

    #[cfg(test)]
    pub(crate) fn checks(&self) -> &[DeferredCheck] {
        &self.checks
    }
}

/// Runs `with` on `vault`'s deferred checks.
fn with_kept(state: &AppState, vault: &str, with: impl FnOnce(&mut DeferredChecks)) {
    let Ok(mut open) = state.deferred_checks.lock() else {
        return;
    };
    match open
        .vault
        .as_deref()
        .map(|open_vault| same_path(open_vault, vault))
    {
        Some(true) => with(&mut open),
        // The vault was switched while the check ran. The check belongs to the vault it ran in,
        // whose file keeps it for that vault's next open. The open list stays locked meanwhile,
        // so reopening that vault cannot read the file between this read and write.
        Some(false) => with(&mut DeferredChecks::load(vault)),
        None => {
            *open = DeferredChecks::load(vault);
            with(&mut open);
        }
    }
}

/// Where a deferred check's note is now.
enum Location {
    At(String),
    /// Deleted: neither the index nor the vault has it any more.
    Gone,
    /// Not readable where the index or the record puts it, and not shown gone either: the file
    /// is there but cannot be read just now, the index has not caught up yet with a move made
    /// while the app was closed, a note the vault scan could not open may be it, or a bulk
    /// change came or went while it scanned. It is looked for again after the next pass.
    Unsure,
}

/// Every note's frontmatter id and path, and whether the walk read every folder and note: one
/// it could not may be a deferred check's note, moved.
type Scan = (Vec<(String, String)>, bool);

/// Finds a deferred check's note: by its id through the index, then at its recorded path, while
/// `settled_notes` holds the note lock, then by its id across the vault, scanned into `scanned`
/// once for every check of a rerun after it lets the lock go. `leases` is the bulk leases granted,
/// read under that lock.
fn locate(
    state: &AppState,
    index: &SemanticIndex,
    vault: &str,
    deferred: &DeferredCheck,
    settled_notes: MutexGuard<'_, ()>,
    leases: usize,
    scanned: &mut Option<Scan>,
) -> Result<Location, String> {
    let indexed = if deferred.id.is_empty() {
        Vec::new()
    } else {
        index.paths_of_note(&deferred.id)?
    };
    let mut unsure = !indexed.is_empty();
    for path in indexed.iter().chain(std::iter::once(&deferred.path)) {
        // Not `read_note`, which reads the trash too: a note in the trash has left the vault.
        match operations::read_vault_note(vault, path) {
            Ok(note) if deferred.id.is_empty() || note.meta.id == deferred.id => {
                return Ok(Location::At(path.clone()));
            }
            // Another note has that path now.
            Ok(_) => {}
            Err(_) if !absent(path) => unsure = true,
            Err(_) => {}
        }
    }
    if unsure {
        return Ok(Location::Unsure);
    }
    // The index can miss a note moved while the app was closed, so the vault has the last word
    // before the check is dropped.
    if !deferred.id.is_empty() {
        // Without the lock, since a save waits for it on the main thread.
        drop(settled_notes);
        let (notes, complete) = scanned.get_or_insert_with(|| {
            let (notes, complete) = scan_ids(vault);
            (
                notes,
                complete && notes_still_settled(state, leases).is_some(),
            )
        });
        if let Some((_, path)) = notes.iter().find(|(id, _)| *id == deferred.id) {
            return Ok(Location::At(path.clone()));
        }
        if !*complete {
            return Ok(Location::Unsure);
        }
    }
    Ok(Location::Gone)
}

/// Nothing is at `path`, or nothing a note could be: a file there that cannot be read, or a
/// folder above it that cannot be, does not count.
fn absent(path: &str) -> bool {
    match std::fs::metadata(path) {
        Ok(metadata) => !metadata.is_file(),
        Err(error) => is_gone(&error),
    }
}

/// Whether `error`, from reaching a path, says nothing is there: the entry is missing, or what
/// is above it is no longer a folder. One test, so placing a check and dropping one agree on
/// what is gone.
fn is_gone(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

#[cfg(test)]
thread_local! {
    /// Runs as a vault scan starts on this thread, so a test can act while it runs.
    pub(crate) static DURING_SCAN: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
    /// Runs once a check on this thread has scored its note, so a test can act before it ends.
    pub(crate) static AFTER_SCORING: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
    /// Runs once [`check_now`] on this thread has kept its check, before it takes its turn.
    pub(crate) static BEFORE_TURN: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
    /// Runs once a rerun on this thread has found a kept check's note, before it checks it.
    pub(crate) static AFTER_LOCATING: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Walks the vault's notes as the index sees them, each read in full: the note list's scan
/// reads only the start of a note, which can cut a long frontmatter short of its id.
fn scan_ids(vault: &str) -> Scan {
    #[cfg(test)]
    DURING_SCAN.with_borrow_mut(|during| during.as_mut().map(|during| during()));
    let root = Path::new(vault);
    let (mut ids, mut complete) = (Vec::new(), true);
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !crate::search::is_ignored_by_index(entry.path(), root))
    {
        let Ok(entry) = entry else {
            complete = false;
            continue;
        };
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("md")
        {
            continue;
        }
        match std::fs::read(path) {
            Ok(raw) => {
                let filename = entry.file_name().to_string_lossy();
                let (meta, _) = crate::vault::frontmatter::parse_note(
                    &String::from_utf8_lossy(&raw),
                    &filename,
                );
                ids.push((meta.id, path.to_string_lossy().to_string()));
            }
            Err(_) => complete = false,
        }
    }
    (ids, complete)
}

/// The note lock, unless something holds it: a save for a moment, or a bulk change for as long
/// as it moves notes.
fn notes_settled(state: &AppState) -> Option<MutexGuard<'_, ()>> {
    match state.note_mutation.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

/// [`notes_settled`], unless a restore, import or sync batch has been granted a lease since
/// `leases` was read under the note lock: one that came and went can have hidden notes meanwhile.
fn notes_still_settled(state: &AppState, leases: usize) -> Option<MutexGuard<'_, ()>> {
    notes_settled(state).filter(|_held| state.bulk_mutation.granted() == leases)
}

/// The open vault and its semantic index, one lock at a time: renaming a notebook holds `config`
/// while it takes the index slot. The slot read again shows no vault switch came between the two.
fn open_index(state: &AppState) -> Option<(Arc<SemanticIndex>, String)> {
    let index = state.semantic_index.lock().ok()?.clone()?;
    let vault = state.config.lock().ok()?.active_vault.clone()?;
    let still = state.semantic_index.lock().ok()?.clone()?;
    Arc::ptr_eq(&index, &still).then_some((index, vault))
}

/// One run of kept checks at a time, so two passes that finish close together never run a check
/// twice, and a rerun never runs one that [`check_now`] is still running.
static CHECKING: Mutex<()> = Mutex::new(());

/// The checks [`check_now`] has kept and not yet ended, each with its vault: reruns leave them to
/// it, since a rerun's card never notifies (#216). Never saved, so after a restart every kept
/// check is the reruns'.
static FIRST_ATTEMPTS: Mutex<Vec<(String, DeferredCheck)>> = Mutex::new(Vec::new());

/// A check in [`FIRST_ATTEMPTS`] until dropped.
struct FirstAttempt((String, DeferredCheck));

impl FirstAttempt {
    fn start(vault: &str, check: &DeferredCheck) -> Self {
        let attempt = (vault.to_string(), check.clone());
        first_attempts().push(attempt.clone());
        Self(attempt)
    }
}

impl Drop for FirstAttempt {
    fn drop(&mut self) {
        let mut attempts = first_attempts();
        if let Some(at) = attempts.iter().position(|attempt| *attempt == self.0) {
            attempts.remove(at);
        }
    }
}

fn first_attempts() -> MutexGuard<'static, Vec<(String, DeferredCheck)>> {
    FIRST_ATTEMPTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Whether `check`, kept for `vault`, waits on its first attempt.
fn in_first_attempt(vault: &str, check: &DeferredCheck) -> bool {
    first_attempts()
        .iter()
        .any(|(kept_in, attempt)| attempt.same_note(check) && same_path(kept_in, vault))
}

/// Runs each of the open vault's deferred checks once, oldest first, on the note's current
/// text, and hands every result with matches to `deliver` (#216). A check that runs leaves the
/// list, matches or not; one that fails again stays; one whose path another note took once it
/// was found stays, and the rest still run; a deleted note's is dropped; one whose first
/// attempt in [`check_now`] has not ended is left to it. They stop, and the rest wait for the
/// next rerun, once the index may no longer be caught up: a rebuild or reconcile started since it
/// caught up or stopped part way, a note was queued or saved since, or settings replaced it. They
/// stop too when something holds the note lock as a check looks for its note or ends, or a bulk
/// change came and went while a check ran.
pub(crate) fn rerun_deferred_checks(state: &AppState, mut deliver: impl FnMut(SimilarityCheck)) {
    let _checking = CHECKING.lock().unwrap_or_else(PoisonError::into_inner);
    let Some((index, vault)) = open_index(state) else {
        return;
    };
    let waiting = match state.deferred_checks.lock() {
        Ok(mut list)
            if list
                .vault
                .as_deref()
                .is_some_and(|open| same_path(open, &vault)) =>
        {
            // A file that could not be read when the vault opened gives up its checks once it can.
            list.recover();
            list.checks.clone()
        }
        _ => return,
    };
    if waiting.is_empty() {
        return;
    }
    let mut settled = Vec::new();
    let (mut found, mut dropped) = (0, 0);
    let mut scanned = None;
    for deferred in &waiting {
        // Its first attempt runs it, and can notify for a quick capture.
        if in_first_attempt(&vault, deferred) {
            continue;
        }
        // A rebuild or reconcile started since the index caught up can leave it short of notes,
        // so this check and the rest wait until the queue is through again.
        let Some(mark) = index.caught_up_mark() else {
            index.owe_caught_up();
            break;
        };
        // A restore, an import or a sync batch holds the note lock while it moves notes, and can
        // leave some missing until it ends: located only while it is free, settled only if none
        // came or went meanwhile, and otherwise this check and the rest wait for the next pass.
        let Some(settled_notes) = notes_settled(state) else {
            index.owe_caught_up();
            break;
        };
        let leases = state.bulk_mutation.granted();
        match locate(
            state,
            &index,
            &vault,
            deferred,
            settled_notes,
            leases,
            &mut scanned,
        ) {
            Ok(Location::At(path)) => {
                #[cfg(test)]
                AFTER_LOCATING.with_borrow_mut(|after| after.as_mut().map(|after| after()));
                // Its id compared again on the read that is judged: `locate` has let the note
                // lock go, and a note written at `path` from outside the app since would
                // otherwise be judged in its place.
                match check(&index, &vault, &path, &deferred.id) {
                    // The mark is read under the lock, so a bulk change cannot reconcile between
                    // it and the lease count.
                    Ok(_)
                        if !notes_still_settled(state, leases)
                            .is_some_and(|_held| index.caught_up_mark() == Some(mark)) =>
                    {
                        index.owe_caught_up();
                        break;
                    }
                    Ok(result) => {
                        settled.push(deferred);
                        if let Some(result) = result {
                            found += 1;
                            deliver(result);
                        }
                    }
                    // Another note took its path once it was found, which says nothing about the
                    // backend: kept, and looked for by its id again after the next pass.
                    Err(CheckError::AnotherNote) => {}
                    // Most likely the backend is away again: the rest wait for the next pass
                    // rather than each waiting out its timeout.
                    Err(CheckError::Failed) => break,
                }
            }
            Ok(Location::Gone) => {
                dropped += 1;
                settled.push(deferred);
            }
            Ok(Location::Unsure) | Err(_) => {}
        }
    }
    // Nothing changed, so nothing to log: a check that cannot run yet would otherwise log after
    // every pass.
    if settled.is_empty() {
        return;
    }
    let ran = settled.len() - dropped;
    let still_waiting = waiting.len() - settled.len();
    if let Ok(mut list) = state.deferred_checks.lock() {
        // Unless the vault was switched meanwhile: its file then keeps every check for its next
        // open, and those that ran run once more.
        if list
            .vault
            .as_deref()
            .is_some_and(|open| same_path(open, &vault))
        {
            list.forget(&settled);
        }
    }
    // Counts only, never a note (docs/log-privacy.md).
    log::info!(
        "Deferred similarity checks: ran={ran} found={found} dropped={dropped} \
         waiting={still_waiting}"
    );
}

/// Reruns the deferred checks off the indexing worker's thread and shows each result with
/// matches as the ordinary card. Oldest first, so the newest lands on top, since cards show
/// newest first. No notification and no summary: the user did not just capture anything.
fn rerun_deferred(app: AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        rerun_deferred_checks(&app.state::<AppState>(), |found| {
            show_card(&app, &found);
        });
    });
}

/// Reruns the open vault's deferred checks when `index` has caught up, as
/// `SemanticIndex::on_caught_up` defines it (#216). For every new index, before its worker starts.
pub fn rerun_when_caught_up(app: &AppHandle, index: &SemanticIndex) {
    let app = app.clone();
    index.on_caught_up(move || rerun_deferred(app.clone()));
}

/// `existing` with the capture added at the end under a dated marker, title line included.
pub fn with_capture_appended(
    existing: &str,
    capture_title: &str,
    capture_body: &str,
    date: &str,
) -> String {
    let capture = [capture_title.trim(), capture_body.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "{}\n\n---\n\n*Added from capture, {date}:*\n\n{capture}\n",
        existing.trim_end()
    )
}

/// Shows `found` as the ordinary card in the main window. False when it is not shown: a check
/// can outlast a vault switch, and its card belongs to the vault it ran in.
fn show_card(app: &AppHandle, found: &SimilarityCheck) -> bool {
    if !vault_still_open(app, &found.vault) {
        return false;
    }
    if let Err(error) = app.emit_to("main", crate::events::SIMILAR_NOTES_FOUND, found) {
        log::warn!("Could not show the similar-notes card: {error}");
        return false;
    }
    true
}

/// Run the check for a quick capture in the background, show what it found in the main
/// window, and raise a notification when the user is looking elsewhere.
pub fn after_capture(app: AppHandle, vault: String, path: String, id: String) {
    // Read now, while it still describes this capture: the next overlay overwrites it.
    let focused_at_capture = MAIN_FOCUSED_AT_CAPTURE.load(Ordering::SeqCst);
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(found) = check_now(&app.state::<AppState>(), &vault, &path, &id) {
            // Its nudge, like its card, belongs to the vault the check ran in.
            if show_card(&app, &found) && !focused_at_capture && !main_window_focused(&app) {
                notify(&app, found.vault);
            }
        }
        // Once any card is sent: scripts/similar-run.mjs reads this as the check's end.
        log::info!("Finished the similarity check for a capture");
    });
}

/// Whether the main window had focus when the capture overlay last opened. The overlay takes
/// focus, so a check that finishes before it hands focus back would otherwise notify a user who
/// was in the app all along.
static MAIN_FOCUSED_AT_CAPTURE: AtomicBool = AtomicBool::new(false);

/// The capture overlay is about to open.
pub fn capture_opening(app: &AppHandle) {
    MAIN_FOCUSED_AT_CAPTURE.store(main_window_focused(app), Ordering::SeqCst);
}

fn vault_still_open(app: &AppHandle, vault: &str) -> bool {
    app.state::<AppState>()
        .config
        .lock()
        .is_ok_and(|config| config.active_vault.as_deref() == Some(vault))
}

/// A queued notification whose turn comes after the user went back to the app, or after a
/// vault switch, is moot.
fn notification_moot(app: &AppHandle, vault: &str) -> bool {
    main_window_focused(app) || !vault_still_open(app, vault)
}

fn main_window_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main").is_some_and(|window| {
        window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
    })
}

/// Captures with a card the user has not looked at since the last notification. Reset when the
/// main window gains focus, so the next notification counts only what is new.
static UNSEEN: AtomicUsize = AtomicUsize::new(0);

/// The main window was focused: every card is in front of the user.
pub fn main_window_seen() {
    UNSEEN.store(0, Ordering::SeqCst);
}

/// One more capture the user has not seen, and the title that counts it.
fn next_notification_title() -> String {
    notification_title(UNSEEN.fetch_add(1, Ordering::SeqCst) + 1)
}

/// The notification's wording. Generic on purpose: it can show on a lock screen and stays in
/// the notification history, so it never names a note.
fn notification_title(unseen: usize) -> String {
    if unseen <= 1 {
        "Similar note found for your capture".to_string()
    } else {
        format!("{unseen} captures have similar notes")
    }
}

const NOTIFICATION_BODY: &str = "Open Second Brain to review.";

/// Bring the main window forward, from a notification click on whatever thread delivers it.
///
/// `activation_token` is the compositor's permission to take focus, which the notification
/// service sends just before a click. Without it GNOME on Wayland refuses the focus request and
/// shows "Second Brain is ready" instead of raising the window.
fn open_main_window(app: &AppHandle, activation_token: Option<String>) {
    main_window_seen();
    let handle = app.clone();
    let opened = app.run_on_main_thread(move || {
        #[cfg(target_os = "linux")]
        if let Some(token) = activation_token {
            use gtk::prelude::GtkWindowExt;
            if let Some(Ok(window)) = handle
                .get_webview_window("main")
                .map(|main| main.gtk_window())
            {
                window.set_startup_id(&token);
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = activation_token;
        crate::show_main_window(&handle);
    });
    if let Err(error) = opened {
        log::warn!("Could not open the main window from the notification: {error}");
    }
}

/// The id GNOME gave the last similar-notes notification, so a newer one replaces it and a click
/// can be matched to it. 0 is none yet.
#[cfg(target_os = "linux")]
static NOTIFICATION_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The desktop's notification service, `org.freedesktop.Notifications`, over the session bus
/// ashpd already uses.
///
/// Not the notification portal that `hotkey::startup::notify_vault_unavailable` uses: a click on
/// a portal notification reaches the app without an activation token (GNOME 50 sends its action
/// with no platform data), and without one GNOME on Wayland refuses to raise the window and shows
/// "… is ready" instead. This service sends the token (`ActivationToken`) just before the click.
///
/// One connection for the app's lifetime: GNOME Shell withdraws an app's notifications the moment
/// the connection that sent them closes, so a connection per notification lost each one at once.
#[cfg(target_os = "linux")]
async fn notification_service() -> ashpd::zbus::Result<&'static ashpd::zbus::Proxy<'static>> {
    static SERVICE: tokio::sync::OnceCell<ashpd::zbus::Proxy<'static>> =
        tokio::sync::OnceCell::const_new();
    SERVICE
        .get_or_try_init(|| async {
            let connection = ashpd::zbus::Connection::session().await?;
            ashpd::zbus::Proxy::new(
                &connection,
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
            )
            .await
        })
        .await
}

#[cfg(target_os = "linux")]
fn notify(app: &AppHandle, vault: String) {
    use ashpd::zbus::zvariant::Value;
    use std::collections::HashMap;

    let desktop_entry = app.config().identifier.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // One send at a time: a second capture must wait for the first's id, or both would go
        // out as new notifications instead of one replacing the other. Counted inside, so the
        // notification that replaces another never shows the smaller count.
        static SENDING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _sending = SENDING.lock().await;
        let sent = async {
            let service = notification_service().await?;
            // Checked after connecting, which can wait: focus or the vault may have changed.
            if notification_moot(&app, &vault) {
                return Ok(());
            }
            let title = next_notification_title();
            let hints = HashMap::from([("desktop-entry", Value::from(desktop_entry.as_str()))]);
            let id: u32 = service
                .call(
                    "Notify",
                    &(
                        "Second Brain",
                        NOTIFICATION_ID.load(Ordering::SeqCst),
                        "",
                        title.as_str(),
                        NOTIFICATION_BODY,
                        vec!["default", "Open"],
                        hints,
                        -1i32,
                    ),
                )
                .await?;
            NOTIFICATION_ID.store(id, Ordering::SeqCst);
            // Only once a notification is out, so a missing service logs one line, not two.
            listen_for_clicks(&app);
            Ok::<(), ashpd::zbus::Error>(())
        }
        .await;
        // A nudge only: the card is already waiting in the main window.
        if let Err(error) = sent {
            log::warn!("Could not show the similar-notes notification: {error}");
        }
    });
}

/// One listener for the app's lifetime. The service announces a click as `ActivationToken` and
/// then `ActionInvoked` for the notification's id; both come on one stream, in order.
#[cfg(target_os = "linux")]
fn listen_for_clicks(app: &AppHandle) {
    use futures::StreamExt;
    static LISTENING: AtomicBool = AtomicBool::new(false);

    if LISTENING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let listening = async {
            let service = notification_service().await?;
            let mut signals = service.receive_all_signals().await?;
            let mut token = None;
            while let Some(signal) = signals.next().await {
                let header = signal.header();
                let Some(member) = header.member() else {
                    continue;
                };
                let Ok((id, value)) = signal.body().deserialize::<(u32, String)>() else {
                    continue;
                };
                if id != NOTIFICATION_ID.load(Ordering::SeqCst) {
                    continue;
                }
                match member.as_str() {
                    "ActivationToken" => token = Some(value),
                    "ActionInvoked" if value == "default" => {
                        if token.is_none() {
                            log::info!("A notification click came without an activation token");
                        }
                        open_main_window(&app, token.take());
                    }
                    _ => {}
                }
            }
            Ok::<(), ashpd::zbus::Error>(())
        }
        .await;
        if let Err(error) = listening {
            // Let the next notification try again.
            LISTENING.store(false, Ordering::SeqCst);
            log::warn!("Notification clicks will not open the app: {error}");
        }
    });
}

/// A toast under a fixed tag, so a newer one replaces the last (#153). Clicking it raises the
/// main window while the app runs, which it does whenever a capture was just made.
#[cfg(target_os = "windows")]
fn notify(app: &AppHandle, vault: String) {
    use windows::core::IInspectable;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::ToastNotification;

    // Checks finish on their own threads; one toast at a time keeps the newest count on top.
    static SENDING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _sending = SENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if notification_moot(app, &vault) {
        return;
    }
    let title = next_notification_title();
    let handle = app.clone();
    let shown =
        crate::hotkey::windows::toast(&title, NOTIFICATION_BODY, "similar-notes", "capture")
            .and_then(|toast| {
                toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
                    move |_, _| {
                        open_main_window(&handle, None);
                        Ok(())
                    },
                ))?;
                crate::hotkey::windows::show_toast(&app.config().identifier, &toast)
            });
    // A nudge only: the card is already waiting in the main window.
    if let Err(error) = shown {
        log::warn!("Could not show the similar-notes notification: {error}");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn notify(_app: &AppHandle, _vault: String) {}

#[cfg(test)]
mod tests {
    use super::{
        deferred_file, notification_title, with_capture_appended, DeferredCheck, DeferredChecks,
        DeferredFile,
    };

    #[test]
    fn opening_a_vault_reads_its_checks_and_reopening_it_keeps_the_open_list() {
        let first = std::env::temp_dir().join(format!("sb-deferred-{}", uuid::Uuid::new_v4()));
        let second = std::env::temp_dir().join(format!("sb-deferred-{}", uuid::Uuid::new_v4()));
        let (first_text, second_text) = (first.to_str().unwrap(), second.to_str().unwrap());
        for vault in [&first, &second] {
            std::fs::create_dir_all(vault).unwrap();
        }
        let kept = DeferredCheck {
            id: "note-id".to_string(),
            path: "Areas/Note.md".to_string(),
        };
        let mut open = DeferredChecks::default();
        open.open(first_text);
        open.keep(kept.clone());
        // As the file read when the vault began opening again, before that check was kept.
        let none: DeferredFile<Vec<DeferredCheck>> = DeferredFile { checks: Vec::new() };
        let file = deferred_file(first_text).unwrap();
        std::fs::write(&file, serde_json::to_vec(&none).unwrap()).unwrap();
        open.open(first_text);
        assert_eq!(open.checks(), [kept], "the open list has the newer check");
        open.open(second_text);
        assert!(open.checks().is_empty(), "another vault's file");
        for vault in [first, second] {
            std::fs::remove_dir_all(vault).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_note_is_in_a_vault_opened_through_a_link_and_never_in_a_folder_that_is_not_one() {
        let root = std::env::temp_dir().join(format!("sb-deferred-{}", uuid::Uuid::new_v4()));
        let (vault, link, plain) = (root.join("Vault"), root.join("Link"), root.join("Plain"));
        std::fs::create_dir_all(vault.join(".helixnotes")).unwrap();
        std::fs::create_dir_all(&plain).unwrap();
        std::os::unix::fs::symlink(&vault, &link).unwrap();
        let (note, stray) = (vault.join("Note.md"), plain.join("Note.md"));
        std::fs::write(&note, "note").unwrap();
        std::fs::write(&stray, "note").unwrap();
        let text = |path: &std::path::Path| path.to_str().unwrap().to_string();
        assert!(
            super::note_in_opened_vault(&text(&link), &text(&note)),
            "opened through a link"
        );
        assert!(super::note_in_opened_vault(&text(&vault), &text(&note)));
        assert!(
            !super::note_in_opened_vault(&text(&plain), &text(&stray)),
            "never opened as a vault"
        );
        assert!(
            !super::note_in_opened_vault(&text(&link), &text(&stray)),
            "outside the vault"
        );
        let (renamed, escaped) = (vault.join("Gone.md"), vault.join("../Plain/Gone.md"));
        assert!(
            super::note_in_opened_vault(&text(&link), &text(&renamed)),
            "renamed since: placed by its folder"
        );
        assert!(
            !super::note_in_opened_vault(&text(&vault), &text(&escaped)),
            "a folder outside the vault"
        );
        let dangling = vault.join("Dangling");
        std::os::unix::fs::symlink(root.join("Missing"), &dangling).unwrap();
        assert!(
            !super::note_in_opened_vault(&text(&vault), &text(&dangling.join("Note.md"))),
            "through a link to nothing, which could point outside the vault"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_note_whose_folder_is_gone_is_placed_by_the_vault_unless_it_climbs_out() {
        let root = std::env::temp_dir().join(format!("sb-deferred-{}", uuid::Uuid::new_v4()));
        let (vault, plain) = (root.join("Vault"), root.join("Plain"));
        std::fs::create_dir_all(vault.join(".helixnotes")).unwrap();
        std::fs::create_dir_all(&plain).unwrap();
        // Beside the vault, so a `..` that Windows resolves before it reaches the disk finds a
        // note there, outside the vault.
        std::fs::write(plain.join("Note.md"), "note").unwrap();
        let text = |path: &std::path::Path| path.to_str().unwrap().to_string();
        // Its folder renamed since, so neither the note nor its folder is there.
        let (in_gone, climbing) = (
            vault.join("Gone/Note.md"),
            vault.join("Gone/../../Plain/Note.md"),
        );
        assert!(
            super::note_in_opened_vault(&text(&vault), &text(&in_gone)),
            "its folder renamed since: placed by the vault"
        );
        assert!(
            !super::note_in_opened_vault(&text(&vault), &text(&climbing)),
            "climbing out below a folder that is gone"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_deferred_file_gives_an_empty_list_and_the_vault_still_opens() {
        let vault = std::env::temp_dir().join(format!("sb-deferred-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&vault).unwrap();
        let vault_text = vault.to_str().unwrap();
        let file = deferred_file(vault_text).unwrap();
        assert!(
            DeferredChecks::load(vault_text).checks().is_empty(),
            "a missing file is an empty list"
        );

        // `load` has no error to return, so opening the vault goes on with an empty list.
        std::fs::write(&file, b"{ not json").unwrap();
        let mut loaded = DeferredChecks::load(vault_text);
        assert!(loaded.checks().is_empty(), "a damaged file");
        // The next change writes a whole file over the damaged one.
        loaded.keep(DeferredCheck {
            id: "note-id".to_string(),
            path: "Areas/Note.md".to_string(),
        });
        assert_eq!(DeferredChecks::load(vault_text).checks(), loaded.checks());

        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        let mut unread = DeferredChecks::load(vault_text);
        assert!(
            unread.checks().is_empty(),
            "a file that cannot be read at all"
        );
        let check = |id: &str, path: &str| DeferredCheck {
            id: id.to_string(),
            path: path.to_string(),
        };
        unread.keep(check("newer-id", "Areas/Newer.md"));
        unread.keep(check("ran-id", "Areas/Ran.md"));
        unread.forget(&[&check("ran-id", "Areas/Ran.md")]);
        assert!(
            file.is_dir(),
            "nothing is written over what could not be read"
        );

        // Once it can be read, its checks come back ahead of the newer ones. A newer check
        // replaces its note's older one, and one that ran meanwhile stays gone.
        std::fs::remove_dir(&file).unwrap();
        let older = DeferredFile {
            checks: vec![
                check("older-id", "Areas/Older.md"),
                check("newer-id", "Areas/Before the move.md"),
                check("ran-id", "Areas/Ran.md"),
            ],
        };
        std::fs::write(&file, serde_json::to_vec(&older).unwrap()).unwrap();
        unread.recover();
        let recovered = [
            check("older-id", "Areas/Older.md"),
            check("newer-id", "Areas/Newer.md"),
        ];
        assert_eq!(unread.checks(), recovered);
        assert_eq!(
            DeferredChecks::load(vault_text).checks(),
            recovered,
            "written back at once, so a restart does not bring back the check that ran"
        );
        unread.keep(check("another-id", "Areas/Another.md"));
        let expected = [
            check("older-id", "Areas/Older.md"),
            check("newer-id", "Areas/Newer.md"),
            check("another-id", "Areas/Another.md"),
        ];
        assert_eq!(unread.checks(), expected);
        assert_eq!(DeferredChecks::load(vault_text).checks(), expected);
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir_all(&vault).unwrap();
    }

    #[test]
    fn an_append_keeps_the_note_and_marks_where_the_capture_starts() {
        assert_eq!(
            with_capture_appended("Existing body.\n\n", "Cedar arrived", "Stacked by the shed.", "2026-10-05"),
            "Existing body.\n\n---\n\n*Added from capture, 2026-10-05:*\n\nCedar arrived\n\nStacked by the shed.\n"
        );
        assert_eq!(
            with_capture_appended("Existing.", "Title only", "", "2026-10-05"),
            "Existing.\n\n---\n\n*Added from capture, 2026-10-05:*\n\nTitle only\n"
        );
    }

    #[test]
    fn the_notification_never_names_a_note_and_counts_unseen_captures() {
        assert_eq!(notification_title(1), "Similar note found for your capture");
        assert_eq!(notification_title(3), "3 captures have similar notes");
    }
}
