use crate::types::NoteMeta;
use crate::vault::para::ParaCategory;
use chrono::{NaiveDate, NaiveDateTime, Utc};
use gray_matter::engine::{Engine, YAML};
use gray_matter::{Matter, Pod};
use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Serialize, Deserialize, Default)]
struct RawFrontmatter {
    #[serde(default, deserialize_with = "deserialize_scalar_text")]
    id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_scalar_text")]
    title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_tags")]
    tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_pinned")]
    pinned: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_scalar_text")]
    created: Option<String>,
    #[serde(default, deserialize_with = "deserialize_scalar_text")]
    modified: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    category: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    source_url: Option<String>,
}

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_yaml::Value::deserialize(deserializer)?;
    Ok(value.as_str().map(str::to_owned))
}

/// A string reads as itself; a boolean or a number reads as the text serde_json prints for it.
/// Anything else, a null, a list or a map, has no text.
fn scalar_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => Some(value.to_string()),
        _ => None,
    }
}

// Each field reads on its own: a value of the wrong type is missing, and the other fields
// still read. The value arrives as JSON from `pod_to_json`, where NaN and the infinities
// are already null.

fn deserialize_scalar_text<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(scalar_text(&serde_json::Value::deserialize(deserializer)?))
}

fn deserialize_tags<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Array(items) => Some(items.iter().filter_map(scalar_text).collect()),
        _ => None,
    })
}

fn deserialize_pinned<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(serde_json::Value::deserialize(deserializer)?.as_bool())
}

/// A note split into its front matter and its body.
///
/// `parse_frontmatter` is the one place that decides what a front matter that will not
/// parse means: the note keeps its body and its front matter text, and only `fields` goes
/// missing. Callers that edit the front matter parse `matter` with `serde_yaml` themselves.
pub(crate) struct ParsedFrontmatter {
    /// The text between the delimiters, trimmed. Empty when the note has no front matter or
    /// an empty one.
    pub(crate) matter: String,
    /// The lines after the closing delimiter, joined with `\n`: `\r\n` becomes `\n`, and the
    /// empty lines at the start and the final newline are dropped. A note without front
    /// matter is all body, and an unterminated front matter leaves every line after the
    /// opening delimiter. A file of three bytes or fewer has an empty body; gray_matter has
    /// always read it that way.
    pub(crate) body: String,
    /// The fields the app reads, or `None` when there is no front matter, it is not valid
    /// YAML, or it is not a map. Each field reads on its own: one with the wrong type is
    /// missing and the others still read.
    fields: Option<RawFrontmatter>,
}

/// Finds the front matter without reading it.
///
/// gray_matter 0.3 fails the whole parse when its engine or its deserialization fails, and
/// then returns neither the front matter text nor the body. Reading the YAML afterwards, in
/// `parse_frontmatter`, keeps a bad front matter from costing the note its body.
struct Delimiters;

impl Engine for Delimiters {
    fn parse(_: &str) -> gray_matter::Result<Pod> {
        Ok(Pod::Null)
    }
}

/// Split a note into its front matter text, its body and the fields the app reads.
pub(crate) fn parse_frontmatter(raw: &str) -> ParsedFrontmatter {
    let parsed = match Matter::<Delimiters>::new().parse::<IgnoredAny>(raw) {
        Ok(parsed) => parsed,
        // Cannot happen: `Delimiters` never fails and `IgnoredAny` accepts any value. Were it
        // to, the note reads as one without front matter, so all of its text stays the body.
        Err(_) => {
            return ParsedFrontmatter {
                matter: String::new(),
                body: raw.to_string(),
                fields: None,
            }
        }
    };
    let fields = if parsed.matter.is_empty() {
        None
    } else {
        YAML::parse(&parsed.matter)
            .ok()
            .map(pod_to_json)
            // Only a map has fields: serde would read a list as a struct, element by element.
            .filter(serde_json::Value::is_object)
            .and_then(|value| serde_json::from_value(value).ok())
    };
    ParsedFrontmatter {
        matter: parsed.matter,
        body: parsed.content,
        fields,
    }
}

/// Convert the YAML value the way gray_matter 0.2 did before deserializing it, so the fields
/// read as they did then. 0.3 deserializes with code of its own, which, for one, drops every
/// field when one holds a number too large for a float (`title: 1e999`), where 0.2 read that
/// field as null and kept the others.
fn pod_to_json(pod: Pod) -> serde_json::Value {
    use serde_json::Value;
    match pod {
        Pod::Null => Value::Null,
        Pod::String(text) => Value::String(text),
        Pod::Integer(number) => Value::from(number),
        // An infinity has no JSON number, so it becomes null, as it did in 0.2.
        Pod::Float(number) => Value::from(number),
        Pod::Boolean(flag) => Value::Bool(flag),
        Pod::Array(items) => Value::Array(items.into_iter().map(pod_to_json).collect()),
        Pod::Hash(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key, pod_to_json(value)))
                .collect(),
        ),
    }
}

/// Try to parse a date string in multiple common formats.
pub fn parse_date_flexible(s: &str) -> Option<chrono::DateTime<Utc>> {
    let s = s.trim();
    // RFC 3339 / ISO 8601 with timezone (e.g. 2024-01-15T10:30:00+00:00)
    if let Ok(dt) = s.parse::<chrono::DateTime<Utc>>() {
        return Some(dt);
    }
    // Try parsing as DateTime with fixed offset then convert
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // YYYY-MM-DD HH:MM:SS (no timezone)
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(ndt.and_utc());
    }
    // YYYY-MM-DDTHH:MM:SS (no timezone, with T separator)
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(ndt.and_utc());
    }
    // YYYY-MM-DD (date only)
    if let Ok(nd) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return nd.and_hms_opt(0, 0, 0).map(|ndt| ndt.and_utc());
    }
    None
}

pub fn parse_note(raw: &str, filename: &str) -> (NoteMeta, String) {
    let parsed = parse_frontmatter(raw);

    let fm: RawFrontmatter = parsed.fields.unwrap_or_default();

    let title = fm.title.unwrap_or_else(|| filename_to_title(filename));

    let created = fm
        .created
        .and_then(|s| parse_date_flexible(&s))
        .unwrap_or_else(Utc::now);

    let modified = fm
        .modified
        .and_then(|s| parse_date_flexible(&s))
        .unwrap_or_else(Utc::now);

    // Don't generate UUID on read - empty string signals "no ID yet"
    let id = fm.id.unwrap_or_default();

    // An unrecognised category value reads as uncategorised rather than erroring: a
    // hand-edited or foreign value should not make the note unopenable.
    let category = fm.category.as_deref().and_then(ParaCategory::from_name);

    let meta = NoteMeta {
        id,
        title,
        tags: fm.tags.unwrap_or_default(),
        pinned: fm.pinned.unwrap_or(false),
        created,
        modified,
        category,
        source_url: fm.source_url,
    };

    (meta, parsed.body)
}

pub fn serialize_frontmatter(meta: &NoteMeta) -> String {
    let tags_str = if meta.tags.is_empty() {
        "[]".to_string()
    } else {
        format!(
            "[{}]",
            meta.tags
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    // Uncategorised notes omit the key entirely rather than writing a null, so an
    // untouched pre-PARA vault keeps frontmatter it would have had before.
    let category_line = match meta.category {
        Some(c) => format!("category: {}\n", c.folder_name()),
        None => String::new(),
    };
    let source_url_line = match &meta.source_url {
        Some(url) => format!("source_url: \"{}\"\n", url.replace('"', "\\\"")),
        None => String::new(),
    };

    format!(
        "---\nid: \"{}\"\ntitle: \"{}\"\ntags: {}\npinned: {}\ncreated: {}\nmodified: {}\n{}{}---\n",
        meta.id,
        meta.title.replace('"', "\\\""),
        tags_str,
        meta.pinned,
        meta.created.to_rfc3339(),
        meta.modified.to_rfc3339(),
        category_line,
        source_url_line,
    )
}

pub fn update_note_raw(meta: &NoteMeta, body: &str) -> String {
    let fm = serialize_frontmatter(meta);
    format!("{}{}", fm, body)
}

/// Merge NoteMeta fields into existing frontmatter, preserving unknown YAML keys.
/// Falls back to serialize_frontmatter() if the original has no frontmatter.
pub fn merge_frontmatter(original_raw: &str, meta: &NoteMeta, body: &str) -> String {
    let parsed = parse_frontmatter(original_raw);

    // If original had no frontmatter, use the simple serializer
    if parsed.matter.trim().is_empty() {
        return update_note_raw(meta, body);
    }

    // Parse existing YAML into a Mapping to preserve all keys
    let mut mapping: serde_yaml::Mapping = match serde_yaml::from_str(&parsed.matter) {
        Ok(m) => m,
        Err(_) => return update_note_raw(meta, body), // unparseable YAML, fall back
    };

    // Update only our known fields
    if !meta.id.is_empty() {
        mapping.insert(
            serde_yaml::Value::String("id".into()),
            serde_yaml::Value::String(meta.id.clone()),
        );
    }
    mapping.insert(
        serde_yaml::Value::String("title".into()),
        serde_yaml::Value::String(meta.title.clone()),
    );
    mapping.insert(
        serde_yaml::Value::String("tags".into()),
        serde_yaml::Value::Sequence(
            meta.tags
                .iter()
                .map(|t| serde_yaml::Value::String(t.clone()))
                .collect(),
        ),
    );
    mapping.insert(
        serde_yaml::Value::String("pinned".into()),
        serde_yaml::Value::Bool(meta.pinned),
    );
    mapping.insert(
        serde_yaml::Value::String("created".into()),
        serde_yaml::Value::String(meta.created.to_rfc3339()),
    );
    mapping.insert(
        serde_yaml::Value::String("modified".into()),
        serde_yaml::Value::String(meta.modified.to_rfc3339()),
    );
    // Removing the key when uncategorised keeps a stale category from outliving a move
    // out of the PARA folders.
    match meta.category {
        Some(c) => {
            mapping.insert(
                serde_yaml::Value::String("category".into()),
                serde_yaml::Value::String(c.folder_name().to_string()),
            );
        }
        None => {
            mapping.remove(serde_yaml::Value::String("category".into()));
        }
    }
    match &meta.source_url {
        Some(url) => {
            mapping.insert(
                serde_yaml::Value::String("source_url".into()),
                serde_yaml::Value::String(url.clone()),
            );
        }
        None => {
            mapping.remove(serde_yaml::Value::String("source_url".into()));
        }
    }

    // Serialize the mapping back to YAML
    let yaml_str = match serde_yaml::to_string(&serde_yaml::Value::Mapping(mapping)) {
        Ok(s) => s,
        Err(_) => return update_note_raw(meta, body),
    };

    format!("---\n{}---\n{}", yaml_str, body)
}

/// Change only the category field while preserving every other YAML value.
///
/// Unlike `merge_frontmatter`, this is deliberately fallible: a move must not replace
/// malformed metadata with generated defaults and then delete the only original copy.
pub fn set_category(raw: &str, category: ParaCategory) -> Result<String, String> {
    let parsed = parse_frontmatter(raw);
    let mut mapping = if parsed.matter.trim().is_empty() {
        serde_yaml::Mapping::new()
    } else {
        serde_yaml::from_str::<serde_yaml::Mapping>(&parsed.matter).map_err(|error| {
            format!("Cannot update category because frontmatter is invalid: {error}")
        })?
    };
    mapping.insert(
        serde_yaml::Value::String("category".into()),
        serde_yaml::Value::String(category.folder_name().to_string()),
    );
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(mapping))
        .map_err(|error| format!("Could not serialize updated category: {error}"))?;
    Ok(format!("---\n{yaml}---\n{}", parsed.body))
}

pub fn filename_to_title(filename: &str) -> String {
    let stem = filename.trim_end_matches(".md");
    // Only replace dashes/underscores acting as word separators (between non-space chars).
    // Keep dashes that are surrounded by spaces (e.g. "Title - Subtitle").
    let mut result = String::with_capacity(stem.len());
    let chars: Vec<char> = stem.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if (ch == '-' || ch == '_')
            && i > 0
            && i < chars.len() - 1
            && chars[i - 1] != ' '
            && chars[i + 1] != ' '
        {
            result.push(' ');
        } else {
            result.push(ch);
        }
    }
    result
}

/// Quick title extraction from raw note content (frontmatter only, no full parse)
pub fn extract_title(raw: &str) -> Option<String> {
    parse_frontmatter(raw).fields?.title
}

pub fn extract_preview(content: &str, max_len: usize) -> String {
    // Filter out headings and empty lines BEFORE stripping markdown,
    // since strip_html_and_markdown collapses all whitespace into one line.
    let filtered: String = content
        .lines()
        .filter(|l| {
            let trimmed = l.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && !trimmed.starts_with("---")
                && !trimmed.starts_with("```")
        })
        .take(5)
        .collect::<Vec<_>>()
        .join("\n");

    let text = strip_html_and_markdown(&filtered);
    let trimmed = text.trim().to_string();

    if trimmed.len() > max_len {
        let mut end = max_len;
        while end > 0 && !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &trimmed[..end])
    } else {
        trimmed
    }
}

fn strip_html_and_markdown(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_tag = false;

    while let Some(ch) = chars.next() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
                // Add a space after closing tags to separate words
                result.push(' ');
            }
            continue;
        }

        if ch == '<' {
            in_tag = true;
            continue;
        }

        // Skip markdown image syntax: ![alt](url)
        if ch == '!' && chars.peek() == Some(&'[') {
            // Consume ![...](...) entirely
            chars.next(); // skip '['
            let mut depth = 1;
            // Skip alt text
            for c in chars.by_ref() {
                if c == '[' {
                    depth += 1;
                }
                if c == ']' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            // Skip (url) if present
            if chars.peek() == Some(&'(') {
                chars.next();
                let mut depth = 1;
                for c in chars.by_ref() {
                    if c == '(' {
                        depth += 1;
                    }
                    if c == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
            }
            continue;
        }

        // Markdown links: [text](url) - keep the text, skip the URL
        if ch == '[' {
            let mut link_text = String::new();
            let mut depth = 1;
            for c in chars.by_ref() {
                if c == '[' {
                    depth += 1;
                }
                if c == ']' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                link_text.push(c);
            }
            // Skip (url) if present
            if chars.peek() == Some(&'(') {
                chars.next();
                let mut depth = 1;
                for c in chars.by_ref() {
                    if c == '(' {
                        depth += 1;
                    }
                    if c == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
                result.push_str(&link_text);
            } else {
                // Not a link, just brackets
                result.push('[');
                result.push_str(&link_text);
                result.push(']');
            }
            continue;
        }

        // Strip bold/italic markers
        if ch == '*' || ch == '_' {
            // Skip consecutive * or _
            while chars.peek() == Some(&ch) {
                chars.next();
            }
            continue;
        }

        // Strip strikethrough ~~
        if ch == '~' && chars.peek() == Some(&'~') {
            chars.next();
            continue;
        }

        // Strip inline code backticks
        if ch == '`' {
            while chars.peek() == Some(&'`') {
                chars.next();
            }
            continue;
        }

        result.push(ch);
    }

    // Collapse multiple spaces/newlines into single spaces
    let mut collapsed = String::with_capacity(result.len());
    let mut last_was_space = false;
    for ch in result.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                collapsed.push(' ');
                last_was_space = true;
            }
        } else {
            collapsed.push(ch);
            last_was_space = false;
        }
    }

    collapsed
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn meta_with_category(category: Option<ParaCategory>) -> NoteMeta {
        NoteMeta {
            id: "note-id".to_string(),
            title: "A note".to_string(),
            tags: Vec::new(),
            pinned: false,
            created: Utc::now(),
            modified: Utc::now(),
            category,
            source_url: None,
        }
    }

    #[test]
    fn category_survives_a_write_then_read() {
        // The acceptance criterion "every note records its category durably" comes down
        // to this round-trip.
        for category in ParaCategory::ALL {
            let raw = update_note_raw(&meta_with_category(Some(category)), "body\n");
            let (parsed, _) = parse_note(&raw, "a-note.md");
            assert_eq!(parsed.category, Some(category));
        }
    }

    #[test]
    fn source_url_survives_a_write_then_read() {
        let mut meta = meta_with_category(Some(ParaCategory::Resources));
        meta.source_url = Some("https://example.com/field-notes?view=full".to_string());

        let raw = update_note_raw(&meta, "body\n");
        let (parsed, _) = parse_note(&raw, "field-notes.md");

        assert_eq!(parsed.source_url, meta.source_url);
    }

    #[test]
    fn an_uncategorised_note_writes_no_category_key() {
        let raw = update_note_raw(&meta_with_category(None), "body\n");
        assert!(!raw.contains("category"));
        let (parsed, _) = parse_note(&raw, "a-note.md");
        assert_eq!(parsed.category, None);
    }

    #[test]
    fn a_note_predating_para_reads_as_uncategorised() {
        let raw = "---\nid: \"x\"\ntitle: \"Old\"\n---\nbody\n";
        let (parsed, _) = parse_note(raw, "old.md");
        assert_eq!(parsed.category, None);
        assert_eq!(parsed.title, "Old");
    }

    #[test]
    fn an_unrecognised_category_reads_as_uncategorised_rather_than_failing() {
        // A hand-edited or foreign value must not make the note unopenable.
        let raw = "---\nid: \"x\"\ntitle: \"Odd\"\ncategory: Inbox\n---\nbody\n";
        let (parsed, _) = parse_note(raw, "odd.md");
        assert_eq!(parsed.category, None);
        assert_eq!(parsed.title, "Odd");
    }

    #[test]
    fn invalid_category_types_preserve_all_other_metadata() {
        for invalid_category in ["42", "true", "[Projects]", "{ name: Projects }", "null"] {
            let raw = format!(
                "---\n\
                 id: \"stable-id\"\n\
                 title: \"Stable title\"\n\
                 tags: [alpha, beta]\n\
                 pinned: true\n\
                 created: \"2024-01-02T03:04:05Z\"\n\
                 modified: \"2024-02-03T04:05:06Z\"\n\
                 category: {invalid_category}\n\
                 ---\n\
                 body stays readable\n"
            );

            let (parsed, body) = parse_note(&raw, "fallback-name.md");

            assert_eq!(
                parsed.id, "stable-id",
                "invalid category {invalid_category} erased the ID"
            );
            assert_eq!(
                parsed.title, "Stable title",
                "invalid category {invalid_category} erased the title"
            );
            assert_eq!(parsed.tags, ["alpha", "beta"]);
            assert!(parsed.pinned);
            assert_eq!(parsed.created.to_rfc3339(), "2024-01-02T03:04:05+00:00");
            assert_eq!(parsed.modified.to_rfc3339(), "2024-02-03T04:05:06+00:00");
            assert_eq!(parsed.category, None);
            assert_eq!(body.trim_end(), "body stays readable");
        }
    }

    #[test]
    fn merging_sets_the_category_and_keeps_unknown_keys() {
        let original = "---\nid: \"x\"\ntitle: \"Note\"\ncustom_field: keep-me\n---\nbody\n";
        let merged = merge_frontmatter(
            original,
            &meta_with_category(Some(ParaCategory::Resources)),
            "body\n",
        );

        assert!(merged.contains("custom_field"));
        assert!(merged.contains("keep-me"));
        let (parsed, _) = parse_note(&merged, "note.md");
        assert_eq!(parsed.category, Some(ParaCategory::Resources));
    }

    #[test]
    fn merging_drops_a_category_that_no_longer_applies() {
        // Moving a note out of the PARA folders must not leave a stale category behind.
        let original = "---\nid: \"x\"\ntitle: \"Note\"\ncategory: Projects\n---\nbody\n";
        let merged = merge_frontmatter(original, &meta_with_category(None), "body\n");

        let (parsed, _) = parse_note(&merged, "note.md");
        assert_eq!(parsed.category, None);
    }

    #[test]
    fn test_extract_preview_basic() {
        let content =
            "## SIM Kaarten\n\n### Artjom\n\n**Telefoonnummer**\n\n**== 0456 74 49 91 ==**\n";
        let preview = extract_preview(content, 120);
        eprintln!("Preview: {:?}", preview);
        assert!(!preview.is_empty(), "Preview should not be empty");
    }

    #[test]
    fn test_extract_preview_no_frontmatter() {
        let content = "# Android flash\n\nAndroid flash\n\n[TWRP](https://twrp.me/)\n\nNotes;\n\n- Samsung S6\n";
        let preview = extract_preview(content, 120);
        eprintln!("Preview: {:?}", preview);
        assert!(!preview.is_empty(), "Preview should not be empty");
    }

    #[test]
    fn test_strip_html_markdown() {
        let input = "**Telefoonnummer**\n\n**== 0456 ==**";
        let stripped = strip_html_and_markdown(input);
        eprintln!("Stripped: {:?}", stripped);
        assert!(stripped.contains("Telefoonnummer"));
    }

    /// One input of the table, and what the app returns for it.
    ///
    /// The expected values were recorded by running the app with gray_matter 0.2.9, in a
    /// scratch project outside the repository, before the move to 0.3. Each field now reads
    /// on its own, so where a wrongly typed field used to make every field read as missing,
    /// the row says what it reads now. Every input spells out its `\n` and `\r\n`, so a
    /// checkout's line-ending setting cannot change it.
    pub(crate) struct Case {
        pub(crate) name: &'static str,
        pub(crate) raw: String,
        /// What `parse_note` reads, given the file name `Table note.md`.
        pub(crate) note: Note,
        /// What `extract_title` returns.
        title: Option<String>,
        /// What `merge_frontmatter` writes for `merge_meta()` and the body `edited body\n`.
        merged: Merged,
        /// What `set_category` does when the note moves to Areas.
        moved: Moved,
    }

    enum Merged {
        /// Only the generated front matter, then the body: what an update writes when the
        /// original front matter is missing or cannot be read.
        Fresh,
        /// The original front matter edited in place: the YAML between the delimiters.
        Edited(String),
    }

    enum Moved {
        /// It refuses, because it cannot read the front matter.
        Refused,
        /// Front matter holding only the category, then the body `parse_note` reads.
        CategoryOnly,
        /// The original front matter with the category set, given as the YAML between the
        /// delimiters. The body `parse_note` reads comes after it.
        Edited(String),
    }

    pub(crate) struct Note {
        id: &'static str,
        title: String,
        tags: &'static [&'static str],
        pinned: bool,
        /// `None` where the date is missing or unreadable, so the read time stands in.
        created: Option<&'static str>,
        modified: Option<&'static str>,
        category: Option<ParaCategory>,
        source_url: Option<&'static str>,
        pub(crate) body: String,
    }

    impl Note {
        /// Every field reads as missing, so the title comes from the file name.
        fn unread(body: String) -> Note {
            Note {
                id: "",
                title: "Table note".into(),
                tags: &[],
                pinned: false,
                created: None,
                modified: None,
                category: None,
                source_url: None,
                body,
            }
        }
    }

    fn merge_meta() -> NoteMeta {
        NoteMeta {
            id: String::new(),
            title: "Merged title".to_string(),
            tags: Vec::new(),
            pinned: false,
            created: "2024-01-02T03:04:05Z".parse().unwrap(),
            modified: "2024-02-03T04:05:06Z".parse().unwrap(),
            category: Some(ParaCategory::Resources),
            source_url: None,
        }
    }

    pub(crate) fn cases() -> Vec<Case> {
        let long = "x".repeat(5000);
        vec![
            Case {
                name: "no_frontmatter",
                raw: "# Heading\n\nJust a body.\n".into(),
                note: Note::unread("# Heading\n\nJust a body.".into()),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::CategoryOnly,
            },
            Case {
                name: "empty_frontmatter",
                raw: "---\n---\nbody\n".into(),
                note: Note::unread("body".into()),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::CategoryOnly,
            },
            Case {
                name: "unterminated",
                raw: "---\ntitle: Open\nbody line\n".into(),
                note: Note::unread("title: Open\nbody line".into()),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::CategoryOnly,
            },
            Case {
                name: "malformed_yaml",
                raw: concat!(
                    "---\n",
                    "id: \"stable-id\"\n",
                    "title: [broken\n",
                    "category: Projects\n",
                    "---\n",
                    "valuable body\n"
                )
                .into(),
                note: Note::unread("valuable body".into()),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::Refused,
            },
            Case {
                name: "tab_indented",
                raw: "---\ntitle: Tabbed\nextra:\n\tchild: 1\n---\nbody\n".into(),
                note: Note::unread("body".into()),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::Refused,
            },
            Case {
                name: "scalar",
                raw: "---\njust a string\n---\nbody\n".into(),
                note: Note::unread("body".into()),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::Refused,
            },
            Case {
                name: "tags_number",
                raw: concat!(
                    "---\n",
                    "id: \"typed-id\"\n",
                    "title: \"Typed\"\n",
                    "tags: 5\n",
                    "category: Areas\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                // gray_matter 0.2.9: every field missing. Now only the tags are.
                note: Note {
                    id: "typed-id",
                    title: "Typed".into(),
                    category: Some(ParaCategory::Areas),
                    ..Note::unread("body".into())
                },
                title: Some("Typed".into()),
                merged: Merged::Edited(
                    concat!(
                        "id: typed-id\n",
                        "title: Merged title\n",
                        "tags: []\n",
                        "category: Resources\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited(
                    "id: typed-id\ntitle: Typed\ntags: 5\ncategory: Areas\n".into(),
                ),
            },
            Case {
                name: "title_number",
                raw: concat!(
                    "---\n",
                    "id: \"typed-id\"\n",
                    "title: 123\n",
                    "category: Areas\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                // gray_matter 0.2.9: every field missing. Now a number reads as its text.
                note: Note {
                    id: "typed-id",
                    title: "123".into(),
                    category: Some(ParaCategory::Areas),
                    ..Note::unread("body".into())
                },
                title: Some("123".into()),
                merged: Merged::Edited(
                    concat!(
                        "id: typed-id\n",
                        "title: Merged title\n",
                        "category: Resources\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited("id: typed-id\ntitle: 123\ncategory: Areas\n".into()),
            },
            Case {
                name: "pinned_yes",
                raw: concat!(
                    "---\n",
                    "id: \"typed-id\"\n",
                    "title: \"Typed\"\n",
                    "pinned: yes\n",
                    "category: Areas\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                // gray_matter 0.2.9: every field missing. Now only `pinned` reads as false.
                note: Note {
                    id: "typed-id",
                    title: "Typed".into(),
                    category: Some(ParaCategory::Areas),
                    ..Note::unread("body".into())
                },
                title: Some("Typed".into()),
                merged: Merged::Edited(
                    concat!(
                        "id: typed-id\n",
                        "title: Merged title\n",
                        "pinned: false\n",
                        "category: Resources\n",
                        "tags: []\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited(
                    "id: typed-id\ntitle: Typed\npinned: yes\ncategory: Areas\n".into(),
                ),
            },
            Case {
                name: "crlf",
                raw: concat!(
                    "---\r\n",
                    "id: \"crlf-id\"\r\n",
                    "title: \"Windows note\"\r\n",
                    "tags: [a, b]\r\n",
                    "alias: old\r\n",
                    "category: Resources\r\n",
                    "---\r\n",
                    "line one\r\n",
                    "line two\r\n"
                )
                .into(),
                note: Note {
                    id: "crlf-id",
                    title: "Windows note".into(),
                    tags: &["a", "b"],
                    category: Some(ParaCategory::Resources),
                    ..Note::unread("line one\nline two".into())
                },
                title: Some("Windows note".into()),
                merged: Merged::Edited(
                    concat!(
                        "id: crlf-id\n",
                        "title: Merged title\n",
                        "tags: []\n",
                        "alias: old\n",
                        "category: Resources\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited(
                    concat!(
                        "id: crlf-id\n",
                        "title: Windows note\n",
                        "tags:\n",
                        "- a\n",
                        "- b\n",
                        "alias: old\n",
                        "category: Areas\n"
                    )
                    .into(),
                ),
            },
            Case {
                name: "crlf_malformed",
                raw: concat!(
                    "---\r\n",
                    "alias: old\r\n",
                    "title: [broken\r\n",
                    "---\r\n",
                    "line one\r\n",
                    "line two\r\n"
                )
                .into(),
                note: Note::unread("line one\nline two".into()),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::Refused,
            },
            Case {
                name: "byte_order_mark",
                raw: concat!(
                    "\u{feff}---\n",
                    "title: \"Hidden\"\n",
                    "category: Projects\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                note: Note::unread(
                    concat!(
                        "\u{feff}---\n",
                        "title: \"Hidden\"\n",
                        "category: Projects\n",
                        "---\n",
                        "body"
                    )
                    .into(),
                ),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::CategoryOnly,
            },
            Case {
                name: "unquoted_dates",
                raw: concat!(
                    "---\n",
                    "title: \"Dated\"\n",
                    "created: 2024-01-02\n",
                    "modified: 2024-02-03T04:05:06Z\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                note: Note {
                    title: "Dated".into(),
                    created: Some("2024-01-02T00:00:00+00:00"),
                    modified: Some("2024-02-03T04:05:06+00:00"),
                    ..Note::unread("body".into())
                },
                title: Some("Dated".into()),
                merged: Merged::Edited(
                    concat!(
                        "title: Merged title\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "category: Resources\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited(
                    concat!(
                        "title: Dated\n",
                        "created: 2024-01-02\n",
                        "modified: 2024-02-03T04:05:06Z\n",
                        "category: Areas\n"
                    )
                    .into(),
                ),
            },
            Case {
                name: "quoted_dates",
                raw: concat!(
                    "---\n",
                    "title: \"Dated\"\n",
                    "created: \"2024-01-02 03:04:05\"\n",
                    "modified: '2024-02-03T04:05:06+02:00'\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                note: Note {
                    title: "Dated".into(),
                    created: Some("2024-01-02T03:04:05+00:00"),
                    modified: Some("2024-02-03T02:05:06+00:00"),
                    ..Note::unread("body".into())
                },
                title: Some("Dated".into()),
                merged: Merged::Edited(
                    concat!(
                        "title: Merged title\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "category: Resources\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited(
                    concat!(
                        "title: Dated\n",
                        "created: 2024-01-02 03:04:05\n",
                        "modified: 2024-02-03T04:05:06+02:00\n",
                        "category: Areas\n"
                    )
                    .into(),
                ),
            },
            Case {
                name: "tags_without_value",
                raw: "---\ntitle: \"Untagged\"\ntags:\n---\nbody\n".into(),
                note: Note {
                    title: "Untagged".into(),
                    ..Note::unread("body".into())
                },
                title: Some("Untagged".into()),
                merged: Merged::Edited(
                    concat!(
                        "title: Merged title\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n",
                        "category: Resources\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited("title: Untagged\ntags: null\ncategory: Areas\n".into()),
            },
            Case {
                name: "anchor_and_alias",
                raw: concat!(
                    "---\n",
                    "base: &b Projects\n",
                    "category: *b\n",
                    "title: &t \"Anchored\"\n",
                    "aliases: [*t]\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                note: Note {
                    title: "Anchored".into(),
                    category: Some(ParaCategory::Projects),
                    ..Note::unread("body".into())
                },
                title: Some("Anchored".into()),
                merged: Merged::Edited(
                    concat!(
                        "base: Projects\n",
                        "category: Resources\n",
                        "title: Merged title\n",
                        "aliases:\n",
                        "- Anchored\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited(
                    concat!(
                        "base: Projects\n",
                        "category: Areas\n",
                        "title: Anchored\n",
                        "aliases:\n",
                        "- Anchored\n"
                    )
                    .into(),
                ),
            },
            Case {
                name: "second_delimiter_in_body",
                raw: "---\ntitle: \"Ruled\"\n---\nabove\n---\nbelow\n".into(),
                note: Note {
                    title: "Ruled".into(),
                    ..Note::unread("above\n---\nbelow".into())
                },
                title: Some("Ruled".into()),
                merged: Merged::Edited(
                    concat!(
                        "title: Merged title\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n",
                        "category: Resources\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited("title: Ruled\ncategory: Areas\n".into()),
            },
            Case {
                name: "duplicate_keys",
                raw: concat!(
                    "---\n",
                    "title: \"First\"\n",
                    "title: \"Second\"\n",
                    "category: Projects\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                note: Note::unread("body".into()),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::Refused,
            },
            Case {
                name: "title_with_colon",
                raw: "---\ntitle: a: b\ncategory: Projects\n---\nbody\n".into(),
                note: Note::unread("body".into()),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::Refused,
            },
            Case {
                name: "spaces_after_delimiter",
                raw: "---   \ntitle: \"Spaced\"\n---  \nbody\n".into(),
                note: Note {
                    title: "Spaced".into(),
                    ..Note::unread("body".into())
                },
                title: Some("Spaced".into()),
                merged: Merged::Edited(
                    concat!(
                        "title: Merged title\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n",
                        "category: Resources\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited("title: Spaced\ncategory: Areas\n".into()),
            },
            Case {
                name: "indented_delimiter",
                raw: " ---\ntitle: \"Indented\"\n ---\nbody\n".into(),
                note: Note::unread(" ---\ntitle: \"Indented\"\n ---\nbody".into()),
                title: None,
                merged: Merged::Fresh,
                moved: Moved::CategoryOnly,
            },
            Case {
                name: "long_title",
                raw: format!("---\ntitle: \"{long}\"\ncategory: Archives\n---\nbody\n"),
                note: Note {
                    title: long.clone(),
                    category: Some(ParaCategory::Archives),
                    ..Note::unread("body".into())
                },
                title: Some(long.clone()),
                merged: Merged::Edited(
                    concat!(
                        "title: Merged title\n",
                        "category: Resources\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited(format!("title: {long}\ncategory: Areas\n")),
            },
            Case {
                name: "unicode_values",
                raw: concat!(
                    "---\n",
                    "title: \"Café ☕ – 日本語\"\n",
                    "tags: [naïve, 東京]\n",
                    "category: Areas\n",
                    "place: Zürich\n",
                    "---\n",
                    "Über body ✓\n"
                )
                .into(),
                note: Note {
                    title: "Café ☕ – 日本語".into(),
                    tags: &["naïve", "東京"],
                    category: Some(ParaCategory::Areas),
                    ..Note::unread("Über body ✓".into())
                },
                title: Some("Café ☕ – 日本語".into()),
                merged: Merged::Edited(
                    concat!(
                        "title: Merged title\n",
                        "tags: []\n",
                        "category: Resources\n",
                        "place: Zürich\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited(
                    concat!(
                        "title: Café ☕ – 日本語\n",
                        "tags:\n",
                        "- naïve\n",
                        "- 東京\n",
                        "category: Areas\n",
                        "place: Zürich\n"
                    )
                    .into(),
                ),
            },
            Case {
                name: "nested_unknown_keys",
                raw: concat!(
                    "---\n",
                    "id: \"nested-id\"\n",
                    "title: \"Nested\"\n",
                    "meta:\n",
                    "  owner: someone\n",
                    "  levels:\n",
                    "    deep: true\n",
                    "list_key:\n",
                    "  - one\n",
                    "  - two: 2\n",
                    "category: Projects\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                note: Note {
                    id: "nested-id",
                    title: "Nested".into(),
                    category: Some(ParaCategory::Projects),
                    ..Note::unread("body".into())
                },
                title: Some("Nested".into()),
                merged: Merged::Edited(
                    concat!(
                        "id: nested-id\n",
                        "title: Merged title\n",
                        "meta:\n",
                        "  owner: someone\n",
                        "  levels:\n",
                        "    deep: true\n",
                        "list_key:\n",
                        "- one\n",
                        "- two: 2\n",
                        "category: Resources\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited(
                    concat!(
                        "id: nested-id\n",
                        "title: Nested\n",
                        "meta:\n",
                        "  owner: someone\n",
                        "  levels:\n",
                        "    deep: true\n",
                        "list_key:\n",
                        "- one\n",
                        "- two: 2\n",
                        "category: Areas\n"
                    )
                    .into(),
                ),
            },
            Case {
                name: "unknown_keys_and_source_url",
                raw: concat!(
                    "---\n",
                    "id: \"clip-id\"\n",
                    "title: \"Clip\"\n",
                    "tags: [web]\n",
                    "pinned: true\n",
                    "created: \"2024-01-02T03:04:05Z\"\n",
                    "modified: \"2024-02-03T04:05:06Z\"\n",
                    "category: Resources\n",
                    "source_url: \"https://example.com/a?b=c\"\n",
                    "obsidian-only: keep-me\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                note: Note {
                    id: "clip-id",
                    title: "Clip".into(),
                    tags: &["web"],
                    pinned: true,
                    created: Some("2024-01-02T03:04:05+00:00"),
                    modified: Some("2024-02-03T04:05:06+00:00"),
                    category: Some(ParaCategory::Resources),
                    source_url: Some("https://example.com/a?b=c"),
                    ..Note::unread("body".into())
                },
                title: Some("Clip".into()),
                merged: Merged::Edited(
                    concat!(
                        "id: clip-id\n",
                        "title: Merged title\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n",
                        "category: Resources\n",
                        "obsidian-only: keep-me\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited(
                    concat!(
                        "id: clip-id\n",
                        "title: Clip\n",
                        "tags:\n",
                        "- web\n",
                        "pinned: true\n",
                        "created: 2024-01-02T03:04:05Z\n",
                        "modified: 2024-02-03T04:05:06Z\n",
                        "category: Areas\n",
                        "source_url: https://example.com/a?b=c\n",
                        "obsidian-only: keep-me\n"
                    )
                    .into(),
                ),
            },
            Case {
                name: "overflowing_float",
                raw: concat!(
                    "---\n",
                    "id: \"float-id\"\n",
                    "title: 1e999\n",
                    "category: Projects\n",
                    "---\n",
                    "body\n"
                )
                .into(),
                note: Note {
                    id: "float-id",
                    category: Some(ParaCategory::Projects),
                    ..Note::unread("body".into())
                },
                // gray_matter 0.2.9: `extract_title` gave "1e999". An infinity now reads as
                // missing there, as it does in `parse_note`.
                title: None,
                merged: Merged::Edited(
                    concat!(
                        "id: float-id\n",
                        "title: Merged title\n",
                        "category: Resources\n",
                        "tags: []\n",
                        "pinned: false\n",
                        "created: 2024-01-02T03:04:05+00:00\n",
                        "modified: 2024-02-03T04:05:06+00:00\n"
                    )
                    .into(),
                ),
                moved: Moved::Edited("id: float-id\ntitle: 1e999\ncategory: Areas\n".into()),
            },
        ]
    }

    /// A date given in the note reads as that date; a missing or unreadable one reads as
    /// the time of the read.
    pub(crate) fn assert_date(
        actual: chrono::DateTime<Utc>,
        expected: Option<&str>,
        read_between: (chrono::DateTime<Utc>, chrono::DateTime<Utc>),
        what: &str,
    ) {
        match expected {
            Some(date) => assert_eq!(actual.to_rfc3339(), date, "{what}"),
            None => assert!(
                read_between.0 <= actual && actual <= read_between.1,
                "{what}: expected the read time, got {actual}"
            ),
        }
    }

    #[test]
    fn each_table_input_reads_as_expected() {
        for case in cases() {
            let name = case.name;
            let expected = &case.note;
            let before = Utc::now();
            let (meta, body) = parse_note(&case.raw, "Table note.md");
            let read_between = (before, Utc::now());

            assert_eq!(meta.id, expected.id, "{name}: id");
            assert_eq!(meta.title, expected.title, "{name}: title");
            assert_eq!(meta.tags, expected.tags, "{name}: tags");
            assert_eq!(meta.pinned, expected.pinned, "{name}: pinned");
            assert_date(
                meta.created,
                expected.created,
                read_between,
                &format!("{name}: created"),
            );
            assert_date(
                meta.modified,
                expected.modified,
                read_between,
                &format!("{name}: modified"),
            );
            assert_eq!(meta.category, expected.category, "{name}: category");
            assert_eq!(
                meta.source_url.as_deref(),
                expected.source_url,
                "{name}: source_url"
            );
            assert_eq!(body, expected.body, "{name}: body");
        }
    }

    #[test]
    fn each_table_input_gives_the_expected_title() {
        for case in cases() {
            assert_eq!(extract_title(&case.raw), case.title, "{}", case.name);
        }
    }

    #[test]
    fn each_table_input_merges_as_expected() {
        for case in cases() {
            let expected = match &case.merged {
                Merged::Fresh => update_note_raw(&merge_meta(), "edited body\n"),
                Merged::Edited(yaml) => format!("---\n{yaml}---\nedited body\n"),
            };
            assert_eq!(
                merge_frontmatter(&case.raw, &merge_meta(), "edited body\n"),
                expected,
                "{}",
                case.name
            );
        }
    }

    /// A move must not replace front matter it cannot read, or it would delete the only
    /// copy of it. Front matter it can read keeps every other key and value.
    #[test]
    fn set_category_refuses_front_matter_it_cannot_read_and_keeps_every_other_key() {
        for case in cases() {
            let name = case.name;
            let result = set_category(&case.raw, ParaCategory::Areas);
            let expected = match &case.moved {
                Moved::CategoryOnly => format!("---\ncategory: Areas\n---\n{}", case.note.body),
                Moved::Edited(yaml) => format!("---\n{yaml}---\n{}", case.note.body),
                Moved::Refused => {
                    let error = result.expect_err(name);
                    assert!(
                        error.starts_with("Cannot update category because frontmatter is invalid"),
                        "{name}: {error}"
                    );
                    continue;
                }
            };
            assert_eq!(result.as_ref(), Ok(&expected), "{name}");
            let (_, body) = parse_note(&expected, "Table note.md");
            assert_eq!(
                body, case.note.body,
                "{name}: the moved note keeps its body"
            );
        }
    }
}
