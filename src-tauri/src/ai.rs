use reqwest::Client;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

use crate::ai_provider::{AiApiProtocol, AiBackendTarget, AiRequestSettings};
use crate::types::AiStreamEvent;

/// A client that gives up rather than waiting indefinitely.
///
/// The backend is often a machine that may be asleep or off the network. Without a bound
/// on connection setup, a request to one can hang for the OS timeout, measured in minutes,
/// with the user watching a spinner. Only connection setup is bounded, not the whole
/// request: a real answer legitimately takes a while to stream.
fn client() -> Client {
    Client::builder()
        .connect_timeout(crate::ai_health::REQUEST_CONNECT_TIMEOUT)
        .read_timeout(crate::ai_health::REQUEST_STALL_TIMEOUT)
        .build()
        .unwrap_or_else(|_| Client::new())
}

/// What a provider stream reports while it runs. `ai_request` tags each with its request id,
/// so two streams running at once (Ask and an editor action) never mix.
#[derive(Debug, PartialEq)]
enum StreamEvent {
    Text(String),
    /// The model started reasoning before it answers. Reported once; the reasoning itself is
    /// not forwarded.
    Thinking,
}

/// A Stop request for one stream. The flag covers a Stop that lands before the stream starts;
/// the notification ends one that is already waiting on the network.
struct Stop {
    cancelled: AtomicBool,
    notify: Notify,
}

/// Requests a Stop can still reach, from registration until their stream ends.
static RUNNING: LazyLock<Mutex<HashMap<String, Arc<Stop>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Makes `request_id` stoppable before its stream exists, so a Stop during Ask's retrieval
/// prevents the model call. Dropping it unregisters, unless `start` took it over.
pub struct Registration {
    request_id: String,
    stop: Arc<Stop>,
    started: bool,
}

impl Registration {
    pub fn is_cancelled(&self) -> bool {
        self.stop.cancelled.load(Ordering::SeqCst)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if !self.started {
            unregister(&self.request_id);
        }
    }
}

pub fn register(request_id: &str) -> Registration {
    let stop = Arc::new(Stop {
        cancelled: AtomicBool::new(false),
        notify: Notify::new(),
    });
    if let Ok(mut running) = RUNNING.lock() {
        running.insert(request_id.to_string(), stop.clone());
    }
    Registration {
        request_id: request_id.to_string(),
        stop,
        started: false,
    }
}

fn unregister(request_id: &str) {
    if let Ok(mut running) = RUNNING.lock() {
        running.remove(request_id);
    }
}

/// Stops `request_id`, whether its stream is running or not started yet. A stopped stream
/// emits nothing afterwards.
pub fn cancel(request_id: &str) {
    if let Some(stop) = RUNNING
        .lock()
        .ok()
        .and_then(|running| running.get(request_id).cloned())
    {
        stop.cancelled.store(true, Ordering::SeqCst);
        stop.notify.notify_one();
    }
}

pub fn ai_request<R: tauri::Runtime>(
    app: AppHandle<R>,
    settings: AiRequestSettings,
    system_prompt: String,
    user_message: String,
    request_id: String,
) {
    start(
        app,
        settings,
        system_prompt,
        user_message,
        register(&request_id),
    );
}

/// Streams the answer for an already registered request, tagging every event with its id.
pub fn start<R: tauri::Runtime>(
    app: AppHandle<R>,
    settings: AiRequestSettings,
    system_prompt: String,
    user_message: String,
    mut registration: Registration,
) {
    registration.started = true;
    let request_id = registration.request_id.clone();
    let stop = registration.stop.clone();
    std::thread::spawn(move || {
        if stop.cancelled.load(Ordering::SeqCst) {
            unregister(&request_id);
            return;
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        let emit = |event_type: &str, text: Option<String>, error: Option<String>| {
            let _ = app.emit(
                crate::events::AI_STREAM,
                AiStreamEvent {
                    request_id: request_id.clone(),
                    event_type: event_type.to_string(),
                    text,
                    error,
                },
            );
        };
        let on_event = |event: StreamEvent| match event {
            StreamEvent::Text(text) => emit("text", Some(text), None),
            StreamEvent::Thinking => emit("thinking", None, None),
        };
        let finished = rt.block_on(async {
            tokio::select! {
                result = stream(&settings, &system_prompt, &user_message, &on_event) => Some(result),
                _ = stop.notify.notified() => None,
            }
        });
        unregister(&request_id);
        match finished {
            Some(Ok(())) => emit("done", None, None),
            Some(Err(error)) => emit("error", None, Some(error)),
            None => {}
        }
    });
}

async fn stream(
    settings: &AiRequestSettings,
    system_prompt: &str,
    user_message: &str,
    on_event: &(dyn Fn(StreamEvent) + Sync),
) -> Result<(), String> {
    // Handle all API keys as optional; ollama and v1 completions doesnt always require it.
    match settings.protocol() {
        AiApiProtocol::OpenAiChatCompletions => {
            stream_openai(
                settings.endpoint(),
                settings.api_key(),
                settings.model(),
                system_prompt,
                user_message,
                on_event,
            )
            .await
        }
        AiApiProtocol::AnthropicMessages => {
            stream_anthropic(
                settings.endpoint(),
                settings.api_key().unwrap_or_default(),
                settings.model(),
                system_prompt,
                user_message,
                on_event,
            )
            .await
        }
    }
}

/// Splits a server-sent-events body into its `data:` payloads as chunks arrive. `handle`
/// returns false once the stream has said it is finished. A body that ends before saying so
/// is an error: a dropped connection must not pass a cut-off answer off as complete.
async fn read_sse(
    response: reqwest::Response,
    mut handle: impl FnMut(&str) -> Result<bool, String>,
) -> Result<(), String> {
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    // Bytes, not text: a network chunk can end inside a multi-byte character, so only a
    // complete record is decoded.
    let mut buffer: Vec<u8> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        // SSE allows CRLF line endings. JSON escapes every carriage return inside a string,
        // so dropping them leaves only LF-delimited records.
        buffer.extend(chunk.iter().filter(|byte| **byte != b'\r'));

        while let Some(event_end) = buffer.windows(2).position(|pair| pair == b"\n\n") {
            let record: Vec<u8> = buffer.drain(..event_end + 2).collect();
            let event_str = String::from_utf8_lossy(&record[..event_end]);

            for line in event_str.lines() {
                // The space after the colon is optional.
                if let Some(data) = line.strip_prefix("data:") {
                    let data = data.strip_prefix(' ').unwrap_or(data);
                    if data == "[DONE]" || !handle(data)? {
                        return Ok(());
                    }
                }
            }
        }
    }
    Err("The connection closed before the answer finished.".to_string())
}

async fn stream_anthropic(
    endpoint: &str,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    on_event: &(dyn Fn(StreamEvent) + Sync),
) -> Result<(), String> {
    let client = client();

    let body = json!({
        "model": model,
        "max_tokens": 4096,
        "stream": true,
        "system": system_prompt,
        "messages": [
            {
                "role": "user",
                "content": user_message
            }
        ]
    });

    let response = client
        .post(endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        return Err(format!("API error {}: {}", status, body_text));
    }

    read_sse(response, |data| {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
            return Ok(true);
        };
        match parsed["type"].as_str().unwrap_or("") {
            "content_block_start" if parsed["content_block"]["type"] == "thinking" => {
                on_event(StreamEvent::Thinking);
            }
            "content_block_delta" => {
                if let Some(text) = parsed["delta"]["text"].as_str() {
                    on_event(StreamEvent::Text(text.to_string()));
                }
            }
            "message_stop" => return Ok(false),
            "error" => {
                return Err(parsed["error"]["message"]
                    .as_str()
                    .unwrap_or("Unknown API error")
                    .to_string());
            }
            _ => {}
        }
        Ok(true)
    })
    .await
}

async fn stream_openai(
    url: &str,
    api_key: Option<&str>,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    on_event: &(dyn Fn(StreamEvent) + Sync),
) -> Result<(), String> {
    let client = client();

    let is_gpt5 = model.starts_with("gpt-5");
    let token_key = if is_gpt5 {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };

    let mut body = json!({
        "model": model,
        "stream": true,
        token_key: 4096,
        "messages": [
            {
                "role": "system",
                "content": system_prompt
            },
            {
                "role": "user",
                "content": user_message
            }
        ]
    });

    // GPT-5 models don't support temperature
    if !is_gpt5 {
        body["temperature"] = json!(0.7);
    }

    let mut req = client.post(url).header("content-type", "application/json");

    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {}", key));
    }

    let response = req
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        return Err(format!("API error {}: {}", status, body_text));
    }

    let mut thinking = false;
    read_sse(response, |data| {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
            return Ok(true);
        };
        let delta = &parsed["choices"][0]["delta"];
        // Ollama reports reasoning as `reasoning`; DeepSeek-style servers as
        // `reasoning_content`.
        let reasoning = ["reasoning", "reasoning_content"]
            .iter()
            .any(|key| delta[key].as_str().is_some_and(|text| !text.is_empty()));
        if reasoning && !thinking {
            thinking = true;
            on_event(StreamEvent::Thinking);
        }
        if let Some(content) = delta["content"].as_str() {
            if !content.is_empty() {
                on_event(StreamEvent::Text(content.to_string()));
            }
        }
        if let Some(reason) = parsed["choices"][0]["finish_reason"].as_str() {
            if reason == "stop" || reason == "length" {
                return Ok(false);
            }
        }
        if let Some(err) = parsed["error"]["message"].as_str() {
            return Err(err.to_string());
        }
        Ok(true)
    })
    .await
}

/// What Ollama uses for a local model when nothing sets a context size: its default on a GPU
/// under 24 GiB, the smallest it documents. Assuming more would let Ollama silently cut the
/// prompt.
const OLLAMA_LOCAL_DEFAULT_CONTEXT: usize = 4096;

/// The context window, in tokens, that Ollama will actually give `model`, or `None` when it
/// cannot say.
///
/// A model's capacity (`/api/show`) is not what a local server allocates: Ollama runs local
/// models at a smaller default unless a Modelfile or the server sets more, and the
/// OpenAI-compatible endpoint cannot ask for more. Cloud models run at full capacity.
pub async fn ollama_context_window(
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
) -> Option<usize> {
    let base_url = base_url.trim_end_matches('/');
    let show = ollama_get_json(
        client()
            .post(format!("{base_url}/api/show"))
            .json(&json!({ "model": model })),
        api_key,
    )
    .await?;
    // The key is prefixed with the model's architecture, as in `llama.context_length`.
    let capacity = show["model_info"]
        .as_object()
        .and_then(|info| {
            info.iter()
                .find(|(key, _)| key.ends_with(".context_length"))
                .and_then(|(_, value)| value.as_u64())
        })
        .map(|tokens| tokens as usize);
    let cloud = model.ends_with("cloud")
        || show.get("remote_host").is_some()
        || reqwest::Url::parse(base_url)
            .ok()
            .and_then(|url| url.host_str().map(|host| host.ends_with("ollama.com")))
            .unwrap_or(false);
    if cloud {
        return capacity;
    }
    let allocated = match modelfile_context(&show) {
        Some(tokens) => tokens,
        None => loaded_context(base_url, api_key, model)
            .await
            .unwrap_or(OLLAMA_LOCAL_DEFAULT_CONTEXT),
    };
    Some(capacity.map_or(allocated, |capacity| capacity.min(allocated)))
}

async fn ollama_get_json(
    request: reqwest::RequestBuilder,
    api_key: Option<&str>,
) -> Option<serde_json::Value> {
    let request = match api_key {
        Some(key) => request.bearer_auth(key),
        None => request,
    };
    let response = request.send().await.ok()?.error_for_status().ok()?;
    response.json().await.ok()
}

/// A `num_ctx` the model's Modelfile sets, from `/api/show`'s `parameters` text.
fn modelfile_context(show: &serde_json::Value) -> Option<usize> {
    show["parameters"].as_str()?.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next() == Some("num_ctx"))
            .then(|| fields.next()?.parse().ok())
            .flatten()
    })
}

/// The context a running server gave `model`, when it is loaded (`/api/ps`). This is how a
/// server-wide `OLLAMA_CONTEXT_LENGTH` shows up.
async fn loaded_context(base_url: &str, api_key: Option<&str>, model: &str) -> Option<usize> {
    let running = ollama_get_json(client().get(format!("{base_url}/api/ps")), api_key).await?;
    running["models"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == model || entry["model"] == model)
        .and_then(|entry| entry["context_length"].as_u64())
        .map(|tokens| tokens as usize)
}

pub async fn test_connection(
    settings: &AiRequestSettings,
    health_target: Option<&AiBackendTarget>,
) -> Result<String, String> {
    if let Some(target) = health_target {
        let status = crate::ai_health::probe(target).await;
        return match status.reason {
            Some(reason) => Err(reason),
            None => Ok(format!("Connected at {}", target.id().endpoint)),
        };
    }

    match settings.protocol() {
        AiApiProtocol::OpenAiChatCompletions => {
            test_openai(settings.endpoint(), settings.api_key(), settings.model()).await
        }
        AiApiProtocol::AnthropicMessages => {
            test_anthropic(
                settings.endpoint(),
                settings.api_key().unwrap_or_default(),
                settings.model(),
            )
            .await
        }
    }
}

async fn test_anthropic(endpoint: &str, api_key: &str, model: &str) -> Result<String, String> {
    let client = client();

    let body = json!({
        "model": model,
        "max_tokens": 20,
        "messages": [
            {
                "role": "user",
                "content": "Hi"
            }
        ]
    });

    let response = client
        .post(endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Connection failed: {}", e))?;

    if response.status().is_success() {
        Ok("Connection successful".to_string())
    } else {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        Err(format!("API error {}: {}", status, body_text))
    }
}

async fn test_openai(url: &str, api_key: Option<&str>, model: &str) -> Result<String, String> {
    let client = client();
    let is_gpt5 = model.starts_with("gpt-5");
    let token_key = if is_gpt5 {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };

    let body = json!({
        "model": model,
        token_key: 20,
        "messages": [
            {
                "role": "user",
                "content": "Hi"
            }
        ]
    });

    let mut req = client.post(url).header("content-type", "application/json");

    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {}", key));
    }

    let response = req
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Connection failed: {}", e))?;

    if response.status().is_success() {
        Ok("Connection successful".to_string())
    } else {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        Err(format!("API error {}: {}", status, body_text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;
    use tauri::Listener;

    /// Answers one request with `body` as a server-sent-events stream. With `hold_open`, the
    /// connection stays open after the body, like a model that is still thinking.
    fn sse_server(body: &'static str, hold_open: bool) -> String {
        sse_server_in_parts(vec![body.as_bytes()], hold_open, None)
    }

    /// Like `sse_server`, writing each part as its own network chunk. `requested` reports
    /// whether a request ever arrived.
    fn sse_server_in_parts(
        parts: Vec<&'static [u8]>,
        hold_open: bool,
        requested: Option<mpsc::Sender<()>>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test server address");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            if let Some(requested) = requested {
                let _ = requested.send(());
            }
            stream
                .set_read_timeout(Some(Duration::from_millis(200)))
                .expect("bound request read");
            let mut buffer = [0_u8; 8192];
            while stream.read(&mut buffer).unwrap_or(0) == buffer.len() {}
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes());
            for part in parts {
                let _ = stream.write_all(part);
                let _ = stream.flush();
                std::thread::sleep(Duration::from_millis(50));
            }
            if hold_open {
                std::thread::sleep(Duration::from_secs(30));
            }
        });
        format!("http://{address}/v1/chat/completions")
    }

    fn settings(endpoint: String) -> AiRequestSettings {
        let config = crate::types::AppConfig {
            ai_provider: Some(crate::types::AiProvider::OpenAiCompatible),
            openai_compatible_base_url: Some(
                endpoint
                    .trim_end_matches("/v1/chat/completions")
                    .to_string(),
            ),
            ai_model: "test-model".to_string(),
            ..crate::types::AppConfig::default()
        };
        crate::ai_provider::ConfiguredAiProvider::from_config(&config)
            .request_settings()
            .expect("test settings")
    }

    fn collect(app: &tauri::App<tauri::test::MockRuntime>) -> mpsc::Receiver<AiStreamEvent> {
        let (sender, receiver) = mpsc::channel();
        app.listen(crate::events::AI_STREAM, move |event| {
            let parsed: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            let _ = sender.send(AiStreamEvent {
                request_id: parsed["request_id"].as_str().unwrap().to_string(),
                event_type: parsed["event_type"].as_str().unwrap().to_string(),
                text: parsed["text"].as_str().map(str::to_string),
                error: parsed["error"].as_str().map(str::to_string),
            });
        });
        receiver
    }

    #[test]
    fn an_openai_stream_reports_reasoning_once_then_the_answer() {
        let url = sse_server(
            "data: {\"choices\":[{\"delta\":{\"reasoning\":\"Let me\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"reasoning\":\" think\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"Coffee [1]\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            false,
        );
        let events = Mutex::new(Vec::new());
        let on_event = |event: StreamEvent| events.lock().unwrap().push(event);
        let runtime = tokio::runtime::Runtime::new().unwrap();

        runtime
            .block_on(stream_openai(&url, None, "m", "system", "user", &on_event))
            .unwrap();

        assert_eq!(
            events.into_inner().unwrap(),
            vec![
                StreamEvent::Thinking,
                StreamEvent::Text("Coffee [1]".to_string())
            ]
        );
    }

    #[test]
    fn an_error_inside_the_stream_is_returned_once() {
        let url = sse_server(
            "data: {\"error\":{\"message\":\"quota exceeded\"}}\n\n",
            false,
        );
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let result = runtime.block_on(stream_openai(&url, None, "m", "s", "u", &|_| {}));

        assert_eq!(result, Err("quota exceeded".to_string()));
    }

    #[test]
    fn concurrent_requests_tag_every_event_with_their_own_id() {
        let app = tauri::test::mock_app();
        let events = collect(&app);
        let first = sse_server("data: {\"choices\":[{\"delta\":{\"content\":\"one\"},\"finish_reason\":\"stop\"}]}\n\n", false);
        let second = sse_server("data: {\"choices\":[{\"delta\":{\"content\":\"two\"},\"finish_reason\":\"stop\"}]}\n\n", false);

        ai_request(
            app.handle().clone(),
            settings(first),
            "s".into(),
            "u".into(),
            "ask-1".into(),
        );
        ai_request(
            app.handle().clone(),
            settings(second),
            "s".into(),
            "u".into(),
            "editor-2".into(),
        );

        let mut received: Vec<(String, String, Option<String>)> = (0..4)
            .map(|_| {
                events
                    .recv_timeout(Duration::from_secs(10))
                    .expect("stream event")
            })
            .map(|event| (event.request_id, event.event_type, event.text))
            .collect();
        received.sort();
        assert_eq!(
            received,
            vec![
                ("ask-1".into(), "done".into(), None),
                ("ask-1".into(), "text".into(), Some("one".into())),
                ("editor-2".into(), "done".into(), None),
                ("editor-2".into(), "text".into(), Some("two".into())),
            ]
        );
    }

    #[test]
    fn a_cancelled_request_stops_without_reporting_done_or_error() {
        let app = tauri::test::mock_app();
        let events = collect(&app);
        let url = sse_server(
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
            true,
        );

        ai_request(
            app.handle().clone(),
            settings(url),
            "s".into(),
            "u".into(),
            "stop-me".into(),
        );
        let first = events
            .recv_timeout(Duration::from_secs(10))
            .expect("first chunk");
        assert_eq!(first.text.as_deref(), Some("partial"));
        cancel("stop-me");

        // The server keeps the stream open for 30 s, so only the cancel can end it this soon.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while RUNNING.lock().unwrap().contains_key("stop-me") {
            assert!(
                std::time::Instant::now() < deadline,
                "the stream should stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(events.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn a_character_split_across_network_chunks_arrives_intact() {
        let record = "data: {\"choices\":[{\"delta\":{\"content\":\"café\"},\"finish_reason\":\"stop\"}]}\n\n"
            .as_bytes();
        // Split inside the two bytes of "é".
        let split = record.iter().position(|byte| *byte == 0xc3).unwrap() + 1;
        let url = sse_server_in_parts(vec![&record[..split], &record[split..]], false, None);
        let events = Mutex::new(Vec::new());
        let on_event = |event: StreamEvent| events.lock().unwrap().push(event);
        let runtime = tokio::runtime::Runtime::new().unwrap();

        runtime
            .block_on(stream_openai(&url, None, "m", "s", "u", &on_event))
            .unwrap();

        assert_eq!(
            events.into_inner().unwrap(),
            vec![StreamEvent::Text("café".to_string())]
        );
    }

    #[test]
    fn a_request_stopped_before_it_starts_never_reaches_the_model() {
        let app = tauri::test::mock_app();
        let events = collect(&app);
        let (requested_tx, requested) = mpsc::channel();
        let url = sse_server_in_parts(
            vec![b"data: {\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\n"],
            false,
            Some(requested_tx),
        );

        let registration = register("stopped-early");
        cancel("stopped-early");
        assert!(registration.is_cancelled());
        start(
            app.handle().clone(),
            settings(url),
            "s".into(),
            "u".into(),
            registration,
        );

        assert!(requested.recv_timeout(Duration::from_millis(500)).is_err());
        assert!(events.recv_timeout(Duration::from_millis(100)).is_err());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while RUNNING.lock().unwrap().contains_key("stopped-early") {
            assert!(
                std::time::Instant::now() < deadline,
                "the request should unregister"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_registration_that_never_starts_unregisters_when_dropped() {
        drop(register("abandoned"));
        assert!(!RUNNING.lock().unwrap().contains_key("abandoned"));
    }

    #[test]
    fn a_crlf_stream_without_spaces_after_colons_still_streams() {
        let url = sse_server(
            "data:{\"choices\":[{\"delta\":{\"content\":\"one\"}}]}\r\n\r\n\
             data: {\"choices\":[{\"delta\":{\"content\":\" two\"}}]}\r\n\r\n\
             data:[DONE]\r\n\r\n",
            false,
        );
        let events = Mutex::new(Vec::new());
        let on_event = |event: StreamEvent| events.lock().unwrap().push(event);
        let runtime = tokio::runtime::Runtime::new().unwrap();

        runtime
            .block_on(stream_openai(&url, None, "m", "s", "u", &on_event))
            .unwrap();

        assert_eq!(
            events.into_inner().unwrap(),
            vec![
                StreamEvent::Text("one".to_string()),
                StreamEvent::Text(" two".to_string())
            ]
        );
    }

    #[test]
    fn a_stream_cut_off_before_it_finishes_is_an_error_after_its_partial_text() {
        let url = sse_server(
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
            false,
        );
        let events = Mutex::new(Vec::new());
        let on_event = |event: StreamEvent| events.lock().unwrap().push(event);
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let result = runtime.block_on(stream_openai(&url, None, "m", "s", "u", &on_event));

        assert_eq!(
            events.into_inner().unwrap(),
            vec![StreamEvent::Text("partial".to_string())]
        );
        assert_eq!(
            result,
            Err("The connection closed before the answer finished.".to_string())
        );
    }

    /// Serves Ollama's `/api/show` and `/api/ps` with the given bodies; a `None` answers 404.
    fn ollama_server(show: &'static str, ps: Option<&'static str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                let mut buffer = [0_u8; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]);
                let body = if request.starts_with("POST /api/show") {
                    Some(show)
                } else {
                    ps
                };
                let response = match body {
                    Some(body) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{address}/")
    }

    const SHOW_128K: &str =
        r#"{"model_info":{"general.architecture":"gptoss","gptoss.context_length":131072}}"#;

    fn context(base_url: &str, model: &str) -> Option<usize> {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(ollama_context_window(base_url, None, model))
    }

    #[test]
    fn a_cloud_model_gets_its_full_capacity() {
        let url = ollama_server(SHOW_128K, None);
        assert_eq!(context(&url, "gpt-oss:120b-cloud"), Some(131_072));
    }

    #[test]
    fn a_local_model_gets_what_its_modelfile_allocates_not_its_capacity() {
        let url = ollama_server(
            r#"{"parameters":"stop \"<end>\"\nnum_ctx 8192","model_info":{"llama.context_length":131072}}"#,
            None,
        );
        assert_eq!(context(&url, "llama3.1:8b"), Some(8192));
    }

    #[test]
    fn a_loaded_local_model_gets_the_context_the_server_gave_it() {
        let url = ollama_server(
            SHOW_128K,
            Some(
                r#"{"models":[{"name":"other:1b","context_length":2048},{"name":"gpt-oss:20b","context_length":16384}]}"#,
            ),
        );
        assert_eq!(context(&url, "gpt-oss:20b"), Some(16_384));
    }

    #[test]
    fn an_unloaded_local_model_assumes_ollamas_smallest_default() {
        let url = ollama_server(SHOW_128K, Some(r#"{"models":[]}"#));
        assert_eq!(context(&url, "gpt-oss:20b"), Some(4096));
    }

    #[test]
    fn a_local_allocation_never_exceeds_the_model_capacity() {
        let url = ollama_server(
            r#"{"parameters":"num_ctx 32768","model_info":{"tiny.context_length":2048}}"#,
            None,
        );
        assert_eq!(context(&url, "tiny"), Some(2048));
    }

    #[test]
    fn an_unreachable_ollama_reports_no_context_window() {
        let address = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let window = runtime.block_on(ollama_context_window(
            &format!("http://{address}"),
            None,
            "m",
        ));

        assert_eq!(window, None);
    }
}
