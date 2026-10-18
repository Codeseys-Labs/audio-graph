//! Qwen3.5 text backbone on candle, shared by the native decision engines (d3 with bidirectional full
//! attention, Clef-flash with the stock causal forward). No engine-specific head lives here: callers take
//! final-norm hidden states from [`Encoder`] and apply their own head.

pub mod config;
mod gdn;
mod model;

pub use config::TextConfig;
pub use model::{Attention, Encoder, LoadOptions, weight_files};
