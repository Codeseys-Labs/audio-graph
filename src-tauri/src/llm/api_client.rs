//! OpenAI-compatible API client for LLM inference.
//!
//! Calls any OpenAI-compatible chat completions endpoint (OpenAI, Ollama,
//! LM Studio, vLLM, OpenRouter, Anthropic via proxy, etc.).
//! Used as an alternative to the native llama-cpp-2 engine.

use audio_graph_ipc_contract::runtime_diagnostic::{RuntimeDiagnostic, RuntimeErrorCode as Code};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::graph::entities::ExtractionResult;
use crate::llm::http_diag::{HttpFailure, INFERENCE, SchemaRejection, same_origin_redirects};
use crate::llm::route::{
    RequestOutputForm, WireOutcome, sanitize_served_model, terminal_status_from_finish_reason,
};

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
            // Presence only: an endpoint can embed a host, path, or credential.
            .field("has_endpoint", &!self.endpoint.is_empty())
            .field(
                "api_key",
                &crate::credentials::redacted_secret_presence(self.api_key.as_deref()),
            )
            // A local server's model id can be a file path.
            .field("has_model", &!self.model.is_empty())
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
    /// Legacy token cap. Accepted by Ollama / vLLM / LM Studio / OpenRouter and
    /// classic OpenAI chat models (gpt-4o etc.), but **rejected** by OpenAI
    /// reasoning / o-series models. Sent only when `max_completion_tokens` is
    /// not. See `token_limit_field` + the provider-API audit
    /// (`/tmp/provider-audit/llm_openai_compat.md` §3b).
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    /// Current OpenAI token cap. **Required** by o-series / reasoning models,
    /// which hard-reject `max_tokens` with HTTP 400. Sent only for those model
    /// families so generic OpenAI-compatible servers keep receiving `max_tokens`.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_outputs: Option<StructuredOutputs>,
}

/// Which token-cap field a given model id expects in the chat/completions body.
///
/// Provider-API audit (`/tmp/provider-audit/llm_openai_compat.md` §3b): OpenAI
/// deprecated `max_tokens` in favor of `max_completion_tokens`, and its
/// **reasoning / o-series** models (`o1`, `o3`, `o4-mini`, GPT-5 reasoning
/// tiers) **hard-reject** `max_tokens` with HTTP 400. Every other documented
/// target — Ollama, vLLM, LM Studio, OpenRouter, and classic OpenAI chat models
/// (gpt-4o / gpt-4 / gpt-3.5) — accepts `max_tokens` and may not understand
/// `max_completion_tokens`. So the correct compatibility rule is a per-model
/// switch keyed off the model id, NOT a blanket swap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TokenLimitField {
    /// Legacy `max_tokens` — the default for all non-reasoning endpoints.
    MaxTokens,
    /// `max_completion_tokens` — OpenAI reasoning / o-series models only.
    MaxCompletionTokens,
}

/// Decide the token-cap field from the model id.
///
/// Matches OpenAI reasoning-family id shapes only (`o1`, `o3`, `o4…`, `gpt-5…`),
/// which are the ids that reject `max_tokens`. Any other id — `llama3.2`,
/// `qwen2.5:3b`, `zai-glm-4.7`, `gpt-4o`, … — keeps `max_tokens`, so
/// Ollama / vLLM / OpenRouter / classic OpenAI are unaffected. Case-insensitive.
fn token_limit_field(model: &str) -> TokenLimitField {
    let id = model.trim().to_ascii_lowercase();

    // OpenAI reasoning / o-series: `o1`, `o1-mini`, `o3`, `o3-mini`, `o4-mini`,
    // etc. Match a leading `o` followed by a single digit so we don't snag
    // unrelated ids that merely start with "o" (e.g. "openchat", "olmo").
    let is_o_series = {
        let mut chars = id.chars();
        matches!(chars.next(), Some('o')) && matches!(chars.next(), Some(d) if d.is_ascii_digit())
    };

    // GPT-5 family reasoning tiers also require `max_completion_tokens`.
    let is_gpt5 = id.starts_with("gpt-5");

    if is_o_series || is_gpt5 {
        TokenLimitField::MaxCompletionTokens
    } else {
        TokenLimitField::MaxTokens
    }
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
    /// Present only for `type: "json_schema"` structured outputs. Omitted for
    /// `json_object` mode so that request stays byte-identical to the legacy
    /// shape.
    ///
    /// Before ADR-0038 this struct carried only `format_type`, so direct Cerebras
    /// — the one endpoint in the table that DOCUMENTS schema-constrained
    /// generation — literally could not express its own guarantee (ADR-0038
    /// defect 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    json_schema: Option<JsonSchemaSpec>,
}

/// The `json_schema` payload of an OpenAI-shape structured-outputs request.
/// Mirrors `openrouter::JsonSchemaSpec` field for field so both wire layers send
/// one identical body shape.
#[derive(Serialize)]
struct JsonSchemaSpec {
    name: String,
    strict: bool,
    schema: serde_json::Value,
}

impl ResponseFormat {
    /// `{ "type": "json_object" }` — generic JSON mode, no schema constraint.
    fn json_object() -> Self {
        Self {
            format_type: "json_object".to_string(),
            json_schema: None,
        }
    }

    /// `{ "type": "json_schema", "json_schema": { name, strict: true, schema } }`.
    fn json_schema(name: &str, schema: serde_json::Value) -> Self {
        Self {
            format_type: "json_schema".to_string(),
            json_schema: Some(JsonSchemaSpec {
                name: name.to_string(),
                strict: true,
                schema,
            }),
        }
    }
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
    /// Top-level `model` echo naming the model that actually served the request.
    /// Sanitized before it is recorded anywhere; can differ from the requested id.
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
    /// OpenAI-compatible stop signal. Dropping this field made a truncated
    /// response indistinguishable from a clean stop (ADR-0038 defect 1).
    #[serde(default)]
    finish_reason: Option<String>,
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
    /// `None` when the HTTP client could not be built. Dispatch then fails
    /// closed rather than retrying construction in a path that panics.
    client: Option<reqwest::blocking::Client>,
    content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

impl ApiClient {
    /// Create a new API client with the given configuration.
    pub fn new(config: ApiConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(API_CONNECT_TIMEOUT)
            .timeout(API_REQUEST_TIMEOUT)
            .redirect(same_origin_redirects())
            .build()
            .ok();
        #[cfg(test)]
        let client = client.filter(|_| !crate::llm::http_diag::client_build::take_failure());
        if client.is_none() {
            log::warn!(
                "API HTTP client setup failed: {}",
                HttpFailure::new(Code::Internal, INFERENCE)
            );
        }

        Self {
            config,
            client,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::block(
                "explicit_policy_required",
            ),
        }
    }

    /// As [`Self::new`], but refuses a client that could never dispatch. A
    /// cache must not keep one: the next build may succeed.
    pub(crate) fn try_new(config: ApiConfig) -> Result<Self, RuntimeDiagnostic> {
        let client = Self::new(config);
        if client.client.is_some() {
            Ok(client)
        } else {
            Err(HttpFailure::new(Code::Internal, INFERENCE).diagnostic)
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

    /// The `(budget, clamped)` completion cap this client will actually
    /// declare on the wire: `config.max_tokens` clamped to the endpoint this
    /// client's `endpoint` resolves to via [`crate::llm::route::route_for_api_endpoint`]
    /// (ADR-0038). Exposed as its own pure step — not just inlined into the
    /// request builder — so it is unit-testable without a live HTTP mock: the
    /// one endpoint with a documented completion ceiling
    /// (`route.cerebras_direct`, 40,960) is matched by exact host
    /// (`CEREBRAS_BASE_URL`), which a local mock server cannot impersonate.
    pub(crate) fn effective_completion_budget(&self) -> (u32, bool) {
        crate::llm::route::route_for_api_endpoint(&self.config.endpoint)
            .clamp_completion_budget(self.config.max_tokens)
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
        self.chat_completion_with_usage(messages, json_mode)
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
        let form = if json_mode {
            RequestOutputForm::JsonObject
        } else {
            RequestOutputForm::Unconstrained
        };
        self.chat_completion_with_wire_outcome(messages, form)
            .map(|(text, tokens, _wire)| (text, tokens))
            .map_err(String::from)
    }

    /// The route-contract entry point (ADR-0038): send one chat completion in the
    /// request form the resolved route selected, and return the reply, the token
    /// total, and the content-free [`WireOutcome`].
    ///
    /// The request form is chosen from the route's
    /// [`ConstrainedDecodingGrade`](crate::llm::route::ConstrainedDecodingGrade),
    /// not from a host substring — that is what lets direct Cerebras request its
    /// documented strict-decoding guarantee (ADR-0038 defect 2).
    pub(super) fn chat_completion_with_wire_outcome(
        &self,
        messages: Vec<(String, String)>,
        form: RequestOutputForm,
    ) -> Result<(String, u32, WireOutcome), HttpFailure> {
        self.content_egress_policy
            .check_prompt("llm.api")
            .map_err(|_| HttpFailure::new(Code::PolicyBlocked, INFERENCE))?;
        let Some(client) = &self.client else {
            return Err(HttpFailure::new(Code::Internal, INFERENCE));
        };

        let api_messages: Vec<ApiMessage> = messages
            .into_iter()
            .map(|(role, content)| ApiMessage { role, content })
            .collect();

        let (response_format, structured_outputs) = match &form {
            RequestOutputForm::Unconstrained => (None, None),
            RequestOutputForm::JsonObject => (Some(ResponseFormat::json_object()), None),
            RequestOutputForm::StrictJsonSchema { name, schema } => (
                Some(ResponseFormat::json_schema(name, schema.clone())),
                None,
            ),
            RequestOutputForm::VllmStructuredOutputs { schema } => (None, Some(schema.clone())),
        };

        // The per-endpoint-clamped completion budget (ADR-0038): a request can
        // never declare more completion tokens than the endpoint documents
        // (e.g. Cerebras's 40,960), regardless of what `llm_api_config.max_tokens`
        // a config file requests.
        let (clamped_budget, _clamped) = self.effective_completion_budget();

        // Route the token cap to the field this model family accepts: OpenAI
        // reasoning / o-series models require `max_completion_tokens` and reject
        // the legacy `max_tokens`; everything else keeps `max_tokens`. See
        // `token_limit_field` + the provider-API audit (§3b).
        let (max_tokens, max_completion_tokens) = match token_limit_field(&self.config.model) {
            TokenLimitField::MaxTokens => (Some(clamped_budget), None),
            TokenLimitField::MaxCompletionTokens => (None, Some(clamped_budget)),
        };

        let request = ChatCompletionRequest {
            model: self.config.model.clone(),
            messages: api_messages,
            max_tokens,
            max_completion_tokens,
            temperature: self.config.temperature,
            response_format,
            structured_outputs: structured_outputs.map(|json| StructuredOutputs { json }),
        };

        let url = format!(
            "{}/chat/completions",
            self.config.endpoint.trim_end_matches('/')
        );
        // The strongest grade this endpoint can be credited with, read from the
        // route table rather than assumed: only the direct-Cerebras row declares
        // `GuaranteedConstrained`.
        let constrained_decoding_ceiling =
            crate::llm::route::route_for_api_endpoint(&self.config.endpoint)
                .capability
                .constrained_decoding;

        let mut req = client.post(&url).json(&request);

        // Trim before deciding: a whitespace-only key ("   ") is effectively
        // unset, and sending it as a bearer token turns "no key" into a hard auth
        // failure (CodeRabbit api_client.rs:187).
        let key = self
            .config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty());
        if let Some(key) = key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        let response = req
            .send()
            .map_err(|error| HttpFailure::request(&error, INFERENCE))?;

        let status = response.status();
        if !status.is_success() {
            return Err(HttpFailure::status(status, INFERENCE, key.is_some()));
        }

        let completion: ChatCompletionResponse = response
            .json()
            .map_err(|error| HttpFailure::response(&error, INFERENCE))?;

        // A missing `usage` block (or a provider that omits `total_tokens`)
        // reports 0 — never fabricated.
        let total_tokens = completion.usage.map(|u| u.total_tokens).unwrap_or(0);
        // Cut to its final path component, then sanitized through the same
        // three guards the OpenRouter telemetry path uses, so neither a local
        // server's model path nor a key a hostile upstream echoes into `model`
        // can reach persisted provenance.
        let served_model = completion.model.as_deref().and_then(sanitize_served_model);

        let choice = completion
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| HttpFailure::new(Code::InvalidResponse, INFERENCE))?;

        let wire = WireOutcome {
            terminal_status: terminal_status_from_finish_reason(choice.finish_reason.as_deref()),
            served_model,
            // Only OpenRouter reports an upstream provider; a direct endpoint IS
            // the upstream.
            served_upstream_provider: None,
            constrained_decoding: form.achieved_grade(constrained_decoding_ceiling),
        };
        Ok((choice.message.content, total_tokens, wire))
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
        self.extract_entities_diagnostic(text, speaker, context)
            .map_err(String::from)
    }

    pub(super) fn extract_entities_diagnostic(
        &self,
        text: &str,
        speaker: &str,
        context: &str,
    ) -> Result<ExtractionResult, HttpFailure> {
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
            match self.chat_completion_with_wire_outcome(
                messages.clone(),
                RequestOutputForm::VllmStructuredOutputs { schema },
            ) {
                Ok((raw, _, _)) => raw,
                // A mode downgrade answers a refused request and nothing else.
                // Resending after any other failure would repeat a timeout's
                // unknown remote effect, bury a rate limit under the retry, or
                // ask a missing model twice.
                //
                // The evidence is the status alone, so a 400 or 422 for an
                // unrelated parameter also resends once. Reading the body to
                // tell them apart was rejected: only its message prose differs,
                // and nothing here classifies on prose. The resend goes to the
                // origin that already received this prompt.
                Err(error) if error.schema_rejection == Some(SchemaRejection::Request) => {
                    log::warn!("vLLM structured output failed; falling back to JSON mode: {error}");
                    self.chat_completion_with_wire_outcome(messages, RequestOutputForm::JsonObject)?
                        .0
                }
                Err(error) => return Err(error),
            }
        } else {
            self.chat_completion_with_wire_outcome(messages, RequestOutputForm::JsonObject)?
                .0
        };

        serde_json::from_str::<ExtractionResult>(&raw)
            .map_err(|_| HttpFailure::new(Code::InvalidResponse, INFERENCE))
    }

    fn extraction_json_schema() -> Result<serde_json::Value, HttpFailure> {
        serde_json::to_value(schemars::schema_for!(ExtractionResult))
            .map_err(|_| HttpFailure::new(Code::Internal, INFERENCE))
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

    /// Like [`chat_with_history_with_usage`](Self::chat_with_history_with_usage)
    /// but also returns the content-free [`WireOutcome`], so the executor can
    /// record the normalized terminal status for the dispatched route.
    pub(crate) fn chat_with_history_with_wire_outcome(
        &self,
        messages: &[crate::llm::engine::ChatMessage],
        graph_context: &str,
    ) -> Result<(String, u32, WireOutcome), RuntimeDiagnostic> {
        self.chat_completion_with_wire_outcome(
            self.history_messages(messages, graph_context),
            RequestOutputForm::Unconstrained,
        )
        .map_err(|failure| failure.diagnostic)
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
        assert!(debug.contains("has_endpoint: true"));
        assert!(!debug.contains("api.openai.com"));
        assert!(debug.contains("has_model: true"));
        assert!(!debug.contains("test-model"));
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
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nX-Request-Id: api-request-canary\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
    fn token_limit_field_selects_completion_tokens_for_reasoning_models() {
        // OpenAI reasoning / o-series + GPT-5 tiers require max_completion_tokens.
        for model in [
            "o1",
            "o1-mini",
            "o1-preview",
            "o3",
            "o3-mini",
            "o4-mini",
            "gpt-5",
            "gpt-5-mini",
            "O3-MINI", // case-insensitive
            "  o1  ",  // trimmed
        ] {
            assert_eq!(
                token_limit_field(model),
                TokenLimitField::MaxCompletionTokens,
                "{model} is a reasoning model and must use max_completion_tokens"
            );
        }

        // Generic OpenAI-compatible targets (Ollama/vLLM/OpenRouter) and classic
        // OpenAI chat models must keep the legacy max_tokens field.
        for model in [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4",
            "gpt-3.5-turbo",
            "llama3.2",
            "qwen2.5:3b",
            "zai-glm-4.7",
            "gpt-oss-120b",
            "mistral-small",
            "openchat", // starts with 'o' but not o-series
            "olmo-2",   // starts with 'o' but not o<digit>
        ] {
            assert_eq!(
                token_limit_field(model),
                TokenLimitField::MaxTokens,
                "{model} must keep the legacy max_tokens field"
            );
        }
    }

    #[test]
    fn request_uses_max_tokens_for_generic_openai_compatible_model() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "ok" } }]
        })
        .to_string();
        let (base, captured) = rt.block_on(spawn_mock(200, "OK", body));

        // Default `config` uses model "test-model" — a generic (non-reasoning) id.
        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let _ = run_blocking(move || {
            client.chat_completion(vec![("user".to_string(), "hi".to_string())], false)
        });

        let req = rt.block_on(async { captured.lock().await.clone() });
        assert!(
            req.contains("\"max_tokens\":64"),
            "generic model must send legacy max_tokens, got:\n{req}"
        );
        assert!(
            !req.contains("max_completion_tokens"),
            "generic model must NOT send max_completion_tokens, got:\n{req}"
        );
    }

    #[test]
    fn request_uses_max_completion_tokens_for_reasoning_model() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "ok" } }]
        })
        .to_string();
        let (base, captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(ApiConfig {
            model: "o3-mini".to_string(),
            ..config(&base, Some("sk-test"))
        })
        .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let _ = run_blocking(move || {
            client.chat_completion(vec![("user".to_string(), "hi".to_string())], false)
        });

        let req = rt.block_on(async { captured.lock().await.clone() });
        assert!(
            req.contains("\"max_completion_tokens\":64"),
            "reasoning model must send max_completion_tokens, got:\n{req}"
        );
        // The bare `"max_tokens"` key must be absent (guard against matching the
        // `max_completion_tokens` substring by requiring the exact key form).
        assert!(
            !req.contains("\"max_tokens\":"),
            "reasoning model must NOT send legacy max_tokens (rejected by o-series), got:\n{req}"
        );
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

        assert_eq!(err, "runtime/policy_blocked");
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

        assert_eq!(err, "runtime/policy_blocked");
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
        let (text, tokens, _) = run_blocking(move || {
            client.chat_completion_with_wire_outcome(
                vec![("user".to_string(), "patch".to_string())],
                RequestOutputForm::VllmStructuredOutputs {
                    schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "operations": { "type": "array" }
                        }
                    }),
                },
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
    fn chat_completion_non_2xx_returns_only_the_diagnostic_token() {
        use crate::llm::http_diag::canary::assert_absent;
        use crate::test_log_capture::{ThreadLogCapture, render};

        let rt = tokio::runtime::Runtime::new().expect("rt");
        let key = "api-credential-canary";
        let body = format!("provider-private-body-canary{}", "b".repeat(5000));
        let path = "private-endpoint-canary";
        let prompt = format!("api-prompt-canary{}", "x".repeat(2048));
        // Exact sizes of the prompt and of the response body.
        let lengths = [prompt.len(), body.len()].map(|len| len.to_string());
        let (base, _) = rt.block_on(spawn_mock(429, "Too Many Requests", body));
        let endpoint = format!("{base}/{path}?private=query-canary");
        let (result, logs) = run_blocking(move || {
            let capture = ThreadLogCapture::start();
            let result = ApiClient::new(config(&endpoint, Some(key)))
                .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow())
                .chat_completion(vec![("user".into(), prompt)], false);
            (result, render(&capture.finish()))
        });
        let err = result.expect_err("provider rejected the request");
        assert_eq!(err, "runtime/rate_limited");
        // Nothing on this path logs today; the capture is for the day something does.
        let observed = format!("{err}\n{logs}");
        assert_absent(
            &observed,
            &[
                key,
                "provider-private-body-canary",
                path,
                "query-canary",
                "api-request-canary",
                "api-prompt-canary",
            ],
        );
        assert_absent(&observed, &lengths.each_ref().map(String::as_str));
    }

    #[test]
    fn a_rejected_key_is_a_credential_failure_only_when_a_key_was_sent() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        for (key, expected) in [
            (Some("sk-test"), "runtime/credential_rejected"),
            (None, "runtime/request_rejected"),
        ] {
            let (base, _) = rt.block_on(spawn_mock(401, "Unauthorized", String::new()));
            let err = run_blocking(move || {
                ApiClient::new(config(&base, key))
                    .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow())
                    .chat_completion(vec![("user".into(), "hi".into())], false)
            })
            .expect_err("the provider answered 401");
            assert_eq!(err, expected, "key sent: {}", key.is_some());
        }
    }

    #[test]
    fn vllm_extraction_resends_only_after_a_rejected_request() {
        // The mock answers one request. A JSON-mode resend would fail some
        // other way and replace the status the caller needs to see.
        let rt = tokio::runtime::Runtime::new().expect("rt");
        for (status, expected) in [
            (429, "runtime/rate_limited"),
            (404, "runtime/request_rejected"),
        ] {
            let (base, _) = rt.block_on(spawn_mock(status, "Refused", String::new()));
            let client = ApiClient::new(config(&format!("{base}/vllm/v1"), None))
                .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
            assert!(client.prefers_vllm_structured_outputs());

            let err = run_blocking(move || client.extract_entities("Alice met Bob", "Alice", ""))
                .expect_err("the endpoint refused the request");
            assert_eq!(err, expected, "status {status}");
        }
    }

    #[test]
    fn a_client_that_failed_to_initialize_fails_closed() {
        let mut client = ApiClient::new(config("http://127.0.0.1:1/v1", None))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        client.client = None;

        let err = client
            .chat_completion(vec![("user".into(), "hi".into())], false)
            .expect_err("there is no HTTP client to dispatch with");
        assert_eq!(err, "runtime/internal");
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
        assert_eq!(err, "runtime/invalid_response");
    }

    // ----- ADR-0038 defect (a): finish_reason on the generic blocking path ---

    #[test]
    fn chat_completion_maps_length_finish_reason_to_truncated() {
        use crate::llm::route::TerminalStatus;
        let rt = tokio::runtime::Runtime::new().expect("rt");
        // OpenAI-compatible servers (and OpenRouter) turn some length overruns
        // into HTTP 200 + finish_reason "length" with a truncated body.
        let body = serde_json::json!({
            "choices": [{
                "message": { "content": "{\"operations\":[" },
                "finish_reason": "length"
            }]
        })
        .to_string();
        let (base, _captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let (_text, _tokens, wire) = run_blocking(move || {
            client.chat_completion_with_wire_outcome(
                vec![("user".to_string(), "hi".to_string())],
                RequestOutputForm::JsonObject,
            )
        })
        .expect("a 200 with finish_reason=length still decodes");
        assert_eq!(wire.terminal_status, TerminalStatus::Truncated);
    }

    #[test]
    fn chat_completion_absent_finish_reason_is_completed() {
        use crate::llm::route::TerminalStatus;
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "{}" } }]
        })
        .to_string();
        let (base, _captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let (_text, _tokens, wire) = run_blocking(move || {
            client.chat_completion_with_wire_outcome(
                vec![("user".to_string(), "hi".to_string())],
                RequestOutputForm::JsonObject,
            )
        })
        .expect("ok");
        assert_eq!(wire.terminal_status, TerminalStatus::Completed);
    }

    // ----- completion-budget clamp is actually wired into the wire request --

    #[test]
    fn effective_completion_budget_clamps_direct_cerebras_but_not_a_generic_endpoint() {
        // The one endpoint in the route table with a documented completion
        // ceiling: a config file requesting far more than 40,960 must be
        // clamped BEFORE it reaches `chat_completion_with_wire_outcome`'s
        // request builder — this is the wiring finding 1/4 required.
        let mut cerebras_config = config(crate::settings::CEREBRAS_BASE_URL, Some("sk-test"));
        cerebras_config.max_tokens = 262_144;
        let cerebras_client = ApiClient::new(cerebras_config);
        assert_eq!(
            cerebras_client.effective_completion_budget(),
            (40_960, true),
            "a config requesting more than Cerebras's documented 40,960 must be clamped"
        );

        // The shipped defaults (2048 / 512) stay far under the ceiling, so the
        // clamp is structural, not load-bearing, for those.
        let mut under_budget = config(crate::settings::CEREBRAS_BASE_URL, Some("sk-test"));
        under_budget.max_tokens = 2_048;
        assert_eq!(
            ApiClient::new(under_budget).effective_completion_budget(),
            (2_048, false)
        );

        // A generic OpenAI-compatible endpoint has no documented ceiling in
        // this table, so the same oversized request passes through unclamped
        // — a fabricated cap would be worse than an absent one.
        let mut generic_config = config("https://api.openai.com/v1", Some("sk-test"));
        generic_config.max_tokens = 262_144;
        assert_eq!(
            ApiClient::new(generic_config).effective_completion_budget(),
            (262_144, false)
        );
    }

    // ----- ADR-0038 defect (b): direct Cerebras requests strict decoding -----

    #[test]
    fn cerebras_route_sends_strict_json_schema_response_format() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "{\"operations\":[]}" } }]
        })
        .to_string();
        let (base, captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let _ = run_blocking(move || {
            client.chat_completion_with_wire_outcome(
                vec![("user".to_string(), "patch".to_string())],
                RequestOutputForm::StrictJsonSchema {
                    name: "projection_patch".to_string(),
                    schema: serde_json::json!({
                        "type": "object",
                        "properties": { "operations": { "type": "array" } }
                    }),
                },
            )
        });

        let req = rt.block_on(async { captured.lock().await.clone() });
        assert!(
            req.contains("\"response_format\":{\"type\":\"json_schema\""),
            "a GuaranteedConstrained route must request json_schema, got:\n{req}"
        );
        assert!(
            req.contains("\"strict\":true"),
            "the strict flag is the documented decoding guarantee, got:\n{req}"
        );
        assert!(
            !req.contains("\"structured_outputs\""),
            "structured_outputs is the vLLM-only field, got:\n{req}"
        );
    }

    #[test]
    fn vllm_form_still_sends_structured_outputs_not_json_schema() {
        // The vLLM body field is a separate, non-standard extension: an endpoint
        // selected by `prefers_vllm_structured_outputs()` must keep receiving it,
        // and must NOT be handed a `response_format` at the same time.
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "{\"operations\":[]}" } }]
        })
        .to_string();
        let (base, captured) = rt.block_on(spawn_mock(200, "OK", body));

        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let _ = run_blocking(move || {
            client.chat_completion_with_wire_outcome(
                vec![("user".to_string(), "patch".to_string())],
                RequestOutputForm::VllmStructuredOutputs {
                    schema: serde_json::json!({ "type": "object" }),
                },
            )
        });

        let req = rt.block_on(async { captured.lock().await.clone() });
        assert!(
            req.contains("\"structured_outputs\":{\"json\":"),
            "the vLLM form must carry structured_outputs, got:\n{req}"
        );
        assert!(
            !req.contains("\"response_format\""),
            "the vLLM form must not also set response_format, got:\n{req}"
        );
    }

    #[test]
    fn strict_schema_rejection_preserves_private_downgrade_evidence() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let (base, _captured) = rt.block_on(spawn_mock(
            400,
            "Bad Request",
            "schema unsupported".to_string(),
        ));

        let client = ApiClient::new(config(&base, Some("sk-test")))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let err = run_blocking(move || {
            client.chat_completion_with_wire_outcome(
                vec![("user".to_string(), "patch".to_string())],
                RequestOutputForm::StrictJsonSchema {
                    name: "projection_patch".to_string(),
                    schema: serde_json::json!({ "type": "object" }),
                },
            )
        })
        .expect_err("a 400 is an error");
        assert_eq!(
            err.schema_rejection,
            Some(crate::llm::http_diag::SchemaRejection::Request)
        );
        assert_eq!(err.to_string(), "runtime/request_rejected");
    }

    #[test]
    fn json_object_mode_records_unconstrained_even_on_a_guaranteed_endpoint() {
        // ADR-0038 sub-decision 5's honesty requirement: the recorded grade is what
        // the request form ACHIEVED, not the route's declared ambition. A JSON-mode
        // call is `Unconstrained` regardless of what the endpoint could have done.
        use crate::llm::route::ConstrainedDecodingGrade;
        assert_eq!(
            RequestOutputForm::JsonObject
                .achieved_grade(ConstrainedDecodingGrade::GuaranteedConstrained),
            ConstrainedDecodingGrade::Unconstrained
        );
        assert_eq!(
            RequestOutputForm::StrictJsonSchema {
                name: "n".to_string(),
                schema: serde_json::json!({})
            }
            .achieved_grade(ConstrainedDecodingGrade::GuaranteedConstrained),
            ConstrainedDecodingGrade::GuaranteedConstrained
        );
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
        assert_eq!(err, "runtime/invalid_response");
    }
}
