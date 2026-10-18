pub mod contract;
#[cfg(feature = "decision-d3")]
pub mod d3;
#[cfg(feature = "decision-laya")]
pub mod laya;

pub use contract::*;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub const LAYA_ENGLISH: &str = "laya-english";
pub const LAYA_MULTILINGUAL: &str = "laya-multilingual";
pub const LAYA_BUNDLE_FILES: &[&str] = &[
    "laya.onnx",
    "laya.onnx.data",
    "laya_config.json",
    "tokenizer/tokenizer.json",
    "tokenizer/tokenizer_config.json",
];

pub const D3_EDGE: &str = "d3-edge";
pub const D3_LITE: &str = "d3-lite";
pub const D3_NANO: &str = "d3-nano";
pub const D3_MINI: &str = "d3-mini";

/// A Decision 3.0 checkpoint: the files the d3 backend reads from `<models_dir>/<id>/`, downloaded from
/// `url` (a Hugging Face `resolve/<revision>` base, pinned to the revision the parity fixtures used).
#[derive(Debug)]
pub struct D3Checkpoint {
    pub id: &'static str,
    pub repo: &'static str,
    pub url: &'static str,
    pub files: &'static [&'static str],
}

pub const D3_CHECKPOINTS: &[D3Checkpoint] = &[
    D3Checkpoint {
        id: D3_EDGE,
        repo: "vllm-sr/d3-edge",
        url: "https://huggingface.co/vllm-sr/d3-edge/resolve/c454ee39b862248d70abd7f84195d2f54ffe6d43",
        files: &[
            "config.json",
            "decision_config.json",
            "readout.safetensors",
            "tokenizer.json",
            "model.safetensors.index.json",
            "model-00001-of-00001.safetensors",
        ],
    },
    D3Checkpoint {
        id: D3_LITE,
        repo: "vllm-sr/d3-lite",
        url: "https://huggingface.co/vllm-sr/d3-lite/resolve/03560c26f5199c3c04129a2ccb38960c913635e0",
        files: &[
            "config.json",
            "decision_config.json",
            "readout.safetensors",
            "tokenizer.json",
            "model.safetensors.index.json",
            "model-00001-of-00001.safetensors",
        ],
    },
    D3Checkpoint {
        id: D3_NANO,
        repo: "vllm-sr/d3-nano",
        url: "https://huggingface.co/vllm-sr/d3-nano/resolve/462efc366229ea268cf4eed3738b3fc4415e4817",
        files: &[
            "config.json",
            "decision_config.json",
            "readout.safetensors",
            "tokenizer.json",
            "model.safetensors",
        ],
    },
    D3Checkpoint {
        id: D3_MINI,
        repo: "vllm-sr/d3-mini",
        url: "https://huggingface.co/vllm-sr/d3-mini/resolve/bd0090dd758d32ba59ef5c749910a8e87fc4391f",
        files: &[
            "config.json",
            "decision_config.json",
            "readout.safetensors",
            "tokenizer.json",
            "model.safetensors.index.json",
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
        ],
    },
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExecutionProvider {
    #[default]
    Cpu,
    Cuda,
    DirectMl,
    CoreMl,
}

impl ExecutionProvider {
    pub fn feature(self) -> Option<&'static str> {
        match self {
            Self::Cpu => None,
            Self::Cuda => Some("cuda"),
            Self::DirectMl => Some("vulkan"),
            Self::CoreMl => Some("metal"),
        }
    }

    pub fn is_compiled(self) -> bool {
        match self {
            Self::Cpu => true,
            Self::Cuda => cfg!(all(
                feature = "cuda",
                any(target_os = "windows", target_os = "linux")
            )),
            Self::DirectMl => cfg!(all(feature = "vulkan", target_os = "windows")),
            Self::CoreMl => cfg!(all(feature = "metal", target_os = "macos")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// The existing model manager's get_models_dir(app), never a second cache.
    pub models_dir: PathBuf,
    pub execution_provider: ExecutionProvider,
    pub allow_large_choice_sets: bool,
}

impl EngineConfig {
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            execution_provider: ExecutionProvider::Cpu,
            allow_large_choice_sets: false,
        }
    }
}

pub type EngineConstructor = fn(&EngineConfig) -> Result<Arc<dyn DecisionEngine>, DecisionError>;

pub struct EngineRegistry {
    constructors: BTreeMap<String, EngineConstructor>,
}

impl Default for EngineRegistry {
    fn default() -> Self {
        let mut constructors: BTreeMap<String, EngineConstructor> = BTreeMap::new();
        constructors.insert("laya".into(), construct_laya);
        constructors.insert(D3_EDGE.into(), construct_d3::<0>);
        constructors.insert(D3_LITE.into(), construct_d3::<1>);
        constructors.insert(D3_NANO.into(), construct_d3::<2>);
        constructors.insert(D3_MINI.into(), construct_d3::<3>);
        Self { constructors }
    }
}

impl EngineRegistry {
    /// Returns the previous constructor when replacing a registered engine.
    pub fn register(
        &mut self,
        id: String,
        constructor: EngineConstructor,
    ) -> Option<EngineConstructor> {
        self.constructors.insert(id, constructor)
    }

    pub fn create(
        &self,
        id: &str,
        config: &EngineConfig,
    ) -> Result<Arc<dyn DecisionEngine>, DecisionError> {
        if let Some(constructor) = self.constructors.get(id) {
            return constructor(config);
        }
        let feature = match id {
            "clef-flash" => "decision-clef",
            _ => return Err(DecisionError::UnsupportedQuestion),
        };
        Err(DecisionError::EngineUnavailable { feature })
    }
}

fn construct_laya(config: &EngineConfig) -> Result<Arc<dyn DecisionEngine>, DecisionError> {
    #[cfg(feature = "decision-laya")]
    {
        Ok(Arc::new(laya::LayaEngine::load(config)?))
    }
    #[cfg(not(feature = "decision-laya"))]
    {
        let _ = config;
        Err(DecisionError::EngineUnavailable {
            feature: "decision-laya",
        })
    }
}

fn construct_d3<const INDEX: usize>(
    config: &EngineConfig,
) -> Result<Arc<dyn DecisionEngine>, DecisionError> {
    #[cfg(feature = "decision-d3")]
    {
        Ok(Arc::new(d3::D3Engine::load(
            config,
            &D3_CHECKPOINTS[INDEX],
        )?))
    }
    #[cfg(not(feature = "decision-d3"))]
    {
        let _ = config;
        Err(DecisionError::EngineUnavailable {
            feature: "decision-d3",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_engines_name_the_required_feature_without_loading_models() {
        let registry = EngineRegistry::default();
        let config = EngineConfig::new(PathBuf::from("nonexistent-models"));
        assert!(
            matches!(registry.create("clef-flash", &config), Err(DecisionError::EngineUnavailable { feature }) if feature == "decision-clef")
        );
        #[cfg(not(feature = "decision-d3"))]
        for id in [D3_EDGE, D3_LITE, D3_NANO, D3_MINI] {
            assert!(
                matches!(registry.create(id, &config), Err(DecisionError::EngineUnavailable { feature }) if feature == "decision-d3")
            );
        }
        // With the backend compiled the engine is created without touching the absent model files.
        #[cfg(feature = "decision-d3")]
        for id in [D3_EDGE, D3_LITE, D3_NANO, D3_MINI] {
            assert_eq!(registry.create(id, &config).unwrap().id(), id);
        }
        #[cfg(not(feature = "decision-laya"))]
        assert!(matches!(
            registry.create("laya", &config),
            Err(DecisionError::EngineUnavailable {
                feature: "decision-laya"
            })
        ));
    }
}
