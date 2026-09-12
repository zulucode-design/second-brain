//! Deciding what a push should do, separately from doing it.
//!
//! Everything here is a pure function of the vault's state and the map. That is deliberate:
//! the decisions are where the correctness lives — whether a note is duplicated, whether a
//! deleted page is leaked, whether a moved note keeps its identity — and none of it should
//! need a Notion account to test. [`super::publish`] performs what this decides and
//! contains no judgement of its own.
//!
//! ## The two gates, and why there are two
//!
//! [`needs_inspection`] compares modification times and nothing else, so an unchanged note
//! costs no *file read*. Polling every five minutes would otherwise re-read the whole vault
//! twelve times an hour to learn nothing.
//!
//! [`decide`] compares content hashes and the data source, and is what actually authorises
//! an API call. The mtime is only a hint: a note arriving from the other machine has a new
//! mtime and identical bytes, and republishing it would be a wasted call on every sync.

use serde::Serialize;

use super::config::DatabaseRegistry;
use super::map::{EntryState, MapEntry};
use crate::vault::para::ParaCategory;

/// A note as found on disk, before it has been read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSnapshot {
    pub note_id: String,
    pub relative_path: String,
    pub category: Option<ParaCategory>,
    /// Filesystem modification time, in nanoseconds; see `commands::mtime_of` for why
    /// not seconds.
    pub mtime: i64,
}

/// A note that has been read, so its content is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedNote {
    pub snapshot: NoteSnapshot,
    pub content_hash: String,
}

/// Why a note will not be published.
///
/// Surfaced as a count in Settings rather than logged and forgotten: "why is this note not
/// in Notion" having no answer anywhere is a worse outcome than the note being absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// No PARA category, so there is no database it belongs in. Publishing it anywhere
    /// would make an uncategorised note look filed, which SPEC §4 refuses.
    Uncategorised,
    /// A sync conflict copy. ADR-0002 requires these never to surface as ordinary notes;
    /// publishing one would put a machine-generated duplicate in the reader's view.
    ConflictCopy,
    /// Setup has not finished for this note's category.
    NoDatabase,
}

/// What the publisher should do about one note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Create a page for a note that has never been published.
    Create {
        note_id: String,
        relative_path: String,
        data_source_id: String,
        content_hash: String,
        mtime: i64,
    },
    /// A previous run was interrupted between writing the map and finishing the create.
    ///
    /// Notion is asked whether the page exists before anything is created, which is the
    /// whole reason the `Creating` state is written before the call rather than after.
    ResolveInterrupted {
        note_id: String,
        relative_path: String,
        data_source_id: String,
        content_hash: String,
        mtime: i64,
    },
    /// The note's text changed; replace the page's content.
    UpdateContent {
        note_id: String,
        relative_path: String,
        page_id: String,
        data_source_id: String,
        content_hash: String,
        mtime: i64,
    },
    /// The note changed category; move the page and rewrite its properties.
    ///
    /// `also_update_content` folds an edit into the same visit, so a note that was both
    /// edited and recategorised is not pushed twice.
    Move {
        note_id: String,
        relative_path: String,
        page_id: String,
        from_data_source_id: Option<String>,
        data_source_id: String,
        content_hash: String,
        mtime: i64,
        also_update_content: bool,
    },
    /// The note was deleted locally; its page is trashed and the tombstone cleared.
    Trash { note_id: String, page_id: String },
    /// Nothing to do, but the reason is worth counting.
    Skip { note_id: String, reason: SkipReason },
    /// The note is published and unchanged.
    UpToDate { note_id: String },
}

/// Source and destination state shared by every action that finishes as published.
pub struct PublishRecord<'a> {
    pub note_id: &'a str,
    pub relative_path: &'a str,
    pub data_source_id: &'a str,
    pub content_hash: &'a str,
    pub mtime: i64,
}

impl Action {
    /// Whether this action calls Notion at all, for reporting how much a run actually did.
    pub fn is_work(&self) -> bool {
        !matches!(self, Action::Skip { .. } | Action::UpToDate { .. })
    }

    /// The map state written after an action creates, recovers, updates, or moves a page.
    pub fn publish_record(&self) -> Option<PublishRecord<'_>> {
        match self {
            Action::Create {
                note_id,
                relative_path,
                data_source_id,
                content_hash,
                mtime,
            }
            | Action::ResolveInterrupted {
                note_id,
                relative_path,
                data_source_id,
                content_hash,
                mtime,
            }
            | Action::UpdateContent {
                note_id,
                relative_path,
                data_source_id,
                content_hash,
                mtime,
                ..
            }
            | Action::Move {
                note_id,
                relative_path,
                data_source_id,
                content_hash,
                mtime,
                ..
            } => Some(PublishRecord {
                note_id,
                relative_path,
                data_source_id,
                content_hash,
                mtime: *mtime,
            }),
            Action::Trash { .. } | Action::Skip { .. } | Action::UpToDate { .. } => None,
        }
    }
}

/// A file the app wrote as a sync conflict copy rather than a note the user made.
///
/// The engine writes `note.sync-conflict-20260903-141500-ABCDEF.md` beside the original.
/// It ends in `.md` and sits in the notes tree, so nothing but this check keeps it out of
/// the published view.
pub fn is_conflict_copy(relative_path: &str) -> bool {
    relative_path.contains(".sync-conflict-")
}

/// Whether a note is worth reading, judged on its modification time alone.
///
/// Returns true when anything is unknown. Reading a file needlessly costs microseconds;
/// skipping one that changed means the reader sees a stale note indefinitely, with nothing
/// to indicate it.
pub fn needs_inspection(entry: Option<&MapEntry>, mtime: i64) -> bool {
    match entry {
        None => true,
        Some(entry) => match entry.state {
            // Both states have outstanding work no mtime can rule out.
            EntryState::Creating | EntryState::Deleted => true,
            EntryState::Published => entry.source_mtime != Some(mtime),
        },
    }
}

/// What to do about a note that has been read.
pub fn decide(
    note: &InspectedNote,
    entry: Option<&MapEntry>,
    registry: &DatabaseRegistry,
) -> Action {
    let note_id = note.snapshot.note_id.clone();

    if is_conflict_copy(&note.snapshot.relative_path) {
        return Action::Skip {
            note_id,
            reason: SkipReason::ConflictCopy,
        };
    }

    let Some(category) = note.snapshot.category else {
        return Action::Skip {
            note_id,
            reason: SkipReason::Uncategorised,
        };
    };

    let Some(link) = registry.link(category) else {
        return Action::Skip {
            note_id,
            reason: SkipReason::NoDatabase,
        };
    };
    let data_source_id = link.data_source_id.clone();
    let content_hash = note.content_hash.clone();
    let relative_path = note.snapshot.relative_path.clone();
    let mtime = note.snapshot.mtime;

    let Some(entry) = entry else {
        return Action::Create {
            note_id,
            relative_path,
            data_source_id,
            content_hash,
            mtime,
        };
    };

    match entry.state {
        EntryState::Creating => Action::ResolveInterrupted {
            note_id,
            relative_path,
            data_source_id,
            content_hash,
            mtime,
        },

        // A tombstoned note that is on disk again was restored from the app's trash before
        // the publisher acted. The page was never touched, so nothing needs saying to
        // Notion — clearing the tombstone is enough, and the page keeps its identity.
        EntryState::Deleted => match &entry.page_id {
            Some(page_id) if entry.data_source_id.as_deref() == Some(&data_source_id) => {
                if entry.content_hash.as_deref() == Some(content_hash.as_str()) {
                    Action::UpToDate { note_id }
                } else {
                    Action::UpdateContent {
                        note_id,
                        relative_path,
                        page_id: page_id.clone(),
                        data_source_id,
                        content_hash,
                        mtime,
                    }
                }
            }
            Some(page_id) => Action::Move {
                note_id,
                relative_path,
                page_id: page_id.clone(),
                from_data_source_id: entry.data_source_id.clone(),
                data_source_id,
                also_update_content: entry.content_hash.as_deref() != Some(content_hash.as_str()),
                content_hash,
                mtime,
            },
            None => Action::Create {
                note_id,
                relative_path,
                data_source_id,
                content_hash,
                mtime,
            },
        },

        EntryState::Published => {
            let Some(page_id) = entry.page_id.clone() else {
                // Published without a page id should not happen, but a hand-edited or
                // half-synced file can produce it. Resolving through Notion is safe;
                // creating blind is not.
                return Action::ResolveInterrupted {
                    note_id,
                    relative_path,
                    data_source_id,
                    content_hash,
                    mtime,
                };
            };

            let moved = entry.data_source_id.as_deref() != Some(data_source_id.as_str());
            let edited = entry.content_hash.as_deref() != Some(content_hash.as_str());

            match (moved, edited) {
                (false, false) => Action::UpToDate { note_id },
                (false, true) => Action::UpdateContent {
                    note_id,
                    relative_path,
                    page_id,
                    data_source_id,
                    content_hash,
                    mtime,
                },
                // A move always rewrites properties, so an edit costs nothing extra to fold
                // in here rather than pushing the same note twice.
                (true, edited) => Action::Move {
                    note_id,
                    relative_path,
                    page_id,
                    from_data_source_id: entry.data_source_id.clone(),
                    data_source_id,
                    also_update_content: edited,
                    content_hash,
                    mtime,
                },
            }
        }
    }
}

/// Turn a create into an adoption when Notion already holds a page for the note.
///
/// A note with no map entry looks never-published, but that is only one of the two things
/// it can mean. The other is that the map was lost — a wiped machine, a damaged file, a
/// directory the sync engine dropped. Creating in that case gives every note a second page.
/// So a create is checked against what Notion actually holds, which is the rebuild the map
/// promises: the note id travels to Notion as a property precisely so it can be read back.
///
/// `existing` is `(page id, data source id)` for the page carrying this note's id. An adopted
/// page's content is refreshed rather than trusted, because nothing records whether the note
/// changed while the map was gone.
pub fn adopt_existing(action: Action, existing: Option<&(String, String)>) -> Action {
    let (
        Action::Create {
            note_id,
            relative_path,
            data_source_id,
            content_hash,
            mtime,
        },
        Some((page_id, found_in)),
    ) = (&action, existing)
    else {
        return action;
    };

    if found_in == data_source_id {
        Action::UpdateContent {
            note_id: note_id.clone(),
            relative_path: relative_path.clone(),
            page_id: page_id.clone(),
            data_source_id: data_source_id.clone(),
            content_hash: content_hash.clone(),
            mtime: *mtime,
        }
    } else {
        // The page is in another category's database: the note was recategorised while
        // the map was gone. Moving it keeps its identity; creating would leave the old one.
        Action::Move {
            note_id: note_id.clone(),
            relative_path: relative_path.clone(),
            page_id: page_id.clone(),
            from_data_source_id: Some(found_in.clone()),
            data_source_id: data_source_id.clone(),
            content_hash: content_hash.clone(),
            mtime: *mtime,
            also_update_content: true,
        }
    }
}

/// Pages that must be trashed because their notes were deleted.
///
/// Driven from tombstones rather than from notes that have gone missing. Those two look
/// identical on disk, and ADR-0002 warns about exactly this: with sync, a file that is
/// absent may simply not have arrived yet. Inferring deletion would trash the pages of
/// notes that are merely in transit.
pub fn tombstone_actions(entries: &[(String, MapEntry)]) -> Vec<Action> {
    entries
        .iter()
        .filter(|(_, entry)| entry.state == EntryState::Deleted)
        .filter_map(|(note_id, entry)| {
            entry.page_id.as_ref().map(|page_id| Action::Trash {
                note_id: note_id.clone(),
                page_id: page_id.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notion::config::DatabaseLink;

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

    fn note(category: Option<ParaCategory>, hash: &str) -> InspectedNote {
        InspectedNote {
            snapshot: NoteSnapshot {
                note_id: "note-1".into(),
                relative_path: "Projects/Thing.md".into(),
                category,
                mtime: 100,
            },
            content_hash: hash.into(),
        }
    }

    fn published(hash: &str, data_source: &str) -> MapEntry {
        MapEntry {
            state: EntryState::Published,
            page_id: Some("page-1".into()),
            data_source_id: Some(data_source.into()),
            content_hash: Some(hash.into()),
            source_mtime: Some(100),
            relative_path: Some("Projects/Thing.md".into()),
            last_error: None,
        }
    }

    #[test]
    fn a_note_never_published_is_created() {
        let action = decide(&note(Some(ParaCategory::Projects), "h1"), None, &registry());
        assert!(matches!(
            action,
            Action::Create { ref data_source_id, .. } if data_source_id == "ds-Projects"
        ));
    }

    #[test]
    fn an_unchanged_note_does_nothing() {
        let entry = published("h1", "ds-Projects");
        let action = decide(
            &note(Some(ParaCategory::Projects), "h1"),
            Some(&entry),
            &registry(),
        );
        assert!(matches!(action, Action::UpToDate { .. }));
        assert!(!action.is_work());
    }

    #[test]
    fn an_edited_note_replaces_its_content() {
        let entry = published("h1", "ds-Projects");
        let action = decide(
            &note(Some(ParaCategory::Projects), "h2"),
            Some(&entry),
            &registry(),
        );
        assert!(matches!(action, Action::UpdateContent { ref page_id, .. } if page_id == "page-1"));
    }

    #[test]
    fn a_recategorised_note_moves_and_keeps_its_page() {
        // The bytes are identical; only the category changed. This is the case a
        // hash-only comparison would miss entirely.
        let entry = published("h1", "ds-Projects");
        let mut moved = note(Some(ParaCategory::Areas), "h1");
        moved.snapshot.relative_path = "Areas/Thing.md".into();

        let action = decide(&moved, Some(&entry), &registry());

        match action {
            Action::Move {
                page_id,
                data_source_id,
                also_update_content,
                ..
            } => {
                assert_eq!(page_id, "page-1", "identity must be preserved");
                assert_eq!(data_source_id, "ds-Areas");
                assert!(!also_update_content, "nothing was edited");
            }
            other => panic!("expected a move, got {other:?}"),
        }
    }

    #[test]
    fn a_note_both_edited_and_recategorised_is_visited_once() {
        let entry = published("h1", "ds-Projects");
        let action = decide(
            &note(Some(ParaCategory::Areas), "h2"),
            Some(&entry),
            &registry(),
        );

        match action {
            Action::Move {
                also_update_content,
                ..
            } => assert!(
                also_update_content,
                "the edit should fold into the move rather than pushing twice"
            ),
            other => panic!("expected a move, got {other:?}"),
        }
    }

    #[test]
    fn an_interrupted_creation_asks_notion_rather_than_creating_again() {
        // The window: page created, process killed before the map was updated. Creating
        // blind here is exactly how the vault ends up with two pages per note.
        let entry = MapEntry::creating();
        let action = decide(
            &note(Some(ParaCategory::Projects), "h1"),
            Some(&entry),
            &registry(),
        );
        assert!(matches!(action, Action::ResolveInterrupted { .. }));
    }

    #[test]
    fn a_published_entry_that_lost_its_page_id_resolves_instead_of_creating() {
        let mut entry = published("h1", "ds-Projects");
        entry.page_id = None;
        let action = decide(
            &note(Some(ParaCategory::Projects), "h1"),
            Some(&entry),
            &registry(),
        );
        assert!(matches!(action, Action::ResolveInterrupted { .. }));
    }

    #[test]
    fn an_uncategorised_note_is_skipped_and_counted() {
        // Publishing it would make an uncategorised note look filed, which SPEC §4 refuses.
        let action = decide(&note(None, "h1"), None, &registry());
        assert_eq!(
            action,
            Action::Skip {
                note_id: "note-1".into(),
                reason: SkipReason::Uncategorised
            }
        );
    }

    #[test]
    fn a_conflict_copy_never_reaches_the_published_view() {
        let mut copy = note(Some(ParaCategory::Projects), "h1");
        copy.snapshot.relative_path =
            "Projects/Thing.sync-conflict-20260903-141500-ABCDEF.md".into();

        let action = decide(&copy, None, &registry());

        assert_eq!(
            action,
            Action::Skip {
                note_id: "note-1".into(),
                reason: SkipReason::ConflictCopy
            }
        );
    }

    #[test]
    fn a_category_with_no_database_is_skipped_rather_than_published_elsewhere() {
        let mut partial = DatabaseRegistry {
            parent_page_id: Some("parent".into()),
            ..Default::default()
        };
        partial.set_link(
            ParaCategory::Areas,
            DatabaseLink {
                database_id: "db".into(),
                data_source_id: "ds".into(),
            },
        );

        let action = decide(&note(Some(ParaCategory::Projects), "h1"), None, &partial);

        assert_eq!(
            action,
            Action::Skip {
                note_id: "note-1".into(),
                reason: SkipReason::NoDatabase
            }
        );
    }

    #[test]
    fn a_note_restored_from_trash_costs_no_api_call() {
        // Deleted then restored before the publisher ran: the page was never touched.
        let mut entry = published("h1", "ds-Projects");
        entry.state = EntryState::Deleted;

        let action = decide(
            &note(Some(ParaCategory::Projects), "h1"),
            Some(&entry),
            &registry(),
        );

        assert!(matches!(action, Action::UpToDate { .. }));
        assert!(!action.is_work());
    }

    #[test]
    fn a_note_restored_and_edited_updates_rather_than_recreating() {
        let mut entry = published("h1", "ds-Projects");
        entry.state = EntryState::Deleted;

        let action = decide(
            &note(Some(ParaCategory::Projects), "h2"),
            Some(&entry),
            &registry(),
        );

        assert!(matches!(action, Action::UpdateContent { ref page_id, .. } if page_id == "page-1"));
    }

    #[test]
    fn a_deleted_note_that_never_had_a_page_is_simply_created() {
        let mut entry = MapEntry::creating();
        entry.state = EntryState::Deleted;
        let action = decide(
            &note(Some(ParaCategory::Projects), "h1"),
            Some(&entry),
            &registry(),
        );
        assert!(matches!(action, Action::Create { .. }));
    }

    fn create_action() -> Action {
        decide(&note(Some(ParaCategory::Projects), "h1"), None, &registry())
    }

    #[test]
    fn a_note_with_no_entry_and_no_page_is_still_created() {
        // Genuinely new: the check finds nothing, and the create goes ahead unchanged.
        let action = adopt_existing(create_action(), None);
        assert!(matches!(action, Action::Create { .. }));
    }

    #[test]
    fn a_lost_map_adopts_the_existing_page_instead_of_duplicating_it() {
        // The map was lost, so the note looks new. Notion already has its page.
        let existing = ("page-kept".to_string(), "ds-Projects".to_string());
        let action = adopt_existing(create_action(), Some(&existing));

        match action {
            Action::UpdateContent { page_id, .. } => assert_eq!(page_id, "page-kept"),
            other => panic!("expected the page to be adopted, got {other:?}"),
        }
    }

    #[test]
    fn a_lost_map_whose_note_changed_category_moves_the_page_rather_than_leaving_it() {
        let existing = ("page-kept".to_string(), "ds-Areas".to_string());
        let action = adopt_existing(create_action(), Some(&existing));

        match action {
            Action::Move {
                page_id,
                data_source_id,
                also_update_content,
                ..
            } => {
                assert_eq!(page_id, "page-kept", "identity preserved");
                assert_eq!(data_source_id, "ds-Projects");
                assert!(
                    also_update_content,
                    "nothing records whether the note changed while the map was gone"
                );
            }
            other => panic!("expected a move, got {other:?}"),
        }
    }

    #[test]
    fn adoption_leaves_every_other_action_alone() {
        let existing = ("page".to_string(), "ds-Projects".to_string());
        let up_to_date = Action::UpToDate {
            note_id: "n".into(),
        };
        assert_eq!(
            adopt_existing(up_to_date.clone(), Some(&existing)),
            up_to_date
        );
    }

    #[test]
    fn tombstones_become_trash_actions() {
        let mut deleted = published("h1", "ds-Projects");
        deleted.state = EntryState::Deleted;
        let entries = vec![
            ("note-1".to_string(), deleted),
            ("note-2".to_string(), published("h1", "ds-Projects")),
        ];

        let actions = tombstone_actions(&entries);

        assert_eq!(actions.len(), 1, "only the tombstone should produce work");
        assert_eq!(
            actions[0],
            Action::Trash {
                note_id: "note-1".into(),
                page_id: "page-1".into()
            }
        );
    }

    #[test]
    fn a_tombstone_with_no_page_produces_no_call() {
        let mut orphan = MapEntry::creating();
        orphan.state = EntryState::Deleted;
        assert!(tombstone_actions(&[("note-1".into(), orphan)]).is_empty());
    }

    #[test]
    fn an_unchanged_modification_time_means_the_file_is_not_even_read() {
        let entry = published("h1", "ds-Projects");
        assert!(!needs_inspection(Some(&entry), 100));
        assert!(needs_inspection(Some(&entry), 101));
    }

    #[test]
    fn a_note_with_no_entry_is_always_read() {
        assert!(needs_inspection(None, 100));
    }

    #[test]
    fn unfinished_work_is_always_reconsidered_whatever_the_timestamps_say() {
        // A `Creating` entry has an unknown outcome and a tombstone has an outstanding
        // call; neither can be ruled out by a file's modification time.
        let mut creating = MapEntry::creating();
        creating.source_mtime = Some(100);
        assert!(needs_inspection(Some(&creating), 100));

        let mut deleted = published("h1", "ds");
        deleted.state = EntryState::Deleted;
        assert!(needs_inspection(Some(&deleted), 100));
    }

    #[test]
    fn a_file_resynced_with_identical_bytes_is_not_republished() {
        // Sync gives an arriving file a new mtime. The mtime gate lets it through to be
        // read; the hash is what stops it costing an API call.
        let entry = published("h1", "ds-Projects");
        let mut touched = note(Some(ParaCategory::Projects), "h1");
        touched.snapshot.mtime = 999;

        assert!(needs_inspection(Some(&entry), 999), "it should be read");
        assert!(
            !decide(&touched, Some(&entry), &registry()).is_work(),
            "but it should not be pushed"
        );
    }

    #[test]
    fn conflict_copies_are_recognised_by_the_engines_own_naming() {
        assert!(is_conflict_copy(
            "Projects/Note.sync-conflict-20260903-141500-ABCDEF.md"
        ));
        assert!(!is_conflict_copy("Projects/Note.md"));
        assert!(
            !is_conflict_copy("Projects/Conflict resolution.md"),
            "an ordinary note about conflicts is not a conflict copy"
        );
    }
}
