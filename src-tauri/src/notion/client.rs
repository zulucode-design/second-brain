//! Talking to Notion, paced so the API does not push back.
//!
//! Notion allows "an average of three requests per second, with some bursts". Exceeding it
//! returns 429 with a `Retry-After` header, and the limit is **per workspace**, shared with
//! anything else the user has connected. So this client is deliberately conservative: it
//! spaces its own requests rather than sprinting until Notion complains, because the cost
//! of complaining is borne by the user's whole workspace, not just this app.
//!
//! Every call goes through [`NotionClient::request`], which is the only place that knows
//! about pacing, retries, or how a Notion error becomes one of ours. Adding an endpoint is
//! a typed method over that, not a new place to get any of it subtly different.
//!
//! ## What the errors mean, and why the distinction is load-bearing
//!
//! - [`NotionError::Unauthorized`] stops the whole run. A revoked token cannot recover by
//!   being tried again, and every retry spends the shared workspace budget on a request
//!   that is certain to fail.
//! - [`NotionError::Validation`] is one note's problem: it is recorded against that note,
//!   which is skipped and retried next run. One malformed note halting the publisher
//!   indefinitely is the failure most likely to go unnoticed for weeks.
//! - [`NotionError::RateLimited`] is only returned after retries are exhausted. The normal
//!   case is handled here and never reaches the caller.

use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::config::{DatabaseLink, API_VERSION, MARKER, NOTE_ID_PROPERTY};

const API_BASE: &str = "https://api.notion.com/v1";

/// Minimum gap between requests: three per second, with the interval rounded up rather
/// than down so rounding error cannot put us over the published average.
const MIN_INTERVAL: Duration = Duration::from_millis(334);

/// How many times a 429 or 529 is waited out before giving up on a note.
const MAX_RETRIES: u32 = 4;

/// Longest we will honour a `Retry-After` before treating it as a failure.
///
/// Notion is entitled to ask for a long wait, but a background publisher blocking for
/// minutes is indistinguishable from one that has hung. Past this, the note is deferred to
/// the next poll, which is a wait the user can see the end of.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotionError {
    /// The token was rejected. Fatal to the run.
    Unauthorized(String),
    /// Notion refused this specific request. One note's problem.
    Validation(String),
    /// Still rate limited after exhausting retries.
    RateLimited,
    /// The page or database is gone, or the integration lost access to it.
    NotFound(String),
    /// Anything else Notion returned.
    Api { status: u16, message: String },
    /// The request never completed — offline, DNS, TLS.
    Network(String),
}

impl NotionError {
    /// Whether this ends the run rather than skipping one note.
    ///
    /// Only a rejected token qualifies. Everything else is either that note's problem or a
    /// transient condition the next poll will find resolved — and the app is offline-first,
    /// so being unable to reach Notion is an ordinary state, not an error to escalate.
    pub fn is_fatal(&self) -> bool {
        matches!(self, NotionError::Unauthorized(_))
    }

    pub fn message(&self) -> String {
        match self {
            NotionError::Unauthorized(detail) => {
                format!("Notion rejected the connection: {detail}")
            }
            NotionError::Validation(detail) => format!("Notion refused the request: {detail}"),
            NotionError::RateLimited => {
                "Notion is rate limiting this workspace; deferred to the next sync".into()
            }
            NotionError::NotFound(detail) => format!("Not found in Notion: {detail}"),
            NotionError::Api { status, message } => format!("Notion returned {status}: {message}"),
            NotionError::Network(detail) => format!("Could not reach Notion: {detail}"),
        }
    }
}

impl std::fmt::Display for NotionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

/// A page the integration can see, for the setup picker.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VisiblePage {
    pub id: String,
    pub title: String,
}

/// Spaces requests so the published average is never exceeded.
///
/// One publisher means one pacer, which is the reason a single machine publishes: two
/// machines pacing themselves independently would each stay under the limit and together
/// exceed it.
struct Pacer {
    interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl Pacer {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Mutex::new(None),
        }
    }

    async fn wait(&self) {
        let mut last = self.last.lock().await;
        if let Some(previous) = *last {
            let elapsed = previous.elapsed();
            if elapsed < self.interval {
                tokio::time::sleep(self.interval - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }
}

pub struct NotionClient {
    http: reqwest::Client,
    token: String,
    base: String,
    pacer: Arc<Pacer>,
}

impl NotionClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self::with_base(token, API_BASE, MIN_INTERVAL)
    }

    /// Construct against another base URL and pace, for tests.
    ///
    /// The interval is a parameter rather than a constant so a test can exercise pacing
    /// without spending a real second per request.
    pub fn with_base(
        token: impl Into<String>,
        base: impl Into<String>,
        interval: Duration,
    ) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            token: token.into(),
            base: base.into().trim_end_matches('/').to_string(),
            pacer: Arc::new(Pacer::new(interval)),
        }
    }

    /// Every request to Notion goes through here.
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, NotionError> {
        let url = format!("{}{}", self.base, path);

        for attempt in 0..=MAX_RETRIES {
            self.pacer.wait().await;

            let mut builder = self
                .http
                .request(method.clone(), &url)
                .bearer_auth(&self.token)
                .header("Notion-Version", API_VERSION);
            if let Some(payload) = &body {
                builder = builder.json(payload);
            }

            let response = match builder.send().await {
                Ok(response) => response,
                Err(error) => return Err(NotionError::Network(error.to_string())),
            };

            let status = response.status();
            // 529 is Notion being overloaded rather than us being greedy, and the docs say
            // to treat it exactly as a 429.
            if status.as_u16() == 429 || status.as_u16() == 529 {
                if attempt == MAX_RETRIES {
                    return Err(NotionError::RateLimited);
                }
                let wait = retry_after(response.headers(), attempt);
                if wait > MAX_BACKOFF {
                    return Err(NotionError::RateLimited);
                }
                log::warn!("Notion asked to wait {:?}; retrying", wait);
                tokio::time::sleep(wait).await;
                continue;
            }

            let text = response.text().await.unwrap_or_default();
            let parsed: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));

            if status.is_success() {
                return Ok(parsed);
            }

            let message = parsed
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or(&text)
                .to_string();

            return Err(match status.as_u16() {
                401 => NotionError::Unauthorized(message),
                400 | 409 => NotionError::Validation(message),
                403 => NotionError::Unauthorized(message),
                404 => NotionError::NotFound(message),
                other => NotionError::Api {
                    status: other,
                    message,
                },
            });
        }

        Err(NotionError::RateLimited)
    }

    /// Confirm the token works, returning the connection's name for the Settings panel.
    pub async fn whoami(&self) -> Result<String, NotionError> {
        let response = self
            .request(reqwest::Method::GET, "/users/me", None)
            .await?;
        Ok(response
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("Notion")
            .to_string())
    }

    /// Pages the integration has been given access to.
    ///
    /// Setup uses this instead of asking for a pasted page URL: the integration can only
    /// see pages the user has explicitly shared with it, so this list is exactly the set of
    /// valid answers, and no id can be mistyped.
    pub async fn visible_pages(&self) -> Result<Vec<VisiblePage>, NotionError> {
        let body = json!({
            "filter": { "value": "page", "property": "object" },
            "page_size": 100,
        });
        let response = self
            .request(reqwest::Method::POST, "/search", Some(body))
            .await?;

        Ok(response
            .get("results")
            .and_then(|r| r.as_array())
            .map(|results| {
                results
                    .iter()
                    .filter(|item| item["object"] == json!("page"))
                    .filter_map(|item| {
                        let id = item["id"].as_str()?.to_string();
                        Some(VisiblePage {
                            title: page_title(item).unwrap_or_else(|| "Untitled".into()),
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Create one PARA database under `parent_page_id`.
    ///
    /// The marker in the description is written for #58, which reclaims databases created by
    /// a previous install. This version never reads it — but a database created without it
    /// could never be recognised later, so it is written from the first release.
    pub async fn create_database(
        &self,
        parent_page_id: &str,
        title: &str,
    ) -> Result<DatabaseLink, NotionError> {
        let body = json!({
            "parent": { "type": "page_id", "page_id": parent_page_id },
            "title": [{ "type": "text", "text": { "content": title } }],
            "description": [{ "type": "text", "text": { "content": MARKER } }],
            "initial_data_source": { "properties": database_schema() },
        });

        let response = self
            .request(reqwest::Method::POST, "/databases", Some(body))
            .await?;

        let database_id = response["id"]
            .as_str()
            .ok_or_else(|| NotionError::Validation("Notion returned no database id".into()))?
            .to_string();

        // Pages are parented to a data source, not to the database, so a database without
        // one is unusable and must not be recorded as if setup succeeded.
        let data_source_id = response["data_sources"][0]["id"]
            .as_str()
            .ok_or_else(|| {
                NotionError::Validation(
                    "Notion created the database without a data source, so no page can be added to it"
                        .into(),
                )
            })?
            .to_string();

        Ok(DatabaseLink {
            database_id,
            data_source_id,
        })
    }

    /// Find a page by the note id it carries, which is how a lost map is rebuilt.
    pub async fn find_page_by_note_id(
        &self,
        data_source_id: &str,
        note_id: &str,
    ) -> Result<Option<String>, NotionError> {
        let body = json!({
            "filter": {
                "property": NOTE_ID_PROPERTY,
                "rich_text": { "equals": note_id },
            },
            "page_size": 1,
        });
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/data_sources/{data_source_id}/query"),
                Some(body),
            )
            .await?;

        Ok(response["results"][0]["id"].as_str().map(str::to_string))
    }

    pub async fn create_page(
        &self,
        data_source_id: &str,
        properties: Value,
        children: Vec<Value>,
    ) -> Result<String, NotionError> {
        let body = json!({
            "parent": { "type": "data_source_id", "data_source_id": data_source_id },
            "properties": properties,
            "children": children,
        });
        let response = self
            .request(reqwest::Method::POST, "/pages", Some(body))
            .await?;

        response["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| NotionError::Validation("Notion returned no page id".into()))
    }

    pub async fn append_blocks(
        &self,
        page_id: &str,
        blocks: Vec<Value>,
    ) -> Result<(), NotionError> {
        if blocks.is_empty() {
            return Ok(());
        }
        self.request(
            reqwest::Method::PATCH,
            &format!("/blocks/{page_id}/children"),
            Some(json!({ "children": blocks })),
        )
        .await
        .map(|_| ())
    }

    pub async fn update_properties(
        &self,
        page_id: &str,
        properties: Value,
    ) -> Result<(), NotionError> {
        self.request(
            reqwest::Method::PATCH,
            &format!("/pages/{page_id}"),
            Some(json!({ "properties": properties })),
        )
        .await
        .map(|_| ())
    }

    /// Remove every block on the page, so new content can replace it.
    ///
    /// One call rather than deleting each block: a note of two hundred blocks would
    /// otherwise cost two hundred requests to update, which at three per second is over a
    /// minute for one edited note.
    pub async fn erase_content(&self, page_id: &str) -> Result<(), NotionError> {
        self.request(
            reqwest::Method::PATCH,
            &format!("/pages/{page_id}"),
            Some(json!({ "erase_content": true })),
        )
        .await
        .map(|_| ())
    }

    /// Move a page to another data source, preserving its identity.
    ///
    /// **The caller must rewrite the page's properties afterwards.** A move drops
    /// `multi_select` values, because their options are registered on the data source that
    /// owns them and the target has never seen them. Verified against the live API; see
    /// `docs/reports/spike-notion-move-2026-09-10.md`.
    pub async fn move_page(&self, page_id: &str, data_source_id: &str) -> Result<(), NotionError> {
        self.request(
            reqwest::Method::POST,
            &format!("/pages/{page_id}/move"),
            Some(json!({
                "parent": { "type": "data_source_id", "data_source_id": data_source_id }
            })),
        )
        .await
        .map(|_| ())
    }

    /// Move a page to Notion's trash, where it is recoverable for 30 days.
    pub async fn trash_page(&self, page_id: &str) -> Result<(), NotionError> {
        self.request(
            reqwest::Method::PATCH,
            &format!("/pages/{page_id}"),
            Some(json!({ "in_trash": true })),
        )
        .await
        .map(|_| ())
    }
}

/// The schema every PARA database is created with.
///
/// No `Category` property: the database *is* the category, and a second copy of that fact
/// is one more thing to keep correct when a note moves between them.
pub fn database_schema() -> Value {
    json!({
        "Name": { "title": {} },
        NOTE_ID_PROPERTY: { "rich_text": {} },
        "Tags": { "multi_select": {} },
        "Created": { "date": {} },
        "Modified": { "date": {} },
    })
}

/// Pull a readable title out of a page object, whatever its title property is called.
fn page_title(page: &Value) -> Option<String> {
    let properties = page.get("properties")?.as_object()?;
    for value in properties.values() {
        if value.get("type").and_then(|t| t.as_str()) == Some("title") {
            let text: String = value
                .get("title")?
                .as_array()?
                .iter()
                .filter_map(|run| run["plain_text"].as_str())
                .collect();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// How long to wait after a 429, honouring `Retry-After` when Notion sends one.
fn retry_after(headers: &reqwest::header::HeaderMap, attempt: u32) -> Duration {
    headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        // No header is not licence to retry immediately: back off on our own schedule
        // rather than adding to the pressure that produced the 429.
        .unwrap_or_else(|| Duration::from_millis(500 * 2_u64.pow(attempt)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};

    /// A server that answers a scripted sequence of responses, one per request, recording
    /// each request it received. Modelled on the probe tests' one-shot server (#17).
    fn scripted_server(responses: Vec<(&str, &str)>) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test server address");
        let (tx, rx) = mpsc::channel();
        let responses: Vec<(String, String)> = responses
            .into_iter()
            .map(|(status, body)| (status.to_string(), body.to_string()))
            .collect();

        std::thread::spawn(move || {
            for (status, body) in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));

                let mut request = Vec::new();
                let mut buffer = [0_u8; 2048];
                loop {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request);
                    // Stop once headers and any declared body have arrived.
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

    fn client(base: &str) -> NotionClient {
        // A near-zero interval: pacing is exercised by its own test, and every other test
        // would otherwise pay a third of a second per request.
        NotionClient::with_base("ntn_test", base, Duration::from_millis(1))
    }

    #[tokio::test]
    async fn a_request_carries_the_token_and_the_pinned_api_version() {
        // The version is not cosmetic: the move endpoint exists only on this one.
        let (base, requests) = scripted_server(vec![("200 OK", r#"{"name":"Spike"}"#)]);
        let name = client(&base).whoami().await.unwrap();

        let request = requests.recv().unwrap();
        assert!(
            request.contains("authorization: Bearer ntn_test")
                || request.contains("Authorization: Bearer ntn_test")
        );
        assert!(request
            .to_lowercase()
            .contains(&format!("notion-version: {API_VERSION}").to_lowercase()));
        assert_eq!(name, "Spike");
    }

    #[tokio::test]
    async fn a_rejected_token_is_fatal_and_never_retried() {
        // Retrying a revoked token spends a shared workspace budget on a certain failure.
        let (base, _requests) = scripted_server(vec![(
            "401 Unauthorized",
            r#"{"message":"API token is invalid."}"#,
        )]);

        let error = client(&base).whoami().await.unwrap_err();
        assert!(error.is_fatal());
        assert!(matches!(error, NotionError::Unauthorized(_)));
        assert!(error.message().contains("API token is invalid."));
    }

    #[tokio::test]
    async fn losing_access_to_the_workspace_reads_as_a_connection_problem() {
        // 403 means the integration was disconnected from the pages, which the user fixes
        // the same way they fix a bad token — by reconnecting.
        let (base, _r) = scripted_server(vec![("403 Forbidden", r#"{"message":"no access"}"#)]);
        let error = client(&base).whoami().await.unwrap_err();
        assert!(error.is_fatal());
    }

    #[tokio::test]
    async fn a_rejected_request_is_one_notes_problem_not_the_runs() {
        let (base, _r) = scripted_server(vec![(
            "400 Bad Request",
            r#"{"message":"body.children[0] is not valid"}"#,
        )]);

        let error = client(&base)
            .create_page("ds", json!({}), vec![])
            .await
            .unwrap_err();

        assert!(
            !error.is_fatal(),
            "one bad note must not halt the publisher"
        );
        assert!(matches!(error, NotionError::Validation(_)));
    }

    #[tokio::test]
    async fn being_rate_limited_waits_the_requested_time_and_then_succeeds() {
        let (base, _r) = scripted_server(vec![
            ("429 Too Many Requests", r#"{"message":"rate_limited"}"#),
            ("200 OK", r#"{"name":"Spike"}"#),
        ]);

        let started = std::time::Instant::now();
        let name = client(&base).whoami().await.unwrap();

        assert_eq!(name, "Spike");
        assert!(
            started.elapsed() >= Duration::from_millis(400),
            "a 429 without Retry-After must still back off, not retry immediately"
        );
    }

    #[tokio::test]
    async fn notion_being_overloaded_is_handled_exactly_like_a_rate_limit() {
        // The docs say to treat 529 identically to 429.
        let (base, _r) = scripted_server(vec![
            ("529 Site Overloaded", r#"{"message":"service_overload"}"#),
            ("200 OK", r#"{"name":"Spike"}"#),
        ]);

        assert_eq!(client(&base).whoami().await.unwrap(), "Spike");
    }

    #[tokio::test]
    async fn a_persistent_rate_limit_gives_up_rather_than_blocking_forever() {
        let responses = vec![("429 Too Many Requests", r#"{"message":"rate_limited"}"#); 6];
        let (base, _r) = scripted_server(responses);

        let error = client(&base).whoami().await.unwrap_err();
        assert_eq!(error, NotionError::RateLimited);
        assert!(!error.is_fatal(), "the next poll should simply try again");
    }

    #[tokio::test]
    async fn an_unreachable_notion_is_an_ordinary_offline_state() {
        // The app is offline-first; being unable to reach Notion is not an error to
        // escalate, and must never be mistaken for a bad token.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);

        let error = client(&format!("http://{address}"))
            .whoami()
            .await
            .unwrap_err();

        assert!(matches!(error, NotionError::Network(_)));
        assert!(!error.is_fatal());
    }

    #[tokio::test]
    async fn requests_are_spaced_to_stay_under_three_per_second() {
        let (base, _r) = scripted_server(vec![
            ("200 OK", r#"{"name":"a"}"#),
            ("200 OK", r#"{"name":"b"}"#),
            ("200 OK", r#"{"name":"c"}"#),
        ]);
        let client = NotionClient::with_base("t", &base, Duration::from_millis(120));

        let started = std::time::Instant::now();
        client.whoami().await.unwrap();
        client.whoami().await.unwrap();
        client.whoami().await.unwrap();

        assert!(
            started.elapsed() >= Duration::from_millis(240),
            "three requests must span at least two intervals, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn creating_a_database_records_both_ids() {
        // Pages are parented to the data source; the database id is what the user sees.
        let (base, requests) = scripted_server(vec![(
            "200 OK",
            r#"{"id":"db-1","data_sources":[{"id":"ds-1","name":"Projects"}]}"#,
        )]);

        let link = client(&base)
            .create_database("parent-page", "Projects")
            .await
            .unwrap();

        assert_eq!(link.database_id, "db-1");
        assert_eq!(link.data_source_id, "ds-1");

        let request = requests.recv().unwrap();
        assert!(request.contains("parent-page"));
        assert!(
            request.contains(NOTE_ID_PROPERTY),
            "the schema must carry Note ID"
        );
        assert!(
            request.contains("Managed by Second Brain"),
            "the marker must be written even though this version never reads it (#58)"
        );
    }

    #[tokio::test]
    async fn a_database_without_a_data_source_is_refused_rather_than_recorded() {
        // Recording it would mark setup complete for a database no page can be added to.
        let (base, _r) = scripted_server(vec![("200 OK", r#"{"id":"db-1"}"#)]);

        let error = client(&base)
            .create_database("parent", "Projects")
            .await
            .unwrap_err();

        assert!(matches!(error, NotionError::Validation(_)));
    }

    #[tokio::test]
    async fn a_page_is_found_by_its_note_id() {
        let (base, requests) =
            scripted_server(vec![("200 OK", r#"{"results":[{"id":"page-9"}]}"#)]);

        let found = client(&base)
            .find_page_by_note_id("ds-1", "note-abc")
            .await
            .unwrap();

        assert_eq!(found.as_deref(), Some("page-9"));
        let request = requests.recv().unwrap();
        assert!(request.contains("/data_sources/ds-1/query"));
        assert!(request.contains("note-abc"));
    }

    #[tokio::test]
    async fn a_note_with_no_page_yet_reports_none_rather_than_failing() {
        let (base, _r) = scripted_server(vec![("200 OK", r#"{"results":[]}"#)]);
        assert_eq!(
            client(&base)
                .find_page_by_note_id("ds-1", "note-abc")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn moving_a_page_targets_the_data_source_not_the_database() {
        // Using database_id starts failing the moment a database gains a second data
        // source, which the API docs warn about explicitly.
        let (base, requests) = scripted_server(vec![("200 OK", r#"{"id":"page-1"}"#)]);

        client(&base)
            .move_page("page-1", "ds-target")
            .await
            .unwrap();

        let request = requests.recv().unwrap();
        assert!(request.contains("/pages/page-1/move"));
        assert!(request.contains("data_source_id"));
        assert!(request.contains("ds-target"));
    }

    #[tokio::test]
    async fn erasing_content_is_one_request_rather_than_one_per_block() {
        // A 200-block note would otherwise cost 200 requests to update, over a minute at
        // three per second.
        let (base, requests) = scripted_server(vec![("200 OK", r#"{"id":"page-1"}"#)]);

        client(&base).erase_content("page-1").await.unwrap();

        let request = requests.recv().unwrap();
        assert!(request.contains("erase_content"));
        assert!(request.contains("PATCH /pages/page-1"));
    }

    #[tokio::test]
    async fn trashing_a_page_is_recoverable_rather_than_destructive() {
        let (base, requests) = scripted_server(vec![("200 OK", r#"{"id":"page-1"}"#)]);

        client(&base).trash_page("page-1").await.unwrap();

        let request = requests.recv().unwrap();
        assert!(request.contains("in_trash"));
    }

    #[tokio::test]
    async fn appending_no_blocks_makes_no_request_at_all() {
        // An empty note is common — a title and nothing else — and must not cost a call.
        let (base, _r) = scripted_server(vec![]);
        assert!(client(&base).append_blocks("page-1", vec![]).await.is_ok());
    }

    #[tokio::test]
    async fn the_page_picker_lists_titles_the_user_will_recognise() {
        let body = r#"{"results":[
            {"object":"page","id":"p1","properties":{"title":{"type":"title","title":[{"plain_text":"Second Brain"}]}}},
            {"object":"page","id":"p2","properties":{"Name":{"type":"title","title":[{"plain_text":"Other"}]}}}
        ]}"#;
        let (base, _r) = scripted_server(vec![("200 OK", body)]);

        let pages = client(&base).visible_pages().await.unwrap();

        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].title, "Second Brain");
        // The title property is not always called "title", so it is found by type.
        assert_eq!(pages[1].title, "Other");
    }

    #[tokio::test]
    async fn a_page_with_no_readable_title_is_still_offered() {
        // Dropping it would hide the only page the user shared, leaving setup impossible
        // with no explanation.
        let body = r#"{"results":[{"object":"page","id":"p1","properties":{}}]}"#;
        let (base, _r) = scripted_server(vec![("200 OK", body)]);

        let pages = client(&base).visible_pages().await.unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].title, "Untitled");
    }

    #[test]
    fn a_retry_after_header_is_honoured_over_our_own_backoff() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "7".parse().unwrap());
        assert_eq!(retry_after(&headers, 0), Duration::from_secs(7));
    }

    #[test]
    fn a_missing_retry_after_backs_off_rather_than_retrying_immediately() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after(&headers, 0), Duration::from_millis(500));
        assert_eq!(retry_after(&headers, 2), Duration::from_millis(2000));
    }

    #[test]
    fn the_schema_has_no_category_property() {
        // The database is the category; a second copy is one more thing to keep correct
        // when a note moves.
        let schema = database_schema();
        assert!(schema.get("Category").is_none());
        assert!(schema.get(NOTE_ID_PROPERTY).is_some());
        assert!(schema.get("Name").is_some());
    }
}
