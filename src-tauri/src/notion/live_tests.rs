//! Verification against a real Notion workspace.
//!
//! Everything else in this module tests against a scripted HTTP server, which proves the
//! code does what it intends. It cannot prove that Notion behaves the way the code assumes —
//! and several decisions here rest on exactly that: that `erase_content` really empties a
//! page, that pacing at three requests a second survives a sustained run, that a moved page
//! keeps its id. This drives the real engine, through the same functions the app calls,
//! against the real API.
//!
//! Ignored by default, because it needs credentials and creates real databases. Run it with
//!
//! ```text
//! NOTION_TEST_PARENT_PAGE=<page id> pnpm test:rust notion::live_tests -- --ignored --nocapture
//! ```
//!
//! The token is read from `~/.config/second-brain-notion-test-token`, or from the file named
//! by `NOTION_TEST_TOKEN_FILE`. It is never taken from the command line, where it would land
//! in shell history. Use a disposable workspace: the four databases created here are trashed
//! at the end, but a run killed part-way leaves them behind.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::client::NotionClient;
use super::commands::{enumerate, note_deleted, setup_databases};
use super::config::{DatabaseRegistry, API_VERSION};
use super::map::{self, MapEntry};
use super::publish::{self, Summary};
use crate::vault::para::ParaCategory;

/// Enough notes that a first sync runs for most of a minute at the paced rate, which is the
/// span over which a pacing mistake would surface as a 429.
const NOTES: usize = 120;

/// Every twenty-fifth note is long enough to need three block batches.
const LONG_EVERY: usize = 25;

struct Live {
    token: String,
    parent: String,
}

fn live() -> Live {
    let token_file = std::env::var("NOTION_TEST_TOKEN_FILE").unwrap_or_else(|_| {
        format!(
            "{}/.config/second-brain-notion-test-token",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let token = std::fs::read_to_string(&token_file)
        .unwrap_or_else(|_| panic!("no token at {token_file}"))
        .trim()
        .to_string();
    let parent = std::env::var("NOTION_TEST_PARENT_PAGE")
        .expect("set NOTION_TEST_PARENT_PAGE to a page shared with the connection");
    Live { token, parent }
}

/// Raw API access for checking what Notion holds, deliberately outside the client so the
/// verification does not trust the code it is verifying.
struct Probe {
    http: reqwest::Client,
    token: String,
}

impl Probe {
    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        // Paced by hand: the probe shares the workspace's limit with the publisher.
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        let mut request = self
            .http
            .request(method, format!("https://api.notion.com/v1{path}"))
            .bearer_auth(&self.token)
            .header("Notion-Version", API_VERSION);
        if let Some(body) = body {
            request = request.json(&body);
        }
        request
            .send()
            .await
            .expect("probe request")
            .json()
            .await
            .expect("probe json")
    }

    async fn pages_carrying(&self, data_source_id: &str, note_id: &str) -> usize {
        let response = self
            .call(
                reqwest::Method::POST,
                &format!("/data_sources/{data_source_id}/query"),
                Some(json!({
                    "filter": { "property": "Note ID", "rich_text": { "equals": note_id } }
                })),
            )
            .await;
        response["results"].as_array().map(Vec::len).unwrap_or(0)
    }

    async fn page(&self, page_id: &str) -> Value {
        self.call(reqwest::Method::GET, &format!("/pages/{page_id}"), None)
            .await
    }

    async fn text_of(&self, page_id: &str) -> String {
        let response = self
            .call(
                reqwest::Method::GET,
                &format!("/blocks/{page_id}/children?page_size=100"),
                None,
            )
            .await;
        let mut text = String::new();
        for block in response["results"].as_array().into_iter().flatten() {
            let kind = block["type"].as_str().unwrap_or("");
            for run in block[kind]["rich_text"].as_array().into_iter().flatten() {
                text.push_str(run["plain_text"].as_str().unwrap_or(""));
            }
            text.push('\n');
        }
        text
    }

    async fn trash_database(&self, database_id: &str) {
        self.call(
            reqwest::Method::PATCH,
            &format!("/databases/{database_id}"),
            Some(json!({ "in_trash": true })),
        )
        .await;
    }
}

fn note_path(vault: &Path, category: ParaCategory, index: usize) -> PathBuf {
    vault
        .join(category.folder_name())
        .join(format!("Live note {index:04}.md"))
}

fn write_note(vault: &Path, category: ParaCategory, index: usize, body: &str) -> PathBuf {
    let path = note_path(vault, category, index);
    let raw = format!(
        "---\nid: \"live-{index:04}\"\ntitle: \"Live note {index:04}\"\ntags: [live, batch-{}]\ncategory: {}\ncreated: 2026-09-10T10:00:00Z\nmodified: 2026-09-10T10:00:00Z\n---\n{body}\n",
        index % 3,
        category.folder_name()
    );
    std::fs::write(&path, raw).unwrap();
    path
}

fn category_of(index: usize) -> ParaCategory {
    ParaCategory::ALL[index % 4]
}

fn body_of(index: usize) -> String {
    if index.is_multiple_of(LONG_EVERY) {
        (0..250)
            .map(|n| format!("Original paragraph {n} of long note {index}."))
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        format!("# Heading\n\nOriginal paragraph for note {index}.\n\n- a list item\n- another")
    }
}

async fn publish_once(vault: &Path, client: &NotionClient, registry: &DatabaseRegistry) -> Summary {
    let (snapshots, mut prepared) = enumerate(vault);
    publish::run(
        vault,
        client,
        registry,
        &tokio::sync::Mutex::new(()),
        snapshots,
        |snapshot| {
            prepared
                .remove(&snapshot.note_id)
                .ok_or_else(|| "not prepared".to_string())
        },
        |_| {},
    )
    .await
    .expect("a run against a valid token must not fail outright")
}

fn data_source_id(registry: &DatabaseRegistry, category: ParaCategory) -> String {
    registry.link(category).unwrap().data_source_id.clone()
}

/// The cheapest possible check that this process can reach Notion at all, run before the
/// full scenario so a network or TLS problem is diagnosed in seconds rather than surfacing
/// as a setup failure half a minute in.
#[tokio::test]
#[ignore = "needs a Notion workspace; see the module docs"]
async fn this_process_can_reach_notion() {
    let live = live();
    let client = NotionClient::new(live.token);
    let started = Instant::now();
    let outcome = client.whoami().await;
    println!(
        "whoami after {:.1}s: {outcome:?}",
        started.elapsed().as_secs_f64()
    );
    outcome.expect("the connection must be reachable");
}

#[tokio::test]
#[ignore = "needs a Notion workspace; see the module docs"]
async fn the_publisher_against_a_real_notion_workspace() {
    let live = live();
    let probe = Probe {
        http: reqwest::Client::new(),
        token: live.token.clone(),
    };
    let vault = std::env::temp_dir().join(format!("notion-live-{}", uuid::Uuid::new_v4()));
    for category in ParaCategory::ALL {
        std::fs::create_dir_all(vault.join(category.folder_name())).unwrap();
    }
    for index in 0..NOTES {
        write_note(&vault, category_of(index), index, &body_of(index));
    }

    // Setup runs inside the guard, not before it: a setup that fails part-way has still
    // created real databases, and they must be cleaned up like any other.
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        let client = NotionClient::new(live.token.clone());
        let registry = setup_databases(&client, &vault, &live.parent)
            .await
            .expect("setup");
        assert!(registry.is_complete(), "all four databases must exist");
        println!(
            "setup: four databases created, {} requests",
            client.requests_sent()
        );
        verify(&vault, &registry, &probe).await;
    }))
    .await;

    // Cleanup reads what setup recorded on disk, which setup saves after each database it
    // creates — so it trashes exactly what exists, even when setup only got partway.
    let recorded = super::config::load_registry(&vault);
    for category in ParaCategory::ALL {
        if let Some(link) = recorded.link(category) {
            probe.trash_database(&link.database_id).await;
        }
    }
    let _ = std::fs::remove_dir_all(&vault);
    println!("cleanup: {} databases trashed", recorded.databases.len());

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn verify(vault: &Path, registry: &DatabaseRegistry, probe: &Probe) {
    // ── a large first sync, paced ──────────────────────────────────────────────────────
    let first = NotionClient::new(probe.token.clone());
    let started = Instant::now();
    let summary = publish_once(vault, &first, registry).await;
    let elapsed = started.elapsed().as_secs_f64();
    let requests = first.requests_sent();
    let rate = requests as f64 / elapsed;
    println!(
        "first sync: {} created, {} failed, {requests} requests in {elapsed:.1}s = {rate:.2} req/s",
        summary.created, summary.failed
    );
    assert_eq!(
        summary.failed, 0,
        "a large first sync must complete without failures"
    );
    assert_eq!(summary.created as usize, NOTES);
    assert!(
        rate <= 3.05,
        "paced above Notion's average of three a second: {rate:.2}"
    );

    // ── an unchanged vault costs nothing ───────────────────────────────────────────────
    let idle = NotionClient::new(probe.token.clone());
    let summary = publish_once(vault, &idle, registry).await;
    println!(
        "unchanged rerun: {} up to date, {} requests",
        summary.up_to_date,
        idle.requests_sent()
    );
    assert_eq!(
        idle.requests_sent(),
        0,
        "an unchanged vault must make no API calls"
    );
    assert_eq!(summary.up_to_date as usize, NOTES);

    // ── an edit replaces the content, via erase_content ────────────────────────────────
    let edited = 1;
    write_note(
        vault,
        category_of(edited),
        edited,
        "Rewritten body carrying marker-7f3a.",
    );
    let client = NotionClient::new(probe.token.clone());
    let summary = publish_once(vault, &client, registry).await;
    assert_eq!(summary.updated, 1);
    let page_id = map::load(vault, "live-0001").unwrap().page_id.unwrap();
    let text = probe.text_of(&page_id).await;
    println!("edit: page now reads {:?}", text.trim());
    assert!(
        text.contains("marker-7f3a"),
        "the new content must be on the page"
    );
    assert!(
        !text.contains("Original paragraph"),
        "erase_content must have removed the old content, not appended beside it"
    );

    // ── a category change moves the page and keeps its tags ───────────────────────────
    let moved = 2; // Resources -> Archives
    let before = map::load(vault, "live-0002").unwrap().page_id.unwrap();
    std::fs::remove_file(note_path(vault, category_of(moved), moved)).unwrap();
    write_note(vault, ParaCategory::Archives, moved, &body_of(moved));
    let summary = publish_once(vault, &NotionClient::new(probe.token.clone()), registry).await;
    assert_eq!(summary.moved, 1);
    let page = probe.page(&before).await;
    let tags: Vec<&str> = page["properties"]["Tags"]["multi_select"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tag| tag["name"].as_str())
        .collect();
    println!(
        "move: page {} now in {}, tags {tags:?}",
        before, page["parent"]["data_source_id"]
    );
    assert_eq!(
        page["parent"]["data_source_id"].as_str(),
        Some(data_source_id(registry, ParaCategory::Archives).as_str()),
        "the page must now be in the Archives database"
    );
    assert_eq!(
        map::load(vault, "live-0002").unwrap().page_id.as_deref(),
        Some(before.as_str()),
        "identity preserved"
    );
    assert!(
        tags.contains(&"live") && tags.contains(&"batch-2"),
        "the move drops multi_select values; the rewrite must restore them"
    );

    // ── a deletion trashes the page ────────────────────────────────────────────────────
    let deleted = 3;
    let doomed = map::load(vault, "live-0003").unwrap().page_id.unwrap();
    let path = note_path(vault, category_of(deleted), deleted);
    note_deleted(vault, path.to_str().unwrap()).unwrap();
    std::fs::remove_file(&path).unwrap();
    let summary = publish_once(vault, &NotionClient::new(probe.token.clone()), registry).await;
    assert_eq!(summary.trashed, 1);
    let page = probe.page(&doomed).await;
    println!("delete: page in_trash = {}", page["in_trash"]);
    assert_eq!(page["in_trash"], json!(true));

    // ── an interrupted create is resolved, not duplicated ──────────────────────────────
    let interrupted = 4;
    let original = map::load(vault, "live-0004").unwrap().page_id.unwrap();
    map::save(vault, "live-0004", &MapEntry::creating()).unwrap();
    let summary = publish_once(vault, &NotionClient::new(probe.token.clone()), registry).await;
    assert_eq!(summary.failed, 0);
    let copies = probe
        .pages_carrying(
            &data_source_id(registry, category_of(interrupted)),
            "live-0004",
        )
        .await;
    println!("interrupted create: {copies} page(s) carry live-0004");
    assert_eq!(
        copies, 1,
        "resolving an interrupted create must not add a second page"
    );
    assert_eq!(
        map::load(vault, "live-0004").unwrap().page_id.as_deref(),
        Some(original.as_str())
    );

    // ── the whole map lost: rebuilt from Notion, nothing duplicated ────────────────────
    for (note_id, _) in map::all(vault) {
        map::remove(vault, &note_id).unwrap();
    }
    let rebuild = NotionClient::new(probe.token.clone());
    let summary = publish_once(vault, &rebuild, registry).await;
    println!(
        "map lost: {} created, {} updated, {} moved, {} failed, {} requests",
        summary.created,
        summary.updated,
        summary.moved,
        summary.failed,
        rebuild.requests_sent()
    );
    assert_eq!(
        summary.created, 0,
        "every page already existed; creating would duplicate"
    );
    assert_eq!(summary.failed, 0);
    for index in [0, 5, 50, 100] {
        let note_id = format!("live-{index:04}");
        let copies = probe
            .pages_carrying(&data_source_id(registry, category_of(index)), &note_id)
            .await;
        assert_eq!(
            copies, 1,
            "{note_id} must have exactly one page after the rebuild"
        );
    }
}
