//! Provider-neutral streaming chat contracts.
//!
//! OpenAI-compatible SSE is one transport, but local llama.cpp, mistral.rs, and
//! Bedrock streaming need the same request context, terminal-frame semantics,
//! and usage accounting without reaching through global application state.

use std::fmt;
use std::sync::{Arc, Mutex};

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
        match provider {
            LlmProvider::LocalLlama => Self {
                provider: "LocalLlama".to_string(),
                model: None,
            },
            LlmProvider::Api { model, .. } => Self {
                provider: "Api".to_string(),
                model: Some(model.clone()),
            },
            LlmProvider::OpenRouter { model, .. } => Self {
                provider: "OpenRouter".to_string(),
                model: Some(model.clone()),
            },
            LlmProvider::AwsBedrock { model_id, .. } => Self {
                provider: "AwsBedrock".to_string(),
                model: Some(model_id.clone()),
            },
            LlmProvider::MistralRs { model_id } => Self {
                provider: "MistralRs".to_string(),
                model: Some(model_id.clone()),
            },
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

impl fmt::Debug for StreamChatRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamChatRequest")
            .field("provider", &self.provider)
            .field("message_count", &self.history.len())
            .field("graph_context_chars", &self.graph_context.chars().count())
            .field("params", &self.params)
            .field("policy", &self.content_egress_policy)
            .finish()
    }
}

/// Shared terminal-frame semantics for all streaming providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamTerminalReason {
    Done { finish_reason: String },
    Error { message: String },
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
    pub fn done(
        full_text: String,
        usage: Option<StreamUsage>,
        finish_reason: String,
        metadata: StreamContextMetadata,
    ) -> Self {
        Self {
            reason: StreamTerminalReason::Done { finish_reason },
            full_text,
            usage,
            metadata,
        }
    }

    pub fn error(message: String, full_text: String, metadata: StreamContextMetadata) -> Self {
        Self {
            reason: StreamTerminalReason::Error { message },
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
        let graph_context = "FAKE_GRAPH_CONTEXT_DO_NOT_LOG";
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
        assert!(debug.contains("message_count: 2"));
        assert!(debug.contains(&format!(
            "graph_context_chars: {}",
            graph_context.chars().count()
        )));
        assert!(debug.contains("params: StreamParams"));
        assert!(debug.contains("max_tokens: 42"));
        assert!(debug.contains("temperature: 0.2"));
        assert!(debug.contains("policy: ProviderContentEgressPolicy"));
    }
}
