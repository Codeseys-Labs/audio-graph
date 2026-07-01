//! OpenAI-compatible API client for LLM inference.
//!
//! Calls any OpenAI-compatible chat completions endpoint (OpenAI, Ollama,
//! LM Studio, vLLM, OpenRouter, Anthropic via proxy, etc.).
//! Used as an alternative to the native llama-cpp-2 engine.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::graph::entities::ExtractionResult;

const API_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const API_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for an OpenAI-compatible API endpoint.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiConfig {
    /// Base URL, e.g. `"https://api.openai.com/v1"` or `"http://localhost:11434/v1"`.
    pub endpoint: String,
    /// Bearer token.  `None` for local servers (Ollama, LM Studio).
    pub api_key: Option<String>,
    /// Model identifier, e.g. `"gpt-4o-mini"`, `"llama3.2"`, `"qwen2.5:3b"`.
    pub model: String,
    /// Maximum tokens to generate (default 512).
    pub max_tokens: u32,
    /// Sampling temperature (default 0.1 for extraction, 0.7 for chat).
    pub temperature: f32,
}

impl std::fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiConfig")
            .field("endpoint", &self.endpoint)
            .field(
                "api_key",
                &crate::credentials::redacted_secret_presence(self.api_key.as_deref()),
            )
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Request / Response types (OpenAI Chat Completions)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ApiMessage>,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_outputs: Option<StructuredOutputs>,
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
struct StructuredOutputs {
    json: serde_json::Value,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
    /// OpenAI/OpenRouter/Bedrock(-proxy) carry a `usage` block on non-streaming
    /// responses. Optional so a provider that omits it deserializes cleanly.
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

/// Token-usage block from an OpenAI-compatible chat completion response.
///
/// `total_tokens` is serde-`default` so a provider omitting it yields 0 rather
/// than a deserialization error — a missing count is reported as 0, never
/// fabricated.
#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    total_tokens: u32,
}

// ---------------------------------------------------------------------------
// ApiClient
// ---------------------------------------------------------------------------

/// OpenAI-compatible API client.
///
/// Thread-safe: `reqwest::blocking::Client` is `Send + Sync`.
///
/// `Clone` is cheap (`reqwest::blocking::Client` is `Arc`-backed) and lets
/// callers release the client mutex before the blocking HTTP request.
#[derive(Clone)]
pub struct ApiClient {
    config: ApiConfig,
    client: reqwest::blocking::Client,
    content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

impl ApiClient {
    /// Create a new API client with the given configuration.
    pub fn new(config: ApiConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(API_CONNECT_TIMEOUT)
            .timeout(API_REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|e| {
                log::warn!(
                    "Failed to build reqwest client with API timeouts; falling back to defaults: {}",
                    e
                );
                reqwest::blocking::Client::new()
            });

        Self {
            config,
            client,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::block(
                "explicit_policy_required",
            ),
        }
    }

    pub(crate) fn with_content_egress_policy(
        mut self,
        policy: crate::asr::ProviderContentEgressPolicy,
    ) -> Self {
        self.content_egress_policy = policy;
        self
    }

    pub(crate) fn content_egress_policy(&self) -> crate::asr::ProviderContentEgressPolicy {
        self.content_egress_policy
    }

    /// Returns `true` if the client has a non-empty endpoint and model.
    pub fn is_configured(&self) -> bool {
        !self.config.endpoint.is_empty() && !self.config.model.is_empty()
    }

    pub(crate) fn config(&self) -> &ApiConfig {
        &self.config
    }

    // ------------------------------------------------------------------
    // Low-level chat completion
    // ------------------------------------------------------------------

    /// Send a chat completion request and return the assistant's reply.
    ///
    /// `messages` is a list of `(role, content)` tuples.
    /// When `json_mode` is true, the request includes `response_format: { type: "json_object" }`.
    ///
    /// Discards the token count; use [`chat_completion_with_usage`](Self::chat_completion_with_usage)
    /// when the caller needs `total_tokens`.
    pub fn chat_completion(
        &self,
        messages: Vec<(String, String)>,
        json_mode: bool,
    ) -> Result<String, String> {
        self.chat_completion_inner(messages, json_mode, None)
            .map(|(text, _tokens)| text)
    }

    /// Like [`chat_completion`](Self::chat_completion) but also returns the
    /// backend-reported `total_tokens` (0 when the provider omits a `usage`
    /// block — reported, never fabricated).
    pub fn chat_completion_with_usage(
        &self,
        messages: Vec<(String, String)>,
        json_mode: bool,
    ) -> Result<(String, u32), String> {
        self.chat_completion_inner(messages, json_mode, None)
    }

    pub(crate) fn chat_completion_with_structured_outputs_with_usage(
        &self,
        messages: Vec<(String, String)>,
        schema: serde_json::Value,
    ) -> Result<(String, u32), String> {
        self.chat_completion_inner(messages, false, Some(schema))
    }

    fn chat_completion_with_structured_outputs(
        &self,
        messages: Vec<(String, String)>,
        schema: serde_json::Value,
    ) -> Result<String, String> {
        self.chat_completion_with_structured_outputs_with_usage(messages, schema)
            .map(|(text, _tokens)| text)
    }

    fn chat_completion_inner(
        &self,
        messages: Vec<(String, String)>,
        json_mode: bool,
        structured_outputs: Option<serde_json::Value>,
    ) -> Result<(String, u32), String> {
        self.content_egress_policy.check_prompt("llm.api")?;

        let api_messages: Vec<ApiMessage> = messages
            .into_iter()
            .map(|(role, content)| ApiMessage { role, content })
            .collect();

        let response_format = if json_mode && structured_outputs.is_none() {
            Some(ResponseFormat {
                format_type: "json_object".to_string(),
            })
        } else {
            None
        };

        let request = ChatCompletionRequest {
            model: self.config.model.clone(),
            messages: api_messages,
            max_tokens: self.config.max_tokens,
            temperature: self.config.temperature,
            response_format,
            structured_outputs: structured_outputs.map(|json| StructuredOutputs { json }),
        };

        let url = format!(
            "{}/chat/completions",
            self.config.endpoint.trim_end_matches('/')
        );

        let mut req = self.client.post(&url).json(&request);

        // Trim before deciding: a whitespace-only key ("   ") is effectively
        // unset, and sending it as a bearer token turns "no key" into a hard auth
        // failure (CodeRabbit api_client.rs:187).
        if let Some(key) = self.config.api_key.as_deref().map(str::trim)
            && !key.is_empty()
        {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        let response = req
            .send()
            .map_err(|e| format!("API request to {} failed: {}", url, e))?;

        if !response.status().is_success() {
            let status = response.status();
            let request_id = response_request_id(response.headers());
            let body = response.text().unwrap_or_default();
            return Err(api_error_message(
                status,
                &url,
                &body,
                request_id.as_deref(),
            ));
        }

        let completion: ChatCompletionResponse = response
            .json()
            .map_err(|e| format!("Failed to parse API response: {}", e))?;

        // A missing `usage` block (or a provider that omits `total_tokens`)
        // reports 0 — never fabricated.
        let total_tokens = completion.usage.map(|u| u.total_tokens).unwrap_or(0);

        completion
            .choices
            .first()
            .map(|c| (c.message.content.clone(), total_tokens))
            .ok_or_else(|| "No response choices from API".to_string())
    }

    // ------------------------------------------------------------------
    // Entity extraction (JSON mode)
    // ------------------------------------------------------------------

    /// Extract entities and relationships from a transcript segment.
    ///
    /// Uses JSON mode to request structured output matching [`ExtractionResult`].
    pub fn extract_entities(
        &self,
        text: &str,
        speaker: &str,
        context: &str,
    ) -> Result<ExtractionResult, String> {
        let system_prompt = crate::ontology::extraction_system_prompt();

        // See OpenRouterClient::extract_entities: recent context for reference
        // resolution; extract only from the current segment.
        let user_prompt = if context.trim().is_empty() {
            format!("[{}]: {}", speaker, text)
        } else {
            format!(
                "Recent conversation (context only — do NOT extract from this):\n{}\n\n\
                 Current segment to extract from:\n[{}]: {}",
                context.trim(),
                speaker,
                text
            )
        };
        let messages = vec![
            ("system".to_string(), system_prompt),
            ("user".to_string(), user_prompt),
        ];

        let raw = if self.prefers_vllm_structured_outputs() {
            let schema = Self::extraction_json_schema()?;
            match self.chat_completion_with_structured_outputs(messages.clone(), schema) {
                Ok(raw) => raw,
                Err(e) => {
                    log::warn!(
                        "vLLM structured outputs failed, falling back to JSON mode: {}",
                        e
                    );
                    self.chat_completion(messages, true)?
                }
            }
        } else {
            self.chat_completion(messages, true)?
        };

        serde_json::from_str::<ExtractionResult>(&raw)
            .map_err(|e| extraction_parse_error("API", "llm.api.extract_entities", &e, &raw))
    }

    fn extraction_json_schema() -> Result<serde_json::Value, String> {
        serde_json::to_value(schemars::schema_for!(ExtractionResult))
            .map_err(|e| format!("Failed to build extraction JSON schema: {}", e))
    }

    pub(crate) fn prefers_vllm_structured_outputs(&self) -> bool {
        let endpoint = self.config.endpoint.to_lowercase();
        endpoint.contains("localhost:8000")
            || endpoint.contains("127.0.0.1:8000")
            || endpoint.contains("0.0.0.0:8000")
            || endpoint.contains("vllm")
    }

    // ------------------------------------------------------------------
    // Chat with knowledge graph context
    // ------------------------------------------------------------------

    /// Chat with the knowledge graph context, using the OpenAI-compatible API.
    pub fn chat(&self, user_message: &str, graph_context: &str) -> Result<String, String> {
        let system_prompt = format!(
            "You are a knowledge graph assistant analyzing a live audio conversation. \
             Here is the current knowledge graph context:\n\n{}\n\n\
             Answer the user's question about the conversation, people, topics, or relationships discussed.",
            graph_context
        );

        // Use a higher temperature for chat
        let messages = vec![
            ("system".to_string(), system_prompt),
            ("user".to_string(), user_message.to_string()),
        ];

        self.chat_completion(messages, false)
    }

    /// Chat with full message history and knowledge graph context.
    ///
    /// Discards the token count; use
    /// [`chat_with_history_with_usage`](Self::chat_with_history_with_usage) when
    /// the caller needs `total_tokens`.
    pub fn chat_with_history(
        &self,
        messages: &[crate::llm::engine::ChatMessage],
        graph_context: &str,
    ) -> Result<String, String> {
        self.chat_completion(self.history_messages(messages, graph_context), false)
    }

    /// Like [`chat_with_history`](Self::chat_with_history) but also returns the
    /// backend-reported `total_tokens`. `0` when the provider omits a `usage`
    /// block (reported, never fabricated). The executor's `chat_api` attempt
    /// (and Bedrock, which routes through `ApiClient`) uses this.
    pub fn chat_with_history_with_usage(
        &self,
        messages: &[crate::llm::engine::ChatMessage],
        graph_context: &str,
    ) -> Result<(String, u32), String> {
        self.chat_completion_with_usage(self.history_messages(messages, graph_context), false)
    }

    /// Build the `(role, content)` message list for the history chat paths:
    /// a knowledge-graph system prompt followed by the conversation messages.
    fn history_messages(
        &self,
        messages: &[crate::llm::engine::ChatMessage],
        graph_context: &str,
    ) -> Vec<(String, String)> {
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
        api_messages
    }
}

fn api_error_message(
    status: reqwest::StatusCode,
    url: &str,
    body: &str,
    request_id: Option<&str>,
) -> String {
    format!(
        "API error: provider=api path={} status={} body_bytes={} body_chars={}{}",
        diagnostic_path(url),
        status.as_u16(),
        body.len(),
        body.chars().count(),
        request_id
            .map(|id| format!(" request_id={id}"))
            .unwrap_or_default()
    )
}

fn diagnostic_path(url: &str) -> String {
    reqwest::Url::parse(url)
        .map(|parsed| parsed.path().to_string())
        .unwrap_or_else(|_| "<unparseable>".to_string())
}

fn response_request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    for name in [
        "x-request-id",
        "request-id",
        "x-openrouter-request-id",
        "cf-ray",
    ] {
        let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) else {
            continue;
        };
        let sanitized: String = value
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
            .take(128)
            .collect();
        if !sanitized.is_empty() {
            return Some(sanitized);
        }
    }
    None
}

fn extraction_parse_error(
    provider: &str,
    path: &str,
    error: &serde_json::Error,
    provider_output: &str,
) -> String {
    let class = match error.classify() {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    };
    format!(
        "Failed to parse extraction JSON from {} ({}): class={}; detail={}; provider_output_bytes={}; provider_output_chars={}",
        provider,
        path,
        class,
        error,
        provider_output.len(),
        provider_output.chars().count()
    )
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
    use tokio::sync::Mutex as TokioMutex;

    fn config(endpoint: &str, api_key: Option<&str>) -> ApiConfig {
        ApiConfig {
            endpoint: endpoint.to_string(),
            api_key: api_key.map(|k| k.to_string()),
            model: "test-model".to_string(),
            max_tokens: 64,
            temperature: 0.1,
        }
    }

    #[test]
    fn api_config_debug_redacts_api_key() {
        let config = config("https://api.openai.com/v1", Some("sk-api-debug-secret"));

        let debug = format!("{config:?}");

        assert!(!debug.contains("sk-api-debug-secret"));
        assert!(debug.contains("<present>"));
        assert!(debug.contains("https://api.openai.com/v1"));
        assert!(debug.contains("test-model"));
    }

    /// Tiny HTTP/1.1 mock that reads one full request (headers + body),
    /// captures the raw bytes, and returns a canned response. Mirrors the
    /// `spawn_mock` idiom in `openrouter.rs` but reads the body too so we can
    /// assert request-shape JSON.
    async fn spawn_mock(
        status: u16,
        status_text: &'static str,
        body: String,
    ) -> (String, Arc<TokioMutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
        let addr = listener.local_addr().expect("local addr");
        let captured = Arc::new(TokioMutex::new(String::new()));
        let captured_for_task = captured.clone();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let mut total = String::new();
                let mut content_len: Option<usize> = None;
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    total.push_str(&String::from_utf8_lossy(&buf[..n]));
                    // Once headers are in, figure out the declared body length
                    // and keep reading until we have all of it.
                    if content_len.is_none()
                        && let Some(hdr_end) = total.find("\r\n\r\n")
                    {
                        let headers = total[..hdr_end].to_ascii_lowercase();
                        content_len = headers
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok());
                    }
                    if let Some(cl) = content_len {
                        if let Some(hdr_end) = total.find("\r\n\r\n") {
                            let body_so_far = total.len() - (hdr_end + 4);
                            if body_so_far >= cl {
                                break;
                            }
                        }
                    } else if total.contains("\r\n\r\n") {
                        break;
                    }
                }
                {
                    let mut guard = captured_for_task.lock().await;
                    *guard = total.clone();
                }
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

    // ----- pure logic -------------------------------------------------------

    #[test]
    fn prefers_vllm_structured_outputs_truth_table() {
        for ep in [
            "http://localhost:8000/v1",
            "http://127.0.0.1:8000/v1",
            "http://0.0.0.0:8000/v1",
            "http://my-vllm-host:9000/v1",
            "https://VLLM.example.com/v1", // case-insensitive
        ] {
            assert!(
                ApiClient::new(config(ep, None)).prefers_vllm_structured_outputs(),
                "{ep} should prefer vLLM structured outputs"
            );
        }
        for ep in [
            "https://api.openai.com/v1",
            "http://localhost:11434/v1", // Ollama, different port
            "https://openrouter.ai/api/v1",
        ] {
            assert!(
                !ApiClient::new(config(ep, None)).prefers_vllm_structured_outputs(),
                "{ep} should NOT prefer vLLM structured outputs"
            );
        }
    }

    #[test]
    fn is_configured_requires_endpoint_and_model() {
        assert!(ApiClient::new(config("https://api.openai.com/v1", None)).is_configured());

        let no_endpoint = ApiClient::new(ApiConfig {
            endpoint: String::new(),
            ..config("x", None)
        });
        assert!(!no_endpoint.is_configured());

        let no_model = ApiClient::new(ApiConfig {
            model: String::new(),
            ..config("https://api.openai.com/v1", None)
        });
        assert!(!no_model.is_configured());
    }

    #[test]
    fn blocked_policy_rejects_chat_completion_before_http_request() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "should not be returned" } }]
        })
        .to_string();
        let (base, captured) = rt.block_on(spawn_mock(200, "OK", body));

        let api_key = "sk-api-policy-secret";
        let prompt = "patient said private diagnosis";
        let client = ApiClient::new(config(&base, Some(api_key))).with_content_egress_policy(
            crate::asr::ProviderContentEgressPolicy::block("local_only"),
        );

        let err = run_blocking(move || {
            client.chat_completion(vec![("user".to_string(), prompt.to_string())], false)
        })
        .expect_err("blocked policy must reject before cloud prompt egress");

        assert!(err.contains("Privacy policy blocked"), "got: {err}");
        assert!(err.contains("llm.api"), "got: {err}");
        assert!(err.contains("local_only"), "got: {err}");
        assert!(
            !err.contains(prompt),
            "policy error must not echo prompt text: {err}"
        );
        assert!(
            !err.contains(api_key),
            "policy error must not echo API key: {err}"
        );

        let req = rt.block_on(async { captured.lock().await.clone() });
        assert!(
            req.is_empty(),
            "blocked policy must return before building or sending HTTP request, got:\n{req}"
        );
    }

    #[test]
    fn default_policy_rejects_chat_completion_before_http_request() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "should not be returned" } }]
        })
        .to_string();
        let (base, captured) = rt.block_on(spawn_mock(200, "OK", body));

        let prompt = "patient said private diagnosis";
        let client = ApiClient::new(config(&base, Some("sk-api-default-secret")));

        let err = run_blocking(move || {
            client.chat_completion(vec![("user".to_string(), prompt.to_string())], false)
        })
        .expect_err("default policy must reject before cloud prompt egress");

        assert!(err.contains("Privacy policy blocked"), "got: {err}");
        assert!(err.contains("llm.api"), "got: {err}");
        assert!(err.contains("explicit_policy_required"), "got: {err}");
        assert!(
            !err.contains(prompt),
            "policy error must not echo prompt text: {err}"
        );

        let req = rt.block_on(async { captured.lock().await.clone() });
        assert!(
            req.is_empty(),
            "default policy must return before building or sending HTTP request, got:\n{req}"
        );
    }

    // ----- request shape via the blocking client + mock --------------------

    fn run_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        // `reqwest::blocking` cannot run inside an active tokio runtime, so the
        // client call must happen on a plain std thread (see openrouter.rs).
        std::thread::spawn(f).join().expect("worker thread panic")
    }

    #[test]
    fn chat_completion_success_parses_content() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "hi there" } }]
        })
        .to_string();
        let (base, _captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let reply = run_blocking(move || {
            client.chat_completion(vec![("user".to_string(), "hi".to_string())], false)
        });
        assert_eq!(reply.expect("ok"), "hi there");
    }

    #[test]
    fn chat_completion_with_usage_surfaces_total_tokens() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        // Canned response WITH an OpenAI-style usage block (FA-7c).
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "hi there" } }],
            "usage": { "prompt_tokens": 11, "completion_tokens": 6, "total_tokens": 17 }
        })
        .to_string();
        let (base, _captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let (text, tokens) = run_blocking(move || {
            client.chat_completion_with_usage(vec![("user".to_string(), "hi".to_string())], false)
        })
        .expect("ok");
        assert_eq!(text, "hi there");
        assert_eq!(
            tokens, 17,
            "usage.total_tokens from the response must flow through to the caller"
        );
    }

    #[test]
    fn chat_completion_with_usage_defaults_to_zero_when_block_absent() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        // No `usage` block at all — must parse cleanly and report 0, not error.
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "no usage here" } }]
        })
        .to_string();
        let (base, _captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let (text, tokens) = run_blocking(move || {
            client.chat_completion_with_usage(vec![("user".to_string(), "hi".to_string())], false)
        })
        .expect("missing usage block must not be a parse error");
        assert_eq!(text, "no usage here");
        assert_eq!(
            tokens, 0,
            "a provider that omits the usage block reports 0, never fabricated"
        );
    }

    #[test]
    fn chat_completion_json_mode_sets_response_format_and_auth() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "{}" } }]
        })
        .to_string();
        let (base, captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-secret")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let _ = run_blocking(move || {
            client.chat_completion(vec![("user".to_string(), "hi".to_string())], true)
        });

        let req = rt.block_on(async { captured.lock().await.clone() });
        let lc = req.to_ascii_lowercase();
        assert!(
            req.contains("\"response_format\":{\"type\":\"json_object\"}"),
            "json_mode must add response_format, got:\n{req}"
        );
        assert!(
            lc.contains("authorization: bearer sk-secret"),
            "non-empty api_key must produce bearer auth, got:\n{req}"
        );
        assert!(
            req.contains("/chat/completions"),
            "URL path must be /chat/completions, got:\n{req}"
        );
    }

    #[test]
    fn structured_outputs_request_carries_schema_and_usage() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "{\"operations\":[]}" } }],
            "usage": { "total_tokens": 19 }
        })
        .to_string();
        let (base, captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-secret")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let (text, tokens) = run_blocking(move || {
            client.chat_completion_with_structured_outputs_with_usage(
                vec![("user".to_string(), "patch".to_string())],
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operations": { "type": "array" }
                    }
                }),
            )
        })
        .expect("structured output response");

        assert_eq!(text, "{\"operations\":[]}");
        assert_eq!(tokens, 19);

        let req = rt.block_on(async { captured.lock().await.clone() });
        assert!(
            req.contains("\"structured_outputs\":{\"json\":"),
            "structured output request must carry schema metadata, got:\n{req}"
        );
        assert!(
            !req.contains("\"response_format\""),
            "schema-constrained requests must not also set json_object response_format, got:\n{req}"
        );
    }

    #[test]
    fn chat_completion_no_response_format_when_not_json_mode_and_no_auth_when_key_empty() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "ok" } }]
        })
        .to_string();
        let (base, captured) = rt.block_on(spawn_mock(200, "OK", body));

        // Empty-string key → no Authorization header (None-or-empty guard).
        let client = ApiClient::new(config(&base, Some("")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let _ = run_blocking(move || {
            client.chat_completion(vec![("user".to_string(), "hi".to_string())], false)
        });

        let req = rt.block_on(async { captured.lock().await.clone() });
        let lc = req.to_ascii_lowercase();
        assert!(
            !req.contains("response_format"),
            "non-json mode must omit response_format, got:\n{req}"
        );
        assert!(
            !lc.contains("authorization:"),
            "empty api_key must omit the Authorization header, got:\n{req}"
        );
    }

    #[test]
    fn chat_completion_non_2xx_uses_metadata_only_diagnostic() {
        let api_key = "sk-test-redact-api-client";
        let prompt_echo = "patient transcript and private graph context";
        let body = format!("slow down; echoed key {api_key}; echoed prompt {prompt_echo}");
        let err = api_error_message(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "https://provider.example/v1/chat/completions",
            &body,
            Some("req_123"),
        );

        assert!(
            err.contains("status=429"),
            "error must carry the status, got: {err}"
        );
        assert!(
            err.contains("provider=api"),
            "error must carry the provider, got: {err}"
        );
        assert!(
            err.contains("path=/v1/chat/completions"),
            "error must carry the request path, got: {err}"
        );
        assert!(
            err.contains("request_id=req_123"),
            "error must carry the provider request id, got: {err}"
        );
        assert!(
            err.contains(&format!("body_bytes={}", body.len())),
            "error must carry the body byte length, got: {err}"
        );
        assert!(
            err.contains(&format!("body_chars={}", body.chars().count())),
            "error must carry the body char length, got: {err}"
        );
        assert!(
            !err.contains(api_key),
            "error must redact the submitted key, got: {err}"
        );
        assert!(
            !err.contains("slow down") && !err.contains(prompt_echo),
            "error must not echo provider body or prompt context: {err}"
        );
    }

    #[test]
    fn chat_completion_empty_choices_reports_no_choices() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({ "choices": [] }).to_string();
        let (base, _captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let err = run_blocking(move || {
            client.chat_completion(vec![("user".to_string(), "hi".to_string())], false)
        })
        .expect_err("empty choices must be Err");
        assert!(err.contains("No response choices"), "got: {err}");
    }

    #[test]
    fn extract_entities_redacts_provider_output_on_parse_failure() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        // Valid chat response, but the content is not valid ExtractionResult JSON.
        let provider_output =
            "not json at all with patient diagnosis and sk-provider-output-secret";
        let body = serde_json::json!({
            "choices": [{ "message": { "content": provider_output } }]
        })
        .to_string();
        let (base, _captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let err = run_blocking(move || {
            client.extract_entities(
                "Alice met Bob about a patient diagnosis",
                "Alice",
                "Earlier private context",
            )
        })
        .expect_err("malformed extraction JSON must be Err");
        assert!(
            err.contains("Failed to parse extraction JSON from API"),
            "got: {err}"
        );
        assert!(
            err.contains("llm.api.extract_entities"),
            "parse error must include the extraction path, got: {err}"
        );
        assert!(
            err.contains("class=syntax"),
            "parse error must include a parse class, got: {err}"
        );
        assert!(
            err.contains("provider_output_bytes="),
            "parse error must include output length, got: {err}"
        );
        assert!(
            err.contains("provider_output_chars="),
            "parse error must include output length, got: {err}"
        );
        assert!(
            !err.contains(provider_output),
            "parse error must not echo provider output: {err}"
        );
        assert!(
            !err.contains("patient diagnosis"),
            "parse error must not echo transcript-derived content: {err}"
        );
        assert!(
            !err.contains("sk-provider-output-secret"),
            "parse error must not echo secret-like provider output: {err}"
        );
        assert!(
            !err.contains("Earlier private context"),
            "parse error must not echo prompt context: {err}"
        );
    }
}
