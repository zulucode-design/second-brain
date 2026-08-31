//! Whether the AI backend can be reached, and what to tell the user when it cannot.
//!
//! The backend usually runs on a different machine — the desktop that holds the models,
//! reached over Tailscale — so it is unreachable often and routinely: the desktop sleeps,
//! the laptop leaves the network. That is an ordinary state, not an error, and everything
//! that does not need inference must keep working through it.
//!
//! Reachability is therefore tracked continuously rather than discovered when a feature is
//! used, and every probe is bounded by a timeout so an unreachable host stalls a
//! background check instead of the user.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;

/// How long to wait for the backend before calling it unreachable.
///
/// An unreachable host on a private network typically fails fast, but a sleeping one can
/// swallow the connection entirely and only fail on the OS timeout, which is measured in
/// minutes. This bound is what keeps that from ever being waited on.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait for a user-facing request to connect.
///
/// Separate from the probe bound so the two can move independently: a probe is a
/// background check nobody is waiting on, a request has someone watching it.
pub const REQUEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a request may go without producing any data before it is abandoned.
///
/// A connection that is accepted and then stalls would otherwise hang forever, since the
/// connect bound is already satisfied. Set well above any believable gap between tokens
/// so a slow model is never cut off mid-answer.
pub const REQUEST_STALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Gap between probes while the backend is answering.
pub const INTERVAL_WHEN_AVAILABLE: Duration = Duration::from_secs(120);

/// Gap between checks when there is no Ollama backend to watch at all.
///
/// Only the provider setting can change this, so the poller just needs to notice that
/// eventually rather than poll for it.
pub const INTERVAL_WHEN_IDLE: Duration = Duration::from_secs(300);

/// Gap between probes while it is not.
///
/// Shorter than the available interval: the user is waiting to get their features back,
/// and a probe against an unreachable host is cheap.
pub const INTERVAL_WHEN_UNAVAILABLE: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Availability {
    /// Not probed yet. Distinct from unavailable: nothing is known, so nothing is claimed.
    Unknown,
    Available,
    Unavailable,
}

/// The backend's reachability, and why, in terms the user can act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiTargetId {
    pub endpoint: String,
    pub model: String,
    pub generation: u64,
}

/// Everything needed to probe Ollama. The credential is deliberately excluded from the
/// serializable identity and this type does not implement `Debug`, preventing accidental
/// logging while still letting credential changes advance the generation.
#[derive(Clone, PartialEq, Eq)]
pub struct OllamaTarget {
    id: AiTargetId,
    bearer_token: Option<String>,
}

impl OllamaTarget {
    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        bearer_token: Option<String>,
        generation: u64,
    ) -> Self {
        Self {
            id: AiTargetId {
                endpoint: endpoint.into(),
                model: model.into(),
                generation,
            },
            bearer_token: bearer_token.filter(|token| !token.trim().is_empty()),
        }
    }

    pub fn id(&self) -> &AiTargetId {
        &self.id
    }

    fn bearer_token(&self) -> Option<&str> {
        self.bearer_token.as_deref()
    }

    pub fn same_settings(&self, other: &Self) -> bool {
        self.id.endpoint == other.id.endpoint
            && self.id.model == other.id.model
            && self.bearer_token == other.bearer_token
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiStatus {
    pub availability: Availability,
    /// Why it is unavailable, phrased as something to do about it. `None` when available.
    pub reason: Option<String>,
    /// The endpoint probed, so the user can see which machine was tried.
    pub endpoint: Option<String>,
    /// Secret-free identity of the settings this result belongs to.
    pub target: Option<AiTargetId>,
}

impl AiStatus {
    pub fn unknown() -> Self {
        Self {
            availability: Availability::Unknown,
            reason: None,
            endpoint: None,
            target: None,
        }
    }

    #[cfg(test)]
    pub fn available(endpoint: &str) -> Self {
        Self {
            availability: Availability::Available,
            reason: None,
            endpoint: Some(endpoint.to_string()),
            target: None,
        }
    }

    #[cfg(test)]
    pub fn unavailable(endpoint: &str, reason: impl Into<String>) -> Self {
        Self {
            availability: Availability::Unavailable,
            reason: Some(reason.into()),
            endpoint: Some(endpoint.to_string()),
            target: None,
        }
    }

    pub fn unknown_for(target: &OllamaTarget) -> Self {
        Self {
            availability: Availability::Unknown,
            reason: None,
            endpoint: Some(target.id.endpoint.clone()),
            target: Some(target.id.clone()),
        }
    }

    fn available_for(target: &OllamaTarget) -> Self {
        Self {
            availability: Availability::Available,
            reason: None,
            endpoint: Some(target.id.endpoint.clone()),
            target: Some(target.id.clone()),
        }
    }

    fn unavailable_for(target: &OllamaTarget, reason: impl Into<String>) -> Self {
        Self {
            availability: Availability::Unavailable,
            reason: Some(reason.into()),
            endpoint: Some(target.id.endpoint.clone()),
            target: Some(target.id.clone()),
        }
    }

    pub fn belongs_to(&self, target: &AiTargetId) -> bool {
        self.target.as_ref() == Some(target)
    }
}

/// How long to wait before probing again.
///
/// `tracking` is false when the configured provider is not Ollama, in which case there is
/// nothing to watch and the poller should idle rather than wake every few seconds to do
/// nothing.
pub fn next_probe_interval(status: &AiStatus, tracking: bool) -> Duration {
    if !tracking {
        return INTERVAL_WHEN_IDLE;
    }
    match status.availability {
        Availability::Available => INTERVAL_WHEN_AVAILABLE,
        // Unknown is treated as unavailable: probe soon, because nothing is known yet.
        Availability::Unknown | Availability::Unavailable => INTERVAL_WHEN_UNAVAILABLE,
    }
}

/// Turn a failed probe into something the user can act on.
///
/// The distinction that matters is between "the machine did not answer" and "something
/// answered but was not the backend", because the fixes are different: wake the desktop or
/// reconnect the network, versus check what is running on that port.
pub fn not_ollama(endpoint: &str, detail: &str) -> String {
    format!("{endpoint} answered, but {detail}. Check what is running on that address.")
}

pub fn describe_failure(endpoint: &str, error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return format!(
            "{endpoint} did not answer within {}s. The machine running Ollama may be asleep, \
             or the private network may be down.",
            PROBE_TIMEOUT.as_secs()
        );
    }
    if error.is_connect() {
        return format!(
            "Could not connect to {endpoint}. Check that Ollama is running on that machine \
             and that both machines are on the same private network."
        );
    }
    format!("Could not reach {endpoint}: {error}")
}

/// Whether a model appears in Ollama's list of installed models.
///
/// Ollama resolves a bare name to its `:latest` tag, so `llama3` means `llama3:latest`
/// specifically, not "any tag of llama3". Matching it against `llama3:8b` would report a
/// model as present that a request would then fail to find.
pub fn model_installed(tags_response: &serde_json::Value, model: &str) -> bool {
    let wanted = model.trim();
    if wanted.is_empty() {
        return false;
    }
    // A name with no tag means the `:latest` tag, which is how Ollama resolves it.
    let latest_tag = if wanted.contains(':') {
        wanted.to_string()
    } else {
        format!("{wanted}:latest")
    };
    let latest_tag = latest_tag.as_str();

    tags_response
        .get("models")
        .and_then(|m| m.as_array())
        .is_some_and(|models| {
            models.iter().any(|entry| {
                entry
                    .get("name")
                    .and_then(|n| n.as_str())
                    .is_some_and(|name| name == wanted || name == latest_tag)
            })
        })
}

/// The URL that lists installed models, used as the reachability probe.
///
/// Cheaper and more telling than a chat request: it confirms the host answers, that what
/// answered is Ollama, and which models it actually has, without running inference.
pub fn tags_url(base_url: &str) -> String {
    format!("{}/api/tags", base_url.trim_end_matches('/'))
}

/// Ask the backend whether it is there and has the model, without running inference.
///
/// Every outcome is a status rather than an error, because an unreachable backend is an
/// ordinary condition here, not a failure of the app.
pub async fn probe(target: &OllamaTarget) -> AiStatus {
    probe_with_timeout(target, PROBE_TIMEOUT).await
}

async fn probe_with_timeout(target: &OllamaTarget, timeout: Duration) -> AiStatus {
    let base_url = target.id.endpoint.as_str();
    let model = target.id.model.as_str();
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(client) => client,
        Err(e) => {
            return AiStatus::unavailable_for(target, format!("Could not start a check: {e}"))
        }
    };

    let mut request = client.get(tags_url(base_url));
    if let Some(token) = target.bearer_token() {
        request = request.bearer_auth(token);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(e) => {
            return AiStatus::unavailable_for(target, describe_failure(base_url, &e));
        }
    };

    if !response.status().is_success() {
        return AiStatus::unavailable_for(
            target,
            not_ollama(base_url, &format!("returned {}", response.status())),
        );
    }

    let tags: serde_json::Value = match response.json().await {
        Ok(tags) => tags,
        Err(_) => {
            return AiStatus::unavailable_for(
                target,
                not_ollama(base_url, "its reply was not Ollama's model list"),
            )
        }
    };

    // Reachable but missing the model is still unavailable, and the fix is specific.
    if !model.trim().is_empty() && !model_installed(&tags, model) {
        return AiStatus::unavailable_for(
            target,
            format!("Ollama is running at {base_url} but does not have \"{model}\". Install it on that machine with: ollama pull {model}"),
        );
    }

    AiStatus::available_for(target)
}

/// The endpoint and model to probe, or `None` when the configured provider is not Ollama.
///
/// Cloud providers are not tracked: their reachability is the user's internet connection,
/// which the app cannot usefully report on, and a wrong claim would disable working
/// features.
pub fn ollama_target(config: &crate::types::AppConfig, generation: u64) -> Option<OllamaTarget> {
    if config.ai_provider.as_deref() != Some("ollama") {
        return None;
    }
    let base = resolve_base_url(config.ollama_base_url.as_deref()).to_string();
    Some(OllamaTarget::new(
        base,
        config.ai_model.clone(),
        config.ollama_api_key.clone(),
        generation,
    ))
}

pub fn same_probe_settings(left: Option<&OllamaTarget>, right: Option<&OllamaTarget>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_settings(right),
        (None, None) => true,
        _ => false,
    }
}

/// Ollama's address when the user has not set one, i.e. running on this machine.
pub const DEFAULT_URL: &str = "http://localhost:11434";

/// The address to talk to Ollama on, given whatever is in the settings.
///
/// Every caller must resolve through this. A blank or whitespace-only setting is the same
/// as no setting, and if the probe and the request disagreed about that, the probe would
/// report a healthy localhost while requests went to a malformed URL.
pub fn resolve_base_url(configured: Option<&str>) -> &str {
    configured
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or(DEFAULT_URL)
}

/// Watch the backend for as long as the app runs, announcing every change.
///
/// Runs on its own task so a probe against a sleeping machine never delays anything the
/// user is doing, and re-probes on a schedule so features return on their own when the
/// backend comes back, with no restart.
pub fn spawn_poller(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let (status, tracking) = check_now(&app).await;
            tokio::time::sleep(next_probe_interval(&status, tracking)).await;
        }
    });
}

/// Probe once, store the result, and announce it if it changed.
///
/// Returns the new status and whether an Ollama backend is being tracked at all, so a
/// caller can decide when to look again.
pub async fn check_now(app: &tauri::AppHandle) -> (AiStatus, bool) {
    use tauri::{Emitter, Manager};

    let state = app.state::<crate::state::AppState>();

    let (target, generation) = {
        let Ok(config) = state.config.lock() else {
            return (AiStatus::unknown(), false);
        };
        let generation = state
            .ai_health
            .lock()
            .map(|health| health.generation)
            .unwrap_or(0);
        (ollama_target(&config, generation), generation)
    };

    let tracking = target.is_some();
    let status = match target.as_ref() {
        Some(target) => probe(target).await,
        // Not using Ollama: nothing is claimed either way.
        None => AiStatus::unknown(),
    };

    let changed = commit_probe_status(&state.ai_health, generation, status.clone());

    if !changed.committed {
        let current = state
            .ai_health
            .lock()
            .map(|health| health.status.clone())
            .unwrap_or_else(|_| AiStatus::unknown());
        return (current, tracking);
    }

    // Only announce transitions: a poller that emitted every probe would make the UI
    // churn on a status that did not move.
    if changed.changed {
        let _ = app.emit("ai-status-changed", status.clone());
    }

    (status, tracking)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommitOutcome {
    committed: bool,
    changed: bool,
}

fn commit_probe_status(
    current: &Mutex<crate::state::AiHealthState>,
    expected_generation: u64,
    status: AiStatus,
) -> CommitOutcome {
    let Ok(mut current) = current.lock() else {
        return CommitOutcome {
            committed: false,
            changed: false,
        };
    };
    if current.generation != expected_generation {
        return CommitOutcome {
            committed: false,
            changed: false,
        };
    }
    let changed = current.status != status;
    current.status = status;
    CommitOutcome {
        committed: true,
        changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};

    fn one_shot_server(
        status: &str,
        body: &str,
        response_delay: Duration,
    ) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test server address");
        let status = status.to_string();
        let body = body.to_string();
        let (request_tx, request_rx) = mpsc::channel();

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept probe request");
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("bound request read");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = request_tx.send(String::from_utf8_lossy(&request).into_owned());
            std::thread::sleep(response_delay);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });

        (format!("http://{address}"), request_rx)
    }

    async fn probe_response(status: &str, body: &str) -> AiStatus {
        let (endpoint, _) = one_shot_server(status, body, Duration::ZERO);
        let target = OllamaTarget::new(endpoint, "gemma3:4b", None, 7);
        probe(&target).await
    }

    #[test]
    fn an_unprobed_backend_is_unknown_rather_than_unavailable() {
        // Claiming a backend is down before asking would disable features that work.
        let status = AiStatus::unknown();
        assert_eq!(status.availability, Availability::Unknown);
        assert!(status.reason.is_none());
    }

    #[test]
    fn an_available_backend_carries_no_reason() {
        let status = AiStatus::available("http://desktop:11434");
        assert_eq!(status.availability, Availability::Available);
        assert!(status.reason.is_none());
        assert_eq!(status.endpoint.as_deref(), Some("http://desktop:11434"));
    }

    #[test]
    fn an_unavailable_backend_always_says_why() {
        let status = AiStatus::unavailable("http://desktop:11434", "asleep");
        assert_eq!(status.availability, Availability::Unavailable);
        assert_eq!(status.reason.as_deref(), Some("asleep"));
    }

    #[test]
    fn an_unavailable_backend_is_reprobed_sooner_than_an_available_one() {
        // The user is waiting to get features back, so check more eagerly when down.
        let down = next_probe_interval(&AiStatus::unavailable("x", "y"), true);
        let up = next_probe_interval(&AiStatus::available("x"), true);
        assert!(down < up, "expected {down:?} < {up:?}");
    }

    #[test]
    fn an_unknown_backend_is_probed_as_eagerly_as_a_down_one() {
        assert_eq!(
            next_probe_interval(&AiStatus::unknown(), true),
            next_probe_interval(&AiStatus::unavailable("x", "y"), true)
        );
    }

    #[test]
    fn with_no_ollama_backend_to_watch_the_poller_idles() {
        // Nothing to probe, so waking every few seconds would burn power to do nothing.
        let idle = next_probe_interval(&AiStatus::unknown(), false);
        assert!(idle > next_probe_interval(&AiStatus::unknown(), true));
    }

    #[test]
    fn the_probe_url_lists_models_without_running_inference() {
        assert_eq!(
            tags_url("http://desktop:11434"),
            "http://desktop:11434/api/tags"
        );
    }

    #[test]
    fn the_probe_url_tolerates_a_trailing_slash() {
        assert_eq!(
            tags_url("http://desktop:11434/"),
            "http://desktop:11434/api/tags"
        );
    }

    #[test]
    fn a_model_is_found_by_its_bare_name() {
        // Ollama reports "llama3:latest"; the user configured "llama3".
        let tags = json!({"models": [{"name": "llama3:latest"}]});
        assert!(model_installed(&tags, "llama3"));
    }

    #[test]
    fn a_model_is_found_by_its_exact_tagged_name() {
        let tags = json!({"models": [{"name": "llama3:8b"}]});
        assert!(model_installed(&tags, "llama3:8b"));
    }

    #[test]
    fn a_bare_name_does_not_match_a_different_tag() {
        // Ollama resolves "llama3" to "llama3:latest". Having only "llama3:8b" means a
        // request for "llama3" fails, so reporting it present would be a false positive.
        let tags = json!({"models": [{"name": "llama3:8b"}]});
        assert!(!model_installed(&tags, "llama3"));
        assert!(model_installed(&tags, "llama3:8b"));
    }

    #[test]
    fn a_model_that_is_not_installed_is_not_found() {
        let tags = json!({"models": [{"name": "llama3:latest"}]});
        assert!(!model_installed(&tags, "mistral"));
        // A prefix of an installed name is a different model, not a match.
        assert!(!model_installed(&tags, "llama"));
    }

    #[test]
    fn a_response_with_no_models_finds_nothing() {
        assert!(!model_installed(&json!({"models": []}), "llama3"));
        assert!(!model_installed(&json!({}), "llama3"));
        assert!(!model_installed(
            &json!({"models": [{"name": "llama3"}]}),
            ""
        ));
    }

    #[tokio::test]
    async fn ollama_probe_socket_matrix_reports_actionable_results() {
        let healthy = probe_response("200 OK", r#"{"models":[{"name":"gemma3:4b"}]}"#).await;
        assert_eq!(healthy.availability, Availability::Available);

        let missing = probe_response("200 OK", r#"{"models":[]}"#).await;
        assert_eq!(missing.availability, Availability::Unavailable);
        assert!(missing
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("ollama pull gemma3:4b")));

        let malformed = probe_response("200 OK", "not-json").await;
        assert!(malformed
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("not Ollama's model list")));

        let server_error = probe_response("500 Internal Server Error", r#"{"error":"boom"}"#).await;
        assert!(server_error
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("returned 500")));

        let unauthorized = probe_response("401 Unauthorized", r#"{"error":"unauthorized"}"#).await;
        assert!(unauthorized
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("returned 401")));

        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve refused port");
        let refused_endpoint = format!(
            "http://{}",
            listener.local_addr().expect("read refused address")
        );
        drop(listener);
        let refused_target = OllamaTarget::new(refused_endpoint, "gemma3:4b", None, 7);
        let refused = probe_with_timeout(&refused_target, Duration::from_millis(200)).await;
        assert!(refused
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Could not connect")));

        let (stalled_endpoint, _) = one_shot_server(
            "200 OK",
            r#"{"models":[{"name":"gemma3:4b"}]}"#,
            Duration::from_millis(100),
        );
        let stalled_target = OllamaTarget::new(stalled_endpoint, "gemma3:4b", None, 7);
        let stalled = probe_with_timeout(&stalled_target, Duration::from_millis(20)).await;
        assert!(stalled
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("did not answer")));
    }

    #[tokio::test]
    async fn authenticated_ollama_health_and_connection_checks_send_bearer_token() {
        let body = r#"{"models":[{"name":"gemma3:4b"}]}"#;

        let (health_endpoint, health_request) = one_shot_server("200 OK", body, Duration::ZERO);
        let health_target = OllamaTarget::new(
            health_endpoint,
            "gemma3:4b",
            Some("health-secret".to_string()),
            1,
        );
        assert_eq!(
            probe(&health_target).await.availability,
            Availability::Available
        );
        assert!(health_request
            .recv_timeout(Duration::from_secs(1))
            .expect("health request")
            .to_ascii_lowercase()
            .contains("authorization: bearer health-secret"));

        let (connection_endpoint, connection_request) =
            one_shot_server("200 OK", body, Duration::ZERO);
        crate::ai::test_connection(
            "ollama",
            "connection-secret",
            "gemma3:4b",
            Some(&connection_endpoint),
        )
        .await
        .expect("connection check should succeed");
        assert!(connection_request
            .recv_timeout(Duration::from_secs(1))
            .expect("connection request")
            .to_ascii_lowercase()
            .contains("authorization: bearer connection-secret"));

        let (blank_endpoint, blank_request) = one_shot_server("200 OK", body, Duration::ZERO);
        let blank_target =
            OllamaTarget::new(blank_endpoint, "gemma3:4b", Some("   ".to_string()), 1);
        assert_eq!(
            probe(&blank_target).await.availability,
            Availability::Available
        );
        assert!(!blank_request
            .recv_timeout(Duration::from_secs(1))
            .expect("blank-key request")
            .to_ascii_lowercase()
            .contains("authorization:"));
    }

    #[test]
    fn ai_status_generation_rejects_stale_probe_results() {
        let old_target = OllamaTarget::new("http://old", "old-model", None, 1);
        let new_target = OllamaTarget::new("http://new", "new-model", None, 2);
        let health = Mutex::new(crate::state::AiHealthState {
            generation: 2,
            status: AiStatus::unknown_for(&new_target),
        });

        let current = commit_probe_status(
            &health,
            2,
            AiStatus::unavailable_for(&new_target, "new target is asleep"),
        );
        let stale = commit_probe_status(&health, 1, AiStatus::available_for(&old_target));

        assert!(current.committed);
        assert!(current.changed);
        assert!(!stale.committed);
        let final_health = health.lock().expect("read final health");
        assert_eq!(final_health.generation, 2);
        assert_eq!(final_health.status.availability, Availability::Unavailable);
        assert!(final_health.status.belongs_to(new_target.id()));
    }

    #[test]
    fn credential_changes_create_a_new_exact_health_target() {
        let old_target = OllamaTarget::new(
            "http://desktop:11434",
            "gemma3:4b",
            Some("old-secret".to_string()),
            1,
        );
        let new_target = OllamaTarget::new(
            "http://desktop:11434",
            "gemma3:4b",
            Some("new-secret".to_string()),
            2,
        );
        let stale_status = AiStatus::unavailable_for(&old_target, "old target unavailable");

        assert!(!old_target.same_settings(&new_target));
        assert!(!stale_status.belongs_to(new_target.id()));
    }
}
