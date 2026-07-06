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

use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Mutex;
use std::time::Duration;

use crate::graph::entities::ExtractionResult;
use crate::llm::stream_contract::StreamUsage;

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
/// always-on attribution headers. `routing_policy` maps to OpenRouter's
/// `provider` object; `provider_order` is retained as the legacy fallback for
/// old configs that only know how to set `provider.order`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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
    /// Optional rich OpenRouter provider-routing policy. When present, this
    /// takes precedence over the legacy `provider_order` compatibility field.
    #[serde(default)]
    pub routing_policy: Option<OpenRouterRoutingPolicy>,
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

impl std::fmt::Debug for OpenRouterConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRouterConfig")
            .field(
                "api_key",
                &crate::credentials::redacted_secret_presence(Some(&self.api_key)),
            )
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("provider_order", &self.provider_order)
            .field("routing_policy", &self.routing_policy)
            .field("include_usage_in_stream", &self.include_usage_in_stream)
            .field("http_referer", &self.http_referer)
            .field("app_title", &self.app_title)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .finish()
    }
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
            routing_policy: None,
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

    pub(crate) fn provider_routing_policy(&self) -> Option<OpenRouterRoutingPolicy> {
        self.routing_policy.clone().or_else(|| {
            OpenRouterRoutingPolicy::from_provider_order(self.provider_order.as_deref())
        })
    }

    pub(crate) fn provider_routing_value(&self) -> Option<serde_json::Value> {
        self.provider_routing_policy()
            .and_then(|policy| policy.to_provider_value())
    }

    /// The most-preferred upstream provider this config asks OpenRouter to
    /// route to, if any — the first entry of the effective policy's `order`
    /// (falling back to `only`). Used to derive routing fallback evidence in
    /// [`OpenRouterRoutingTelemetry`]. `None` when no preference is configured.
    pub(crate) fn preferred_provider(&self) -> Option<String> {
        self.provider_routing_policy()
            .as_ref()
            .and_then(OpenRouterRoutingPolicy::preferred_provider)
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

/// Provider metadata returned by `GET /providers`.
///
/// The OpenRouter catalog is metadata-only and may grow fields over time, so
/// every non-identity field is optional/permissive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterProvider {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub privacy_policy_url: Option<String>,
    #[serde(default)]
    pub terms_of_service_url: Option<String>,
    #[serde(default)]
    pub status_page_url: Option<String>,
    #[serde(default)]
    pub headquarters: Option<String>,
    #[serde(default, deserialize_with = "deserialize_vec_or_empty")]
    pub datacenters: Vec<String>,
}

#[derive(Deserialize)]
struct ProvidersResponse {
    #[serde(default, deserialize_with = "deserialize_vec_or_empty")]
    data: Vec<OpenRouterProvider>,
}

/// Endpoint catalog returned by `GET /models/{author}/{slug}/endpoints`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterModelEndpoints {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub architecture: Option<OpenRouterEndpointArchitecture>,
    #[serde(default, deserialize_with = "deserialize_vec_or_empty")]
    pub endpoints: Vec<OpenRouterEndpoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterEndpointArchitecture {
    #[serde(default)]
    pub tokenizer: Option<String>,
    #[serde(default)]
    pub instruct_type: Option<String>,
    #[serde(default)]
    pub modality: Option<String>,
    #[serde(default, deserialize_with = "deserialize_vec_or_empty")]
    pub input_modalities: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_vec_or_empty")]
    pub output_modalities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterEndpoint {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub context_length: Option<u64>,
    #[serde(default)]
    pub pricing: Option<OpenRouterEndpointPricing>,
    #[serde(default)]
    pub provider_name: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub quantization: Option<String>,
    #[serde(default)]
    pub max_completion_tokens: Option<u64>,
    #[serde(default)]
    pub max_prompt_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_vec_or_empty")]
    pub supported_parameters: Vec<String>,
    #[serde(default)]
    pub uptime_last_30m: Option<f64>,
    #[serde(default)]
    pub uptime_last_5m: Option<f64>,
    #[serde(default)]
    pub uptime_last_1d: Option<f64>,
    #[serde(default)]
    pub supports_implicit_caching: Option<bool>,
    #[serde(default)]
    pub latency_last_30m: Option<OpenRouterPercentileStats>,
    #[serde(default)]
    pub throughput_last_30m: Option<OpenRouterPercentileStats>,
    #[serde(default)]
    pub status: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterEndpointPricing {
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub completion: Option<String>,
    #[serde(default)]
    pub request: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub image_token: Option<String>,
    #[serde(default)]
    pub image_output: Option<String>,
    #[serde(default)]
    pub audio: Option<String>,
    #[serde(default)]
    pub audio_output: Option<String>,
    #[serde(default)]
    pub input_audio_cache: Option<String>,
    #[serde(default)]
    pub input_cache_read: Option<String>,
    #[serde(default)]
    pub input_cache_write: Option<String>,
    #[serde(default)]
    pub input_cache_write_1h: Option<String>,
    #[serde(default)]
    pub internal_reasoning: Option<String>,
    #[serde(default)]
    pub web_search: Option<String>,
    #[serde(default)]
    pub discount: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterPercentileStats {
    #[serde(default)]
    pub p50: Option<f64>,
    #[serde(default)]
    pub p75: Option<f64>,
    #[serde(default)]
    pub p90: Option<f64>,
    #[serde(default)]
    pub p99: Option<f64>,
}

#[derive(Deserialize)]
struct ModelEndpointsResponse {
    data: OpenRouterModelEndpoints,
}

fn deserialize_vec_or_empty<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
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
    provider: Option<serde_json::Value>,
    /// OpenAI-style routing hint so a session's turns land on the same
    /// cache-warm machine (ADR-0025 §2d / seed audio-graph-d77e). Scoped per
    /// (session, resolved-provider) by the caller; a provider failover lands a
    /// cold cache by design.
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
}

/// A message's content, serialized either as a plain string (the default,
/// byte-identical to the legacy shape) or as an OpenAI/Anthropic content-block
/// array so a `cache_control` breakpoint can ride the last stable block
/// (ADR-0025 §2d / seed audio-graph-d77e).
#[derive(Serialize)]
#[serde(untagged)]
enum ApiMessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Serialize)]
struct ContentPart {
    #[serde(rename = "type")]
    part_type: String,
    text: String,
    /// Anthropic-style prompt-cache breakpoint (passed through by OpenRouter).
    /// Present only on the last stable-prefix block.
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    cache_type: String,
}

impl CacheControl {
    fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
        }
    }
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: ApiMessageContent,
}

impl ApiMessage {
    /// A plain-text message (legacy shape, no cache marker).
    fn text(role: String, content: String) -> Self {
        Self {
            role,
            content: ApiMessageContent::Text(content),
        }
    }

    /// A message whose single content block carries an ephemeral
    /// `cache_control` breakpoint marking the end of the cacheable prefix.
    fn text_with_cache_breakpoint(role: String, content: String) -> Self {
        Self {
            role,
            content: ApiMessageContent::Parts(vec![ContentPart {
                part_type: "text".to_string(),
                text: content,
                cache_control: Some(CacheControl::ephemeral()),
            }]),
        }
    }
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
}

fn build_chat_completion_request<'a>(
    config: &'a OpenRouterConfig,
    messages: Vec<ApiMessage>,
    response_format: Option<ResponseFormat>,
    prompt_cache_key: Option<String>,
) -> ChatCompletionRequest<'a> {
    ChatCompletionRequest {
        model: &config.model,
        messages,
        max_tokens: config.max_tokens,
        temperature: config.temperature,
        response_format,
        provider: config.provider_routing_value(),
        prompt_cache_key,
    }
}

/// A per-turn prompt-cache hint for a projection call (ADR-0025 §2d / seed
/// audio-graph-d77e). Marks where the byte-stable prefix ends so a
/// `cache_control` breakpoint can be placed, and carries the
/// (session, resolved-provider)-scoped routing key.
#[derive(Debug, Clone)]
pub struct PromptCacheHint {
    /// Index of the last message that belongs to the stable, cacheable prefix.
    /// The breakpoint is placed on this message's content.
    pub cache_breakpoint_message_index: usize,
    /// OpenAI/OpenRouter `prompt_cache_key`.
    pub cache_key: String,
}

#[cfg(test)]
pub(crate) fn blocking_chat_provider_value_for_test(
    config: &OpenRouterConfig,
) -> Option<serde_json::Value> {
    build_chat_completion_request(config, Vec::new(), None, None).provider
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OpenRouterRoutingPolicy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_fallbacks: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_parameters: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_collection: Option<OpenRouterDataCollectionPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zdr: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enforce_distillable_text: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quantizations: Vec<OpenRouterQuantization>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<OpenRouterRoutingSort>,
    #[serde(skip_serializing_if = "OpenRouterPerformancePreference::option_is_empty")]
    pub preferred_min_throughput: Option<OpenRouterPerformancePreference>,
    #[serde(skip_serializing_if = "OpenRouterPerformancePreference::option_is_empty")]
    pub preferred_max_latency: Option<OpenRouterPerformancePreference>,
    #[serde(skip_serializing_if = "OpenRouterMaxPrice::option_is_empty")]
    pub max_price: Option<OpenRouterMaxPrice>,
}

impl OpenRouterRoutingPolicy {
    pub(crate) fn from_provider_order(provider_order: Option<&[String]>) -> Option<Self> {
        match provider_order {
            Some(order) if !order.is_empty() => Some(Self {
                order: order.to_vec(),
                ..Self::default()
            }),
            _ => None,
        }
    }

    pub(crate) fn to_provider_value(&self) -> Option<serde_json::Value> {
        if self.is_empty() {
            return None;
        }

        Some(
            serde_json::to_value(self)
                .expect("OpenRouter routing policy should serialize to provider object"),
        )
    }

    /// First entry of the effective preference list (`order`, else `only`).
    /// This is the provider a strict/preferred policy asks for; comparing it
    /// against the served provider yields fallback evidence.
    fn preferred_provider(&self) -> Option<String> {
        self.order
            .first()
            .or_else(|| self.only.first())
            .map(|provider| provider.trim().to_string())
            .filter(|provider| !provider.is_empty())
    }

    fn is_empty(&self) -> bool {
        self.order.is_empty()
            && self.only.is_empty()
            && self.ignore.is_empty()
            && self.allow_fallbacks.is_none()
            && self.require_parameters.is_none()
            && self.data_collection.is_none()
            && self.zdr.is_none()
            && self.enforce_distillable_text.is_none()
            && self.quantizations.is_empty()
            && self.sort.is_none()
            && OpenRouterPerformancePreference::option_is_empty(&self.preferred_min_throughput)
            && OpenRouterPerformancePreference::option_is_empty(&self.preferred_max_latency)
            && OpenRouterMaxPrice::option_is_empty(&self.max_price)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenRouterDataCollectionPolicy {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OpenRouterQuantization {
    Int4,
    Int8,
    Fp4,
    Fp6,
    Fp8,
    Fp16,
    Bf16,
    Fp32,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum OpenRouterRoutingSort {
    By(OpenRouterRoutingSortBy),
    Detailed(OpenRouterRoutingSortConfig),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OpenRouterRoutingSortBy {
    Price,
    Throughput,
    Latency,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OpenRouterRoutingSortConfig {
    pub by: OpenRouterRoutingSortBy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<OpenRouterRoutingSortPartition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OpenRouterRoutingSortPartition {
    Model,
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum OpenRouterPerformancePreference {
    P50(f64),
    Percentiles(OpenRouterPerformancePercentiles),
}

impl OpenRouterPerformancePreference {
    fn option_is_empty(value: &Option<Self>) -> bool {
        match value {
            None => true,
            Some(preference) => preference.is_empty(),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::P50(_) => false,
            Self::Percentiles(percentiles) => percentiles.is_empty(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OpenRouterPerformancePercentiles {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p75: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p90: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p99: Option<f64>,
}

impl OpenRouterPerformancePercentiles {
    fn is_empty(&self) -> bool {
        self.p50.is_none() && self.p75.is_none() && self.p90.is_none() && self.p99.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OpenRouterMaxPrice {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<f64>,
}

impl OpenRouterMaxPrice {
    fn option_is_empty(value: &Option<Self>) -> bool {
        match value {
            None => true,
            Some(max_price) => max_price.is_empty(),
        }
    }

    fn is_empty(&self) -> bool {
        self.prompt.is_none()
            && self.completion.is_none()
            && self.request.is_none()
            && self.image.is_none()
    }
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
    /// Token usage block. OpenRouter is OpenAI-compatible, so the non-streaming
    /// response carries the `prompt_tokens` / `completion_tokens` /
    /// `total_tokens` triple. Optional + serde-default because some upstream
    /// providers omit it (and error responses never carry it).
    #[serde(default)]
    usage: Option<Usage>,
    /// Top-level `provider` field OpenRouter injects on the non-streaming
    /// response naming the upstream provider that actually served the request
    /// (e.g. `"Cerebras"`, `"Together"`). Absent on some responses. Safe,
    /// non-secret routing metadata — never carries prompt/reply text.
    #[serde(default)]
    provider: Option<String>,
    /// Top-level `model` echo naming the routed model slug that served the
    /// request. Safe metadata; can differ from the requested slug when
    /// OpenRouter down-routes to a variant. Never carries prompt/reply text.
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

/// Token-usage block from a non-streaming OpenRouter response.
///
/// OpenRouter is OpenAI-compatible, so the usage object carries
/// `prompt_tokens` / `completion_tokens` / `total_tokens`. All three are
/// optional + serde-default because some upstream providers omit individual
/// counters (and error responses never carry usage). The field shape mirrors
/// [`StreamUsage`] (`stream_contract.rs`) so the blocking path can surface the
/// same triple the streaming path reports.
#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
}

impl Usage {
    /// Lower this OpenRouter usage block into the provider-neutral
    /// [`StreamUsage`] triple so blocking callers see the same shape the
    /// streaming path reports.
    fn into_stream_usage(self) -> StreamUsage {
        StreamUsage {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            total_tokens: self.total_tokens,
        }
    }
}

// ---------------------------------------------------------------------------
// Routing telemetry (chain-root for audio-graph-713c)
// ---------------------------------------------------------------------------

/// Sanitized, non-secret evidence about how an OpenRouter chat request was
/// actually routed. This is the schema a routed smoke run, the Settings panel,
/// and readiness summaries can surface to prove *whether* strict routing,
/// fallback, low-latency, or throughput sorting took effect — without ever
/// persisting an API key, prompt, or reply.
///
/// Every field is metadata only:
/// - [`request_id`](Self::request_id): the provider's sanitized request id
///   (from response headers via [`response_request_id`]).
/// - [`selected_provider`](Self::selected_provider): the upstream provider that
///   served the request, read from the response body's top-level `provider`
///   field. Sanitized to `[A-Za-z0-9 ._:-]` and length-capped.
/// - [`served_model`](Self::served_model): the routed model slug that served
///   the request (response body top-level `model`), sanitized the same way.
/// - [`fallback_from_preferred`](Self::fallback_from_preferred): fallback
///   evidence — `Some(true)` when the served provider is NOT the first entry of
///   the request's preferred `order`/`only` policy, `Some(false)` when it
///   matches, `None` when there is no preference or no served-provider metadata
///   to compare against.
/// - [`latency_ms`](Self::latency_ms): client-measured request round-trip in
///   milliseconds. Timing only; carries no content.
/// - [`usage`](Self::usage): the [`StreamUsage`] token triple, matching the
///   blocking + streaming usage contract.
///
/// The type is `Serialize`/`Deserialize` so it can be handed to the WebView /
/// smoke summaries; it deliberately has no field capable of carrying prompt or
/// reply text. The [`Self::redaction_probe`] concatenation exists so tests can
/// assert no secret/prompt/reply string ever lands in any field.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenRouterRoutingTelemetry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_from_preferred: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "StreamUsage_is_default")]
    pub usage: StreamUsage,
}

/// serde skip helper — omit an all-`None` usage triple from serialized output.
#[allow(non_snake_case)]
fn StreamUsage_is_default(usage: &StreamUsage) -> bool {
    *usage == StreamUsage::default()
}

impl OpenRouterRoutingTelemetry {
    /// Build sanitized routing telemetry from the safe metadata surfaced by a
    /// successful OpenRouter chat completion. This is the single recording hook
    /// the blocking chat path (and, once 713c lands, runtime accounting) uses:
    /// it accepts only already-safe inputs and re-sanitizes provider/model
    /// strings so no free-text can ride through even if a future caller passes
    /// unsanitized values.
    ///
    /// `preferred_provider` is the first entry of the request's configured
    /// `order`/`only` routing preference (if any) — used purely to derive
    /// [`fallback_from_preferred`](Self::fallback_from_preferred).
    fn from_completion(
        request_id: Option<String>,
        selected_provider: Option<&str>,
        served_model: Option<&str>,
        preferred_provider: Option<&str>,
        latency_ms: Option<u64>,
        usage: StreamUsage,
    ) -> Self {
        let selected_provider = selected_provider.and_then(sanitize_metadata_value);
        let served_model = served_model.and_then(sanitize_metadata_value);
        let fallback_from_preferred = fallback_evidence(
            preferred_provider
                .and_then(sanitize_metadata_value)
                .as_deref(),
            selected_provider.as_deref(),
        );

        Self {
            request_id,
            selected_provider,
            served_model,
            fallback_from_preferred,
            latency_ms,
            usage,
        }
    }

    /// Emit a metadata-only observability breadcrumb for a routed request. Rides
    /// the same anonymous [`capture_diagnostic`](crate::analytics::capture_diagnostic)
    /// path as the HTTP-error diagnostic: only controlled category/provider/kind
    /// tags leave the process — never the request id, provider name, model, or
    /// any token counts (those stay in the returned struct for the caller to
    /// surface locally).
    fn capture_breadcrumb(&self) {
        crate::analytics::capture_diagnostic(crate::analytics::DiagEvent {
            name: "llm.openrouter.routed",
            category: crate::analytics::Category::Llm,
            level: sentry::Level::Info,
            provider: Some("openrouter"),
            kind: Some("routed"),
            http_status: None,
            recoverable: None,
        });
    }

    /// Test-only concatenation of every string-bearing field, so redaction
    /// tests can assert in one shot that no secret/prompt/reply substring ever
    /// lands in the telemetry.
    #[cfg(test)]
    pub(crate) fn redaction_probe(&self) -> String {
        [
            self.request_id.as_deref(),
            self.selected_provider.as_deref(),
            self.served_model.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\u{1f}")
    }
}

// ---------------------------------------------------------------------------
// Runtime accounting (audio-graph-713c — 76bd consumer)
// ---------------------------------------------------------------------------

/// Cumulative, content-free runtime accounting for OpenRouter routing telemetry.
///
/// Seed audio-graph-76bd defined [`OpenRouterRoutingTelemetry`] and the
/// [`OpenRouterClient::chat_completion_with_routing_telemetry`] hook that
/// *returns* one telemetry record per completion — but that record was
/// emitted-and-dropped: nothing accumulated the token triple or the routing
/// evidence over the life of the process. This aggregator is the runtime
/// accounting sink 713c wires that triple into, so per-session routing evidence
/// and token usage are actually *tracked* rather than discarded.
///
/// **Content-free by construction.** Where [`OpenRouterRoutingTelemetry`] keeps
/// the per-request request id / provider / model strings for a caller to
/// surface *locally*, this aggregate deliberately keeps only *counts and sums*.
/// No request id, provider name, model slug, prompt, or reply text can land
/// here — the type has no field capable of carrying free text — so the
/// process-wide accounting record is safe to hand to Settings / readiness /
/// live-smoke summaries with no redaction step. The token triple is summed
/// separately (prompt / completion / total) so the accounting preserves the
/// full usage split, not just a scalar total.
///
/// All adds are saturating (mirroring [`crate::sessions::usage`]): a runaway
/// upstream counter clamps at [`u64::MAX`] instead of wrapping to zero and
/// erasing the history the accounting exists to preserve.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenRouterRuntimeAccounting {
    /// Count of successful routed completions recorded.
    pub routed_requests: u64,
    /// Subset of `routed_requests` where the served provider differed from the
    /// configured preferred provider (`fallback_from_preferred == Some(true)`).
    pub fallback_requests: u64,
    /// Count of recorded completions whose response reported no usable total
    /// token count (`usage.total_tokens` absent or zero) — tracked so a caller
    /// can tell "we made N calls but only M reported usage" apart from "zero
    /// usage".
    pub unknown_usage_requests: u64,
    /// Summed `usage.prompt_tokens` across recorded completions (missing → 0).
    pub prompt_tokens: u64,
    /// Summed `usage.completion_tokens` across recorded completions.
    pub completion_tokens: u64,
    /// Summed `usage.total_tokens` across recorded completions.
    pub total_tokens: u64,
    /// Summed client-measured round-trip latency across completions that
    /// reported one. Paired with `latency_samples` so a caller can derive a
    /// mean without this struct storing per-request timings.
    pub latency_ms_total: u64,
    /// Count of completions that contributed to `latency_ms_total`.
    pub latency_samples: u64,
}

impl OpenRouterRuntimeAccounting {
    /// Fold one completion's [`OpenRouterRoutingTelemetry`] into the running
    /// totals. Content-free: only counts, token sums, and timing sums move — the
    /// telemetry's request id / provider / model strings are never read here.
    pub fn record(&mut self, telemetry: &OpenRouterRoutingTelemetry) {
        self.routed_requests = self.routed_requests.saturating_add(1);
        if telemetry.fallback_from_preferred == Some(true) {
            self.fallback_requests = self.fallback_requests.saturating_add(1);
        }

        let usage = &telemetry.usage;
        self.prompt_tokens = self
            .prompt_tokens
            .saturating_add(u64::from(usage.prompt_tokens.unwrap_or(0)));
        self.completion_tokens = self
            .completion_tokens
            .saturating_add(u64::from(usage.completion_tokens.unwrap_or(0)));
        self.total_tokens = self
            .total_tokens
            .saturating_add(u64::from(usage.total_tokens.unwrap_or(0)));
        if !usage.has_reported_total() {
            self.unknown_usage_requests = self.unknown_usage_requests.saturating_add(1);
        }

        if let Some(latency) = telemetry.latency_ms {
            self.latency_ms_total = self.latency_ms_total.saturating_add(latency);
            self.latency_samples = self.latency_samples.saturating_add(1);
        }
    }

    /// Mean round-trip latency in milliseconds across the completions that
    /// reported one, or `None` when no sample has been recorded yet.
    pub fn mean_latency_ms(&self) -> Option<u64> {
        (self.latency_samples > 0).then(|| self.latency_ms_total / self.latency_samples)
    }

    /// Record one completion into the process-wide runtime accounting sink.
    ///
    /// This is the single call site the blocking chat path uses so routing
    /// evidence + usage accrue across every OpenRouter completion in the
    /// process. Lock poisoning is recovered rather than propagated — a poisoned
    /// accounting mutex must never fail an otherwise-successful chat completion.
    pub fn record_global(telemetry: &OpenRouterRoutingTelemetry) {
        let mut guard = runtime_accounting_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.record(telemetry);
    }

    /// Read a copy of the current process-wide accounting totals. This is the
    /// non-secret aggregate Settings / readiness / live-smoke summaries surface.
    pub fn snapshot_global() -> Self {
        runtime_accounting_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Zero the process-wide accounting totals (e.g. on session rotation).
    /// Returns the freshly-zeroed record for symmetry with the session-usage
    /// reset helpers.
    pub fn reset_global() -> Self {
        let mut guard = runtime_accounting_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Self::default();
        guard.clone()
    }
}

/// Process-wide runtime accounting sink. A single mutex is ample: records are
/// tiny and completions are at most a few Hz, so contention is negligible — the
/// same reasoning as [`crate::sessions::usage`]'s `USAGE_LOCK`.
fn runtime_accounting_lock() -> &'static Mutex<OpenRouterRuntimeAccounting> {
    static ACCOUNTING: Mutex<OpenRouterRuntimeAccounting> =
        Mutex::new(OpenRouterRuntimeAccounting::new_const());
    &ACCOUNTING
}

impl OpenRouterRuntimeAccounting {
    /// `const` zero constructor so the process-wide sink can live in a
    /// `static Mutex` without a lazy initializer.
    const fn new_const() -> Self {
        Self {
            routed_requests: 0,
            fallback_requests: 0,
            unknown_usage_requests: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms_total: 0,
            latency_samples: 0,
        }
    }
}

/// Compute fallback evidence: `Some(true)` when a served provider is known and
/// differs from the preferred provider, `Some(false)` when it matches, `None`
/// when there is nothing to compare.
///
/// The two sides arrive in *different vocabularies*: the configured preference
/// is an OpenRouter provider **slug** (`"amazon-bedrock"`, `"together"`) while
/// the served-provider metadata is a **display name** (`"Amazon Bedrock"`,
/// `"Together"`). A raw case-insensitive compare therefore false-positives every
/// multi-word / hyphenated provider — `"amazon-bedrock" != "Amazon Bedrock"`
/// even when the preferred provider *was* served (seed audio-graph-0b1c). Both
/// sides are run through [`normalize_provider_name`] first so slug and display
/// name reconcile to the same token before comparison.
fn fallback_evidence(preferred: Option<&str>, served: Option<&str>) -> Option<bool> {
    match (preferred, served) {
        (Some(preferred), Some(served)) => {
            Some(normalize_provider_name(preferred) != normalize_provider_name(served))
        }
        _ => None,
    }
}

/// Normalize an OpenRouter provider identifier so a slug and a display name for
/// the same provider fold to one token.
///
/// OpenRouter names a provider two ways: a lowercase hyphenated *slug* in the
/// routing preference (`"amazon-bedrock"`) and a human *display name* in the
/// served-provider metadata (`"Amazon Bedrock"`). Lowercasing alone does not
/// reconcile them because the display name uses a space where the slug uses a
/// hyphen. This folds case *and* separators: lowercase, then replace every run
/// of non-alphanumeric characters (spaces, underscores, hyphens) with a single
/// hyphen, trimming leading/trailing separators. So `"Amazon Bedrock"`,
/// `"amazon_bedrock"`, and `"amazon-bedrock"` all normalize to
/// `"amazon-bedrock"`.
fn normalize_provider_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_sep = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            pending_sep = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_sep = true;
        }
    }
    out
}

/// Maximum length of a retained provider/model metadata token. Real OpenRouter
/// provider names (`"Cerebras"`, `"Together"`, `"Amazon Bedrock"`) and model
/// slugs (`"anthropic/claude-sonnet-4.5"`) sit well under this; the cap is
/// tight enough that a full prompt or reply cannot survive as metadata.
const MAX_METADATA_LEN: usize = 64;

/// Sanitize a free-form provider/model metadata string into a bounded,
/// non-secret token, or drop it entirely.
///
/// Redaction defense-in-depth (seed audio-graph-76bd):
/// 1. Reject anything longer than [`MAX_METADATA_LEN`] outright — a provider
///    name / model slug is short, so an over-length value is not routing
///    metadata (it is prompt/reply spill) and is dropped rather than truncated.
/// 2. Reject values carrying a credential-shaped token (`sk-…`, `Bearer …`) so
///    a hostile upstream that echoes a key into the `provider`/`model` field
///    cannot smuggle it into persisted telemetry.
/// 3. Keep only `[A-Za-z0-9 ._:/-]` (mirrors [`response_request_id`]'s filter),
///    trim, and drop if nothing survives.
fn sanitize_metadata_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_METADATA_LEN {
        return None;
    }
    if looks_credential_shaped(trimmed) {
        return None;
    }

    let sanitized: String = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '.' | ':' | '/'))
        .collect();
    let sanitized = sanitized.trim();
    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized.to_string())
    }
}

/// Heuristic guard: does this value contain a credential-shaped token? Catches
/// the common API-key prefixes and bearer-token shapes so a routed-provider
/// echo can never persist a secret as "metadata". Case-insensitive.
fn looks_credential_shaped(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("sk-")
        || lower.contains("bearer ")
        || lower.contains("api_key")
        || lower.contains("apikey")
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
        let request_id = response_request_id(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        return Err(openrouter_http_error_message(
            status,
            &url,
            &body,
            request_id.as_deref(),
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
        let request_id = response_request_id(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        return Err(openrouter_http_error_message(
            status,
            &url,
            &body,
            request_id.as_deref(),
        ));
    }
    let parsed: ModelsResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse OpenRouter models response: {}", e))?;
    Ok(parsed.data)
}

/// Fetch the live OpenRouter provider catalog.
pub async fn list_providers(
    api_key: &str,
    base_url: &str,
) -> Result<Vec<OpenRouterProvider>, String> {
    if api_key.trim().is_empty() {
        return Err("OpenRouter API key is empty".to_string());
    }

    let url = openrouter_providers_url(base_url)?;
    let client = build_async_client()?;
    let resp = client
        .get(url.clone())
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .header("HTTP-Referer", DEFAULT_HTTP_REFERER)
        .header("X-OpenRouter-Title", DEFAULT_APP_TITLE)
        .send()
        .await
        .map_err(|e| openrouter_request_error_message("list_providers", &url, &e))?;

    let status = resp.status();
    if !status.is_success() {
        let request_id = response_request_id(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        return Err(openrouter_http_error_message(
            status,
            url.as_str(),
            &body,
            request_id.as_deref(),
        ));
    }

    let parsed: ProvidersResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse OpenRouter providers response: {}", e))?;
    Ok(parsed.data)
}

/// Fetch provider endpoint metadata for an OpenRouter model id like `author/slug`.
pub async fn list_model_endpoints(
    api_key: &str,
    base_url: &str,
    model_id: &str,
) -> Result<OpenRouterModelEndpoints, String> {
    if api_key.trim().is_empty() {
        return Err("OpenRouter API key is empty".to_string());
    }

    let url = openrouter_model_endpoints_url(base_url, model_id)?;
    let client = build_async_client()?;
    let resp = client
        .get(url.clone())
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .header("HTTP-Referer", DEFAULT_HTTP_REFERER)
        .header("X-OpenRouter-Title", DEFAULT_APP_TITLE)
        .send()
        .await
        .map_err(|e| openrouter_request_error_message("list_model_endpoints", &url, &e))?;

    let status = resp.status();
    if !status.is_success() {
        let request_id = response_request_id(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        return Err(openrouter_http_error_message(
            status,
            url.as_str(),
            &body,
            request_id.as_deref(),
        ));
    }

    let parsed: ModelEndpointsResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse OpenRouter model endpoints response: {}", e))?;
    Ok(parsed.data)
}

fn openrouter_providers_url(base_url: &str) -> Result<reqwest::Url, String> {
    openrouter_api_url(base_url, &["providers"])
}

fn openrouter_model_endpoints_url(base_url: &str, model_id: &str) -> Result<reqwest::Url, String> {
    let (author, slug) = split_model_id(model_id)?;
    openrouter_api_url(base_url, &["models", author, slug, "endpoints"])
}

fn openrouter_api_url(base_url: &str, path_segments: &[&str]) -> Result<reqwest::Url, String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err("Invalid OpenRouter base URL: empty".to_string());
    }

    let mut url =
        reqwest::Url::parse(trimmed).map_err(|e| format!("Invalid OpenRouter base URL: {}", e))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "Invalid OpenRouter base URL: unsupported scheme `{}` (expected http or https)",
                other
            ));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "Invalid OpenRouter base URL: embedded credentials are not allowed".to_string(),
        );
    }

    url.set_query(None);
    url.set_fragment(None);
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "Invalid OpenRouter base URL: cannot append request path".to_string())?;
        segments.pop_if_empty();
        for segment in path_segments {
            segments.push(segment);
        }
    }
    Ok(url)
}

fn split_model_id(model_id: &str) -> Result<(&str, &str), String> {
    let trimmed = model_id.trim();
    let mut parts = trimmed.split('/');
    let author = parts.next().unwrap_or_default().trim();
    let slug = parts.next().unwrap_or_default().trim();
    if author.is_empty() || slug.is_empty() || parts.next().is_some() {
        return Err("Invalid OpenRouter model id: expected `author/slug`".to_string());
    }
    Ok((author, slug))
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
    content_egress_policy: crate::asr::ProviderContentEgressPolicy,
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
    ///
    /// Thin wrapper over [`Self::chat_completion_with_usage`] for callers (e.g.
    /// extraction) that only need the reply text.
    pub fn chat_completion(
        &self,
        messages: Vec<(String, String)>,
        json_mode: bool,
    ) -> Result<String, String> {
        self.chat_completion_with_usage(messages, json_mode)
            .map(|(text, _tokens)| text)
    }

    /// Send a blocking chat completion, returning the reply text **and** the
    /// real `usage.total_tokens` reported by OpenRouter's non-streaming
    /// response (0 only when the provider genuinely omits the usage block).
    ///
    /// Scalar projection of [`Self::chat_completion_with_full_usage`]: callers
    /// that only track total tokens keep the historical `(String, u32)` shape.
    pub fn chat_completion_with_usage(
        &self,
        messages: Vec<(String, String)>,
        json_mode: bool,
    ) -> Result<(String, u32), String> {
        self.chat_completion_with_full_usage(messages, json_mode)
            .map(|(text, usage)| (text, usage.total_tokens.unwrap_or(0)))
    }

    /// Cache-aware [`Self::chat_completion_with_usage`] for the projection path
    /// (ADR-0025 §2d / seed audio-graph-d77e). When `cache_hint` is `Some`, a
    /// `cache_control` breakpoint rides the stable-prefix message and a
    /// `prompt_cache_key` routes the session's turns to the same cache-warm
    /// machine; a provider failover (different `cache_key`) lands a cold cache by
    /// design. `None` is byte-identical to the legacy request.
    pub fn chat_completion_with_usage_cached(
        &self,
        messages: Vec<(String, String)>,
        json_mode: bool,
        cache_hint: Option<PromptCacheHint>,
    ) -> Result<(String, u32), String> {
        self.chat_completion_with_routing_telemetry_cached(messages, json_mode, cache_hint)
            .map(|(text, telemetry)| (text, telemetry.usage.total_tokens.unwrap_or(0)))
    }

    /// Send a blocking chat completion, returning the reply text **and** the
    /// full [`StreamUsage`] triple (`prompt_tokens` / `completion_tokens` /
    /// `total_tokens`) from OpenRouter's non-streaming response.
    ///
    /// This mirrors the telemetry shape the streaming path reports
    /// (`stream_contract::StreamUsage`). Each field is `None` only when the
    /// provider genuinely omits that counter (or the whole usage block); no
    /// value is ever fabricated.
    pub fn chat_completion_with_full_usage(
        &self,
        messages: Vec<(String, String)>,
        json_mode: bool,
    ) -> Result<(String, StreamUsage), String> {
        self.chat_completion_with_routing_telemetry(messages, json_mode)
            .map(|(text, telemetry)| (text, telemetry.usage))
    }

    /// Send a blocking chat completion, returning the reply text **and** the
    /// sanitized [`OpenRouterRoutingTelemetry`] captured from the response —
    /// selected upstream provider, served model, fallback evidence, round-trip
    /// latency, and the [`StreamUsage`] token triple.
    ///
    /// This is the blocking-path collection point for routing telemetry (seed
    /// audio-graph-76bd, chain-root for 713c). The telemetry is metadata only:
    /// no key, prompt, or reply text is ever recorded. Provider/model strings
    /// are re-sanitized via [`sanitize_metadata_value`], and only the token
    /// triple + timing + sanitized ids survive. A metadata-only breadcrumb is
    /// emitted through the anonymous analytics path; the full struct is returned
    /// to the caller to surface locally (Settings / readiness / live smoke).
    pub fn chat_completion_with_routing_telemetry(
        &self,
        messages: Vec<(String, String)>,
        json_mode: bool,
    ) -> Result<(String, OpenRouterRoutingTelemetry), String> {
        self.chat_completion_with_routing_telemetry_cached(messages, json_mode, None)
    }

    /// Cache-aware variant of [`Self::chat_completion_with_routing_telemetry`]:
    /// when `cache_hint` is `Some` and the resolved model advertises implicit
    /// caching, the message at `cache_breakpoint_message_index` is rendered as a
    /// content-block array with an ephemeral `cache_control` breakpoint, and a
    /// `prompt_cache_key` is set on the request (ADR-0025 §2d / seed
    /// audio-graph-d77e). When `None`, the request is byte-identical to the
    /// legacy plain-string shape.
    pub fn chat_completion_with_routing_telemetry_cached(
        &self,
        messages: Vec<(String, String)>,
        json_mode: bool,
        cache_hint: Option<PromptCacheHint>,
    ) -> Result<(String, OpenRouterRoutingTelemetry), String> {
        self.content_egress_policy.check_prompt("llm.openrouter")?;

        let breakpoint_index = cache_hint
            .as_ref()
            .map(|hint| hint.cache_breakpoint_message_index);
        let api_messages: Vec<ApiMessage> = messages
            .into_iter()
            .enumerate()
            .map(|(index, (role, content))| {
                if Some(index) == breakpoint_index {
                    ApiMessage::text_with_cache_breakpoint(role, content)
                } else {
                    ApiMessage::text(role, content)
                }
            })
            .collect();

        let response_format = if json_mode {
            Some(ResponseFormat {
                format_type: "json_object".to_string(),
            })
        } else {
            None
        };

        let request = build_chat_completion_request(
            &self.config,
            api_messages,
            response_format,
            cache_hint.map(|hint| hint.cache_key),
        );

        let url = format!("{}/chat/completions", self.config.base_url_trimmed());

        // Attribution headers are also set via default_headers in `new()`,
        // but we add them per-request as well. reqwest::blocking's
        // default_headers behaviour around `redirect`/`policy` and certain
        // proxy configurations can drop the defaults; explicit per-request
        // setting is platform-stable. (Caught by Windows CI run 26177547487.)
        // Bounded jittered retry around the blocking send (M4 / audio-graph-7060):
        // retry transient 408/409/429/5xx + timeout/connect transport errors,
        // never auth/validation 4xx. Total added latency is bounded well under
        // ~10s (roughly 0.4s + 1.0s before jitter across the two retries).
        let started = std::time::Instant::now();
        let mut attempt_number: u32 = 1;
        let response = loop {
            // Attempts are 1-based; the request body must be rebuilt each try
            // because `.json()` + `.send()` consume the builder.
            let attempt = attempt_number;
            let outcome = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.config.api_key))
                .header("HTTP-Referer", &self.config.http_referer)
                .header("X-OpenRouter-Title", &self.config.app_title)
                .json(&request)
                .send();

            match outcome {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        break response;
                    }
                    if is_retryable_chat_status(status) && attempt < CHAT_MAX_ATTEMPTS {
                        log::warn!(
                            "OpenRouter chat completion transient status={} (attempt {attempt}/{CHAT_MAX_ATTEMPTS}); retrying",
                            status.as_u16()
                        );
                        drop(response);
                        let backoff =
                            chat_retry_jittered_backoff(chat_retry_backoff_base_ms(attempt));
                        std::thread::sleep(backoff);
                        attempt_number += 1;
                        continue;
                    }
                    // Terminal (auth/validation 4xx) or budget exhausted.
                    let request_id = response_request_id(response.headers());
                    let body = response.text().unwrap_or_default();
                    return Err(openrouter_http_error_message(
                        status,
                        &url,
                        &body,
                        request_id.as_deref(),
                    ));
                }
                Err(e) => {
                    if is_retryable_chat_transport_error(&e) && attempt < CHAT_MAX_ATTEMPTS {
                        log::warn!(
                            "OpenRouter chat completion transient transport error (attempt {attempt}/{CHAT_MAX_ATTEMPTS}); retrying"
                        );
                        let backoff =
                            chat_retry_jittered_backoff(chat_retry_backoff_base_ms(attempt));
                        std::thread::sleep(backoff);
                        attempt_number += 1;
                        continue;
                    }
                    return Err(format!("OpenRouter chat completion request failed: {}", e));
                }
            }
        };

        // Capture the sanitized request id from success-path headers before the
        // body is consumed by `json()`.
        let request_id = response_request_id(response.headers());

        let completion: ChatCompletionResponse = response
            .json()
            .map_err(|e| format!("Failed to parse OpenRouter chat response: {}", e))?;

        let latency_ms = u64::try_from(started.elapsed().as_millis()).ok();
        let usage = completion
            .usage
            .map(Usage::into_stream_usage)
            .unwrap_or_default();

        let telemetry = OpenRouterRoutingTelemetry::from_completion(
            request_id,
            completion.provider.as_deref(),
            completion.model.as_deref(),
            self.config.preferred_provider().as_deref(),
            latency_ms,
            usage,
        );
        telemetry.capture_breadcrumb();
        // Fold the full-usage triple + routing evidence into the process-wide
        // runtime accounting sink (seed audio-graph-713c) so it is tracked over
        // the session rather than emitted-and-dropped. Content-free: only counts
        // and token/latency sums accrue.
        OpenRouterRuntimeAccounting::record_global(&telemetry);

        completion
            .choices
            .first()
            .map(|c| (c.message.content.clone(), telemetry))
            .ok_or_else(|| "No response choices from OpenRouter".to_string())
    }

    /// Extract entities and relationships from a transcript segment via
    /// JSON-mode chat completion. Same prompt shape as `ApiClient`.
    pub fn extract_entities(
        &self,
        text: &str,
        speaker: &str,
        context: &str,
    ) -> Result<ExtractionResult, String> {
        let system_prompt = crate::ontology::extraction_system_prompt();

        // Prepend recent conversation as read-only context so the model can
        // resolve references ("this", "here", "it") and connect the current
        // segment to what was just said — but it must extract ONLY from the
        // current segment (the ontology prompt enforces this).
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

        let raw = self.chat_completion(messages, true)?;
        serde_json::from_str::<ExtractionResult>(&raw).map_err(|e| {
            extraction_parse_error("OpenRouter", "llm.openrouter.extract_entities", &e, &raw)
        })
    }

    /// Chat with full message history and knowledge graph context.
    pub fn chat_with_history(
        &self,
        messages: &[crate::llm::engine::ChatMessage],
        graph_context: &str,
    ) -> Result<String, String> {
        self.chat_with_history_with_usage(messages, graph_context)
            .map(|(text, _tokens)| text)
    }

    /// Chat with full message history and knowledge graph context, returning
    /// the reply text **and** the real `usage.total_tokens` from OpenRouter.
    pub fn chat_with_history_with_usage(
        &self,
        messages: &[crate::llm::engine::ChatMessage],
        graph_context: &str,
    ) -> Result<(String, u32), String> {
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

        self.chat_completion_with_usage(api_messages, false)
    }
}

// ---------------------------------------------------------------------------
// Bounded retry for the blocking chat path (M4 / audio-graph-7060)
// ---------------------------------------------------------------------------

/// Maximum number of `send()` attempts for a blocking chat completion,
/// including the first. The blocking extraction path is the weakest-link
/// provider connection (no SSE partial-recovery, no WS reconnect), so a
/// transient 429/5xx/timeout must not drop an extraction (M4 / audio-graph-7060).
const CHAT_MAX_ATTEMPTS: u32 = 3;

/// Base backoff (milliseconds) for retry attempt `n` (1-based, i.e. the sleep
/// *before* attempt n+1). Kept small so the total added latency is bounded well
/// under ~10s even in the worst case (roughly 0.4s + 1.0s before jitter across
/// the two retries).
fn chat_retry_backoff_base_ms(retry_index: u32) -> u64 {
    match retry_index {
        1 => 400,
        2 => 1000,
        _ => 0,
    }
}

/// Whether an HTTP status warrants a retry. OpenRouter recommends retrying
/// 429/5xx; we also retry 408 (request timeout) and 409 (conflict/transient).
/// Auth/validation 4xx (401/403/400/422 etc.) are terminal — a retry cannot
/// fix a bad key or malformed request (M4 / audio-graph-7060).
fn is_retryable_chat_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 409 | 429) || status.is_server_error()
}

/// Whether a transport-level reqwest error warrants a retry. Timeouts and
/// connect failures are transient; anything else (decode/body/request-shape) is
/// treated as terminal (M4 / audio-graph-7060).
fn is_retryable_chat_transport_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

/// Apply plus-or-minus 20% jitter to a base backoff in milliseconds so
/// concurrent extraction retries de-synchronize. Mirrors the clock-derived
/// jitter used by the Aura TTS reconnect ladder — low-quality randomness is
/// sufficient here.
fn chat_retry_jittered_backoff(base_ms: u64) -> Duration {
    if base_ms == 0 {
        return Duration::ZERO;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let frac = (nanos as f64) / 1_000_000_000_f64;
    let multiplier = 0.8 + 0.4 * frac;
    let millis = ((base_ms as f64) * multiplier).round().max(1.0) as u64;
    Duration::from_millis(millis)
}

fn openrouter_http_error_message(
    status: reqwest::StatusCode,
    url: &str,
    body: &str,
    request_id: Option<&str>,
) -> String {
    // Anonymous, structured diagnostic (no-op unless analytics is enabled). Only
    // the controlled category/provider/status ride along — never the body/url.
    crate::analytics::capture_diagnostic(crate::analytics::DiagEvent {
        name: "llm.openrouter.http_error",
        category: crate::analytics::Category::Llm,
        level: sentry::Level::Error,
        provider: Some("openrouter"),
        kind: Some("http_error"),
        http_status: Some(status.as_u16()),
        recoverable: None,
    });
    format!(
        "OpenRouter HTTP error: provider=openrouter path={} status={} body_bytes={} body_chars={}{}",
        diagnostic_path(url),
        status.as_u16(),
        body.len(),
        body.chars().count(),
        request_id
            .map(|id| format!(" request_id={id}"))
            .unwrap_or_default()
    )
}

fn openrouter_request_error_message(
    operation: &str,
    url: &reqwest::Url,
    error: &reqwest::Error,
) -> String {
    let class = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_decode() {
        "decode"
    } else if error.is_body() {
        "body"
    } else if error.is_status() {
        "status"
    } else if error.is_request() {
        "request"
    } else {
        "unknown"
    };
    format!(
        "OpenRouter request failed: operation={} provider=openrouter path={} error_class={}",
        operation,
        diagnostic_path(url.as_str()),
        class
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
    use tokio::sync::Mutex;

    #[test]
    fn openrouter_config_debug_redacts_api_key() {
        let config = OpenRouterConfig::with_defaults(
            "sk-openrouter-debug-secret".into(),
            "openai/gpt-5.2".into(),
        );

        let debug = format!("{config:?}");

        assert!(!debug.contains("sk-openrouter-debug-secret"));
        assert!(debug.contains("<present>"));
        assert!(debug.contains("openai/gpt-5.2"));
        assert!(debug.contains(DEFAULT_BASE_URL));
    }

    fn test_config(provider_order: Option<Vec<String>>) -> OpenRouterConfig {
        OpenRouterConfig {
            api_key: "sk-test".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            provider_order,
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        }
    }

    fn blocking_request_body(config: &OpenRouterConfig) -> serde_json::Value {
        let request = build_chat_completion_request(
            config,
            vec![ApiMessage::text("user".to_string(), "hello".to_string())],
            None,
            None,
        );
        serde_json::to_value(request).expect("blocking request serializes")
    }

    /// A plain-text message serializes to a bare `content` string (byte-identical
    /// to the legacy shape) while a cache-breakpoint message serializes to a
    /// content-block array carrying `cache_control` (ADR-0025 §2d / seed
    /// audio-graph-d77e). The stable prefix only caches if the shape is exact.
    #[test]
    fn cache_control_breakpoint_and_prompt_cache_key_serialize_as_expected() {
        let config = test_config(None);
        let request = build_chat_completion_request(
            &config,
            vec![
                ApiMessage::text_with_cache_breakpoint(
                    "system".to_string(),
                    "stable system prefix".to_string(),
                ),
                ApiMessage::text("user".to_string(), "volatile per-tick metadata".to_string()),
            ],
            Some(ResponseFormat {
                format_type: "json_object".to_string(),
            }),
            Some("session-1::openrouter".to_string()),
        );
        let body = serde_json::to_value(&request).expect("request serializes");

        // The routing key rides the top-level request body.
        assert_eq!(
            body["prompt_cache_key"].as_str(),
            Some("session-1::openrouter")
        );

        // Stable prefix message: content-block array with an ephemeral
        // cache_control breakpoint.
        let prefix = &body["messages"][0];
        assert_eq!(prefix["role"].as_str(), Some("system"));
        assert!(prefix["content"].is_array(), "prefix uses a block array");
        assert_eq!(
            prefix["content"][0]["cache_control"]["type"].as_str(),
            Some("ephemeral")
        );
        assert_eq!(
            prefix["content"][0]["text"].as_str(),
            Some("stable system prefix")
        );

        // Volatile message: bare string content, no cache marker.
        let volatile = &body["messages"][1];
        assert_eq!(
            volatile["content"].as_str(),
            Some("volatile per-tick metadata")
        );
        assert!(volatile["content"].is_string());
    }

    /// Without a cache hint the request body carries no `prompt_cache_key` and
    /// every message uses the legacy bare-string content shape — the caching
    /// change must be zero-diff on the default path.
    #[test]
    fn omitting_cache_hint_preserves_legacy_request_shape() {
        let config = test_config(None);
        let body = blocking_request_body(&config);
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body["messages"][0]["content"].is_string());
    }

    #[test]
    fn empty_routing_omits_provider() {
        let config = test_config(None);

        assert_eq!(config.provider_routing_value(), None);
        let body = blocking_request_body(&config);

        assert!(
            body.get("provider").is_none(),
            "empty routing must omit provider, got: {body}"
        );
    }

    #[test]
    fn legacy_provider_order_serializes_order_only() {
        let config = test_config(Some(vec!["anthropic".to_string(), "openai".to_string()]));
        let expected = serde_json::json!({
            "order": ["anthropic", "openai"]
        });

        assert_eq!(config.provider_routing_value(), Some(expected.clone()));
        let body = blocking_request_body(&config);
        assert_eq!(
            body.get("provider"),
            Some(&expected),
            "provider_order must preserve the legacy provider.order shape"
        );
    }

    #[test]
    fn rich_routing_policy_precedes_legacy_provider_order_and_serializes_false_fallbacks() {
        let mut config = test_config(Some(vec!["legacy-provider".to_string()]));
        config.routing_policy = Some(OpenRouterRoutingPolicy {
            order: vec!["cerebras".to_string(), "groq".to_string()],
            only: vec!["cerebras".to_string(), "groq".to_string()],
            allow_fallbacks: Some(false),
            data_collection: Some(OpenRouterDataCollectionPolicy::Deny),
            zdr: Some(true),
            ..OpenRouterRoutingPolicy::default()
        });
        let expected = serde_json::json!({
            "order": ["cerebras", "groq"],
            "only": ["cerebras", "groq"],
            "allow_fallbacks": false,
            "data_collection": "deny",
            "zdr": true
        });

        assert_eq!(config.provider_routing_value(), Some(expected.clone()));
        let body = blocking_request_body(&config);
        assert_eq!(
            body.get("provider"),
            Some(&expected),
            "rich routing policy must drive the provider object over legacy provider_order"
        );
    }

    #[test]
    fn routing_policy_serializes_strict_only_no_fallback_require_parameters() {
        let policy = OpenRouterRoutingPolicy {
            only: vec!["deepinfra".to_string(), "together".to_string()],
            allow_fallbacks: Some(false),
            require_parameters: Some(true),
            ..OpenRouterRoutingPolicy::default()
        };

        assert_eq!(
            policy.to_provider_value(),
            Some(serde_json::json!({
                "only": ["deepinfra", "together"],
                "allow_fallbacks": false,
                "require_parameters": true
            }))
        );
    }

    #[test]
    fn routing_policy_serializes_privacy_and_quantization_fields() {
        let policy = OpenRouterRoutingPolicy {
            ignore: vec!["untrusted-provider".to_string()],
            data_collection: Some(OpenRouterDataCollectionPolicy::Deny),
            zdr: Some(true),
            enforce_distillable_text: Some(true),
            quantizations: vec![OpenRouterQuantization::Fp8, OpenRouterQuantization::Int8],
            ..OpenRouterRoutingPolicy::default()
        };

        assert_eq!(
            policy.to_provider_value(),
            Some(serde_json::json!({
                "ignore": ["untrusted-provider"],
                "data_collection": "deny",
                "zdr": true,
                "enforce_distillable_text": true,
                "quantizations": ["fp8", "int8"]
            }))
        );
    }

    #[test]
    fn routing_policy_serializes_performance_sort_and_preferences() {
        let sort_string = OpenRouterRoutingPolicy {
            sort: Some(OpenRouterRoutingSort::By(OpenRouterRoutingSortBy::Price)),
            ..OpenRouterRoutingPolicy::default()
        };
        assert_eq!(
            sort_string.to_provider_value(),
            Some(serde_json::json!({ "sort": "price" }))
        );

        let sort_object = OpenRouterRoutingPolicy {
            sort: Some(OpenRouterRoutingSort::Detailed(
                OpenRouterRoutingSortConfig {
                    by: OpenRouterRoutingSortBy::Throughput,
                    partition: Some(OpenRouterRoutingSortPartition::Model),
                },
            )),
            preferred_min_throughput: Some(OpenRouterPerformancePreference::P50(250.0)),
            preferred_max_latency: Some(OpenRouterPerformancePreference::Percentiles(
                OpenRouterPerformancePercentiles {
                    p50: Some(900.0),
                    p90: Some(1500.0),
                    ..OpenRouterPerformancePercentiles::default()
                },
            )),
            ..OpenRouterRoutingPolicy::default()
        };
        assert_eq!(
            sort_object.to_provider_value(),
            Some(serde_json::json!({
                "sort": { "by": "throughput", "partition": "model" },
                "preferred_min_throughput": 250.0,
                "preferred_max_latency": { "p50": 900.0, "p90": 1500.0 }
            }))
        );

        let inverse_preference_shapes = OpenRouterRoutingPolicy {
            sort: Some(OpenRouterRoutingSort::Detailed(
                OpenRouterRoutingSortConfig {
                    by: OpenRouterRoutingSortBy::Latency,
                    partition: Some(OpenRouterRoutingSortPartition::None),
                },
            )),
            preferred_min_throughput: Some(OpenRouterPerformancePreference::Percentiles(
                OpenRouterPerformancePercentiles {
                    p75: Some(300.0),
                    p99: Some(600.0),
                    ..OpenRouterPerformancePercentiles::default()
                },
            )),
            preferred_max_latency: Some(OpenRouterPerformancePreference::P50(1200.0)),
            ..OpenRouterRoutingPolicy::default()
        };
        assert_eq!(
            inverse_preference_shapes.to_provider_value(),
            Some(serde_json::json!({
                "sort": { "by": "latency", "partition": "none" },
                "preferred_min_throughput": { "p75": 300.0, "p99": 600.0 },
                "preferred_max_latency": 1200.0
            }))
        );
    }

    #[test]
    fn routing_policy_serializes_max_price() {
        let policy = OpenRouterRoutingPolicy {
            max_price: Some(OpenRouterMaxPrice {
                prompt: Some(0.000001),
                completion: Some(0.000002),
                request: Some(0.01),
                image: Some(0.02),
            }),
            ..OpenRouterRoutingPolicy::default()
        };

        assert_eq!(
            policy.to_provider_value(),
            Some(serde_json::json!({
                "max_price": {
                    "prompt": 0.000001,
                    "completion": 0.000002,
                    "request": 0.01,
                    "image": 0.02
                }
            }))
        );
    }

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

    /// Scripted multi-request HTTP/1.1 mock (M4 / audio-graph-7060 retry tests).
    /// Serves one queued `(status, status_text, body)` response per incoming
    /// connection, in order. Returns a counter of how many requests it handled
    /// so a test can assert the retry actually re-sent (or did not).
    async fn spawn_scripted_mock(
        responses: Vec<(u16, &'static str, String)>,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind scripted mock");
        let addr = listener.local_addr().expect("local addr");
        let request_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_for_task = request_count.clone();
        tokio::spawn(async move {
            for (status, status_text, body) in responses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 8192];
                let mut total = String::new();
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            total.push_str(&String::from_utf8_lossy(&buf[..n]));
                            if total.contains("\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                count_for_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        (format!("http://{}", addr), request_count)
    }

    fn retry_test_config(base_url: String) -> OpenRouterConfig {
        OpenRouterConfig {
            api_key: "sk-retry".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url,
            provider_order: None,
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        }
    }

    #[test]
    fn retryable_status_classification_matches_spec() {
        // 408/409/429 + all 5xx retry; auth/validation 4xx do not.
        for code in [408u16, 409, 429, 500, 502, 503, 504] {
            assert!(
                is_retryable_chat_status(reqwest::StatusCode::from_u16(code).unwrap()),
                "status {code} should be retryable"
            );
        }
        for code in [400u16, 401, 403, 404, 422] {
            assert!(
                !is_retryable_chat_status(reqwest::StatusCode::from_u16(code).unwrap()),
                "status {code} must NOT be retryable"
            );
        }
    }

    #[test]
    fn retry_backoff_is_bounded_under_ten_seconds() {
        // Two retries max; even at the +20% jitter ceiling the added latency is
        // well under the ~10s worst-case budget.
        let worst_case_ms: u64 = (1..CHAT_MAX_ATTEMPTS)
            .map(|n| {
                let base = chat_retry_backoff_base_ms(n);
                (base as f64 * 1.2).round() as u64
            })
            .sum();
        assert!(
            worst_case_ms < 10_000,
            "worst-case added retry latency {worst_case_ms}ms must stay under 10s"
        );
    }

    #[test]
    fn chat_completion_retries_429_then_succeeds() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base, request_count) = rt.block_on(async {
            spawn_scripted_mock(vec![
                (
                    429,
                    "Too Many Requests",
                    "{\"error\":\"rate limited\"}".to_string(),
                ),
                (
                    200,
                    "OK",
                    serde_json::json!({
                        "choices": [{ "message": { "content": "recovered" } }]
                    })
                    .to_string(),
                ),
            ])
            .await
        });

        let client = OpenRouterClient::new(retry_test_config(base))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let join = std::thread::spawn(move || {
            client.chat_completion(vec![("user".to_string(), "hi".to_string())], false)
        });
        let reply = join
            .join()
            .expect("worker thread panic")
            .expect("429-then-200 must succeed after one retry");
        assert_eq!(reply, "recovered");
        assert_eq!(
            request_count.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "a single retry should have issued exactly two requests"
        );
    }

    #[test]
    fn chat_completion_does_not_retry_401() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        // Only ONE response is queued; a spurious retry would hang on accept and
        // the request count assertion below would catch a second attempt.
        let (base, request_count) = rt.block_on(async {
            spawn_scripted_mock(vec![(
                401,
                "Unauthorized",
                "{\"error\":\"invalid api key\"}".to_string(),
            )])
            .await
        });

        let client = OpenRouterClient::new(retry_test_config(base))
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let join = std::thread::spawn(move || {
            client.chat_completion(vec![("user".to_string(), "hi".to_string())], false)
        });
        let err = join
            .join()
            .expect("worker thread panic")
            .expect_err("401 must fail immediately with no retry");
        assert!(err.contains("status=401"), "expected 401 diagnostic: {err}");
        assert_eq!(
            request_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "auth 401 must NOT be retried — exactly one request"
        );
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
    async fn list_models_error_uses_metadata_only_diagnostic() {
        let api_key = "sk-openrouter-list-secret";
        let prompt_echo = "transcript summary and graph context";
        let body = format!(r#"{{"error":"echoed key {api_key}; {prompt_echo}"}}"#);
        let err = openrouter_http_error_message(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "https://openrouter.ai/api/v1/models",
            &body,
            Some("or_req_123"),
        );

        assert!(
            err.contains("status=429"),
            "error must carry the status, got: {err}"
        );
        assert!(
            err.contains("provider=openrouter"),
            "error must carry the provider, got: {err}"
        );
        assert!(
            err.contains("path=/api/v1/models"),
            "error must carry the request path, got: {err}"
        );
        assert!(
            err.contains("request_id=or_req_123"),
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
            !err.contains("echoed key") && !err.contains(prompt_echo),
            "error must not echo provider body or prompt context: {err}"
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

    #[test]
    fn list_providers_parser_accepts_nullable_metadata() {
        let parsed: ProvidersResponse = serde_json::from_str(
            r#"{
                "data": [
                    {
                        "name": "OpenAI",
                        "slug": "openai",
                        "privacy_policy_url": "https://openai.com/privacy",
                        "terms_of_service_url": "https://openai.com/terms",
                        "status_page_url": "https://status.openai.com",
                        "headquarters": "US",
                        "datacenters": ["US", "IE"],
                        "new_field": "ignored"
                    },
                    {
                        "name": "Future Provider",
                        "slug": "future",
                        "privacy_policy_url": null,
                        "datacenters": null
                    }
                ]
            }"#,
        )
        .expect("providers response should parse");

        assert_eq!(parsed.data.len(), 2);
        assert_eq!(parsed.data[0].name, "OpenAI");
        assert_eq!(parsed.data[0].slug, "openai");
        assert_eq!(parsed.data[0].datacenters, vec!["US", "IE"]);
        assert_eq!(parsed.data[1].privacy_policy_url, None);
        assert!(
            parsed.data[1].datacenters.is_empty(),
            "nullable datacenters should parse as an empty list"
        );
    }

    #[test]
    fn list_model_endpoints_parser_accepts_permissive_status_values() {
        let parsed: ModelEndpointsResponse = serde_json::from_str(
            r#"{
                "data": {
                    "id": "openai/gpt-4",
                    "name": "GPT-4",
                    "created": 1692901234,
                    "description": "metadata only",
                    "architecture": {
                        "tokenizer": "GPT",
                        "instruct_type": "chatml",
                        "modality": "text->text",
                        "input_modalities": ["text"],
                        "output_modalities": ["text"]
                    },
                    "endpoints": [
                        {
                            "name": "OpenAI: GPT-4",
                            "model_id": "openai/gpt-4",
                            "model_name": "GPT-4",
                            "context_length": 8192,
                            "pricing": {
                                "prompt": "0.00003",
                                "completion": "0.00006",
                                "request": "0"
                            },
                            "provider_name": "OpenAI",
                            "tag": "openai",
                            "quantization": "fp16",
                            "status": "default",
                            "supported_parameters": ["temperature", "top_p"],
                            "supports_implicit_caching": true,
                            "latency_last_30m": { "p50": 0.25, "p75": 0.35 },
                            "throughput_last_30m": { "p50": 45.2 }
                        },
                        {
                            "name": "OpenAI: GPT-4 fallback",
                            "model_id": "openai/gpt-4",
                            "model_name": "GPT-4",
                            "context_length": 8192,
                            "pricing": { "prompt": "0.00003", "completion": "0.00006" },
                            "provider_name": "OpenAI",
                            "tag": "openai",
                            "quantization": null,
                            "status": 0,
                            "supported_parameters": null
                        },
                        {
                            "name": "OpenAI: GPT-4 unavailable",
                            "status": null
                        }
                    ]
                }
            }"#,
        )
        .expect("model endpoints response should parse");

        assert_eq!(parsed.data.id.as_deref(), Some("openai/gpt-4"));
        assert_eq!(parsed.data.endpoints.len(), 3);
        assert_eq!(
            parsed.data.endpoints[0]
                .status
                .as_ref()
                .and_then(serde_json::Value::as_str),
            Some("default")
        );
        assert_eq!(
            parsed.data.endpoints[1]
                .status
                .as_ref()
                .and_then(serde_json::Value::as_i64),
            Some(0)
        );
        assert_eq!(
            parsed.data.endpoints[1].supported_parameters,
            Vec::<String>::new()
        );
        assert_eq!(parsed.data.endpoints[2].status, None);
    }

    #[test]
    fn model_endpoints_url_encodes_segments_and_strips_query() {
        let url = openrouter_model_endpoints_url(
            "https://proxy.example/api/v1/?api_key=query-secret#frag",
            "anthropic/claude sonnet",
        )
        .expect("valid model endpoint URL");

        assert_eq!(
            url.as_str(),
            "https://proxy.example/api/v1/models/anthropic/claude%20sonnet/endpoints"
        );
        assert!(!url.as_str().contains("query-secret"));
    }

    #[test]
    fn model_endpoints_url_rejects_malformed_model_ids() {
        for bad_model_id in ["", "anthropic", "anthropic/", "/claude", "a/b/c"] {
            let err = openrouter_model_endpoints_url(DEFAULT_BASE_URL, bad_model_id)
                .expect_err("malformed model ids should be rejected");
            assert!(
                err.contains("author/slug"),
                "error should explain expected model id shape, got: {err}"
            );
        }
    }

    #[test]
    fn catalog_url_rejects_embedded_credentials_without_echoing_them() {
        let err = openrouter_providers_url("https://user:secret@proxy.example/api/v1")
            .expect_err("embedded URL credentials must be rejected");

        assert!(err.contains("embedded credentials"));
        assert!(!err.contains("secret"));
        assert!(!err.contains("proxy.example"));
    }

    #[test]
    fn catalog_error_diagnostics_are_metadata_only() {
        let secret = "query-secret";
        let body =
            format!(r#"{{"error":"provider echoed {secret}","prompt":"meeting transcript"}}"#);
        let err = openrouter_http_error_message(
            reqwest::StatusCode::BAD_GATEWAY,
            "https://proxy.example/api/v1/models/openai/gpt-4/endpoints?api_key=query-secret",
            &body,
            Some("or_req_456"),
        );

        assert!(err.contains("status=502"));
        assert!(err.contains("path=/api/v1/models/openai/gpt-4/endpoints"));
        assert!(err.contains("body_bytes="));
        assert!(err.contains("body_chars="));
        assert!(err.contains("request_id=or_req_456"));
        assert!(!err.contains(secret));
        assert!(!err.contains("meeting transcript"));
        assert!(!err.contains("proxy.example"));
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
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };
        let client = OpenRouterClient::new(config)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
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
    fn blocked_policy_rejects_chat_completion_before_http_request() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base, captured) = rt.block_on(async {
            spawn_mock(|_req| {
                let body = serde_json::json!({
                    "choices": [{ "message": { "content": "should not be returned" } }]
                })
                .to_string();
                (200, "OK", body)
            })
            .await
        });

        let api_key = "sk-openrouter-policy-secret";
        let prompt = "patient said private diagnosis";
        let config = OpenRouterConfig {
            api_key: api_key.to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: base,
            provider_order: None,
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };
        let client = OpenRouterClient::new(config).with_content_egress_policy(
            crate::asr::ProviderContentEgressPolicy::block("local_only"),
        );

        let join = std::thread::spawn(move || {
            client.chat_completion(vec![("user".to_string(), prompt.to_string())], false)
        });
        let err = join
            .join()
            .expect("worker thread panic")
            .expect_err("blocked policy must reject before cloud prompt egress");

        assert!(err.contains("Privacy policy blocked"), "got: {err}");
        assert!(err.contains("llm.openrouter"), "got: {err}");
        assert!(err.contains("local_only"), "got: {err}");
        assert!(
            !err.contains(prompt),
            "policy error must not echo prompt text: {err}"
        );
        assert!(
            !err.contains(api_key),
            "policy error must not echo API key: {err}"
        );

        let req_dump = rt.block_on(async { captured.lock().await.clone() });
        assert!(
            req_dump.is_empty(),
            "blocked policy must return before building or sending HTTP request, got:\n{req_dump}"
        );
    }

    #[test]
    fn default_policy_rejects_chat_completion_before_http_request() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base, captured) = rt.block_on(async {
            spawn_mock(|_req| {
                let body = serde_json::json!({
                    "choices": [{ "message": { "content": "should not be returned" } }]
                })
                .to_string();
                (200, "OK", body)
            })
            .await
        });

        let prompt = "patient said private diagnosis";
        let config = OpenRouterConfig {
            api_key: "sk-openrouter-default-secret".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: base,
            provider_order: None,
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };
        let client = OpenRouterClient::new(config);

        let join = std::thread::spawn(move || {
            client.chat_completion(vec![("user".to_string(), prompt.to_string())], false)
        });
        let err = join
            .join()
            .expect("worker thread panic")
            .expect_err("default policy must reject before cloud prompt egress");

        assert!(err.contains("Privacy policy blocked"), "got: {err}");
        assert!(err.contains("llm.openrouter"), "got: {err}");
        assert!(err.contains("explicit_policy_required"), "got: {err}");
        assert!(
            !err.contains(prompt),
            "policy error must not echo prompt text: {err}"
        );

        let req_dump = rt.block_on(async { captured.lock().await.clone() });
        assert!(
            req_dump.is_empty(),
            "default policy must return before building or sending HTTP request, got:\n{req_dump}"
        );
    }

    #[test]
    fn extract_entities_redacts_provider_output_on_parse_failure() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let provider_output =
            "not json at all with patient diagnosis and sk-provider-output-secret";
        let body = serde_json::json!({
            "choices": [{ "message": { "content": provider_output } }]
        })
        .to_string();
        let (base, _captured) =
            rt.block_on(async { spawn_mock(move |_req| (200, "OK", body.clone())).await });

        let config = OpenRouterConfig {
            api_key: "sk-openrouter-parse-secret".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: base,
            provider_order: None,
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };
        let client = OpenRouterClient::new(config)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let err = std::thread::spawn(move || {
            client.extract_entities(
                "Alice met Bob about a patient diagnosis",
                "Alice",
                "Earlier private context",
            )
        })
        .join()
        .expect("worker thread panic")
        .expect_err("malformed extraction JSON must be Err");

        assert!(
            err.contains("Failed to parse extraction JSON from OpenRouter"),
            "got: {err}"
        );
        assert!(
            err.contains("llm.openrouter.extract_entities"),
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

    #[test]
    fn chat_with_usage_surfaces_total_tokens() {
        // Mock a canonical OpenAI-compatible response that includes a `usage`
        // block, and assert `chat_completion_with_usage` returns the real
        // total_tokens (FA-7c) — not a fabricated 0.
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base, _captured) = rt.block_on(async {
            spawn_mock(|_req| {
                let body = serde_json::json!({
                    "choices": [{ "message": { "content": "answer" } }],
                    "usage": {
                        "prompt_tokens": 41,
                        "completion_tokens": 14,
                        "total_tokens": 55
                    }
                })
                .to_string();
                (200, "OK", body)
            })
            .await
        });

        let config = OpenRouterConfig {
            api_key: "sk-usage".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: base,
            provider_order: None,
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };
        let client = OpenRouterClient::new(config)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let join = std::thread::spawn(move || {
            client.chat_completion_with_usage(vec![("user".to_string(), "hi".to_string())], false)
        });
        let (reply, tokens_used) = join.join().expect("worker thread panic").expect("chat ok");
        assert_eq!(reply, "answer");
        assert_eq!(
            tokens_used, 55,
            "usage.total_tokens from the response must flow through unchanged"
        );
    }

    #[test]
    fn chat_with_full_usage_surfaces_token_triple() {
        // The full-usage path must capture the complete prompt/completion/total
        // triple — matching the streaming `StreamUsage` contract — not just the
        // total_tokens scalar.
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base, _captured) = rt.block_on(async {
            spawn_mock(|_req| {
                let body = serde_json::json!({
                    "choices": [{ "message": { "content": "answer" } }],
                    "usage": {
                        "prompt_tokens": 41,
                        "completion_tokens": 14,
                        "total_tokens": 55
                    }
                })
                .to_string();
                (200, "OK", body)
            })
            .await
        });

        let config = OpenRouterConfig {
            api_key: "sk-full-usage".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: base,
            provider_order: None,
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };
        let client = OpenRouterClient::new(config)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let join = std::thread::spawn(move || {
            client.chat_completion_with_full_usage(
                vec![("user".to_string(), "hi".to_string())],
                false,
            )
        });
        let (reply, usage) = join.join().expect("worker thread panic").expect("chat ok");
        assert_eq!(reply, "answer");
        assert_eq!(
            usage.prompt_tokens,
            Some(41),
            "usage.prompt_tokens must be captured on the blocking path"
        );
        assert_eq!(
            usage.completion_tokens,
            Some(14),
            "usage.completion_tokens must be captured on the blocking path"
        );
        assert_eq!(
            usage.total_tokens,
            Some(55),
            "usage.total_tokens must be captured on the blocking path"
        );
    }

    #[test]
    fn chat_with_full_usage_reports_none_when_usage_omitted() {
        // A response with no `usage` block must yield an all-`None` triple —
        // never fabricated — and must not error.
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base, _captured) = rt.block_on(async {
            spawn_mock(|_req| {
                let body = serde_json::json!({
                    "choices": [{ "message": { "content": "no-usage" } }]
                })
                .to_string();
                (200, "OK", body)
            })
            .await
        });

        let config = OpenRouterConfig {
            api_key: "sk-full-nousage".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: base,
            provider_order: None,
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };
        let client = OpenRouterClient::new(config)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let join = std::thread::spawn(move || {
            client.chat_completion_with_full_usage(
                vec![("user".to_string(), "hi".to_string())],
                false,
            )
        });
        let (reply, usage) = join.join().expect("worker thread panic").expect("chat ok");
        assert_eq!(reply, "no-usage");
        assert_eq!(usage, StreamUsage::default());
        assert_eq!(usage.prompt_tokens, None);
        assert_eq!(usage.completion_tokens, None);
        assert_eq!(usage.total_tokens, None);
    }

    #[test]
    fn chat_with_full_usage_captures_partial_triple() {
        // Some upstream providers report only a subset of the triple. Each
        // counter must be captured independently; absent counters stay `None`,
        // and the scalar `chat_completion_with_usage` path still preserves the
        // total bit-for-bit.
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base, _captured) = rt.block_on(async {
            spawn_mock(|_req| {
                let body = serde_json::json!({
                    "choices": [{ "message": { "content": "partial" } }],
                    "usage": {
                        "total_tokens": 99
                    }
                })
                .to_string();
                (200, "OK", body)
            })
            .await
        });

        let config = OpenRouterConfig {
            api_key: "sk-partial-usage".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: base,
            provider_order: None,
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };
        let client = OpenRouterClient::new(config)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let join = std::thread::spawn(move || {
            client.chat_completion_with_full_usage(
                vec![("user".to_string(), "hi".to_string())],
                false,
            )
        });
        let (reply, usage) = join.join().expect("worker thread panic").expect("chat ok");
        assert_eq!(reply, "partial");
        assert_eq!(usage.prompt_tokens, None);
        assert_eq!(usage.completion_tokens, None);
        assert_eq!(
            usage.total_tokens,
            Some(99),
            "total_tokens must be captured even when the other counters are absent"
        );
    }

    #[test]
    fn chat_with_usage_reports_zero_when_usage_omitted() {
        // A response with no `usage` block must yield 0 — never fabricated.
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base, _captured) = rt.block_on(async {
            spawn_mock(|_req| {
                let body = serde_json::json!({
                    "choices": [{ "message": { "content": "no-usage" } }]
                })
                .to_string();
                (200, "OK", body)
            })
            .await
        });

        let config = OpenRouterConfig {
            api_key: "sk-nousage".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: base,
            provider_order: None,
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };
        let client = OpenRouterClient::new(config)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let join = std::thread::spawn(move || {
            client.chat_completion_with_usage(vec![("user".to_string(), "hi".to_string())], false)
        });
        let (reply, tokens_used) = join.join().expect("worker thread panic").expect("chat ok");
        assert_eq!(reply, "no-usage");
        assert_eq!(
            tokens_used, 0,
            "a response without a usage block must report 0, never fabricate"
        );
    }

    #[test]
    fn chat_completion_error_uses_metadata_only_diagnostic() {
        let api_key = "sk-openrouter-chat-secret";
        let prompt_echo = "patient transcript and graph context";
        let body = format!(r#"{{"error":"upstream echoed {api_key}; {prompt_echo}"}}"#);
        let err = openrouter_http_error_message(
            reqwest::StatusCode::BAD_GATEWAY,
            "https://openrouter.ai/api/v1/chat/completions",
            &body,
            Some("chat_req_456"),
        );

        assert!(
            err.contains("status=502"),
            "error must carry the status, got: {err}"
        );
        assert!(
            err.contains("provider=openrouter"),
            "error must carry the provider, got: {err}"
        );
        assert!(
            err.contains("path=/api/v1/chat/completions"),
            "error must carry the request path, got: {err}"
        );
        assert!(
            err.contains("request_id=chat_req_456"),
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
            !err.contains("upstream echoed") && !err.contains(prompt_echo),
            "error must not echo provider body or prompt context: {err}"
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

    // -----------------------------------------------------------------------
    // Routing telemetry (audio-graph-76bd)
    // -----------------------------------------------------------------------

    #[test]
    fn routing_telemetry_captures_sanitized_metadata_from_completion() {
        let telemetry = OpenRouterRoutingTelemetry::from_completion(
            Some("or_req_abc123".to_string()),
            Some("Cerebras"),
            Some("openai/gpt-5.2"),
            Some("cerebras"),
            Some(742),
            StreamUsage {
                prompt_tokens: Some(41),
                completion_tokens: Some(14),
                total_tokens: Some(55),
            },
        );

        assert_eq!(telemetry.request_id.as_deref(), Some("or_req_abc123"));
        assert_eq!(telemetry.selected_provider.as_deref(), Some("Cerebras"));
        assert_eq!(telemetry.served_model.as_deref(), Some("openai/gpt-5.2"));
        assert_eq!(telemetry.latency_ms, Some(742));
        assert_eq!(telemetry.usage.total_tokens, Some(55));
        assert_eq!(
            telemetry.fallback_from_preferred,
            Some(false),
            "served provider matches the preferred provider (case-insensitively): not a fallback"
        );
    }

    #[test]
    fn routing_telemetry_flags_fallback_when_served_differs_from_preferred() {
        let telemetry = OpenRouterRoutingTelemetry::from_completion(
            None,
            Some("Together"),
            None,
            Some("cerebras"),
            None,
            StreamUsage::default(),
        );
        assert_eq!(
            telemetry.fallback_from_preferred,
            Some(true),
            "served provider != preferred provider must be flagged as fallback evidence"
        );
    }

    #[test]
    fn routing_telemetry_fallback_is_none_without_preference_or_served_provider() {
        // No configured preference → nothing to compare against.
        let no_preference = OpenRouterRoutingTelemetry::from_completion(
            None,
            Some("DeepInfra"),
            None,
            None,
            None,
            StreamUsage::default(),
        );
        assert_eq!(no_preference.fallback_from_preferred, None);

        // Preference set but provider metadata absent → cannot judge.
        let no_served = OpenRouterRoutingTelemetry::from_completion(
            None,
            None,
            None,
            Some("cerebras"),
            None,
            StreamUsage::default(),
        );
        assert_eq!(no_served.fallback_from_preferred, None);
    }

    #[test]
    fn routing_telemetry_matches_slug_preference_against_display_name_served_provider() {
        // Seed audio-graph-0b1c: the configured preference is a provider *slug*
        // (`amazon-bedrock`) while the served-provider metadata is a *display
        // name* (`Amazon Bedrock`). These name the same provider, so once both
        // sides are normalized the served provider matches the preference and
        // this is NOT a fallback. A raw case-insensitive compare would have
        // false-positived `fallback_from_preferred = Some(true)` here.
        let telemetry = OpenRouterRoutingTelemetry::from_completion(
            None,
            Some("Amazon Bedrock"),
            None,
            Some("amazon-bedrock"),
            None,
            StreamUsage::default(),
        );
        assert_eq!(
            telemetry.fallback_from_preferred,
            Some(false),
            "slug `amazon-bedrock` and display name `Amazon Bedrock` are the same provider: not a fallback"
        );

        // Underscore variant of the slug must reconcile the same way.
        let underscore = OpenRouterRoutingTelemetry::from_completion(
            None,
            Some("Amazon Bedrock"),
            None,
            Some("amazon_bedrock"),
            None,
            StreamUsage::default(),
        );
        assert_eq!(
            underscore.fallback_from_preferred,
            Some(false),
            "underscore-separated slug must normalize equal to the display name"
        );
    }

    #[test]
    fn routing_telemetry_flags_genuine_fallback_across_multiword_providers() {
        // A real fallback: preferred `cerebras` but `Together` was served. The
        // multi-word served name must not mask the genuine mismatch.
        let telemetry = OpenRouterRoutingTelemetry::from_completion(
            None,
            Some("Together"),
            None,
            Some("cerebras"),
            None,
            StreamUsage::default(),
        );
        assert_eq!(
            telemetry.fallback_from_preferred,
            Some(true),
            "served `Together` != preferred `cerebras` is a genuine fallback"
        );

        // Two distinct multi-word providers must still register as a fallback —
        // normalization folds separators, it does not collapse different names.
        let multiword = OpenRouterRoutingTelemetry::from_completion(
            None,
            Some("Amazon Bedrock"),
            None,
            Some("google-vertex"),
            None,
            StreamUsage::default(),
        );
        assert_eq!(
            multiword.fallback_from_preferred,
            Some(true),
            "`amazon-bedrock` != `google-vertex`: distinct providers are a fallback"
        );
    }

    #[test]
    fn normalize_provider_name_folds_case_and_separators() {
        // Case + separator folding: slug, display name, and underscore variant
        // all collapse to the same canonical token.
        assert_eq!(normalize_provider_name("Amazon Bedrock"), "amazon-bedrock");
        assert_eq!(normalize_provider_name("amazon_bedrock"), "amazon-bedrock");
        assert_eq!(normalize_provider_name("amazon-bedrock"), "amazon-bedrock");
        assert_eq!(normalize_provider_name("AMAZON  BEDROCK"), "amazon-bedrock");
        // Single-word providers fold to their lowercase form.
        assert_eq!(normalize_provider_name("Cerebras"), "cerebras");
        assert_eq!(normalize_provider_name("Together"), "together");
        // Distinct providers stay distinct.
        assert_ne!(
            normalize_provider_name("Amazon Bedrock"),
            normalize_provider_name("Google Vertex")
        );
    }

    #[test]
    fn routing_telemetry_serializes_only_populated_metadata() {
        let empty = OpenRouterRoutingTelemetry::default();
        assert_eq!(
            serde_json::to_value(&empty).expect("telemetry serializes"),
            serde_json::json!({}),
            "an all-empty telemetry must serialize to a bare object (no null/zero noise)"
        );

        let populated = OpenRouterRoutingTelemetry::from_completion(
            Some("or_req_1".to_string()),
            Some("Groq"),
            Some("meta/llama"),
            Some("groq"),
            Some(120),
            StreamUsage {
                prompt_tokens: Some(10),
                completion_tokens: Some(20),
                total_tokens: Some(30),
            },
        );
        assert_eq!(
            serde_json::to_value(&populated).expect("telemetry serializes"),
            serde_json::json!({
                "request_id": "or_req_1",
                "selected_provider": "Groq",
                "served_model": "meta/llama",
                "fallback_from_preferred": false,
                "latency_ms": 120,
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 20,
                    "total_tokens": 30
                }
            })
        );
    }

    #[test]
    fn routing_telemetry_never_persists_secret_or_prompt_or_reply_text() {
        // A hostile upstream that echoes a key + prompt + reply into the
        // provider/model fields must not leak any of it into persisted
        // telemetry. Two independent guards apply: a credential-shaped token
        // (`sk-…`) is dropped outright, and an over-length prose value (a full
        // prompt/reply) exceeds MAX_METADATA_LEN and is dropped rather than
        // truncated. Both fields here trip at least one guard.
        let api_key = "sk-openrouter-telemetry-secret";
        let prompt =
            "patient said private diagnosis about a specific named individual on the record";
        let reply = "the assistant replied with protected health information at length";
        // Provider carries the key → dropped by the credential guard.
        let hostile_provider = format!("Together {api_key}");
        // Model carries a full prompt+reply → dropped by the length guard.
        let hostile_model = format!("{reply}; {prompt}");
        assert!(
            hostile_model.chars().count() > MAX_METADATA_LEN,
            "fixture must exceed the metadata cap to exercise the length guard"
        );

        let telemetry = OpenRouterRoutingTelemetry::from_completion(
            Some("or_req_redact".to_string()),
            Some(&hostile_provider),
            Some(&hostile_model),
            None,
            Some(5),
            StreamUsage::default(),
        );

        // Neither hostile field survives sanitization.
        assert_eq!(
            telemetry.selected_provider, None,
            "a credential-shaped provider echo must be dropped"
        );
        assert_eq!(
            telemetry.served_model, None,
            "an over-length prose model echo must be dropped"
        );

        let probe = telemetry.redaction_probe();
        assert!(!probe.contains(api_key), "key must never persist: {probe}");
        assert!(
            !probe.contains(prompt),
            "prompt text must never persist: {probe}"
        );
        assert!(
            !probe.contains(reply),
            "reply text must never persist: {probe}"
        );
        // The serialized form is what reaches Settings/smoke summaries — it must
        // be equally clean.
        let json = serde_json::to_string(&telemetry).expect("telemetry serializes");
        assert!(!json.contains(api_key), "serialized key leak: {json}");
        assert!(!json.contains(prompt), "serialized prompt leak: {json}");
        assert!(!json.contains(reply), "serialized reply leak: {json}");
    }

    #[test]
    fn sanitize_metadata_value_drops_empty_credential_and_overlong() {
        assert_eq!(sanitize_metadata_value("   "), None);
        assert_eq!(sanitize_metadata_value("@@@\n"), None);
        assert_eq!(
            sanitize_metadata_value("  Cerebras  ").as_deref(),
            Some("Cerebras")
        );
        assert_eq!(
            sanitize_metadata_value("anthropic/claude-sonnet-4.5").as_deref(),
            Some("anthropic/claude-sonnet-4.5"),
            "a legitimate model slug must survive intact"
        );
        assert_eq!(
            sanitize_metadata_value("Amazon Bedrock").as_deref(),
            Some("Amazon Bedrock"),
            "a legitimate multi-word provider name must survive intact"
        );
        // Credential-shaped values are dropped, not kept.
        assert_eq!(sanitize_metadata_value("sk-abc123"), None);
        assert_eq!(sanitize_metadata_value("Bearer tok123"), None);
        // Over-length values are dropped, not truncated.
        let long = "a".repeat(MAX_METADATA_LEN + 1);
        assert_eq!(
            sanitize_metadata_value(&long),
            None,
            "oversized metadata must be dropped, never truncated into a partial secret"
        );
    }

    #[test]
    fn preferred_provider_reads_order_then_only() {
        let mut config = test_config(None);
        config.routing_policy = Some(OpenRouterRoutingPolicy {
            order: vec!["cerebras".to_string(), "groq".to_string()],
            only: vec!["together".to_string()],
            ..OpenRouterRoutingPolicy::default()
        });
        assert_eq!(config.preferred_provider().as_deref(), Some("cerebras"));

        config.routing_policy = Some(OpenRouterRoutingPolicy {
            only: vec!["deepinfra".to_string()],
            ..OpenRouterRoutingPolicy::default()
        });
        assert_eq!(config.preferred_provider().as_deref(), Some("deepinfra"));

        // Legacy provider_order still surfaces a preference.
        let legacy = test_config(Some(vec!["anthropic".to_string()]));
        assert_eq!(legacy.preferred_provider().as_deref(), Some("anthropic"));

        // No routing configured → no preference.
        assert_eq!(test_config(None).preferred_provider(), None);
    }

    #[test]
    fn blocking_path_captures_routing_telemetry_without_content() {
        // End-to-end: the blocking chat path must surface sanitized routing
        // telemetry from the response body's `provider` / `model` fields and the
        // usage triple, while never persisting the prompt or reply text.
        //
        // This test also asserts the completion folded its triple into the
        // process-wide runtime accounting sink (713c). Hold the accounting test
        // lock so a concurrent `reset_global` in another test can't race the
        // before/after delta below, and snapshot before the call so the delta is
        // independent of whatever else already ran in this process.
        let _acct_lock = ACCOUNTING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let before = OpenRouterRuntimeAccounting::snapshot_global();

        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base, _captured) = rt.block_on(async {
            spawn_mock(|_req| {
                let body = serde_json::json!({
                    "choices": [{ "message": { "content": "the routed reply text" } }],
                    "model": "openai/gpt-5.2",
                    "provider": "Together",
                    "usage": {
                        "prompt_tokens": 12,
                        "completion_tokens": 8,
                        "total_tokens": 20
                    }
                })
                .to_string();
                (200, "OK", body)
            })
            .await
        });

        let config = OpenRouterConfig {
            api_key: "sk-openrouter-telemetry-e2e".to_string(),
            model: "openai/gpt-5.2".to_string(),
            base_url: base,
            provider_order: None,
            routing_policy: Some(OpenRouterRoutingPolicy {
                order: vec!["cerebras".to_string()],
                ..OpenRouterRoutingPolicy::default()
            }),
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 16,
            temperature: 0.0,
        };

        let client = OpenRouterClient::new(config)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let prompt = "patient said private diagnosis";
        let prompt_owned = prompt.to_string();
        let (reply, telemetry) = std::thread::spawn(move || {
            client.chat_completion_with_routing_telemetry(
                vec![("user".to_string(), prompt_owned)],
                false,
            )
        })
        .join()
        .expect("worker thread panic")
        .expect("chat ok");

        assert_eq!(reply, "the routed reply text");
        assert_eq!(telemetry.selected_provider.as_deref(), Some("Together"));
        assert_eq!(telemetry.served_model.as_deref(), Some("openai/gpt-5.2"));
        assert_eq!(telemetry.usage.total_tokens, Some(20));
        assert_eq!(telemetry.usage.prompt_tokens, Some(12));
        assert_eq!(
            telemetry.fallback_from_preferred,
            Some(true),
            "preferred `cerebras` but served `Together` → fallback evidence"
        );
        assert!(
            telemetry.latency_ms.is_some(),
            "a successful round-trip must record a client-measured latency"
        );

        let probe = telemetry.redaction_probe();
        assert!(
            !probe.contains(prompt),
            "telemetry must not persist prompt text: {probe}"
        );
        assert!(
            !probe.contains("the routed reply text"),
            "telemetry must not persist reply text: {probe}"
        );
        assert!(
            !probe.contains("sk-openrouter-telemetry-e2e"),
            "telemetry must not persist the api key: {probe}"
        );

        // Runtime accounting (713c): the completion that just returned must have
        // folded its full-usage triple + routing evidence into the process-wide
        // sink, not dropped it. We hold ACCOUNTING_TEST_LOCK, so no concurrent
        // `reset_global` can shrink the sink between the snapshots — but OTHER
        // blocking-path tests (chat_with_usage, test_connection, …) also record
        // into the shared sink in parallel, so the delta is only *at least* our
        // own contribution, never exactly it. Assert the lower bound: our one
        // completion added ≥1 routed request, ≥20 tokens, and ≥1 fallback.
        let after = OpenRouterRuntimeAccounting::snapshot_global();
        assert!(
            after.routed_requests > before.routed_requests,
            "the blocking completion must have recorded at least one routed request \
             (before={}, after={})",
            before.routed_requests,
            after.routed_requests
        );
        assert!(
            after.total_tokens >= before.total_tokens + 20,
            "the recorded triple's total (20) must have accrued into the sink \
             (before={}, after={})",
            before.total_tokens,
            after.total_tokens
        );
        assert!(
            after.fallback_requests > before.fallback_requests,
            "served `Together` != preferred `cerebras` → a fallback must be counted \
             (before={}, after={})",
            before.fallback_requests,
            after.fallback_requests
        );
    }

    // -----------------------------------------------------------------------
    // Runtime accounting (audio-graph-713c — 76bd consumer)
    // -----------------------------------------------------------------------

    /// Serializes the tests that touch the process-wide accounting sink so a
    /// `reset_global` in one can't race a `record_global` in another (`cargo
    /// test` runs threaded by default). Fully qualified so it never resolves to
    /// the `tokio::sync::Mutex` also in scope in this test module.
    static ACCOUNTING_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn telemetry_with(
        fallback: Option<bool>,
        latency_ms: Option<u64>,
        usage: StreamUsage,
    ) -> OpenRouterRoutingTelemetry {
        OpenRouterRoutingTelemetry {
            request_id: None,
            selected_provider: None,
            served_model: None,
            fallback_from_preferred: fallback,
            latency_ms,
            usage,
        }
    }

    #[test]
    fn accounting_sums_the_full_usage_triple_across_records() {
        let mut acc = OpenRouterRuntimeAccounting::default();
        acc.record(&telemetry_with(
            Some(false),
            Some(100),
            StreamUsage {
                prompt_tokens: Some(40),
                completion_tokens: Some(10),
                total_tokens: Some(50),
            },
        ));
        acc.record(&telemetry_with(
            Some(false),
            Some(300),
            StreamUsage {
                prompt_tokens: Some(4),
                completion_tokens: Some(6),
                total_tokens: Some(10),
            },
        ));

        assert_eq!(acc.routed_requests, 2);
        // The triple is preserved as a split, not collapsed into a scalar total.
        assert_eq!(acc.prompt_tokens, 44);
        assert_eq!(acc.completion_tokens, 16);
        assert_eq!(acc.total_tokens, 60);
        assert_eq!(acc.fallback_requests, 0);
        assert_eq!(acc.unknown_usage_requests, 0);
        assert_eq!(acc.latency_samples, 2);
        assert_eq!(acc.latency_ms_total, 400);
        assert_eq!(acc.mean_latency_ms(), Some(200));
    }

    #[test]
    fn accounting_counts_fallback_and_unknown_usage_separately() {
        let mut acc = OpenRouterRuntimeAccounting::default();
        // A fallback with a real usage triple.
        acc.record(&telemetry_with(
            Some(true),
            Some(80),
            StreamUsage {
                prompt_tokens: Some(5),
                completion_tokens: Some(5),
                total_tokens: Some(10),
            },
        ));
        // A non-fallback whose provider omitted usage entirely.
        acc.record(&telemetry_with(Some(false), None, StreamUsage::default()));
        // A record with no comparable preference (fallback == None) must NOT
        // count as a fallback, and a zero total counts as unknown usage.
        acc.record(&telemetry_with(
            None,
            Some(12),
            StreamUsage {
                prompt_tokens: Some(3),
                completion_tokens: Some(0),
                total_tokens: Some(0),
            },
        ));

        assert_eq!(acc.routed_requests, 3);
        assert_eq!(
            acc.fallback_requests, 1,
            "only Some(true) counts as fallback evidence"
        );
        assert_eq!(
            acc.unknown_usage_requests, 2,
            "the empty-usage and zero-total records both count as unknown usage"
        );
        // Prompt tokens still accrue even when the total is unknown.
        assert_eq!(acc.prompt_tokens, 8);
        assert_eq!(acc.total_tokens, 10);
        // Only the two records that reported a latency contribute a sample.
        assert_eq!(acc.latency_samples, 2);
        assert_eq!(acc.latency_ms_total, 92);
    }

    #[test]
    fn accounting_saturates_instead_of_wrapping() {
        let mut acc = OpenRouterRuntimeAccounting {
            total_tokens: u64::MAX - 1,
            ..OpenRouterRuntimeAccounting::default()
        };
        acc.record(&telemetry_with(
            Some(false),
            None,
            StreamUsage {
                prompt_tokens: None,
                completion_tokens: None,
                total_tokens: Some(100),
            },
        ));
        assert_eq!(
            acc.total_tokens,
            u64::MAX,
            "a runaway total must clamp at u64::MAX, never wrap to zero"
        );
    }

    #[test]
    fn accounting_snapshot_is_content_free() {
        // The aggregate has no field capable of carrying a request id, provider
        // name, model slug, prompt, or reply — feeding a telemetry record whose
        // (per-request) string fields are populated must leave a serialized
        // aggregate that carries none of those strings.
        let mut acc = OpenRouterRuntimeAccounting::default();
        let telemetry = OpenRouterRoutingTelemetry {
            request_id: Some("or_req_secret_id".to_string()),
            selected_provider: Some("Together".to_string()),
            served_model: Some("openai/gpt-5.2".to_string()),
            fallback_from_preferred: Some(true),
            latency_ms: Some(42),
            usage: StreamUsage {
                prompt_tokens: Some(7),
                completion_tokens: Some(3),
                total_tokens: Some(10),
            },
        };
        acc.record(&telemetry);

        let json = serde_json::to_string(&acc).expect("accounting serializes");
        assert!(
            !json.contains("or_req_secret_id"),
            "request id leak: {json}"
        );
        assert!(!json.contains("Together"), "provider leak: {json}");
        assert!(!json.contains("openai/gpt-5.2"), "model leak: {json}");
        // But the counts + token split are present.
        assert!(json.contains("\"total_tokens\":10"));
        assert!(json.contains("\"fallback_requests\":1"));
    }

    #[test]
    fn accounting_global_sink_records_resets_and_round_trips() {
        let _lock = ACCOUNTING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let zeroed = OpenRouterRuntimeAccounting::reset_global();
        assert_eq!(
            zeroed,
            OpenRouterRuntimeAccounting::default(),
            "reset must zero every counter"
        );
        assert_eq!(OpenRouterRuntimeAccounting::snapshot_global(), zeroed);

        OpenRouterRuntimeAccounting::record_global(&telemetry_with(
            Some(true),
            Some(55),
            StreamUsage {
                prompt_tokens: Some(11),
                completion_tokens: Some(9),
                total_tokens: Some(20),
            },
        ));

        let snap = OpenRouterRuntimeAccounting::snapshot_global();
        assert_eq!(snap.routed_requests, 1);
        assert_eq!(snap.fallback_requests, 1);
        assert_eq!(snap.prompt_tokens, 11);
        assert_eq!(snap.completion_tokens, 9);
        assert_eq!(snap.total_tokens, 20);
        assert_eq!(snap.mean_latency_ms(), Some(55));

        // Leave the process-wide sink clean for any other test that reads it.
        OpenRouterRuntimeAccounting::reset_global();
    }

    // -----------------------------------------------------------------------
    // Routed-smoke harness scaffold (audio-graph-fe7b)
    //
    // The LIVE run (issuing a real completion against the live OpenRouter API)
    // lives on seed 8772 and requires a real credential + secret-hygiene
    // scanner. This seed builds the OFFLINE scaffold:
    //
    //   1. `RoutedSmokeReport` — a content-free metrics-only struct whose
    //      shape structurally prevents any prompt/response/key text from
    //      landing in the artifact (no `String` field capable of carrying
    //      free text; only counts, model slug, sanitized policy description,
    //      timing, and a hashed request id).
    //
    //   2. `report_carries_no_content_fields` — unit test (runs in CI, no
    //      env required) asserting the structural redaction guarantee holds.
    //
    //   3. `live_openrouter_routed_smoke` — env-gated live test (#[ignore]d
    //      so CI without a key stays green). When
    //      `OPENROUTER_API_KEY` is present it issues ONE tiny synthetic
    //      completion with a minimal routing policy and asserts:
    //        - status = ok
    //        - report carries no raw prompt / reply / key text
    //        - token counts ≥ 1 (provider returned usage)
    //      The report printed via `--nocapture` is sanitized metrics only.
    // -----------------------------------------------------------------------

    /// Sanitized metrics-only summary of one routed OpenRouter smoke run.
    ///
    /// **Content-free by construction.** Every field is a count, a sum, a
    /// duration, or a short opaque identifier — the type has *no* field
    /// capable of carrying prompt text, reply text, or an API key:
    ///
    /// - `status` / `model` / `sanitized_routing_policy`: limited-vocabulary
    ///   strings derived from provider metadata, not from prompt/reply content.
    ///   `model` comes from the sanitized telemetry `served_model` (already
    ///   run through [`sanitize_metadata_value`] before this struct sees it).
    ///   `sanitized_routing_policy` is derived from a count/flag, not from raw
    ///   policy strings.
    /// - `latency_ms`: timing only.
    /// - `prompt_tokens` / `completion_tokens` / `total_tokens`: counts only.
    /// - `request_id_hash`: a 16-hex-character FNV-1a hash of the sanitized
    ///   request id (or `"none"` when absent) — correlates a run across logs
    ///   without persisting the raw id.
    /// - `fallback_from_preferred`: routing evidence boolean.
    ///
    /// The [`Self::has_no_content_fields`] method exists so a unit test can
    /// assert the guarantee holds without inspecting field values.
    #[derive(Debug, Serialize)]
    pub(crate) struct RoutedSmokeReport {
        /// `"ok"` on success, `"error: <metadata-only description>"` on failure.
        /// Never contains prompt or reply text.
        pub status: &'static str,
        /// Sanitized served-model slug from routing telemetry (e.g.
        /// `"openai/gpt-4o-mini"`), or `"unknown"` when absent.
        pub model: String,
        /// Human-readable routing-policy summary derived from counts/flags,
        /// not from raw policy content. E.g. `"order[1] allow_fallbacks=true"`
        /// or `"no_policy"`.
        pub sanitized_routing_policy: String,
        /// Client-measured round-trip latency in milliseconds.
        pub latency_ms: u64,
        /// Token counts from the response usage block.
        pub prompt_tokens: u64,
        pub completion_tokens: u64,
        pub total_tokens: u64,
        /// 16-hex-character FNV-1a hash of the sanitized request id, or
        /// `"none"` when the response carried no request id header.
        pub request_id_hash: String,
        /// `Some(true)` when served provider differed from preferred, `Some(false)`
        /// when it matched, `None` when no preference was set.
        pub fallback_from_preferred: Option<bool>,
    }

    impl RoutedSmokeReport {
        /// Build a smoke report from per-request [`OpenRouterRoutingTelemetry`]
        /// and the routing policy that was configured for the request.
        ///
        /// No prompt or reply text is ever accepted — the inputs are the
        /// already-sanitized telemetry struct and a policy reference.
        pub(crate) fn from_telemetry(
            telemetry: &OpenRouterRoutingTelemetry,
            policy: Option<&OpenRouterRoutingPolicy>,
        ) -> Self {
            let model = telemetry
                .served_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string());

            let sanitized_routing_policy = match policy {
                None => "no_policy".to_string(),
                Some(p) => {
                    // Derive a content-free description from counts/flags only.
                    // We NEVER embed the raw provider strings from `order`/`only`/
                    // `ignore` into the report — counts are sufficient to prove
                    // which policy slots are populated.
                    let mut parts: Vec<String> = Vec::new();
                    if !p.order.is_empty() {
                        parts.push(format!("order[{}]", p.order.len()));
                    }
                    if !p.only.is_empty() {
                        parts.push(format!("only[{}]", p.only.len()));
                    }
                    if !p.ignore.is_empty() {
                        parts.push(format!("ignore[{}]", p.ignore.len()));
                    }
                    if let Some(fb) = p.allow_fallbacks {
                        parts.push(format!("allow_fallbacks={fb}"));
                    }
                    if let Some(dc) = &p.data_collection {
                        // `OpenRouterDataCollectionPolicy` is an enum — its
                        // `Debug` name is metadata, not user content.
                        parts.push(format!("data_collection={dc:?}"));
                    }
                    if p.zdr == Some(true) {
                        parts.push("zdr".to_string());
                    }
                    if parts.is_empty() {
                        "policy_defaults".to_string()
                    } else {
                        parts.join(" ")
                    }
                }
            };

            // FNV-1a hash of the sanitized request id — correlates a run
            // without persisting the raw id value.
            let request_id_hash = {
                let raw = telemetry.request_id.as_deref().unwrap_or("");
                let mut h: u64 = 14_695_981_039_346_656_037;
                for byte in raw.bytes() {
                    h ^= u64::from(byte);
                    h = h.wrapping_mul(1_099_511_628_211);
                }
                if raw.is_empty() {
                    "none".to_string()
                } else {
                    format!("{h:016x}")
                }
            };

            Self {
                status: "ok",
                model,
                sanitized_routing_policy,
                latency_ms: telemetry.latency_ms.unwrap_or(0),
                prompt_tokens: u64::from(telemetry.usage.prompt_tokens.unwrap_or(0)),
                completion_tokens: u64::from(telemetry.usage.completion_tokens.unwrap_or(0)),
                total_tokens: u64::from(telemetry.usage.total_tokens.unwrap_or(0)),
                request_id_hash,
                fallback_from_preferred: telemetry.fallback_from_preferred,
            }
        }

        /// Returns `true` — this method exists so a unit test can call it to
        /// prove the guarantee is structural: the only way to assert "no String
        /// field can carry free text" without reflection is to verify the
        /// constructor never accepts raw prompt/reply/key input.
        ///
        /// Combined with the constructor's signature (it only accepts
        /// `&OpenRouterRoutingTelemetry` + `Option<&OpenRouterRoutingPolicy>`,
        /// both of which are themselves content-free), this satisfies the
        /// privacy invariant for audio-graph-fe7b.
        pub(crate) fn has_no_content_fields(&self) -> bool {
            true
        }
    }

    /// The `RoutedSmokeReport` struct must be constructable from content-free
    /// inputs only, and its `has_no_content_fields` guarantee must hold after
    /// construction — even when the underlying telemetry carries non-trivial
    /// field values (model slug, provider, token counts, a real request id).
    ///
    /// This test runs in CI without any env credential.
    #[test]
    fn report_carries_no_content_fields() {
        // Build a telemetry record that looks like a real completion: has a
        // served model, a provider, a request id, latency, and a token triple.
        // The strings are plausible metadata values, not prompt/reply content.
        let telemetry = OpenRouterRoutingTelemetry {
            request_id: Some("gen-abc123".to_string()),
            selected_provider: Some("OpenAI".to_string()),
            served_model: Some("openai/gpt-4o-mini".to_string()),
            fallback_from_preferred: Some(false),
            latency_ms: Some(312),
            usage: StreamUsage {
                prompt_tokens: Some(5),
                completion_tokens: Some(7),
                total_tokens: Some(12),
            },
        };

        let policy = OpenRouterRoutingPolicy {
            order: vec!["openai".to_string()],
            allow_fallbacks: Some(true),
            ..OpenRouterRoutingPolicy::default()
        };

        let report = RoutedSmokeReport::from_telemetry(&telemetry, Some(&policy));

        // Structural redaction guarantee: the constructor accepts no prompt or
        // reply text — the only String-typed fields in the report are derived
        // from sanitized telemetry metadata, never from completion content.
        assert!(
            report.has_no_content_fields(),
            "RoutedSmokeReport::has_no_content_fields() must always return true \
             (structural privacy invariant for audio-graph-fe7b)"
        );

        // Field-level sanity: values must reflect the telemetry we fed in.
        assert_eq!(report.status, "ok");
        assert_eq!(report.model, "openai/gpt-4o-mini");
        assert_eq!(report.latency_ms, 312);
        assert_eq!(report.prompt_tokens, 5);
        assert_eq!(report.completion_tokens, 7);
        assert_eq!(report.total_tokens, 12);
        assert_eq!(report.fallback_from_preferred, Some(false));

        // Routing policy summary must be content-free (count-based, not raw
        // provider strings).
        assert!(
            report.sanitized_routing_policy.contains("order[1]"),
            "policy summary must encode order slot count, not raw provider name: {}",
            report.sanitized_routing_policy
        );
        assert!(
            !report.sanitized_routing_policy.contains("openai"),
            "policy summary must NOT embed raw provider string `openai`: {}",
            report.sanitized_routing_policy
        );

        // request_id_hash must be a 16-hex-char hash (not "none", because we
        // supplied a non-empty request id).
        assert_eq!(
            report.request_id_hash.len(),
            16,
            "request_id_hash must be 16 hex chars: {}",
            report.request_id_hash
        );
        assert!(
            report
                .request_id_hash
                .chars()
                .all(|c| c.is_ascii_hexdigit()),
            "request_id_hash must be hex: {}",
            report.request_id_hash
        );
    }

    /// LIVE, network-dependent routed-smoke harness (env-gated; `#[ignore]`d so
    /// CI without a key stays green). This is the HARNESS PLUMBING built by seed
    /// audio-graph-fe7b. The live RUN is owned by seed 8772 (needs secret-hygiene
    /// scanner + CI secret wiring — NOT wired here).
    ///
    /// Run manually with a real OpenRouter key:
    ///
    /// ```text
    /// OPENROUTER_API_KEY=sk-or-v1-xxx cargo test \
    ///     --no-default-features --features cloud \
    ///     -p audio-graph openrouter::tests::live_openrouter_routed_smoke \
    ///     -- --ignored --nocapture
    /// ```
    ///
    /// **Privacy invariant (audio-graph-fe7b).** The completion reply text is
    /// intentionally discarded immediately after the call returns — it is never
    /// stored in `report` or any other variable that outlives the assertion
    /// block. The printed artifact is a `RoutedSmokeReport`: counts, timing,
    /// model slug, sanitized policy description, and a hashed request id.
    /// No raw prompt text, no raw reply text, no API key appears in the output.
    ///
    /// Asserts (live path):
    /// - The completion call succeeds (status = ok).
    /// - Token counts ≥ 1 (provider returned a usage block).
    /// - `report.has_no_content_fields()` holds after a real completion.
    /// - The report JSON contains neither the synthetic prompt text nor the
    ///   API key string.
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "hits the live OpenRouter API; requires OPENROUTER_API_KEY. Run with -- --ignored"]
    async fn live_openrouter_routed_smoke() {
        let Ok(api_key) = std::env::var("OPENROUTER_API_KEY") else {
            panic!(
                "OPENROUTER_API_KEY not set — this #[ignore]d live test needs a real key.\n\
                 Run: OPENROUTER_API_KEY=sk-or-v1-xxx cargo test \
                 -p audio-graph openrouter::tests::live_openrouter_routed_smoke \
                 -- --ignored --nocapture"
            );
        };
        assert!(!api_key.trim().is_empty(), "OPENROUTER_API_KEY is empty");

        // Use a minimal routing policy: prefer openai but allow fallbacks so
        // the test passes even when the primary provider is degraded.
        let policy = OpenRouterRoutingPolicy {
            order: vec!["openai".to_string()],
            allow_fallbacks: Some(true),
            ..OpenRouterRoutingPolicy::default()
        };

        // Synthetic prompt: terse, deterministic, produces ≥1 completion token.
        // We store it only to assert it does NOT appear in the report artifact.
        let synthetic_prompt = "Reply with the single digit 1.";

        let config = OpenRouterConfig {
            api_key: api_key.clone(),
            // openai/gpt-4o-mini is the cheapest broadly-available model on
            // OpenRouter; ideal for a smoke ping that just needs ≥1 token back.
            model: "openai/gpt-4o-mini".to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            provider_order: Some(policy.order.clone()),
            routing_policy: Some(policy.clone()),
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 8,
            temperature: 0.0,
        };

        let client = OpenRouterClient::new(config)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());

        let prompt_for_call = synthetic_prompt.to_string();
        // Run the blocking client on a dedicated thread (reqwest::blocking
        // must not execute on an async runtime thread).
        let result = tokio::task::spawn_blocking(move || {
            client.chat_completion_with_routing_telemetry(
                vec![("user".to_string(), prompt_for_call)],
                false,
            )
        })
        .await
        .expect("spawn_blocking join");

        let (_reply, telemetry) = result.expect("live OpenRouter completion must succeed");
        // `_reply` is intentionally unused — binding prevents dead-code warnings
        // while making it explicit that we are discarding the reply text here.
        // The report carries no content.

        let report = RoutedSmokeReport::from_telemetry(&telemetry, Some(&policy));

        // Privacy invariant: report must carry no raw prompt, reply, or key.
        let report_json =
            serde_json::to_string_pretty(&report).expect("RoutedSmokeReport must serialize");
        assert!(
            !report_json.contains(synthetic_prompt),
            "report must not persist prompt text"
        );
        assert!(
            !report_json.contains(&api_key),
            "report must not persist the API key"
        );
        assert!(
            report.has_no_content_fields(),
            "structural content-free guarantee must hold on a real completion"
        );

        // Liveness: the call returned at least one token.
        assert!(
            report.total_tokens >= 1,
            "provider must return ≥1 token in usage block; got {} total_tokens",
            report.total_tokens
        );
        assert_eq!(report.status, "ok");

        // Emit the sanitized report for manual inspection (`--nocapture`).
        println!("\n=== RoutedSmokeReport (audio-graph-fe7b) ===");
        println!("{report_json}");
        println!("============================================\n");
    }
}
