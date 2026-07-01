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

fn build_chat_completion_request<'a>(
    config: &'a OpenRouterConfig,
    messages: Vec<ApiMessage>,
    response_format: Option<ResponseFormat>,
) -> ChatCompletionRequest<'a> {
    ChatCompletionRequest {
        model: &config.model,
        messages,
        max_tokens: config.max_tokens,
        temperature: config.temperature,
        response_format,
        provider: config.provider_routing_value(),
    }
}

#[cfg(test)]
pub(crate) fn blocking_chat_provider_value_for_test(
    config: &OpenRouterConfig,
) -> Option<serde_json::Value> {
    build_chat_completion_request(config, Vec::new(), None).provider
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
        self.content_egress_policy.check_prompt("llm.openrouter")?;

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

        let request = build_chat_completion_request(&self.config, api_messages, response_format);

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
            let request_id = response_request_id(response.headers());
            let body = response.text().unwrap_or_default();
            return Err(openrouter_http_error_message(
                status,
                &url,
                &body,
                request_id.as_deref(),
            ));
        }

        let completion: ChatCompletionResponse = response
            .json()
            .map_err(|e| format!("Failed to parse OpenRouter chat response: {}", e))?;

        let usage = completion
            .usage
            .map(Usage::into_stream_usage)
            .unwrap_or_default();
        completion
            .choices
            .first()
            .map(|c| (c.message.content.clone(), usage))
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
            vec![ApiMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            None,
        );
        serde_json::to_value(request).expect("blocking request serializes")
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
}
