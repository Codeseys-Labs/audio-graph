pub mod contract;
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
            "d3-edge" | "d3-lite" | "d3-nano" | "d3-mini" => "decision-d3",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_engines_name_the_required_feature_without_loading_models() {
        let registry = EngineRegistry::default();
        let config = EngineConfig::new(PathBuf::from("nonexistent-models"));
        for (id, required) in [("d3-nano", "decision-d3"), ("clef-flash", "decision-clef")] {
            assert!(
                matches!(registry.create(id, &config), Err(DecisionError::EngineUnavailable { feature }) if feature == required)
            );
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
