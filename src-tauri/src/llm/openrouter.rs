//! OpenRouter LLM client (first-class provider — see ADR-0005).
//!
//! Wraps an OpenAI-compatible HTTP client with:
//! - Hardcoded base URL default (`https://openrouter.ai/api/v1`).
//! - Attribution headers (`HTTP-Referer`, `X-OpenRouter-Title`) sent by default
//!   on every request so AudioGraph appears on openrouter.ai's app rankings.
//! - `GET /api/v1/models` discovery for the settings model picker.
//! - `chat_completion` is BLOCKING in this plan (A2). Streaming chat is the
//!   subject of plan A3 / ADR-0006 — that work converts this to SSE later.
//!
//! Reuses `crate::llm::api_client::ApiClient` for chat completions where
//! possible, attaching the OpenRouter-specific headers via reqwest's
//! `default_headers` builder.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::graph::entities::ExtractionResult;

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Default OpenRouter base URL (per ADR-0005, verified 2026-05-19).
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// Default `HTTP-Referer` value sent on every request.
pub const DEFAULT_HTTP_REFERER: &str = "https://github.com/Codeseys-Labs/audio-graph";
/// Default `X-OpenRouter-Title` value sent on every request.
pub const DEFAULT_APP_TITLE: &str = "AudioGraph";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Runtime configuration for the OpenRouter client.
///
/// Mirrors `ApiConfig` but with the base URL pinned to OpenRouter's API and
/// always-on attribution headers. The `provider_order` field maps to
/// `provider.order` in the chat-completion request body (passthrough).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterConfig {
    /// Bearer token. Required (unlike the generic ApiConfig where local
    /// servers may run unauthenticated).
    pub api_key: String,
    /// Model identifier (OpenRouter slug, e.g. `"anthropic/claude-sonnet-4.5"`).
    pub model: String,
    /// Base URL — defaults to [`DEFAULT_BASE_URL`].
    pub base_url: String,
    /// Optional ordered preferred-provider list passed through to
    /// `provider.order` in the chat completion request body.
    pub provider_order: Option<Vec<String>>,
    /// When `true`, request usage in the streaming final chunk via
    /// `stream_options.include_usage`. Plan A2 uses blocking chat so this
    /// flag is recorded but only takes effect when A3 lands the streaming path.
    pub include_usage_in_stream: bool,
    /// `HTTP-Referer` header. Defaults to [`DEFAULT_HTTP_REFERER`].
    pub http_referer: String,
    /// `X-OpenRouter-Title` header (alias `X-Title` is also accepted by the
    /// server). Defaults to [`DEFAULT_APP_TITLE`].
    pub app_title: String,
    /// Maximum tokens to generate. Default 512.
    pub max_tokens: u32,
    /// Sampling temperature. Default 0.1 for extraction, 0.7 for chat.
    pub temperature: f32,
}

impl OpenRouterConfig {
    /// Construct a client config with the canonical OpenRouter defaults
    /// applied for everything except the API key + model.
    pub fn with_defaults(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            base_url: DEFAULT_BASE_URL.to_string(),
            provider_order: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 512,
            temperature: 0.1,
        }
    }

    fn base_url_trimmed(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }
}

// ---------------------------------------------------------------------------
// Model catalog response
// ---------------------------------------------------------------------------

/// A single model entry returned by `GET /api/v1/models`.
///
/// Mirrors the canonical OpenRouter response shape, but is permissive about
/// missing optional fields (the catalog occasionally omits `pricing` for
/// brand-new models, and free models have all-zero pricing).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub pricing: Option<OpenRouterPricing>,
}

/// Pricing block — strings because OpenRouter returns scientific-notation
/// floats as strings (e.g. `"0.000003"`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterPricing {
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub completion: String,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<OpenRouterModel>,
}

// ---------------------------------------------------------------------------
// Request / response types for chat completions
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ApiMessage>,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<ProviderRouting>,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
}

#[derive(Serialize)]
struct ProviderRouting {
    order: Vec<String>,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

// ---------------------------------------------------------------------------
// Free-function key/connectivity probes (for Tauri commands)
// ---------------------------------------------------------------------------

/// Hit `GET /api/v1/models` with the supplied API key + attribution headers.
///
/// Returns `Ok(())` on HTTP 200, `Err(diagnostic)` on 401/403/network errors.
/// Used by `test_openrouter_connection_cmd`.
pub async fn test_connection(api_key: &str, base_url: &str) -> Result<(), String> {
    if api_key.trim().is_empty() {
        return Err("OpenRouter API key is empty".to_string());
    }

    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = build_async_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .header("HTTP-Referer", DEFAULT_HTTP_REFERER)
        .header("X-OpenRouter-Title", DEFAULT_APP_TITLE)
        .send()
        .await
        .map_err(|e| format!("OpenRouter connection failed: {}", e))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(format!(
            "invalid_key: OpenRouter rejected the API key (HTTP {})",
            status.as_u16()
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "OpenRouter returned HTTP {} from {}: {}",
            status,
            url,
            body.chars().take(200).collect::<String>()
        ));
    }
    Ok(())
}

/// Fetch the live OpenRouter model catalog.
pub async fn list_models(api_key: &str, base_url: &str) -> Result<Vec<OpenRouterModel>, String> {
    if api_key.trim().is_empty() {
        return Err("OpenRouter API key is empty".to_string());
    }

    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = build_async_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .header("HTTP-Referer", DEFAULT_HTTP_REFERER)
        .header("X-OpenRouter-Title", DEFAULT_APP_TITLE)
        .send()
        .await
        .map_err(|e| format!("OpenRouter list_models request failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "OpenRouter returned HTTP {} from {}: {}",
            status,
            url,
            body.chars().take(200).collect::<String>()
        ));
    }
    let parsed: ModelsResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse OpenRouter models response: {}", e))?;
    Ok(parsed.data)
}

fn build_async_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

// ---------------------------------------------------------------------------
// OpenRouterClient — blocking chat completion (mirrors ApiClient surface)
// ---------------------------------------------------------------------------

/// Blocking chat-completion client for OpenRouter (plan A2 surface).
///
/// Streaming variants land in plan A3 / ADR-0006. Until then this is wired
/// into the LLM executor via the same dispatch shape as `ApiClient`.
///
/// `Clone` is cheap: `reqwest::blocking::Client` is `Arc`-backed and the config
/// is a small owned struct. Cloning lets callers release the client mutex
/// before issuing the (blocking, up-to-60s) HTTP request so a long-running
/// background extraction never blocks an interactive chat on the same lock.
#[derive(Clone)]
pub struct OpenRouterClient {
    config: OpenRouterConfig,
    client: reqwest::blocking::Client,
}

impl OpenRouterClient {
    /// Build a client with attribution headers baked in via reqwest's
    /// default-headers mechanism, so every outbound chat completion carries
    /// `HTTP-Referer` + `X-OpenRouter-Title` without per-request plumbing.
    pub fn new(config: OpenRouterConfig) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&config.http_referer) {
            headers.insert("HTTP-Referer", value);
        }
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&config.app_title) {
            headers.insert("X-OpenRouter-Title", value);
        }

        let client = reqwest::blocking::Client::builder()
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .timeout(HTTP_REQUEST_TIMEOUT)
            .default_headers(headers)
            .build()
            .unwrap_or_else(|e| {
                log::warn!(
                    "Failed to build OpenRouter reqwest client; falling back to defaults: {}",
                    e
                );
                reqwest::blocking::Client::new()
            });

        Self { config, client }
    }

    /// Returns `true` when the config has the minimum fields populated to
    /// dispatch a chat completion (api key + model).
    pub fn is_configured(&self) -> bool {
        !self.config.api_key.trim().is_empty() && !self.config.model.trim().is_empty()
    }

    pub(crate) fn config(&self) -> &OpenRouterConfig {
        &self.config
    }

    /// Send a blocking chat completion. Mirrors `ApiClient::chat_completion`.
    ///
    /// `messages` is a `(role, content)` list. `json_mode = true` adds
    /// `response_format: { type: "json_object" }` to the request body.
    pub fn chat_completion(
        &self,
        messages: Vec<(String, String)>,
        json_mode: bool,
    ) -> Result<String, String> {
        let api_messages: Vec<ApiMessage> = messages
            .into_iter()
            .map(|(role, content)| ApiMessage { role, content })
            .collect();

        let response_format = if json_mode {
            Some(ResponseFormat {
                format_type: "json_object".to_string(),
            })
        } else {
            None
        };

        let provider = self
            .config
            .provider_order
            .as_ref()
            .filter(|order| !order.is_empty())
            .map(|order| ProviderRouting {
                order: order.clone(),
            });

        let request = ChatCompletionRequest {
            model: &self.config.model,
            messages: api_messages,
            max_tokens: self.config.max_tokens,
            temperature: self.config.temperature,
            response_format,
            provider,
        };

        let url = format!("{}/chat/completions", self.config.base_url_trimmed());

        // Attribution headers are also set via default_headers in `new()`,
        // but we add them per-request as well. reqwest::blocking's
        // default_headers behaviour around `redirect`/`policy` and certain
        // proxy configurations can drop the defaults; explicit per-request
        // setting is platform-stable. (Caught by Windows CI run 26177547487.)
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("HTTP-Referer", &self.config.http_referer)
            .header("X-OpenRouter-Title", &self.config.app_title)
            .json(&request)
            .send()
            .map_err(|e| format!("OpenRouter chat completion request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(format!(
                "OpenRouter returned HTTP {} from {}: {}",
                status, url, body
            ));
        }

        let completion: ChatCompletionResponse = response
            .json()
            .map_err(|e| format!("Failed to parse OpenRouter chat response: {}", e))?;

        completion
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| "No response choices from OpenRouter".to_string())
    }

    /// Extract entities and relationships from a transcript segment via
    /// JSON-mode chat completion. Same prompt shape as `ApiClient`.
    pub fn extract_entities(&self, text: &str, speaker: &str) -> Result<ExtractionResult, String> {
        let system_prompt = crate::ontology::extraction_system_prompt();

        let user_prompt = format!("[{}]: {}", speaker, text);
        let messages = vec![
            ("system".to_string(), system_prompt),
            ("user".to_string(), user_prompt),
        ];

        let raw = self.chat_completion(messages, true)?;
        serde_json::from_str::<ExtractionResult>(&raw).map_err(|e| {
            format!(
                "Failed to parse extraction JSON from OpenRouter: {} — raw: {}",
                e, raw
            )
        })
    }

    /// Chat with full message history and knowledge graph context.
    pub fn chat_with_history(
        &self,
        messages: &[crate::llm::engine::ChatMessage],
        graph_context: &str,
    ) -> Result<String, String> {
        let system_prompt = format!(
            "You are a knowledge graph assistant analyzing a live audio conversation. \
             Here is the current knowledge graph context:\n\n{}\n\n\
             Answer the user's question about the conversation, people, topics, or relationships discussed.",
            graph_context
        );

        let mut api_messages = vec![("system".to_string(), system_prompt)];
        for msg in messages {
            api_messages.push((msg.role.clone(), msg.content.clone()));
        }

        self.chat_completion(api_messages, false)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    /// Tiny HTTP/1.1 mock server. Reads one request, runs `handler` to produce
    /// a response, writes it, closes. Captures the raw request bytes for the
    /// test to assert on (e.g. attribution headers).
    async fn spawn_mock(
        handler: impl Fn(&str) -> (u16, &'static str, String) + Send + Sync + 'static,
    ) -> (String, Arc<Mutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");
        let captured = Arc::new(Mutex::new(String::new()));
        let captured_for_task = captured.clone();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let mut total = String::new();
                // Read until we see the end of headers (\r\n\r\n). We don't
                // care about the body for these tests.
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    total.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if total.contains("\r\n\r\n") {
                        break;
                    }
                }
                {
                    let mut guard = captured_for_task.lock().await;
                    *guard = total.clone();
                }
                let (status, status_text, body) = handler(&total);
                let response = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    status_text,
                    body.len(),
                    body,
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (format!("http://{}", addr), captured)
    }

    #[tokio::test]
    async fn test_connection_succeeds_on_200() {
        let (base, _captured) = spawn_mock(|_req| (200, "OK", "{\"data\":[]}".to_string())).await;
        test_connection("sk-test", &base)
            .await
            .expect("test_connection must return Ok on HTTP 200");
    }

    #[tokio::test]
    async fn test_connection_fails_on_401() {
        let (base, _captured) = spawn_mock(|_req| {
            (
                401,
                "Unauthorized",
                "{\"error\":\"invalid api key\"}".to_string(),
            )
        })
        .await;
        let err = test_connection("sk-bad", &base)
            .await
            .expect_err("test_connection must return Err on HTTP 401");
        assert!(
            err.contains("invalid_key"),
            "401 must produce an invalid_key diagnostic, got: {err}"
        );
    }

    #[tokio::test]
    async fn list_models_parses_data_array() {
        let body = serde_json::json!({
            "data": [
                {
                    "id": "anthropic/claude-sonnet-4.5",
                    "name": "Anthropic: Claude Sonnet 4.5",
                    "context_length": 200000,
                    "pricing": { "prompt": "0.000003", "completion": "0.000015" }
                },
                {
                    "id": "openai/gpt-5.2",
                    "name": "OpenAI: GPT-5.2",
                    "context_length": 400000,
                    "pricing": { "prompt": "0.000005", "completion": "0.0000125" }
                }
            ]
        })
        .to_string();
        let body_clone = body.clone();
        let (base, _captured) = spawn_mock(move |_req| (200, "OK", body_clone.clone())).await;

        let models = list_models("sk-test", &base)
            .await
            .expect("list_models must succeed on canonical response");
        assert_eq!(models.len(), 2, "two models in fixture");
        assert_eq!(models[0].id, "anthropic/claude-sonnet-4.5");
        assert_eq!(models[1].id, "openai/gpt-5.2");
        assert_eq!(models[0].context_length, Some(200000));
        let pricing = models[0]
            .pricing
            .as_ref()
            .expect("first model fixture has pricing");
        assert_eq!(pricing.prompt, "0.000003");
        assert_eq!(pricing.completion, "0.000015");
    }

    #[tokio::test]
    async fn test_connection_sends_attribution_headers() {
        let (base, captured) = spawn_mock(|_req| (200, "OK", "{\"data\":[]}".to_string())).await;
        test_connection("sk-test", &base)
            .await
            .expect("test_connection must succeed");
        let req_dump = captured.lock().await.clone();
        // Header names are case-insensitive per RFC 7230 §3.2; reqwest
        // normalises them differently across Linux vs Windows runners.
        // Lowercase both sides before substring-asserting.
        let dump_lc = req_dump.to_ascii_lowercase();
        let referer_marker = format!("http-referer: {}", DEFAULT_HTTP_REFERER).to_ascii_lowercase();
        let title_marker =
            format!("x-openrouter-title: {}", DEFAULT_APP_TITLE).to_ascii_lowercase();
        assert!(
            dump_lc.contains(&referer_marker),
            "request must include HTTP-Referer header, got:\n{req_dump}"
        );
        assert!(
            dump_lc.contains(&title_marker),
            "request must include X-OpenRouter-Title header, got:\n{req_dump}"
        );
        assert!(
            dump_lc.contains("authorization: bearer sk-test"),
            "request must include bearer auth, got:\n{req_dump}"
        );
    }

    #[test]
    fn chat_request_includes_attribution_headers() {
        // Stand up the mock on a runtime, then point the blocking client at
        // it. We use blocking::Client (the OpenRouterClient default) but
        // drive the mock from a tokio runtime.
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base, captured) = rt.block_on(async {
            spawn_mock(|_req| {
                let body = serde_json::json!({
                    "choices": [{ "message": { "content": "hello" } }]
                })
                .to_string();
                (200, "OK", body)
            })
            .await
        });

        let config = OpenRouterConfig {
            api_key: "sk-blocking".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: base,
            provider_order: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };
        let client = OpenRouterClient::new(config);
        // Issue the call on a worker thread because `reqwest::blocking` cannot
        // run inside an active tokio runtime.
        let join = std::thread::spawn(move || {
            client.chat_completion(vec![("user".to_string(), "hi".to_string())], false)
        });
        let reply = join.join().expect("worker thread panic").expect("chat ok");
        assert_eq!(reply, "hello");

        let req_dump = rt.block_on(async { captured.lock().await.clone() });
        // Case-insensitive — see test_connection_sends_attribution_headers.
        let dump_lc = req_dump.to_ascii_lowercase();
        let referer_marker = format!("http-referer: {}", DEFAULT_HTTP_REFERER).to_ascii_lowercase();
        let title_marker =
            format!("x-openrouter-title: {}", DEFAULT_APP_TITLE).to_ascii_lowercase();
        assert!(
            dump_lc.contains(&referer_marker),
            "chat request must include HTTP-Referer, got:\n{req_dump}"
        );
        assert!(
            dump_lc.contains(&title_marker),
            "chat request must include X-OpenRouter-Title, got:\n{req_dump}"
        );
        assert!(
            dump_lc.contains("authorization: bearer sk-blocking"),
            "chat request must include bearer auth, got:\n{req_dump}"
        );
    }

    #[test]
    fn config_defaults_apply_canonical_attribution() {
        let cfg = OpenRouterConfig::with_defaults("sk-x".into(), "anthropic/claude".into());
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert_eq!(cfg.http_referer, DEFAULT_HTTP_REFERER);
        assert_eq!(cfg.app_title, DEFAULT_APP_TITLE);
        assert!(cfg.include_usage_in_stream);
        assert!(cfg.provider_order.is_none());
    }
}
