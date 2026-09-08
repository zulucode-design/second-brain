use reqwest::Client;
use serde_json::json;
use tauri::{AppHandle, Emitter};

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

#[allow(clippy::too_many_arguments)]
pub fn ai_request(
    app: AppHandle,
    settings: AiRequestSettings,
    system_prompt: String,
    user_message: String,
    request_id: String,
) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Handle all API keys as optional; ollama and v1 completions doesnt always require it.
            let result = match settings.protocol() {
                AiApiProtocol::OpenAiChatCompletions => {
                    stream_openai(
                        &app,
                        settings.endpoint(),
                        settings.api_key(),
                        settings.model(),
                        &system_prompt,
                        &user_message,
                        &request_id,
                    )
                    .await
                }
                AiApiProtocol::AnthropicMessages => {
                    stream_anthropic(
                        &app,
                        settings.endpoint(),
                        settings.api_key().unwrap_or_default(),
                        settings.model(),
                        &system_prompt,
                        &user_message,
                        &request_id,
                    )
                    .await
                }
            };
            if let Err(e) = result {
                let _ = app.emit(
                    "ai-stream",
                    AiStreamEvent {
                        event_type: "error".to_string(),
                        text: None,
                        error: Some(e),
                    },
                );
            }
        });
    });
}

async fn stream_anthropic(
    app: &AppHandle,
    endpoint: &str,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    _request_id: &str,
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

    // Parse SSE stream
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete SSE events from buffer
        while let Some(event_end) = buffer.find("\n\n") {
            let event_str = buffer[..event_end].to_string();
            buffer = buffer[event_end + 2..].to_string();

            for line in event_str.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        let _ = app.emit(
                            "ai-stream",
                            AiStreamEvent {
                                event_type: "done".to_string(),
                                text: None,
                                error: None,
                            },
                        );
                        return Ok(());
                    }

                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                        let event_type = parsed["type"].as_str().unwrap_or("");

                        match event_type {
                            "content_block_delta" => {
                                if let Some(text) = parsed["delta"]["text"].as_str() {
                                    let _ = app.emit(
                                        "ai-stream",
                                        AiStreamEvent {
                                            event_type: "text".to_string(),
                                            text: Some(text.to_string()),
                                            error: None,
                                        },
                                    );
                                }
                            }
                            "message_stop" => {
                                let _ = app.emit(
                                    "ai-stream",
                                    AiStreamEvent {
                                        event_type: "done".to_string(),
                                        text: None,
                                        error: None,
                                    },
                                );
                                return Ok(());
                            }
                            "error" => {
                                let msg = parsed["error"]["message"]
                                    .as_str()
                                    .unwrap_or("Unknown API error");
                                let _ = app.emit(
                                    "ai-stream",
                                    AiStreamEvent {
                                        event_type: "error".to_string(),
                                        text: None,
                                        error: Some(msg.to_string()),
                                    },
                                );
                                return Err(msg.to_string());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    let _ = app.emit(
        "ai-stream",
        AiStreamEvent {
            event_type: "done".to_string(),
            text: None,
            error: None,
        },
    );

    Ok(())
}

async fn stream_openai(
    app: &AppHandle,
    url: &str,
    api_key: Option<&str>,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    _request_id: &str,
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

    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(event_end) = buffer.find("\n\n") {
            let event_str = buffer[..event_end].to_string();
            buffer = buffer[event_end + 2..].to_string();

            for line in event_str.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        let _ = app.emit(
                            "ai-stream",
                            AiStreamEvent {
                                event_type: "done".to_string(),
                                text: None,
                                error: None,
                            },
                        );
                        return Ok(());
                    }

                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                        // OpenAI streaming: choices[0].delta.content
                        if let Some(content) = parsed["choices"][0]["delta"]["content"].as_str() {
                            if !content.is_empty() {
                                let _ = app.emit(
                                    "ai-stream",
                                    AiStreamEvent {
                                        event_type: "text".to_string(),
                                        text: Some(content.to_string()),
                                        error: None,
                                    },
                                );
                            }
                        }

                        // Check finish_reason
                        if let Some(reason) = parsed["choices"][0]["finish_reason"].as_str() {
                            if reason == "stop" || reason == "length" {
                                let _ = app.emit(
                                    "ai-stream",
                                    AiStreamEvent {
                                        event_type: "done".to_string(),
                                        text: None,
                                        error: None,
                                    },
                                );
                                return Ok(());
                            }
                        }

                        // Check for error in stream
                        if let Some(err) = parsed["error"]["message"].as_str() {
                            let _ = app.emit(
                                "ai-stream",
                                AiStreamEvent {
                                    event_type: "error".to_string(),
                                    text: None,
                                    error: Some(err.to_string()),
                                },
                            );
                            return Err(err.to_string());
                        }
                    }
                }
            }
        }
    }

    let _ = app.emit(
        "ai-stream",
        AiStreamEvent {
            event_type: "done".to_string(),
            text: None,
            error: None,
        },
    );

    Ok(())
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
