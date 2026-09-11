//! Performing what [`super::plan`] decides.
//!
//! This module holds no judgement about what should happen to a note — that is the
//! planner's job, and it is pure so it can be tested without a Notion account. What lives
//! here is the order of API calls, when the map is written relative to them, and what a
//! failure does to the rest of the run.
//!
//! ## Write the map before the call, not after
//!
//! A page is created in two steps that cannot be atomic: Notion creates it, and we record
//! that it exists. A process killed between them leaves a page nothing knows about, and the
//! next run creates a second one. So the map is written *first*, in
//! [`EntryState::Creating`], and the next run treats that state as "ask Notion whether this
//! page exists" rather than "create". The cost is one extra file write per new note; the
//! alternative is silent duplication that only shows up as two copies of everything.
//!
//! ## A move is always two calls
//!
//! `POST /v1/pages/{id}/move` preserves the page, its content, and its text properties, but
//! drops `multi_select` values — their options are registered on the data source that owns
//! them, and the target has never seen them. So a move is always followed by a property
//! rewrite, *even when the note itself has not changed*. Verified against the live API; see
//! `docs/reports/spike-notion-move-2026-09-10.md`.
//!
//! ## One bad note does not stop the run
//!
//! A note Notion refuses is recorded against its own map entry and the run continues. The
//! opposite — halting — is the failure most likely to go unnoticed for weeks, because a
//! publisher that stops publishing looks exactly like one with nothing to do. Only a
//! rejected token ends a run, because retrying it cannot succeed and spends a budget shared
//! with everything else in the user's workspace.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

use super::blocks;
use super::client::{NotionClient, NotionError};
use super::config::DatabaseRegistry;
use super::map::{self, EntryState, MapEntry};
use super::plan::{self, Action, InspectedNote, NoteSnapshot, SkipReason};
use super::properties;
use crate::types::NoteMeta;
use crate::vault::para::ParaCategory;

/// What a run did, for the Settings panel.
///
/// Deserialized as well as serialized: the last run is written to a machine-local file so
/// the panel can report it after a restart, and so the background process #57 introduces
/// and the app agree on what happened without talking to each other.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub created: u32,
    pub updated: u32,
    pub moved: u32,
    pub trashed: u32,
    pub up_to_date: u32,
    pub failed: u32,
    /// Counted by reason, because "3 notes not published" with no reason is not an answer.
    pub skipped: BTreeMap<String, u32>,
}

impl Summary {
    fn skip(&mut self, reason: SkipReason) {
        let key = serde_json::to_value(reason)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".into());
        *self.skipped.entry(key).or_insert(0) += 1;
    }

    pub fn total_skipped(&self) -> u32 {
        self.skipped.values().sum()
    }
}

/// How far a run has got, for a first sync that may take minutes.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// A note the publisher is about to consider.
///
/// Carries the parsed frontmatter and the body separately, because the two become different
/// parts of a Notion page: properties and blocks.
pub struct NoteSource {
    pub snapshot: NoteSnapshot,
    pub meta: NoteMeta,
    pub body: String,
}

pub fn content_hash(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Read one note from disk into the shape the publisher wants.
pub fn read_note(
    vault_path: &Path,
    relative_path: &str,
) -> Result<(NoteMeta, String, String), String> {
    let full = vault_path.join(relative_path);
    let raw = std::fs::read_to_string(&full).map_err(|error| error.to_string())?;
    let filename = Path::new(relative_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(relative_path);
    let (meta, body) = crate::vault::frontmatter::parse_note(&raw, filename);
    Ok((meta, body, content_hash(&raw)))
}

/// Publish every changed note, and clear anything the last run left outstanding.
///
/// `report` is called as work completes so a first sync — which can be thousands of notes —
/// is visible rather than looking like a hang. It is a callback rather than a Tauri event so
/// this runs unchanged inside the background process #57 introduces, which has no window to
/// emit to.
/// `read` is called only for notes that survive the modification-time gate, which is the
/// point of taking an enumeration rather than the notes themselves: a caller that had to
/// read every note to call this would have paid the cost the gate exists to avoid.
pub async fn run(
    vault_path: &Path,
    client: &NotionClient,
    registry: &DatabaseRegistry,
    snapshots: Vec<NoteSnapshot>,
    mut read: impl FnMut(&NoteSnapshot) -> Result<NoteSource, String>,
    mut report: impl FnMut(Progress),
) -> Result<Summary, NotionError> {
    let mut summary = Summary::default();

    // Deletions first. A page whose note is gone should not linger while a long first sync
    // works through everything else, and trashing is cheap.
    let outstanding = map::all(vault_path);
    let tombstones = plan::tombstone_actions(&outstanding);
    let total = tombstones.len() + snapshots.len();
    let mut done = 0;

    for action in tombstones {
        if let Action::Trash { note_id, page_id } = action {
            match client.trash_page(&page_id).await {
                Ok(()) => {
                    let _ = map::remove(vault_path, &note_id);
                    summary.trashed += 1;
                }
                // Already gone in Notion — trashed by hand, or a retry of a call that did
                // land. The tombstone has done its job either way.
                Err(NotionError::NotFound(_)) => {
                    let _ = map::remove(vault_path, &note_id);
                    summary.trashed += 1;
                }
                Err(error) if error.is_fatal() => return Err(error),
                Err(error) => {
                    log::warn!("Could not trash the Notion page for {note_id}: {error}");
                    summary.failed += 1;
                }
            }
        }
        done += 1;
        report(Progress { done, total });
    }

    // First pass: decide everything, executing nothing. Deciding first is what lets a run
    // check its creates against Notion in one sweep before any of them happen.
    let mut planned: Vec<(Action, NoteSource)> = Vec::new();
    for snapshot in snapshots {
        let note_id = snapshot.note_id.clone();
        let entry = map::load(vault_path, &note_id);

        // The cheap gate: an unchanged modification time means the file is not read at all.
        if !plan::needs_inspection(entry.as_ref(), snapshot.mtime) {
            summary.up_to_date += 1;
            done += 1;
            report(Progress { done, total });
            continue;
        }

        let note = match read(&snapshot) {
            Ok(note) => note,
            Err(error) => {
                // A note that cannot be read is this note's problem, not the run's — it may
                // be mid-write by the editor, or arriving from the other machine.
                log::warn!("Skipping {note_id}, which could not be read: {error}");
                summary.failed += 1;
                done += 1;
                report(Progress { done, total });
                continue;
            }
        };

        let inspected = InspectedNote {
            snapshot: note.snapshot.clone(),
            content_hash: content_hash_of(&note),
        };
        let action = plan::decide(&inspected, entry.as_ref(), registry);
        planned.push((action, note));
    }

    // A note with no map entry may be new, or its entry may have been lost. Only Notion can
    // say which, so any run about to create asks it once, up front, rather than per note.
    // Runs with nothing to create — every idle poll — skip this and still cost nothing.
    if planned
        .iter()
        .any(|(action, _)| matches!(action, Action::Create { .. }))
    {
        let existing = existing_pages(client, registry).await?;
        let mut adopted = 0;
        planned = planned
            .into_iter()
            .map(|(action, note)| {
                let found = existing.get(&note.snapshot.note_id);
                let action = plan::adopt_existing(action, found);
                if found.is_some() && !matches!(action, Action::Create { .. }) {
                    adopted += 1;
                }
                (action, note)
            })
            .collect();
        if adopted > 0 {
            log::warn!(
                "Adopted {adopted} Notion pages that already existed for notes with no map entry — \
                 the map had been lost or damaged, and these would otherwise have been duplicated"
            );
        }
    }

    for (action, note) in planned {
        let note_id = note.snapshot.note_id.clone();
        match execute(vault_path, client, registry, &action, &note).await {
            Ok(()) => tally(&mut summary, &action),
            Err(error) if error.is_fatal() => return Err(error),
            Err(error) => {
                record_failure(vault_path, &note_id, &error);
                summary.failed += 1;
            }
        }

        done += 1;
        report(Progress { done, total });
    }

    log::info!(
        "Notion: {} created, {} updated, {} moved, {} trashed, {} unchanged, {} skipped, {} failed ({} requests)",
        summary.created,
        summary.updated,
        summary.moved,
        summary.trashed,
        summary.up_to_date,
        summary.total_skipped(),
        summary.failed,
        client.requests_sent(),
    );
    Ok(summary)
}

/// Every page this app has published, by note id, as `(page id, data source id)`.
///
/// All four data sources, not just the note's own: a note recategorised while its map
/// entry was missing has its page in the old category's database, and only a full sweep
/// finds it there.
async fn existing_pages(
    client: &NotionClient,
    registry: &DatabaseRegistry,
) -> Result<std::collections::HashMap<String, (String, String)>, NotionError> {
    let mut existing = std::collections::HashMap::new();
    for category in ParaCategory::ALL {
        let Some(link) = registry.link(category) else {
            continue;
        };
        for (page_id, note_id) in client.list_note_pages(&link.data_source_id).await? {
            existing.insert(note_id, (page_id, link.data_source_id.clone()));
        }
    }
    Ok(existing)
}

fn content_hash_of(note: &NoteSource) -> String {
    // The hash covers the whole file, frontmatter included: a tag change alters no body
    // text but does change what the page should say.
    content_hash(&format!(
        "{}\u{0}{}\u{0}{}",
        note.meta.title,
        note.meta.tags.join(","),
        note.body
    ))
}

fn tally(summary: &mut Summary, action: &Action) {
    match action {
        Action::Create { .. } | Action::ResolveInterrupted { .. } => summary.created += 1,
        Action::UpdateContent { .. } => summary.updated += 1,
        Action::Move { .. } => summary.moved += 1,
        Action::Trash { .. } => summary.trashed += 1,
        Action::UpToDate { .. } => summary.up_to_date += 1,
        Action::Skip { reason, .. } => summary.skip(*reason),
    }
}

fn record_failure(vault_path: &Path, note_id: &str, error: &NotionError) {
    // Recorded against the note rather than only logged, so Settings can say which notes
    // are failing and why. A silent skip is how one unpublishable note goes unnoticed.
    let mut entry = map::load(vault_path, note_id).unwrap_or_else(MapEntry::creating);
    entry.last_error = Some(error.message());
    if let Err(problem) = map::save(vault_path, note_id, &entry) {
        log::warn!("Could not record a Notion failure for {note_id}: {problem}");
    }
}

async fn execute(
    vault_path: &Path,
    client: &NotionClient,
    registry: &DatabaseRegistry,
    action: &Action,
    note: &NoteSource,
) -> Result<(), NotionError> {
    // Nothing that does not call Notion needs an executor arm.
    if !action.is_work() {
        return Ok(());
    }

    match action {
        Action::UpToDate { .. } | Action::Skip { .. } => Ok(()),

        Action::Create {
            data_source_id,
            note_id,
            ..
        } => {
            // Written before the call, so an interrupted create leaves a trace.
            let _ = map::save(vault_path, note_id, &MapEntry::creating());
            let page_id = create_page(client, data_source_id, note).await?;
            publish_entry(vault_path, action, &page_id);
            Ok(())
        }

        Action::ResolveInterrupted {
            note_id,
            data_source_id,
            ..
        } => {
            // Ask Notion before creating anything. The page may exist from the interrupted
            // run; creating blind is how a vault ends up with two pages per note.
            match find_existing(client, registry, note_id, data_source_id).await? {
                Some((page_id, found_in)) => {
                    if found_in != *data_source_id {
                        client.move_page(&page_id, data_source_id).await?;
                    }
                    replace_content(client, &page_id, note).await?;
                    client
                        .update_properties(&page_id, properties::for_note(&note.meta))
                        .await?;
                    publish_entry(vault_path, action, &page_id);
                    Ok(())
                }
                None => {
                    let page_id = create_page(client, data_source_id, note).await?;
                    publish_entry(vault_path, action, &page_id);
                    Ok(())
                }
            }
        }

        Action::UpdateContent { page_id, .. } => {
            replace_content(client, page_id, note).await?;
            client
                .update_properties(page_id, properties::for_note(&note.meta))
                .await?;
            publish_entry(vault_path, action, page_id);
            Ok(())
        }

        Action::Move {
            page_id,
            data_source_id,
            also_update_content,
            ..
        } => {
            client.move_page(page_id, data_source_id).await?;

            // Unconditional, and not an oversight when the note is unchanged: the move
            // itself drops multi_select values, so the rewrite is what puts the tags back.
            client
                .update_properties(page_id, properties::for_note(&note.meta))
                .await?;

            if *also_update_content {
                replace_content(client, page_id, note).await?;
            }
            publish_entry(vault_path, action, page_id);
            Ok(())
        }

        Action::Trash { note_id, page_id } => {
            client.trash_page(page_id).await?;
            let _ = map::remove(vault_path, note_id);
            Ok(())
        }
    }
}

async fn create_page(
    client: &NotionClient,
    data_source_id: &str,
    note: &NoteSource,
) -> Result<String, NotionError> {
    let mut batches = blocks::batches(blocks::to_blocks(&note.body)).into_iter();
    let first = batches.next().unwrap_or_default();

    let page_id = client
        .create_page(data_source_id, properties::for_note(&note.meta), first)
        .await?;

    // Notion takes 100 blocks per request, so a long note arrives in instalments.
    for batch in batches {
        client.append_blocks(&page_id, batch).await?;
    }
    Ok(page_id)
}

/// Replace a page's content wholesale.
///
/// One erase then one append per hundred blocks, rather than deleting blocks individually:
/// a 200-block note would otherwise cost 200 requests, over a minute at three per second.
/// This does destroy Notion comments anchored to the replaced blocks, which is accepted for
/// a read-only view and recorded on the ticket rather than discovered later.
async fn replace_content(
    client: &NotionClient,
    page_id: &str,
    note: &NoteSource,
) -> Result<(), NotionError> {
    client.erase_content(page_id).await?;
    for batch in blocks::batches(blocks::to_blocks(&note.body)) {
        client.append_blocks(page_id, batch).await?;
    }
    Ok(())
}

/// Look for a page carrying this note's id, starting where it should be.
///
/// The other three data sources are searched too, because a note's category can change
/// while an interrupted run is outstanding — the page would then exist somewhere other than
/// where the map expects. Four queries is a poor trade in general and a good one here: this
/// runs only for a note whose state is already known to be unresolved.
async fn find_existing(
    client: &NotionClient,
    registry: &DatabaseRegistry,
    note_id: &str,
    preferred: &str,
) -> Result<Option<(String, String)>, NotionError> {
    if let Some(page_id) = client.find_page_by_note_id(preferred, note_id).await? {
        return Ok(Some((page_id, preferred.to_string())));
    }

    for category in ParaCategory::ALL {
        let Some(link) = registry.link(category) else {
            continue;
        };
        if link.data_source_id == preferred {
            continue;
        }
        if let Some(page_id) = client
            .find_page_by_note_id(&link.data_source_id, note_id)
            .await?
        {
            return Ok(Some((page_id, link.data_source_id.clone())));
        }
    }
    Ok(None)
}

fn publish_entry(vault_path: &Path, action: &Action, page_id: &str) {
    let Some(record) = action.publish_record() else {
        log::error!("Tried to record a non-publishing Notion action as published");
        return;
    };
    let entry = MapEntry {
        state: EntryState::Published,
        page_id: Some(page_id.to_string()),
        data_source_id: Some(record.data_source_id.to_string()),
        content_hash: Some(record.content_hash.to_string()),
        source_mtime: Some(record.mtime),
        relative_path: Some(record.relative_path.to_string()),
        last_error: None,
    };
    if let Err(error) = map::save(vault_path, record.note_id, &entry) {
        // The page exists; only the record of it failed. The next run finds a `Creating`
        // entry or none, asks Notion, and adopts the page rather than duplicating it.
        log::error!(
            "Published {} to Notion but could not record it: {error}",
            record.note_id
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notion::config::DatabaseLink;
    use chrono::{TimeZone, Utc};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    fn scripted_server(responses: Vec<(&str, &str)>) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let responses: Vec<(String, String)> = responses
            .into_iter()
            .map(|(s, b)| (s.to_string(), b.to_string()))
            .collect();

        std::thread::spawn(move || {
            for (status, body) in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(index) = text.find("\r\n\r\n") {
                        let declared = text
                            .to_ascii_lowercase()
                            .split("content-length:")
                            .nth(1)
                            .and_then(|rest| {
                                rest.split("\r\n").next()?.trim().parse::<usize>().ok()
                            })
                            .unwrap_or(0);
                        if request.len() >= index + 4 + declared {
                            break;
                        }
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&request).into_owned());
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        (format!("http://{address}"), rx)
    }

    /// Drive [`run`] from whole notes, the way every test but the gate test wants to.
    ///
    /// `run` takes an enumeration plus a reader rather than the notes themselves, because a
    /// caller that read every note up front would have paid exactly the cost the
    /// modification-time gate exists to avoid.
    async fn run_notes(
        vault_path: &std::path::Path,
        client: &NotionClient,
        registry: &DatabaseRegistry,
        notes: Vec<NoteSource>,
        report: impl FnMut(Progress),
    ) -> Result<Summary, NotionError> {
        let snapshots: Vec<NoteSnapshot> = notes.iter().map(|n| n.snapshot.clone()).collect();
        let mut by_id: std::collections::HashMap<String, NoteSource> = notes
            .into_iter()
            .map(|n| (n.snapshot.note_id.clone(), n))
            .collect();
        run(
            vault_path,
            client,
            registry,
            snapshots,
            move |snapshot| {
                by_id
                    .remove(&snapshot.note_id)
                    .ok_or_else(|| "no such note".to_string())
            },
            report,
        )
        .await
    }

    /// A server for a run that creates: it first answers the sweep of all four databases
    /// that checks whether the pages already exist, then the given responses.
    fn creating_server(responses: Vec<(&str, &str)>) -> (String, Receiver<String>) {
        let mut all = vec![("200 OK", r#"{"results":[],"has_more":false}"#); 4];
        all.extend(responses);
        scripted_server(all)
    }

    fn client(base: &str) -> NotionClient {
        NotionClient::with_base("t", base, Duration::from_millis(1))
    }

    fn registry() -> DatabaseRegistry {
        let mut registry = DatabaseRegistry {
            parent_page_id: Some("parent".into()),
            ..Default::default()
        };
        for category in ParaCategory::ALL {
            registry.set_link(
                category,
                DatabaseLink {
                    database_id: format!("db-{}", category.folder_name()),
                    data_source_id: format!("ds-{}", category.folder_name()),
                },
            );
        }
        registry
    }

    fn vault() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("notion-publish-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn note(id: &str, category: ParaCategory, body: &str) -> NoteSource {
        NoteSource {
            snapshot: NoteSnapshot {
                note_id: id.into(),
                relative_path: format!("{}/{id}.md", category.folder_name()),
                category: Some(category),
                mtime: 100,
            },
            meta: NoteMeta {
                id: id.into(),
                title: "A note".into(),
                tags: vec!["spike".into()],
                pinned: false,
                created: Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
                modified: Utc.with_ymd_and_hms(2026, 9, 2, 0, 0, 0).unwrap(),
                category: Some(category),
                source_url: None,
            },
            body: body.into(),
        }
    }

    #[tokio::test]
    async fn a_new_note_is_created_and_recorded() {
        let vault = vault();
        let (base, requests) = creating_server(vec![("200 OK", r#"{"id":"page-1"}"#)]);

        let summary = run_notes(
            &vault,
            &client(&base),
            &registry(),
            vec![note("note-1", ParaCategory::Projects, "# Hello")],
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(summary.created, 1);
        let request = {
            for _ in 0..4 {
                requests.recv().unwrap();
            }
            requests.recv().unwrap()
        };
        assert!(
            request.contains("ds-Projects"),
            "into its category's database"
        );
        assert!(request.contains("note-1"), "carrying its note id");

        let entry = map::load(&vault, "note-1").expect("the mapping must be recorded");
        assert_eq!(entry.state, EntryState::Published);
        assert_eq!(entry.page_id.as_deref(), Some("page-1"));
    }

    #[tokio::test]
    async fn a_second_run_over_unchanged_notes_calls_notion_not_at_all() {
        // The whole point of the hash and the mtime: a poll every five minutes must not
        // re-upload the vault.
        let vault = vault();
        let (base, _r) = creating_server(vec![("200 OK", r#"{"id":"page-1"}"#)]);
        let notes = || vec![note("note-1", ParaCategory::Projects, "# Hello")];

        run_notes(&vault, &client(&base), &registry(), notes(), |_| {})
            .await
            .unwrap();

        // A fresh client, so its request count covers the second run alone.
        let second = client(&base);
        let summary = run_notes(&vault, &second, &registry(), notes(), |_| {})
            .await
            .unwrap();

        assert_eq!(
            second.requests_sent(),
            0,
            "an unchanged vault must cost no API calls at all"
        );

        assert_eq!(summary.up_to_date, 1);
        assert_eq!(summary.created, 0);
        assert_eq!(summary.updated, 0);
    }

    #[tokio::test]
    async fn a_recategorised_note_moves_and_always_rewrites_its_properties() {
        // The move drops multi_select values, so the rewrite is not optional even though
        // the note itself did not change.
        let vault = vault();
        map::save(
            &vault,
            "note-1",
            &MapEntry {
                state: EntryState::Published,
                page_id: Some("page-1".into()),
                data_source_id: Some("ds-Projects".into()),
                content_hash: Some(content_hash_of(&note(
                    "note-1",
                    ParaCategory::Areas,
                    "body",
                ))),
                source_mtime: Some(1),
                relative_path: Some("Projects/note-1.md".into()),
                last_error: None,
            },
        )
        .unwrap();

        let (base, requests) = scripted_server(vec![
            ("200 OK", r#"{"id":"page-1"}"#),
            ("200 OK", r#"{"id":"page-1"}"#),
        ]);

        let summary = run_notes(
            &vault,
            &client(&base),
            &registry(),
            vec![note("note-1", ParaCategory::Areas, "body")],
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(summary.moved, 1);

        let move_request = requests.recv().unwrap();
        assert!(move_request.contains("/pages/page-1/move"));
        assert!(move_request.contains("ds-Areas"));

        let rewrite = requests.recv().unwrap();
        assert!(
            rewrite.contains("multi_select") && rewrite.contains("spike"),
            "the tags dropped by the move must be written back: {rewrite}"
        );

        let entry = map::load(&vault, "note-1").unwrap();
        assert_eq!(entry.data_source_id.as_deref(), Some("ds-Areas"));
        assert_eq!(
            entry.page_id.as_deref(),
            Some("page-1"),
            "identity preserved"
        );
    }

    #[tokio::test]
    async fn an_interrupted_creation_adopts_the_existing_page_instead_of_duplicating() {
        let vault = vault();
        map::save(&vault, "note-1", &MapEntry::creating()).unwrap();

        let (base, requests) = scripted_server(vec![
            // The query that finds the page the interrupted run created.
            ("200 OK", r#"{"results":[{"id":"page-orphan"}]}"#),
            ("200 OK", r#"{"id":"page-orphan"}"#), // erase
            ("200 OK", r#"{"id":"page-orphan"}"#), // append
            ("200 OK", r#"{"id":"page-orphan"}"#), // properties
        ]);

        let summary = run_notes(
            &vault,
            &client(&base),
            &registry(),
            vec![note("note-1", ParaCategory::Projects, "body")],
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(summary.created, 1);
        let query = requests.recv().unwrap();
        assert!(query.contains("/data_sources/ds-Projects/query"));

        let entry = map::load(&vault, "note-1").unwrap();
        assert_eq!(
            entry.page_id.as_deref(),
            Some("page-orphan"),
            "the orphaned page must be adopted, not left behind with a duplicate beside it"
        );
    }

    #[tokio::test]
    async fn a_lost_map_adopts_existing_pages_instead_of_duplicating_them() {
        // The acceptance criterion: the mapping can be rebuilt if lost. No map entry exists,
        // so the note looks new — but Notion already has its page, carrying its note id.
        let vault = vault();
        let page = r#"{"results":[{"id":"page-kept","properties":{"Note ID":{"rich_text":[{"plain_text":"note-1"}]}}}],"has_more":false}"#;
        let empty = r#"{"results":[],"has_more":false}"#;
        let (base, requests) = scripted_server(vec![
            ("200 OK", page), // Projects: the page is found here
            ("200 OK", empty),
            ("200 OK", empty),
            ("200 OK", empty),
            ("200 OK", r#"{"id":"page-kept"}"#), // erase
            ("200 OK", r#"{"id":"page-kept"}"#), // append
            ("200 OK", r#"{"id":"page-kept"}"#), // properties
        ]);

        let summary = run_notes(
            &vault,
            &client(&base),
            &registry(),
            vec![note("note-1", ParaCategory::Projects, "body")],
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(summary.created, 0, "creating here is exactly the duplicate");
        assert_eq!(summary.updated, 1);

        let sent: Vec<String> = (0..7).map(|_| requests.recv().unwrap()).collect();
        assert!(
            !sent.iter().any(|r| r.starts_with("POST /pages ")),
            "no page may be created for a note Notion already holds"
        );

        let entry = map::load(&vault, "note-1").expect("the map entry is rebuilt");
        assert_eq!(entry.page_id.as_deref(), Some("page-kept"));
    }

    #[tokio::test]
    async fn a_page_without_a_note_id_is_never_adopted() {
        // Someone added a page to the database by hand. It carries no note id, so it is not
        // ours: it must not be claimed, and the note still gets its own page.
        let vault = vault();
        let stranger = r#"{"results":[{"id":"page-by-hand","properties":{"Note ID":{"rich_text":[]}}}],"has_more":false}"#;
        let empty = r#"{"results":[],"has_more":false}"#;
        let (base, _r) = scripted_server(vec![
            ("200 OK", stranger),
            ("200 OK", empty),
            ("200 OK", empty),
            ("200 OK", empty),
            ("200 OK", r#"{"id":"page-new"}"#),
        ]);

        let summary = run_notes(
            &vault,
            &client(&base),
            &registry(),
            vec![note("note-1", ParaCategory::Projects, "body")],
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(summary.created, 1);
        assert_eq!(
            map::load(&vault, "note-1").unwrap().page_id.as_deref(),
            Some("page-new")
        );
    }

    #[tokio::test]
    async fn a_tombstoned_page_is_trashed_and_its_entry_removed() {
        let vault = vault();
        map::save(
            &vault,
            "gone",
            &MapEntry {
                state: EntryState::Deleted,
                page_id: Some("page-9".into()),
                data_source_id: Some("ds-Projects".into()),
                content_hash: Some("h".into()),
                source_mtime: Some(1),
                relative_path: Some("Projects/note-1.md".into()),
                last_error: None,
            },
        )
        .unwrap();

        let (base, requests) = scripted_server(vec![("200 OK", r#"{"id":"page-9"}"#)]);

        let summary = run_notes(&vault, &client(&base), &registry(), vec![], |_| {})
            .await
            .unwrap();

        assert_eq!(summary.trashed, 1);
        assert!(requests.recv().unwrap().contains("in_trash"));
        assert!(
            map::load(&vault, "gone").is_none(),
            "the tombstone should be cleared once the page is gone"
        );
    }

    #[tokio::test]
    async fn a_page_already_gone_from_notion_clears_its_tombstone_rather_than_retrying_forever() {
        let vault = vault();
        map::save(
            &vault,
            "gone",
            &MapEntry {
                state: EntryState::Deleted,
                page_id: Some("page-9".into()),
                data_source_id: None,
                content_hash: None,
                source_mtime: None,
                relative_path: None,
                last_error: None,
            },
        )
        .unwrap();

        let (base, _r) = scripted_server(vec![("404 Not Found", r#"{"message":"gone"}"#)]);

        let summary = run_notes(&vault, &client(&base), &registry(), vec![], |_| {})
            .await
            .unwrap();

        assert_eq!(summary.trashed, 1);
        assert!(map::load(&vault, "gone").is_none());
    }

    #[tokio::test]
    async fn one_rejected_note_does_not_stop_the_others() {
        // A publisher that stops publishing looks exactly like one with nothing to do,
        // which is why this must not halt.
        let vault = vault();
        let (base, _r) = creating_server(vec![
            (
                "400 Bad Request",
                r#"{"message":"body.children[0] invalid"}"#,
            ),
            ("200 OK", r#"{"id":"page-2"}"#),
        ]);

        let summary = run_notes(
            &vault,
            &client(&base),
            &registry(),
            vec![
                note("bad", ParaCategory::Projects, "body"),
                note("good", ParaCategory::Projects, "body"),
            ],
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(summary.failed, 1);
        assert_eq!(summary.created, 1);

        let failed = map::load(&vault, "bad").expect("the failure must be recorded");
        assert!(
            failed.last_error.is_some(),
            "a silent skip is how one unpublishable note goes unnoticed for weeks"
        );
    }

    #[tokio::test]
    async fn a_rejected_token_stops_the_run_immediately() {
        let vault = vault();
        let (base, _r) = scripted_server(vec![
            ("401 Unauthorized", r#"{"message":"API token is invalid."}"#),
            ("200 OK", r#"{"id":"page-2"}"#),
        ]);

        let error = run_notes(
            &vault,
            &client(&base),
            &registry(),
            vec![
                note("one", ParaCategory::Projects, "body"),
                note("two", ParaCategory::Projects, "body"),
            ],
            |_| {},
        )
        .await
        .unwrap_err();

        assert!(error.is_fatal());
        assert!(
            map::load(&vault, "two").is_none(),
            "the second note should never have been attempted"
        );
    }

    #[tokio::test]
    async fn a_conflict_copy_is_skipped_and_counted() {
        let vault = vault();
        let (base, _r) = scripted_server(vec![]);
        let mut copy = note("note-1", ParaCategory::Projects, "body");
        copy.snapshot.relative_path =
            "Projects/Note.sync-conflict-20260903-141500-ABCDEF.md".into();

        let summary = run_notes(&vault, &client(&base), &registry(), vec![copy], |_| {})
            .await
            .unwrap();

        assert_eq!(summary.total_skipped(), 1);
        assert_eq!(summary.skipped.get("conflict_copy"), Some(&1));
        assert_eq!(summary.created, 0);
    }

    #[tokio::test]
    async fn an_uncategorised_note_is_skipped_with_a_reason_the_panel_can_show() {
        let vault = vault();
        let (base, _r) = scripted_server(vec![]);
        let mut orphan = note("note-1", ParaCategory::Projects, "body");
        orphan.snapshot.category = None;

        let summary = run_notes(&vault, &client(&base), &registry(), vec![orphan], |_| {})
            .await
            .unwrap();

        assert_eq!(summary.skipped.get("uncategorised"), Some(&1));
    }

    #[tokio::test]
    async fn a_long_note_is_sent_in_batches_of_a_hundred_blocks() {
        let vault = vault();
        let body = (0..250)
            .map(|n| format!("Paragraph {n}."))
            .collect::<Vec<_>>()
            .join("\n\n");

        let (base, requests) = creating_server(vec![
            ("200 OK", r#"{"id":"page-1"}"#),
            ("200 OK", r#"{"id":"page-1"}"#),
            ("200 OK", r#"{"id":"page-1"}"#),
        ]);

        let summary = run_notes(
            &vault,
            &client(&base),
            &registry(),
            vec![note("long", ParaCategory::Projects, &body)],
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(summary.created, 1);
        let create = {
            for _ in 0..4 {
                requests.recv().unwrap();
            }
            requests.recv().unwrap()
        };
        assert!(create.contains("POST /pages"));
        // Two appends follow the create: 250 blocks is 100 + 100 + 50.
        assert!(requests.recv().unwrap().contains("/blocks/page-1/children"));
        assert!(requests.recv().unwrap().contains("/blocks/page-1/children"));
    }

    #[tokio::test]
    async fn progress_is_reported_for_every_note_so_a_first_sync_is_not_a_hang() {
        let vault = vault();
        let (base, _r) = creating_server(vec![
            ("200 OK", r#"{"id":"p1"}"#),
            ("200 OK", r#"{"id":"p2"}"#),
        ]);

        let mut seen = Vec::new();
        run_notes(
            &vault,
            &client(&base),
            &registry(),
            vec![
                note("one", ParaCategory::Projects, "a"),
                note("two", ParaCategory::Areas, "b"),
            ],
            |progress| seen.push((progress.done, progress.total)),
        )
        .await
        .unwrap();

        assert_eq!(seen, vec![(1, 2), (2, 2)]);
    }

    #[tokio::test]
    async fn an_unchanged_note_is_never_even_read_from_disk() {
        // The reason `run` takes an enumeration and a reader rather than the notes: at
        // twelve polls an hour, re-reading the whole vault to learn nothing is the cost
        // this exists to avoid.
        let vault = vault();
        map::save(
            &vault,
            "note-1",
            &MapEntry {
                state: EntryState::Published,
                page_id: Some("page-1".into()),
                data_source_id: Some("ds-Projects".into()),
                content_hash: Some("whatever".into()),
                source_mtime: Some(100),
                relative_path: Some("Projects/note-1.md".into()),
                last_error: None,
            },
        )
        .unwrap();

        let (base, _r) = scripted_server(vec![]);
        let snapshot = note("note-1", ParaCategory::Projects, "body").snapshot;
        let mut reads = 0;

        let summary = run(
            &vault,
            &client(&base),
            &registry(),
            vec![snapshot],
            |_| {
                reads += 1;
                Err("the file should never have been opened".to_string())
            },
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(reads, 0, "an unchanged mtime must not cost a file read");
        assert_eq!(summary.up_to_date, 1);
    }

    #[tokio::test]
    async fn a_note_that_cannot_be_read_is_skipped_without_stopping_the_run() {
        // Mid-write by the editor, or still arriving from the other machine.
        let vault = vault();
        let (base, _r) = creating_server(vec![("200 OK", r#"{"id":"page-2"}"#)]);
        let unreadable = note("locked", ParaCategory::Projects, "body").snapshot;
        let readable = note("fine", ParaCategory::Projects, "body");
        let readable_snapshot = readable.snapshot.clone();
        let mut readable = Some(readable);

        let summary = run(
            &vault,
            &client(&base),
            &registry(),
            vec![unreadable, readable_snapshot],
            |snapshot| {
                if snapshot.note_id == "fine" {
                    readable.take().ok_or_else(|| "already taken".to_string())
                } else {
                    Err("permission denied".to_string())
                }
            },
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(summary.failed, 1);
        assert_eq!(summary.created, 1, "the readable note still publishes");
    }

    #[test]
    fn a_tag_change_alone_counts_as_a_change() {
        // The body is untouched, but the page would show the wrong tags.
        let mut tagged = note("note-1", ParaCategory::Projects, "same body");
        let before = content_hash_of(&tagged);
        tagged.meta.tags = vec!["different".into()];
        assert_ne!(before, content_hash_of(&tagged));
    }

    #[test]
    fn a_title_change_alone_counts_as_a_change() {
        let mut renamed = note("note-1", ParaCategory::Projects, "same body");
        let before = content_hash_of(&renamed);
        renamed.meta.title = "Renamed".into();
        assert_ne!(before, content_hash_of(&renamed));
    }
}
