use crate::types::NoteMeta;
use chrono::Utc;
use gray_matter::engine::YAML;
use gray_matter::Matter;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Default)]
struct RawFrontmatter {
    id: Option<String>,
    title: Option<String>,
    tags: Option<Vec<String>>,
    pinned: Option<bool>,
    created: Option<String>,
    modified: Option<String>,
}

pub fn parse_note(raw: &str, filename: &str) -> (NoteMeta, String) {
    let matter = Matter::<YAML>::new();
    let result = matter.parse(raw);

    let fm: RawFrontmatter = result
        .data
        .and_then(|d| d.deserialize().ok())
        .unwrap_or_default();

    let title = fm.title.unwrap_or_else(|| filename_to_title(filename));

    let created = fm
        .created
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(Utc::now);

    let modified = fm
        .modified
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(Utc::now);

    let id = fm.id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let meta = NoteMeta {
        id,
        title,
        tags: fm.tags.unwrap_or_default(),
        pinned: fm.pinned.unwrap_or(false),
        created,
        modified,
    };

    let content = result.content;
    (meta, content)
}

pub fn serialize_frontmatter(meta: &NoteMeta) -> String {
    let tags_str = if meta.tags.is_empty() {
        "[]".to_string()
    } else {
        format!(
            "[{}]",
            meta.tags
                .iter()
                .map(|t| format!("{}", t))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    format!(
        "---\nid: \"{}\"\ntitle: \"{}\"\ntags: {}\npinned: {}\ncreated: {}\nmodified: {}\n---\n",
        meta.id,
        meta.title.replace('"', "\\\""),
        tags_str,
        meta.pinned,
        meta.created.to_rfc3339(),
        meta.modified.to_rfc3339(),
    )
}

pub fn update_note_raw(meta: &NoteMeta, body: &str) -> String {
    let fm = serialize_frontmatter(meta);
    format!("{}{}", fm, body)
}

fn filename_to_title(filename: &str) -> String {
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
    let matter = Matter::<YAML>::new();
    let result = matter.parse(raw);
    let fm: RawFrontmatter = result
        .data
        .and_then(|d| d.deserialize().ok())
        .unwrap_or_default();
    fm.title
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
            while let Some(c) = chars.next() {
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
                while let Some(c) = chars.next() {
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

        // Markdown links: [text](url) — keep the text, skip the URL
        if ch == '[' {
            let mut link_text = String::new();
            let mut depth = 1;
            while let Some(c) = chars.next() {
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
                while let Some(c) = chars.next() {
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
mod tests {
    use super::*;

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
}
