//! A note's frontmatter as Notion page properties.
//!
//! The body becomes blocks; everything else becomes properties, so a reader sees clean
//! prose with its metadata in the property panel rather than a YAML fence at the top of
//! every page.
//!
//! There is no `Category` property. The database *is* the category, so a second copy of
//! that fact would be one more thing to keep correct every time a note moves between them.

use serde_json::{json, Value};

use super::config::NOTE_ID_PROPERTY;
use crate::types::NoteMeta;

/// Notion's cap on one rich-text run, which the title shares.
const MAX_TEXT: usize = 2000;

/// Notion's cap on options in one multi-select value.
const MAX_TAGS: usize = 100;

/// Longest a multi-select option name may be.
const MAX_TAG_LENGTH: usize = 100;

/// Build the property object for a note.
pub fn for_note(meta: &NoteMeta) -> Value {
    json!({
        "Name": { "title": title_runs(&meta.title) },
        NOTE_ID_PROPERTY: { "rich_text": [{ "text": { "content": clamp(&meta.id) } }] },
        "Tags": { "multi_select": tag_options(&meta.tags) },
        "Created": { "date": { "start": meta.created.to_rfc3339() } },
        "Modified": { "date": { "start": meta.modified.to_rfc3339() } },
    })
}

/// A note with no title still needs one: Notion shows an empty title as "Untitled", which
/// is indistinguishable from a page that failed to write.
fn title_runs(title: &str) -> Value {
    let text = if title.trim().is_empty() {
        "Untitled note"
    } else {
        title
    };
    json!([{ "text": { "content": clamp(text) } }])
}

/// Truncate to Notion's per-run limit.
///
/// By characters, not bytes: slicing a UTF-8 string at a byte offset panics mid-codepoint,
/// and a title is exactly where an emoji tends to appear.
fn clamp(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

/// Turn a note's tags into multi-select options.
///
/// Two constraints Notion imposes and does not forgive: an option name may not contain a
/// comma, and there is a limit on how many options one value may hold. Both are enforced
/// here, because either would fail the whole page write — and a note is not worth losing
/// over a tag.
fn tag_options(tags: &[String]) -> Value {
    let options: Vec<Value> = tags
        .iter()
        .map(|tag| tag.replace(',', " "))
        .map(|tag| tag.trim().chars().take(MAX_TAG_LENGTH).collect::<String>())
        .filter(|tag| !tag.is_empty())
        .take(MAX_TAGS)
        .map(|tag| json!({ "name": tag }))
        .collect();
    json!(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn meta() -> NoteMeta {
        NoteMeta {
            id: "note-abc".into(),
            title: "A note".into(),
            tags: vec!["rust".into(), "design".into()],
            pinned: false,
            created: Utc.with_ymd_and_hms(2026, 9, 1, 10, 0, 0).unwrap(),
            modified: Utc.with_ymd_and_hms(2026, 9, 2, 11, 30, 0).unwrap(),
            category: Some(crate::vault::para::ParaCategory::Projects),
            source_url: None,
        }
    }

    #[test]
    fn the_note_id_travels_as_a_property_so_the_map_can_be_rebuilt() {
        let properties = for_note(&meta());
        assert_eq!(
            properties[NOTE_ID_PROPERTY]["rich_text"][0]["text"]["content"],
            json!("note-abc")
        );
    }

    #[test]
    fn the_title_becomes_the_pages_title() {
        let properties = for_note(&meta());
        assert_eq!(properties["Name"]["title"][0]["text"]["content"], json!("A note"));
    }

    #[test]
    fn an_untitled_note_is_named_rather_than_left_blank() {
        // An empty Notion title renders as "Untitled", which reads as a failed write.
        let mut meta = meta();
        meta.title = "   ".into();
        let properties = for_note(&meta);
        assert_eq!(
            properties["Name"]["title"][0]["text"]["content"],
            json!("Untitled note")
        );
    }

    #[test]
    fn tags_become_multi_select_options() {
        let properties = for_note(&meta());
        let tags = properties["Tags"]["multi_select"].as_array().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0]["name"], json!("rust"));
    }

    #[test]
    fn a_comma_in_a_tag_is_removed_rather_than_failing_the_page() {
        // Notion rejects a multi-select option containing a comma, and the rejection takes
        // the whole page with it. A note is not worth losing over a tag.
        let mut meta = meta();
        meta.tags = vec!["one,two".into()];
        let properties = for_note(&meta);
        assert_eq!(
            properties["Tags"]["multi_select"][0]["name"],
            json!("one two")
        );
    }

    #[test]
    fn an_empty_tag_is_dropped_rather_than_sent() {
        let mut meta = meta();
        meta.tags = vec!["".into(), "  ".into(), ",".into(), "real".into()];
        let properties = for_note(&meta);
        let tags = properties["Tags"]["multi_select"].as_array().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0]["name"], json!("real"));
    }

    #[test]
    fn absurd_numbers_of_tags_are_capped_at_notions_limit() {
        let mut meta = meta();
        meta.tags = (0..150).map(|n| format!("tag{n}")).collect();
        let properties = for_note(&meta);
        assert_eq!(
            properties["Tags"]["multi_select"].as_array().unwrap().len(),
            MAX_TAGS
        );
    }

    #[test]
    fn a_very_long_tag_is_truncated_rather_than_rejected() {
        let mut meta = meta();
        meta.tags = vec!["t".repeat(400)];
        let properties = for_note(&meta);
        let name = properties["Tags"]["multi_select"][0]["name"].as_str().unwrap();
        assert_eq!(name.chars().count(), MAX_TAG_LENGTH);
    }

    #[test]
    fn a_very_long_title_is_truncated_without_panicking_on_a_multibyte_boundary() {
        let mut meta = meta();
        meta.title = "é".repeat(3000);
        let properties = for_note(&meta);
        let title = properties["Name"]["title"][0]["text"]["content"]
            .as_str()
            .unwrap();
        assert_eq!(title.chars().count(), MAX_TEXT);
    }

    #[test]
    fn dates_are_sent_in_a_format_notion_accepts() {
        let properties = for_note(&meta());
        assert_eq!(
            properties["Created"]["date"]["start"],
            json!("2026-09-01T10:00:00+00:00")
        );
    }

    #[test]
    fn there_is_no_category_property() {
        // The database is the category. A second copy is one more thing to keep correct.
        let properties = for_note(&meta());
        assert!(properties.get("Category").is_none());
    }
}
