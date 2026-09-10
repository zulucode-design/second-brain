//! Publishing the vault to Notion as a read-only view.
//!
//! Notion is not the sync hub and is not a writer. The two machines sync to each other
//! directly (ADR-0002); Notion exists so the notes can be read on a phone or in a browser
//! when neither machine is to hand. Nothing flows back — not edits, not new pages. A note
//! created in Notion would have no PARA category, and this vault refuses uncategorised
//! notes by construction (SPEC §4). Write-back is #30.
//!
//! ## The shape of it
//!
//! Four Notion databases, one per PARA category. A note is one page in the database for
//! its category, and changing a note's category moves that page rather than replacing it.
//!
//! ## Three things that are easy to get wrong
//!
//! **Notion cannot read.** It has no agent and no access to the vault, and there is no
//! server in this architecture. Every page exists because a process on one machine called
//! the API. Exactly one machine therefore publishes — two machines can both observe "no
//! page for this note" and both create one. The published view is still the *merged* state
//! of both machines, because the publisher reads the vault after sync has written to it.
//!
//! **A category change is two calls, not one.** `POST /v1/pages/{id}/move` preserves the
//! page id, its content, and its text properties, but drops `multi_select` values: those
//! reference options registered on the data source that owns them, and the target data
//! source has never seen them. So a move is always followed by a property rewrite, even
//! when the note itself has not changed. Verified against the live API; see
//! `docs/reports/spike-notion-move-2026-09-10.md`.
//!
//! **Attachments never upload.** Notion receives text, metadata, and transcripts only.
//! A note read in Notion references attachments it cannot display, which is intended: the
//! binaries sync between the machines instead, and the free tier caps uploads at 5MB
//! regardless.

pub mod blocks;
pub mod client;
pub mod config;
pub mod map;
