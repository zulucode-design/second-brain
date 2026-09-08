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

use crate::ai_provider::{AiBackendTarget, AiTargetId, ConfiguredAiProvider, ProbeProtocol};

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

/// Gap between checks when there is no probeable backend to watch at all.
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

    pub fn unknown_for(target: &AiBackendTarget) -> Self {
        Self {
            availability: Availability::Unknown,
            reason: None,
            endpoint: Some(target.id().endpoint.clone()),
            target: Some(target.id().clone()),
        }
    }

    fn available_for(target: &AiBackendTarget) -> Self {
        Self {
            availability: Availability::Available,
            reason: None,
            endpoint: Some(target.id().endpoint.clone()),
            target: Some(target.id().clone()),
        }
    }

    fn unavailable_for(target: &AiBackendTarget, reason: impl Into<String>) -> Self {
        Self {
            availability: Availability::Unavailable,
            reason: Some(reason.into()),
            endpoint: Some(target.id().endpoint.clone()),
            target: Some(target.id().clone()),
        }
    }

    pub fn belongs_to(&self, target: &AiTargetId) -> bool {
        self.target.as_ref() == Some(target)
    }
}

/// Return the already-known failure only when it belongs to the backend configured now.
///
/// A result from an earlier generation must never disable a newly selected endpoint,
/// model, credential, or provider protocol while its first probe is still in flight.
pub fn known_unavailability(
    status: &AiStatus,
    current_target: Option<&AiTargetId>,
) -> Option<String> {
    if status.availability != Availability::Unavailable
        || current_target.is_none_or(|target| !status.belongs_to(target))
    {
        return None;
    }
    Some(
        status
            .reason
            .clone()
            .unwrap_or_else(|| "The AI backend is unreachable.".to_string()),
    )
}

/// How long to wait before probing again.
///
/// `tracking` is false when the configured provider cannot be probed, in which case the
/// poller should idle rather than wake every few seconds to do nothing.
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

fn describe_probe_failure(target: &AiBackendTarget, error: &reqwest::Error) -> String {
    if target.protocol() == ProbeProtocol::Ollama {
        return describe_failure(&target.id().endpoint, error);
    }
    let endpoint = &target.id().endpoint;
    if error.is_timeout() {
        return format!(
            "{endpoint} did not answer within {}s. The machine running the OpenAI-compatible backend may be asleep, or the private network may be down.",
            PROBE_TIMEOUT.as_secs()
        );
    }
    if error.is_connect() {
        return format!(
            "Could not connect to {endpoint}. Check that the OpenAI-compatible backend is running on that machine and that both machines are on the same private network."
        );
    }
    format!("Could not reach the OpenAI-compatible backend at {endpoint}: {error}")
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

    tags_response
        .get("models")
        .and_then(|m| m.as_array())
        .is_some_and(|models| {
            models.iter().any(|entry| {
                entry
                    .get("name")
                    .and_then(|n| n.as_str())
                    .is_some_and(|name| model_name_matches(name, wanted))
            })
        })
}

fn model_name_matches(available: &str, configured: &str) -> bool {
    available == configured
        || (!configured.contains(':') && available == format!("{configured}:latest"))
}

/// The URL that lists installed models, used as the reachability probe.
///
/// Cheaper and more telling than a chat request: it confirms the host answers, that what
/// answered is Ollama, and which models it actually has, without running inference.
pub fn tags_url(base_url: &str) -> String {
    format!("{}/api/tags", base_url.trim_end_matches('/'))
}

fn models_url(target: &AiBackendTarget) -> String {
    match target.protocol() {
        ProbeProtocol::Ollama => tags_url(&target.id().endpoint),
        ProbeProtocol::OpenAiCompatible => {
            format!("{}/v1/models", target.id().endpoint.trim_end_matches('/'))
        }
    }
}

fn configured_model_is_available(target: &AiBackendTarget, response: &serde_json::Value) -> bool {
    match target.protocol() {
        ProbeProtocol::Ollama => model_installed(response, &target.id().model),
        ProbeProtocol::OpenAiCompatible => response
            .get("data")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|models| {
                models.iter().any(|entry| {
                    entry
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|available| model_name_matches(available, &target.id().model))
                })
            }),
    }
}

fn unexpected_backend(target: &AiBackendTarget, detail: &str) -> String {
    match target.protocol() {
        ProbeProtocol::Ollama => not_ollama(&target.id().endpoint, detail),
        ProbeProtocol::OpenAiCompatible => format!(
            "{} answered, but {detail}. Check that this address exposes an OpenAI-compatible API.",
            target.id().endpoint
        ),
    }
}

/// Ask the backend whether it is there and has the model, without running inference.
///
/// Every outcome is a status rather than an error, because an unreachable backend is an
/// ordinary condition here, not a failure of the app.
pub async fn probe(target: &AiBackendTarget) -> AiStatus {
    probe_with_timeout(target, PROBE_TIMEOUT).await
}

async fn probe_with_timeout(target: &AiBackendTarget, timeout: Duration) -> AiStatus {
    let base_url = target.id().endpoint.as_str();
    let model = target.id().model.as_str();
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(client) => client,
        Err(e) => {
            return AiStatus::unavailable_for(target, format!("Could not start a check: {e}"))
        }
    };

    let mut request = client.get(models_url(target));
    if let Some(token) = target.bearer_token() {
        request = request.bearer_auth(token);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(e) => {
            return AiStatus::unavailable_for(target, describe_probe_failure(target, &e));
        }
    };

    if !response.status().is_success() {
        return AiStatus::unavailable_for(
            target,
            unexpected_backend(target, &format!("returned {}", response.status())),
        );
    }

    let models: serde_json::Value = match response.json().await {
        Ok(models) => models,
        Err(_) => {
            let detail = match target.protocol() {
                ProbeProtocol::Ollama => "its reply was not Ollama's model list",
                ProbeProtocol::OpenAiCompatible => {
                    "its reply was not a valid OpenAI-compatible model list"
                }
            };
            return AiStatus::unavailable_for(target, unexpected_backend(target, detail));
        }
    };

    // Reachable but missing the model is still unavailable, and the fix is specific.
    if !model.trim().is_empty() && !configured_model_is_available(target, &models) {
        let reason = match target.protocol() {
            ProbeProtocol::Ollama => format!(
                "Ollama is running at {base_url} but does not have \"{model}\". Install it on that machine with: ollama pull {model}"
            ),
            ProbeProtocol::OpenAiCompatible => format!(
                "The OpenAI-compatible backend at {base_url} does not list the configured model \"{model}\". Check the model name and backend configuration."
            ),
        };
        return AiStatus::unavailable_for(target, reason);
    }

    AiStatus::available_for(target)
}

/// The endpoint and model to probe, or `None` when the configured provider is not probeable.
///
/// Cloud providers are not tracked: their reachability is the user's internet connection,
/// which the app cannot usefully report on, and a wrong claim would disable working
/// features.
pub fn health_target(config: &crate::types::AppConfig, generation: u64) -> Option<AiBackendTarget> {
    ConfiguredAiProvider::from_config(config).health_target(generation)
}

pub fn same_probe_settings(
    left: Option<&AiBackendTarget>,
    right: Option<&AiBackendTarget>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_settings(right),
        (None, None) => true,
        _ => false,
    }
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
        (health_target(&config, generation), generation)
    };

    let tracking = target.is_some();
    let status = match target.as_ref() {
        Some(target) => probe(target).await,
        // No probeable backend: nothing is claimed either way.
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
    use crate::ai_provider::ConfiguredAiProvider;
    use crate::types::{AiProvider, AppConfig};
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};

    fn ollama_test_target(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        bearer_token: Option<String>,
        generation: u64,
    ) -> AiBackendTarget {
        AiBackendTarget::new(
            endpoint,
            model,
            ProbeProtocol::Ollama,
            bearer_token,
            generation,
        )
    }

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
        let target = ollama_test_target(endpoint, "gemma3:4b", None, 7);
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
        let refused_target = ollama_test_target(refused_endpoint, "gemma3:4b", None, 7);
        let refused = probe_with_timeout(&refused_target, Duration::from_millis(200)).await;
        let refused_reason = refused
            .reason
            .as_deref()
            .expect("a refused connection should explain why the probe failed");
        let timed_out_prefix = format!("{} did not answer within ", refused_target.id().endpoint);
        assert!(
            refused_reason.starts_with("Could not connect to ")
                || refused_reason.starts_with("Could not reach ")
                || refused_reason.starts_with(&timed_out_prefix),
            "unexpected refused-connection guidance: {refused_reason}"
        );
        assert!(
            refused_reason.contains(refused_target.id().endpoint.as_str()),
            "refused-connection guidance should identify the attempted endpoint: {refused_reason}"
        );

        let (stalled_endpoint, _) = one_shot_server(
            "200 OK",
            r#"{"models":[{"name":"gemma3:4b"}]}"#,
            Duration::from_millis(100),
        );
        let stalled_target = ollama_test_target(stalled_endpoint, "gemma3:4b", None, 7);
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
        let health_target = ollama_test_target(
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
        let connection_config = AppConfig {
            ai_provider: Some(AiProvider::Ollama),
            ai_model: "gemma3:4b".to_string(),
            ollama_base_url: Some(connection_endpoint),
            ollama_api_key: Some("connection-secret".to_string()),
            ..AppConfig::default()
        };
        let configured = ConfiguredAiProvider::from_config(&connection_config);
        let settings = configured.request_settings().expect("request settings");
        let connection_target = configured.health_target(1).expect("health target");
        crate::ai::test_connection(&settings, Some(&connection_target))
            .await
            .expect("connection check should succeed");
        assert!(connection_request
            .recv_timeout(Duration::from_secs(1))
            .expect("connection request")
            .to_ascii_lowercase()
            .contains("authorization: bearer connection-secret"));

        let (blank_endpoint, blank_request) = one_shot_server("200 OK", body, Duration::ZERO);
        let blank_target =
            ollama_test_target(blank_endpoint, "gemma3:4b", Some("   ".to_string()), 1);
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

    #[tokio::test]
    async fn an_openai_compatible_local_server_is_probed_through_its_public_interface() {
        let body = r#"{"object":"list","data":[{"id":"gemma3:latest"}]}"#;
        let (endpoint, request) = one_shot_server("200 OK", body, Duration::ZERO);
        let config = AppConfig {
            ai_provider: Some(AiProvider::OpenAiCompatible),
            ai_model: "gemma3".to_string(),
            openai_compatible_base_url: Some(format!("{endpoint}/v1")),
            openai_compatible_api_key: Some("compatible-secret".to_string()),
            ..AppConfig::default()
        };
        let target = ConfiguredAiProvider::from_config(&config)
            .health_target(4)
            .expect("configured compatible backend");

        let status = probe(&target).await;

        assert_eq!(status.availability, Availability::Available);
        assert!(status.belongs_to(target.id()));
        let request = request
            .recv_timeout(Duration::from_secs(1))
            .expect("health request");
        assert!(request.starts_with("GET /v1/models HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer compatible-secret"));
    }

    #[tokio::test]
    async fn openai_compatible_connection_failures_name_the_configured_backend() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve refused port");
        let endpoint = format!("http://{}", listener.local_addr().expect("read address"));
        drop(listener);
        let target = AiBackendTarget::new(
            endpoint,
            "gemma3:4b",
            ProbeProtocol::OpenAiCompatible,
            None,
            1,
        );

        let status = probe_with_timeout(&target, Duration::from_millis(200)).await;
        let reason = status.reason.expect("failed probe guidance");

        assert!(reason.contains("OpenAI-compatible"), "{reason}");
        assert!(!reason.contains("Ollama"), "{reason}");
    }

    #[test]
    fn ai_status_generation_rejects_stale_probe_results() {
        let old_target = ollama_test_target("http://old", "old-model", None, 1);
        let new_target = ollama_test_target("http://new", "new-model", None, 2);
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
        let old_target = ollama_test_target(
            "http://desktop:11434",
            "gemma3:4b",
            Some("old-secret".to_string()),
            1,
        );
        let new_target = ollama_test_target(
            "http://desktop:11434",
            "gemma3:4b",
            Some("new-secret".to_string()),
            2,
        );
        let stale_status = AiStatus::unavailable_for(&old_target, "old target unavailable");

        assert!(!old_target.same_settings(&new_target));
        assert!(!stale_status.belongs_to(new_target.id()));
    }

    #[test]
    fn switching_provider_protocol_invalidates_the_tracked_target() {
        let ollama = AiBackendTarget::new(
            "http://desktop:11434",
            "gemma3:4b",
            ProbeProtocol::Ollama,
            None,
            1,
        );
        let compatible = AiBackendTarget::new(
            "http://desktop:11434",
            "gemma3:4b",
            ProbeProtocol::OpenAiCompatible,
            None,
            1,
        );

        assert!(!same_probe_settings(Some(&ollama), Some(&compatible)));
    }

    #[test]
    fn fast_refusal_applies_only_to_the_exact_current_target() {
        let current = AiBackendTarget::new(
            "http://desktop:11434",
            "gemma3:4b",
            ProbeProtocol::OpenAiCompatible,
            None,
            2,
        );
        let stale = AiBackendTarget::new(
            "http://desktop:11434",
            "gemma3:4b",
            ProbeProtocol::OpenAiCompatible,
            None,
            1,
        );
        let status = AiStatus::unavailable_for(&current, "backend is asleep");

        assert_eq!(
            known_unavailability(&status, Some(current.id())).as_deref(),
            Some("backend is asleep")
        );
        assert_eq!(known_unavailability(&status, Some(stale.id())), None);
        assert_eq!(known_unavailability(&status, None), None);
        assert_eq!(
            known_unavailability(&AiStatus::unknown_for(&current), Some(current.id())),
            None
        );
    }
}
