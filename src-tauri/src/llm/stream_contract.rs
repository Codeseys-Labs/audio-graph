//! Provider-neutral streaming chat contracts.
//!
//! OpenAI-compatible SSE is one transport, but local llama.cpp, mistral.rs, and
//! Bedrock streaming need the same request context, terminal-frame semantics,
//! and usage accounting without reaching through global application state.

use std::fmt;
use std::sync::{Arc, Mutex};

use audio_graph_ipc_contract::runtime_diagnostic::{
    RuntimeDiagnostic, RuntimeDiagnosticContext, RuntimeOperation,
};

use serde::{Deserialize, Serialize};

use crate::asr::ProviderContentEgressPolicy;
use crate::llm::api_client::ApiClient;
use crate::llm::engine::{ChatMessage, LlmEngine};
use crate::llm::mistralrs_engine::MistralRsEngine;
use crate::llm::openrouter::OpenRouterClient;
use crate::settings::LlmProvider;

/// Token-usage block reported by any streaming chat provider.
///
/// OpenAI-compatible streams deserialize this from `usage`, but the type lives
/// here so non-SSE adapters can report the same telemetry shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct StreamUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    #[serde(default)]
    pub total_tokens: Option<u32>,
}

impl StreamUsage {
    pub fn has_reported_total(&self) -> bool {
        self.total_tokens.is_some_and(|tokens| tokens > 0)
    }
}

/// User-configured sampling settings for a streaming chat request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamParams {
    pub max_tokens: u32,
    pub temperature: f32,
}

impl Default for StreamParams {
    /// Matches the blocking chat path's fallback when no `llm_api_config`
    /// is present (`commands::*_config_from_runtime_settings`).
    fn default() -> Self {
        Self {
            max_tokens: 512,
            temperature: 0.1,
        }
    }
}

/// Explicit backend handles available to streaming provider adapters.
///
/// `stream_chat` should receive this bundle from its caller when an adapter
/// needs runtime engines or clients. Adapters must not recover these handles
/// from process globals.
#[derive(Clone, Default)]
pub struct StreamBackendHandles {
    pub local_llama: Option<Arc<Mutex<Option<LlmEngine>>>>,
    pub api_client: Option<Arc<Mutex<Option<ApiClient>>>>,
    pub openrouter_client: Option<Arc<Mutex<Option<OpenRouterClient>>>>,
    pub mistralrs_engine: Option<Arc<Mutex<Option<MistralRsEngine>>>>,
}

impl StreamBackendHandles {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(
        local_llama: Arc<Mutex<Option<LlmEngine>>>,
        api_client: Arc<Mutex<Option<ApiClient>>>,
        openrouter_client: Arc<Mutex<Option<OpenRouterClient>>>,
        mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,
    ) -> Self {
        Self {
            local_llama: Some(local_llama),
            api_client: Some(api_client),
            openrouter_client: Some(openrouter_client),
            mistralrs_engine: Some(mistralrs_engine),
        }
    }

    pub fn has_handle_for(&self, provider: &LlmProvider) -> bool {
        match provider {
            LlmProvider::LocalLlama => self.local_llama.is_some(),
            LlmProvider::Api { .. } => self.api_client.is_some(),
            LlmProvider::OpenRouter { .. } => self.openrouter_client.is_some(),
            LlmProvider::MistralRs { .. } => self.mistralrs_engine.is_some(),
            LlmProvider::AwsBedrock { .. } => false,
        }
    }
}

impl fmt::Debug for StreamBackendHandles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamBackendHandles")
            .field("local_llama", &self.local_llama.is_some())
            .field("api_client", &self.api_client.is_some())
            .field("openrouter_client", &self.openrouter_client.is_some())
            .field("mistralrs_engine", &self.mistralrs_engine.is_some())
            .finish()
    }
}

/// Provider/model metadata attached to a streaming request or terminal event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct StreamBackendMetadata {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl StreamBackendMetadata {
    pub fn from_provider(provider: &LlmProvider) -> Self {
        let model = match provider {
            LlmProvider::LocalLlama => None,
            LlmProvider::Api { model, .. } | LlmProvider::OpenRouter { model, .. } => {
                Some(model.clone())
            }
            LlmProvider::AwsBedrock { model_id, .. } | LlmProvider::MistralRs { model_id } => {
                Some(model_id.clone())
            }
        };
        Self {
            provider: provider_kind(provider).to_string(),
            model,
        }
    }
}

/// Optional source/session metadata for a streaming request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct StreamSourceMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl StreamSourceMetadata {
    pub fn is_empty(&self) -> bool {
        self.session_id.is_none() && self.source_id.is_none() && self.request_id.is_none()
    }
}

/// Provider-neutral context metadata for observability and future adapters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct StreamContextMetadata {
    pub backend: StreamBackendMetadata,
    #[serde(default, skip_serializing_if = "StreamSourceMetadata::is_empty")]
    pub source: StreamSourceMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
}

impl StreamContextMetadata {
    pub fn from_provider(provider: &LlmProvider) -> Self {
        Self {
            backend: StreamBackendMetadata::from_provider(provider),
            source: StreamSourceMetadata::default(),
            context_id: None,
        }
    }
}

/// Complete provider-neutral streaming chat request.
#[derive(Clone)]
pub struct StreamChatRequest {
    pub provider: LlmProvider,
    pub history: Vec<ChatMessage>,
    pub graph_context: String,
    pub params: StreamParams,
    pub content_egress_policy: ProviderContentEgressPolicy,
    pub backend_handles: StreamBackendHandles,
    pub metadata: StreamContextMetadata,
}

impl StreamChatRequest {
    pub fn new(
        provider: LlmProvider,
        history: Vec<ChatMessage>,
        graph_context: String,
        params: StreamParams,
    ) -> Self {
        let metadata = StreamContextMetadata::from_provider(&provider);
        Self {
            provider,
            history,
            graph_context,
            params,
            content_egress_policy: ProviderContentEgressPolicy::block("explicit_policy_required"),
            backend_handles: StreamBackendHandles::empty(),
            metadata,
        }
    }

    pub fn with_content_egress_policy(mut self, policy: ProviderContentEgressPolicy) -> Self {
        self.content_egress_policy = policy;
        self
    }

    pub fn with_backend_handles(mut self, backend_handles: StreamBackendHandles) -> Self {
        self.backend_handles = backend_handles;
        self
    }

    pub fn with_source_metadata(mut self, source: StreamSourceMetadata) -> Self {
        self.metadata.source = source;
        self
    }

    pub fn with_context_id(mut self, context_id: impl Into<String>) -> Self {
        self.metadata.context_id = Some(context_id.into());
        self
    }
}

/// Closed kind of an [`LlmProvider`], without its endpoint, model, or key.
const fn provider_kind(provider: &LlmProvider) -> &'static str {
    match provider {
        LlmProvider::LocalLlama => "LocalLlama",
        LlmProvider::Api { .. } => "Api",
        LlmProvider::OpenRouter { .. } => "OpenRouter",
        LlmProvider::AwsBedrock { .. } => "AwsBedrock",
        LlmProvider::MistralRs { .. } => "MistralRs",
    }
}

impl fmt::Debug for StreamChatRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Presence only: an endpoint or an exact content size is not for logs.
        f.debug_struct("StreamChatRequest")
            .field(
                "provider",
                &format_args!("{}", provider_kind(&self.provider)),
            )
            .field("has_history", &!self.history.is_empty())
            .field("has_graph_context", &!self.graph_context.is_empty())
            .field("params", &self.params)
            .field("policy", &self.content_egress_policy)
            .finish()
    }
}

/// Closed finish-reason vocabulary shared by every streaming adapter.
///
/// A provider's raw value never crosses the terminal boundary: it either
/// matches this allowlist or becomes [`FinishReason::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    Length,
    ContentFilter,
    ToolCalls,
    /// The provider ended the turn itself reporting a fault.
    Error,
    Unknown,
}

impl FinishReason {
    pub fn from_provider(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "stop" | "end_turn" | "stop_sequence" => Self::Stop,
            "length" | "max_tokens" | "model_length" => Self::Length,
            "content_filter" | "content_filtered" | "guardrail_intervened" | "refusal" => {
                Self::ContentFilter
            }
            "tool_calls" | "tool_use" | "function_call" => Self::ToolCalls,
            "error" => Self::Error,
            _ => Self::Unknown,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Length => "length",
            Self::ContentFilter => "content_filter",
            Self::ToolCalls => "tool_calls",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }
}

/// Shared terminal-frame semantics for all streaming providers. Every variant
/// is closed: there is no free-text terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamTerminalReason {
    Done { finish_reason: FinishReason },
    Diagnostic(RuntimeDiagnostic),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamTerminalEvent {
    pub reason: StreamTerminalReason,
    pub full_text: String,
    pub usage: Option<StreamUsage>,
    pub metadata: StreamContextMetadata,
}

impl StreamTerminalEvent {
    /// A completed turn. A turn the provider ended by reporting its own fault
    /// is a failure whatever text came before it, classified like any other
    /// fault that names no code.
    pub fn done(
        full_text: String,
        usage: Option<StreamUsage>,
        finish_reason: FinishReason,
        metadata: StreamContextMetadata,
    ) -> Self {
        if finish_reason == FinishReason::Error {
            let context = RuntimeDiagnosticContext::new(RuntimeOperation::LanguageModelInference);
            return Self::failure(RuntimeDiagnostic::internal(context), full_text, metadata);
        }
        Self {
            reason: StreamTerminalReason::Done { finish_reason },
            full_text,
            usage,
            metadata,
        }
    }

    pub(super) fn failure(
        diagnostic: RuntimeDiagnostic,
        full_text: String,
        metadata: StreamContextMetadata,
    ) -> Self {
        Self {
            reason: StreamTerminalReason::Diagnostic(diagnostic),
            full_text,
            usage: None,
            metadata,
        }
    }

    pub fn cancelled(full_text: String, metadata: StreamContextMetadata) -> Self {
        Self {
            reason: StreamTerminalReason::Cancelled,
            full_text,
            usage: None,
            metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_chat_request_debug_reports_shape_without_prompt_or_graph_content() {
        let graph_context = format!("FAKE_GRAPH_CONTEXT_DO_NOT_LOG{}", "x".repeat(7890));
        let graph_context = graph_context.as_str();
        assert_eq!(graph_context.chars().count(), 7919);
        let request = StreamChatRequest::new(
            LlmProvider::LocalLlama,
            vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "FAKE_PROMPT_DO_NOT_LOG".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: "SECOND_FAKE_PROMPT_DO_NOT_LOG".to_string(),
                },
            ],
            graph_context.to_string(),
            StreamParams {
                max_tokens: 42,
                temperature: 0.2,
            },
        )
        .with_content_egress_policy(ProviderContentEgressPolicy::block("local_only"));

        let debug = format!("{request:?}");

        assert!(!debug.contains("FAKE_PROMPT_DO_NOT_LOG"));
        assert!(!debug.contains("SECOND_FAKE_PROMPT_DO_NOT_LOG"));
        assert!(!debug.contains(graph_context));
        assert!(debug.contains("provider: LocalLlama"));
        assert!(debug.contains("has_history: true"));
        assert!(debug.contains("has_graph_context: true"));
        assert!(debug.contains("params: StreamParams"));
        assert!(debug.contains("max_tokens: 42"));
        assert!(debug.contains("temperature: 0.2"));
        assert!(debug.contains("policy: ProviderContentEgressPolicy"));
        // Neither an exact content size nor a provider endpoint is rendered.
        assert!(!debug.contains("7919"));

        let remote = StreamChatRequest::new(
            LlmProvider::Api {
                endpoint: "https://private-endpoint-canary.example/v1".to_string(),
                api_key: "debug-credential-canary".to_string(),
                model: "test-model".to_string(),
            },
            vec![],
            String::new(),
            StreamParams::default(),
        );
        let debug = format!("{remote:?}");
        assert!(debug.contains("provider: Api"));
        assert!(debug.contains("has_history: false"));
        assert!(!debug.contains("private-endpoint-canary"));
        assert!(!debug.contains("debug-credential-canary"));
    }

    #[test]
    fn finish_reason_is_an_allowlist() {
        for (raw, reason) in [
            ("stop", FinishReason::Stop),
            (" END_TURN ", FinishReason::Stop),
            ("length", FinishReason::Length),
            ("content_filter", FinishReason::ContentFilter),
            ("tool_calls", FinishReason::ToolCalls),
            ("error", FinishReason::Error),
            ("cancelled", FinishReason::Unknown),
            ("error: finish-reason-canary", FinishReason::Unknown),
            ("", FinishReason::Unknown),
        ] {
            assert_eq!(FinishReason::from_provider(raw), reason, "{raw:?}");
        }
    }
}
