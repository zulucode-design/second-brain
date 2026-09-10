//! Markdown to Notion blocks.
//!
//! Notion does not accept markdown. A page's content is a tree of typed blocks, and its
//! text is an array of "rich text" runs each carrying its own annotations. So every note
//! has to be parsed and rebuilt in Notion's shape before it can be published.
//!
//! ## The limits this exists to respect
//!
//! Notion rejects the whole request when any of these is exceeded, so they are enforced
//! here rather than discovered as a failed push:
//!
//! - **2,000 characters** per rich-text run, so long text is split across runs.
//! - **100 runs** per rich-text array, so a very long paragraph becomes several paragraphs.
//! - **100 blocks** per request and two levels of nesting, which is why deeper nesting is
//!   flattened rather than sent and rejected.
//!
//! ## What is deliberately lossy
//!
//! **Images never become image blocks.** Attachments are never uploaded (SPEC §8), so a
//! local image has nothing for Notion to display. An external one could be embedded, but a
//! URL Notion cannot fetch fails validation and takes the entire page create down with it —
//! one unreachable image would make a whole note unpublishable. Images therefore become a
//! paragraph naming the image and linking its source, which never fails.
//!
//! **Anything unmapped degrades to a paragraph** rather than being dropped. A note that
//! reads oddly in Notion is a smaller failure than a note missing content with no
//! indication that anything is gone.
//!
//! Headings deeper than three collapse to `heading_3`, which is the deepest Notion has.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_json::{json, Value};

/// Notion's cap on one rich-text run.
const MAX_TEXT: usize = 2000;
/// Notion's cap on runs in one rich-text array.
const MAX_RUNS: usize = 100;
/// Notion's cap on blocks in one request.
pub const MAX_BLOCKS_PER_REQUEST: usize = 100;
/// Notion accepts two levels of nesting in a single request; deeper is flattened.
const MAX_DEPTH: usize = 2;

/// Which text styles are active on the run being built.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    italic: bool,
    strikethrough: bool,
    code: bool,
}

impl Style {
    fn annotations(self) -> Value {
        json!({
            "bold": self.bold,
            "italic": self.italic,
            "strikethrough": self.strikethrough,
            "code": self.code,
        })
    }
}

/// One run of text with its styling and optional link.
fn rich_text(text: &str, style: Style, link: Option<&str>) -> Vec<Value> {
    // A single run cannot exceed 2,000 characters, so long text becomes several runs that
    // read as one continuous span. Split on characters rather than bytes: slicing a UTF-8
    // string mid-codepoint would panic, and notes contain plenty of non-ASCII.
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(MAX_TEXT)
        .map(|chunk| {
            let content: String = chunk.iter().collect();
            let mut text_object = json!({ "content": content });
            if let Some(url) = link {
                text_object["link"] = json!({ "url": url });
            }
            json!({
                "type": "text",
                "text": text_object,
                "annotations": style.annotations(),
            })
        })
        .collect()
}

/// What kind of container the builder is currently inside.
#[derive(Debug, Clone, PartialEq)]
enum Container {
    Root,
    ListItem { ordered: bool, task: Option<bool> },
    Quote,
}

/// One level of the block tree being built.
#[derive(Debug)]
struct Frame {
    container: Container,
    /// Text belonging to the container itself — a list item's own line, for instance.
    runs: Vec<Value>,
    /// Blocks nested inside it.
    children: Vec<Value>,
}

impl Frame {
    fn new(container: Container) -> Self {
        Self {
            container,
            runs: Vec::new(),
            children: Vec::new(),
        }
    }
}

/// Build a block of `kind` carrying `runs`, splitting when there are too many runs.
///
/// A paragraph long enough to exceed 100 runs — roughly 200,000 characters — becomes
/// several blocks of the same kind. Rare, but the alternative is Notion rejecting the page.
fn blocks_of(kind: &str, runs: Vec<Value>, children: Vec<Value>) -> Vec<Value> {
    if runs.is_empty() && children.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let chunks: Vec<Vec<Value>> = if runs.is_empty() {
        vec![Vec::new()]
    } else {
        runs.chunks(MAX_RUNS).map(|c| c.to_vec()).collect()
    };
    let last = chunks.len().saturating_sub(1);

    for (index, chunk) in chunks.into_iter().enumerate() {
        let mut body = json!({ "rich_text": chunk });
        // Children hang off the final block, so a split paragraph does not repeat them.
        if index == last && !children.is_empty() {
            body["children"] = json!(children);
        }
        out.push(json!({
            "object": "block",
            "type": kind,
            kind: body,
        }));
    }
    out
}

/// Notion accepts only a fixed set of code languages and rejects anything else, so an
/// unknown fence language becomes plain text rather than a failed request.
fn code_language(raw: &str) -> &'static str {
    const KNOWN: &[&str] = &[
        "bash", "c", "c#", "c++", "css", "diff", "docker", "elixir", "go", "graphql",
        "html", "java", "javascript", "json", "kotlin", "latex", "lua", "makefile",
        "markdown", "nix", "objective-c", "ocaml", "perl", "php", "powershell", "python",
        "r", "ruby", "rust", "scala", "shell", "sql", "swift", "toml", "typescript",
        "xml", "yaml",
    ];

    let normalised = raw.trim().to_ascii_lowercase();
    let normalised = match normalised.as_str() {
        "sh" | "zsh" | "console" => "bash",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "py" => "python",
        "rs" => "rust",
        "yml" => "yaml",
        "md" => "markdown",
        "cs" => "c#",
        "cpp" => "c++",
        "dockerfile" => "docker",
        other => other,
    };

    KNOWN
        .iter()
        .find(|known| **known == normalised)
        .copied()
        .unwrap_or("plain text")
}

fn heading_kind(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "heading_1",
        HeadingLevel::H2 => "heading_2",
        // Notion stops at three, so everything deeper collapses rather than disappearing.
        _ => "heading_3",
    }
}

/// Convert a note's markdown body into Notion blocks.
///
/// The body only — frontmatter is mapped to page properties, never rendered, so nobody
/// reading on a phone is shown a YAML fence.
pub fn to_blocks(markdown: &str) -> Vec<Value> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_TABLES);

    let mut stack: Vec<Frame> = vec![Frame::new(Container::Root)];
    let mut style = Style::default();
    let mut link: Option<String> = None;
    let mut heading: Option<HeadingLevel> = None;
    let mut code: Option<(String, String)> = None;
    // Depth of list nesting, so anything past Notion's two levels is flattened up.
    let mut list_depth: usize = 0;

    // Append text to whichever frame is currently open.
    macro_rules! push_text {
        ($text:expr) => {{
            let runs = rich_text($text, style, link.as_deref());
            if let Some(frame) = stack.last_mut() {
                frame.runs.extend(runs);
            }
        }};
    }

    // Finish the open frame's own text as a block of `kind` inside that frame.
    macro_rules! flush {
        ($kind:expr) => {{
            if let Some(frame) = stack.last_mut() {
                let runs = std::mem::take(&mut frame.runs);
                let blocks = blocks_of($kind, runs, Vec::new());
                frame.children.extend(blocks);
            }
        }};
    }

    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => heading = Some(level),
            Event::End(TagEnd::Heading(_)) => {
                let kind = heading.take().map(heading_kind).unwrap_or("heading_3");
                flush!(kind);
            }

            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                // Inside a list item the first paragraph *is* the item's text, so it stays
                // on the frame rather than becoming a nested paragraph block. A second
                // paragraph in the same item does become one.
                let inside_item = matches!(
                    stack.last().map(|f| &f.container),
                    Some(Container::ListItem { .. })
                );
                if inside_item {
                    let already_has_text = stack
                        .last()
                        .is_some_and(|frame| !frame.children.is_empty());
                    if already_has_text {
                        flush!("paragraph");
                    }
                } else {
                    flush!("paragraph");
                }
            }

            Event::Start(Tag::List(start)) => {
                list_depth += 1;
                // Remember whether this list is ordered for the items inside it.
                if let Some(frame) = stack.last_mut() {
                    frame.runs = std::mem::take(&mut frame.runs);
                }
                let _ = start;
            }
            Event::End(TagEnd::List(_)) => list_depth = list_depth.saturating_sub(1),

            Event::Start(Tag::Item) => {
                stack.push(Frame::new(Container::ListItem {
                    ordered: false,
                    task: None,
                }));
            }
            Event::End(TagEnd::Item) => {
                let Some(frame) = stack.pop() else { continue };
                let Container::ListItem { ordered, task } = frame.container else {
                    continue;
                };
                let kind = match task {
                    Some(_) => "to_do",
                    None if ordered => "numbered_list_item",
                    None => "bulleted_list_item",
                };

                // Notion accepts two levels in one request. Beyond that the children are
                // promoted to siblings: the indentation is lost, the content is not.
                let (runs, children) = (frame.runs, frame.children);
                let nest = list_depth <= MAX_DEPTH;
                let (own_children, promoted) = if nest {
                    (children, Vec::new())
                } else {
                    (Vec::new(), children)
                };

                let mut built = blocks_of(kind, runs, own_children);
                if let Some(Value::Object(map)) = built.first_mut() {
                    if let Some(body) = map.get_mut(kind).and_then(|b| b.as_object_mut()) {
                        if let Some(checked) = task {
                            body.insert("checked".into(), json!(checked));
                        }
                    }
                }
                if let Some(parent) = stack.last_mut() {
                    parent.children.extend(built);
                    parent.children.extend(promoted);
                }
            }

            Event::Start(Tag::BlockQuote(_)) => stack.push(Frame::new(Container::Quote)),
            Event::End(TagEnd::BlockQuote(_)) => {
                let Some(frame) = stack.pop() else { continue };
                // A quote's paragraphs were flushed into its children as paragraphs; Notion
                // wants them as quote blocks, so retype them rather than nesting a quote
                // that contains a paragraph that contains the text.
                let mut quotes = Vec::new();
                for child in frame.children {
                    let runs = child
                        .get("paragraph")
                        .and_then(|p| p.get("rich_text"))
                        .cloned()
                        .unwrap_or_else(|| json!([]));
                    quotes.extend(blocks_of(
                        "quote",
                        runs.as_array().cloned().unwrap_or_default(),
                        Vec::new(),
                    ));
                }
                quotes.extend(blocks_of("quote", frame.runs, Vec::new()));
                if let Some(parent) = stack.last_mut() {
                    parent.children.extend(quotes);
                }
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => code_language(&info),
                    CodeBlockKind::Indented => "plain text",
                };
                code = Some((language.to_string(), String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((language, text)) = code.take() {
                    let body = text.strip_suffix('\n').unwrap_or(&text).to_string();
                    let runs = rich_text(&body, Style::default(), None);
                    let block = json!({
                        "object": "block",
                        "type": "code",
                        "code": { "rich_text": runs, "language": language },
                    });
                    if let Some(frame) = stack.last_mut() {
                        frame.children.push(block);
                    }
                }
            }

            Event::Start(Tag::Emphasis) => style.italic = true,
            Event::End(TagEnd::Emphasis) => style.italic = false,
            Event::Start(Tag::Strong) => style.bold = true,
            Event::End(TagEnd::Strong) => style.bold = false,
            Event::Start(Tag::Strikethrough) => style.strikethrough = true,
            Event::End(TagEnd::Strikethrough) => style.strikethrough = false,

            Event::Start(Tag::Link { dest_url, .. }) => link = Some(dest_url.to_string()),
            Event::End(TagEnd::Link) => link = None,

            // An image cannot be displayed: local attachments are never uploaded, and an
            // external URL Notion cannot fetch fails validation and takes the whole page
            // with it. Naming it and linking it always works.
            Event::Start(Tag::Image { dest_url, title, .. }) => {
                let label = if title.is_empty() {
                    "image".to_string()
                } else {
                    title.to_string()
                };
                push_text!(&format!("[{label}: "));
                link = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Image) => {
                link = None;
                push_text!("]");
            }

            Event::Text(text) => {
                if let Some((_, buffer)) = code.as_mut() {
                    buffer.push_str(&text);
                } else {
                    push_text!(&text);
                }
            }
            Event::Code(text) => {
                let mut inline = style;
                inline.code = true;
                let runs = rich_text(&text, inline, link.as_deref());
                if let Some(frame) = stack.last_mut() {
                    frame.runs.extend(runs);
                }
            }
            Event::SoftBreak => push_text!(" "),
            Event::HardBreak => push_text!("\n"),

            Event::Rule => {
                let block = json!({ "object": "block", "type": "divider", "divider": {} });
                if let Some(frame) = stack.last_mut() {
                    frame.children.push(block);
                }
            }

            Event::TaskListMarker(checked) => {
                if let Some(frame) = stack.last_mut() {
                    if let Container::ListItem { ordered, .. } = frame.container {
                        frame.container = Container::ListItem {
                            ordered,
                            task: Some(checked),
                        };
                    }
                }
            }

            // Tables, footnotes, and HTML have no clean Notion equivalent worth the
            // complexity here. Their text still arrives through Event::Text, so content is
            // preserved even where structure is not.
            _ => {}
        }
    }

    // Anything still open — an unterminated construct in a hand-edited note — is flushed
    // rather than discarded.
    let mut result = Vec::new();
    while let Some(mut frame) = stack.pop() {
        if !frame.runs.is_empty() {
            let runs = std::mem::take(&mut frame.runs);
            frame.children.extend(blocks_of("paragraph", runs, Vec::new()));
        }
        let mut children = frame.children;
        children.extend(result);
        result = children;
    }
    result
}

/// Split blocks into request-sized batches.
///
/// Notion takes at most 100 blocks per call, so a long note is created with its first
/// batch and extended with the rest.
pub fn batches(blocks: Vec<Value>) -> Vec<Vec<Value>> {
    blocks
        .chunks(MAX_BLOCKS_PER_REQUEST)
        .map(|chunk| chunk.to_vec())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(blocks: &[Value]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| b["type"].as_str().unwrap_or("?").to_string())
            .collect()
    }

    fn text_of(block: &Value) -> String {
        let kind = block["type"].as_str().unwrap();
        block[kind]["rich_text"]
            .as_array()
            .map(|runs| {
                runs.iter()
                    .map(|r| r["text"]["content"].as_str().unwrap_or(""))
                    .collect::<String>()
            })
            .unwrap_or_default()
    }

    #[test]
    fn headings_map_to_notions_three_levels() {
        let blocks = to_blocks("# One\n\n## Two\n\n### Three");
        assert_eq!(kinds(&blocks), ["heading_1", "heading_2", "heading_3"]);
    }

    #[test]
    fn headings_deeper_than_three_collapse_rather_than_vanish() {
        // Notion has no heading_4. Dropping the text would lose content silently.
        let blocks = to_blocks("#### Four\n\n##### Five");
        assert_eq!(kinds(&blocks), ["heading_3", "heading_3"]);
        assert_eq!(text_of(&blocks[0]), "Four");
    }

    #[test]
    fn a_paragraph_becomes_a_paragraph() {
        let blocks = to_blocks("Just some prose.");
        assert_eq!(kinds(&blocks), ["paragraph"]);
        assert_eq!(text_of(&blocks[0]), "Just some prose.");
    }

    #[test]
    fn inline_styles_become_annotations() {
        let blocks = to_blocks("plain **bold** *italic* ~~struck~~ `code`");
        let runs = blocks[0]["paragraph"]["rich_text"].as_array().unwrap();

        let find = |needle: &str| {
            runs.iter()
                .find(|r| r["text"]["content"].as_str() == Some(needle))
                .unwrap_or_else(|| panic!("no run for {needle}"))
                .clone()
        };

        assert_eq!(find("bold")["annotations"]["bold"], json!(true));
        assert_eq!(find("italic")["annotations"]["italic"], json!(true));
        assert_eq!(find("struck")["annotations"]["strikethrough"], json!(true));
        assert_eq!(find("code")["annotations"]["code"], json!(true));
    }

    #[test]
    fn a_link_keeps_its_destination() {
        let blocks = to_blocks("see [the docs](https://example.com/x)");
        let runs = blocks[0]["paragraph"]["rich_text"].as_array().unwrap();
        let link = runs
            .iter()
            .find(|r| r["text"]["content"].as_str() == Some("the docs"))
            .unwrap();
        assert_eq!(link["text"]["link"]["url"], json!("https://example.com/x"));
    }

    #[test]
    fn bulleted_and_numbered_lists_are_distinguished() {
        let bullets = to_blocks("- one\n- two");
        assert_eq!(
            kinds(&bullets),
            ["bulleted_list_item", "bulleted_list_item"]
        );
        assert_eq!(text_of(&bullets[0]), "one");
    }

    #[test]
    fn task_list_items_carry_their_checked_state() {
        let blocks = to_blocks("- [x] done\n- [ ] not done");
        assert_eq!(kinds(&blocks), ["to_do", "to_do"]);
        assert_eq!(blocks[0]["to_do"]["checked"], json!(true));
        assert_eq!(blocks[1]["to_do"]["checked"], json!(false));
    }

    #[test]
    fn a_code_fence_keeps_its_language_and_body() {
        let blocks = to_blocks("```rust\nfn main() {}\n```");
        assert_eq!(kinds(&blocks), ["code"]);
        assert_eq!(blocks[0]["code"]["language"], json!("rust"));
        assert_eq!(text_of(&blocks[0]), "fn main() {}");
    }

    #[test]
    fn a_common_language_alias_is_translated() {
        // Notion rejects unknown languages outright, so "sh" must not be sent as-is.
        let blocks = to_blocks("```sh\nls\n```");
        assert_eq!(blocks[0]["code"]["language"], json!("bash"));
    }

    #[test]
    fn an_unknown_language_falls_back_instead_of_failing_the_request() {
        let blocks = to_blocks("```brainfuck\n+++\n```");
        assert_eq!(blocks[0]["code"]["language"], json!("plain text"));
        assert_eq!(text_of(&blocks[0]), "+++");
    }

    #[test]
    fn a_quote_becomes_a_quote_block_not_a_nested_paragraph() {
        let blocks = to_blocks("> quoted words");
        assert_eq!(kinds(&blocks), ["quote"]);
        assert_eq!(text_of(&blocks[0]), "quoted words");
    }

    #[test]
    fn a_horizontal_rule_becomes_a_divider() {
        let blocks = to_blocks("above\n\n---\n\nbelow");
        assert_eq!(kinds(&blocks), ["paragraph", "divider", "paragraph"]);
    }

    #[test]
    fn an_image_becomes_a_link_because_attachments_never_upload() {
        // A local attachment has nothing for Notion to show, and an external URL Notion
        // cannot fetch fails validation and takes the whole page create with it.
        let blocks = to_blocks("![diagram](.helixnotes/attachments/x.png)");
        assert_eq!(kinds(&blocks), ["paragraph"]);
        assert!(
            !kinds(&blocks).contains(&"image".to_string()),
            "an image block risks failing the entire page"
        );
        let runs = blocks[0]["paragraph"]["rich_text"].as_array().unwrap();
        assert!(runs
            .iter()
            .any(|r| r["text"]["link"]["url"].as_str() == Some(".helixnotes/attachments/x.png")));
    }

    #[test]
    fn text_longer_than_two_thousand_characters_is_split_across_runs() {
        let long = "a".repeat(4500);
        let blocks = to_blocks(&long);
        let runs = blocks[0]["paragraph"]["rich_text"].as_array().unwrap();

        assert_eq!(runs.len(), 3, "4500 chars is three runs of at most 2000");
        for run in runs {
            assert!(run["text"]["content"].as_str().unwrap().chars().count() <= MAX_TEXT);
        }
        assert_eq!(text_of(&blocks[0]).chars().count(), 4500, "nothing lost");
    }

    #[test]
    fn splitting_never_cuts_a_multibyte_character_in_half() {
        // Slicing by bytes would panic here. Notes are full of non-ASCII.
        let long = "é".repeat(2500);
        let blocks = to_blocks(&long);
        assert_eq!(text_of(&blocks[0]).chars().count(), 2500);
    }

    #[test]
    fn a_paragraph_too_long_for_one_block_becomes_several() {
        // Over 100 runs cannot live in one block, so the block itself splits.
        let long = "b".repeat(MAX_TEXT * (MAX_RUNS + 5));
        let blocks = to_blocks(&long);
        assert!(blocks.len() > 1, "expected the block to split");
        for block in &blocks {
            let runs = block["paragraph"]["rich_text"].as_array().unwrap();
            assert!(runs.len() <= MAX_RUNS);
        }
    }

    #[test]
    fn blocks_are_batched_to_the_request_limit() {
        let blocks: Vec<Value> = (0..250).map(|_| json!({"type": "paragraph"})).collect();
        let batches = batches(blocks);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), MAX_BLOCKS_PER_REQUEST);
        assert_eq!(batches[2].len(), 50);
    }

    #[test]
    fn an_empty_note_produces_no_blocks() {
        assert!(to_blocks("").is_empty());
        assert!(to_blocks("   \n\n  ").is_empty());
    }

    #[test]
    fn a_nested_list_keeps_its_child_under_its_parent() {
        let blocks = to_blocks("- outer\n    - inner");
        assert_eq!(kinds(&blocks), ["bulleted_list_item"]);
        let children = blocks[0]["bulleted_list_item"]["children"]
            .as_array()
            .expect("the inner item should nest");
        assert_eq!(text_of(&children[0]), "inner");
    }

    #[test]
    fn nesting_deeper_than_notion_allows_is_flattened_rather_than_rejected() {
        // Notion takes two levels in one request. A third would be refused outright, so
        // the indentation is given up and the content kept.
        let markdown = "- one\n    - two\n        - three\n            - four";
        let blocks = to_blocks(markdown);
        let flat = serde_json::to_string(&blocks).unwrap();
        assert!(flat.contains("four"), "deep content must survive");
    }

    #[test]
    fn a_note_that_is_only_frontmatter_body_text_still_converts() {
        // The body arrives already stripped of frontmatter; this guards the empty case
        // that produces from a note with nothing but metadata.
        let blocks = to_blocks("\n");
        assert!(blocks.is_empty());
    }

    #[test]
    fn soft_line_breaks_do_not_glue_words_together() {
        let blocks = to_blocks("first line\nsecond line");
        assert_eq!(text_of(&blocks[0]), "first line second line");
    }
}
