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
//! The speech processor and chat commands try the user's preferred backend first,
//! then fallback alternatives, then rule-based extraction as a final fallback.

pub mod api_client;
pub mod engine;
pub mod executor;
pub mod mistralrs_engine;
pub mod openrouter;
pub mod sse;
pub mod streaming;

pub use api_client::{ApiClient, ApiConfig};
pub use engine::LlmEngine;
pub use executor::{LlmExecutor, LlmPriority};
pub use mistralrs_engine::MistralRsEngine;
pub use openrouter::{OpenRouterClient, OpenRouterConfig, OpenRouterModel};
