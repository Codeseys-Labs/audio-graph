//! Qwen3.5 text backbone on candle, shared by the native decision engines (d3 with bidirectional full
//! attention, Clef-flash with the stock causal forward). No engine-specific head lives here: callers take
//! final-norm hidden states from [`Encoder`] and apply their own head.

pub mod config;
mod gdn;
mod model;

pub use config::TextConfig;
pub use model::{Attention, Encoder, LoadOptions, weight_files};

use crate::ExecutionProvider;
use audio_graph_decision_contract::DecisionError;
use candle_core::Device;

/// The candle device for an execution provider. candle has CUDA and Metal backends only; DirectML
/// requests are refused (Vulkan-only GPUs: audio-graph-26b4).
pub fn device_for(provider: ExecutionProvider) -> Result<Device, DecisionError> {
    match provider {
        ExecutionProvider::Cpu => Ok(Device::Cpu),
        #[cfg(all(feature = "cuda", any(target_os = "windows", target_os = "linux")))]
        ExecutionProvider::Cuda => {
            let device = Device::new_cuda(0)
                .map_err(|_| DecisionError::EngineUnavailable { feature: "cuda" })?;
            // One stream and no cross-stream sharing: candle's per-tensor event tracking is pure overhead.
            // SAFETY: every tensor of the engines lives on this device's single stream.
            unsafe {
                device
                    .as_cuda_device()
                    .map_err(|_| DecisionError::Internal)?
                    .disable_event_tracking()
            };
            Ok(device)
        }
        #[cfg(all(feature = "metal", target_os = "macos"))]
        ExecutionProvider::CoreMl => {
            Device::new_metal(0).map_err(|_| DecisionError::EngineUnavailable { feature: "metal" })
        }
        #[allow(unreachable_patterns)]
        _ => Err(DecisionError::EngineUnavailable { feature: "cuda" }),
    }
}
