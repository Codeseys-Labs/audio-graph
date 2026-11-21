//! Qwen3.5 text backbone on candle, shared by native decision engines (d3 today; the Clef-flash
//! backbone is also Qwen3.5). No engine-specific readout lives here: callers take final-norm hidden
//! states from [`Encoder`] and apply their own head.

pub mod config;
mod gdn;
mod model;

pub use config::TextConfig;
pub use model::{Attention, Encoder, LoadOptions};
