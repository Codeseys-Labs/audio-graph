//! LLM inference backends.
//!
//! Four backends are available:
//! - **Native** (`engine`): In-process GGUF model inference via llama-cpp-2.
//! - **API** (`api_client`): Generic OpenAI-compatible HTTP API (Ollama, LM Studio, vLLM, etc.).
//! - **OpenRouter** (`openrouter`): First-class OpenRouter client (ADR-0005) — same
//!   OpenAI-compatible wire shape as `api_client` but with hardcoded base URL,
//!   attribution headers, and a dedicated test/list-models surface. Streaming
//!   chat is plan A3 / ADR-0006; this module ships the blocking surface only.
//! - **MistralRs** (`mistralrs_engine`): Rust-native GGUF inference via mistral.rs (Candle),
//!   with JSON Schema-constrained structured generation for entity extraction.
//!
//! Which backend a request reaches is decided by the named route table in
//! [`route`] (ADR-0038): each content-bearing dispatch resolves **exactly one**
//! authorized route and there is no automatic cross-provider fallback. Entity
//! extraction still has the local rule-based extractor as its final fallback —
//! that is a local, non-egress substitution, not a provider hop.

pub mod api_client;
pub mod bedrock;
pub mod engine;
pub mod executor;
pub(crate) mod http_diag;
pub mod mistralrs_engine;
pub mod openrouter;
pub mod route;
pub mod sse;
pub mod stream_contract;
pub mod streaming;

pub use api_client::{ApiClient, ApiConfig};
pub use engine::LlmEngine;
pub use executor::{LlmExecutor, LlmPriority, ProjectionPatchAttempt, ProjectionPatchOutcome};
pub use mistralrs_engine::MistralRsEngine;
pub use openrouter::{OpenRouterClient, OpenRouterConfig, OpenRouterModel};
