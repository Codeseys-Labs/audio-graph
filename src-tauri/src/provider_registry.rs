//! Backend-owned provider capability registry.
//!
//! The provider metadata itself lives in the lightweight
//! `audio-graph-provider-registry` crate so the TypeScript generator can run
//! without linking the full Tauri app. This module keeps the Tauri command and
//! settings-enum mapping close to the app code that consumes them.

pub use audio_graph_provider_registry::*;

use crate::settings::{AsrProvider, LlmProvider, TtsProvider};

#[tauri::command]
pub fn get_provider_registry_cmd() -> Vec<ProviderDescriptor> {
    provider_registry().to_vec()
}

pub fn descriptor_for_asr_provider(provider: &AsrProvider) -> &'static ProviderDescriptor {
    descriptor_by_id(match provider {
        AsrProvider::LocalWhisper => "asr.local_whisper",
        AsrProvider::Api { .. } => "asr.api",
        AsrProvider::AwsTranscribe { .. } => "asr.aws_transcribe",
        AsrProvider::DeepgramStreaming { .. } => "asr.deepgram",
        AsrProvider::AssemblyAI { .. } => "asr.assemblyai",
        AsrProvider::Soniox { .. } => "asr.soniox",
        AsrProvider::SherpaOnnx { .. } => "asr.sherpa_onnx",
        AsrProvider::Moonshine { .. } => "asr.moonshine",
        AsrProvider::OpenAiRealtimeTranscription { .. } => "asr.openai_realtime",
    })
}

pub fn descriptor_for_llm_provider(provider: &LlmProvider) -> &'static ProviderDescriptor {
    descriptor_by_id(match provider {
        LlmProvider::LocalLlama => "llm.local_llama",
        LlmProvider::Api { endpoint, .. } if crate::settings::is_cerebras_endpoint(endpoint) => {
            "llm.cerebras"
        }
        LlmProvider::Api { .. } => "llm.api",
        LlmProvider::OpenRouter { .. } => "llm.openrouter",
        LlmProvider::AwsBedrock { .. } => "llm.aws_bedrock",
        LlmProvider::MistralRs { .. } => "llm.mistralrs",
    })
}

pub fn descriptor_for_tts_provider(provider: &TtsProvider) -> &'static ProviderDescriptor {
    descriptor_by_id(match provider {
        TtsProvider::None => "tts.none",
        TtsProvider::DeepgramAura { .. } => "tts.deepgram_aura",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{AwsCredentialSource, GeminiAuthMode};

    #[test]
    fn descriptor_credential_keys_are_allowed() {
        for descriptor in provider_registry() {
            for key in descriptor.credential_keys {
                assert!(
                    crate::credentials::is_allowed_key(key),
                    "{} references credential key not in ALLOWED_CREDENTIAL_KEYS: {}",
                    descriptor.id,
                    key
                );
            }
        }
    }

    #[test]
    fn asr_variants_have_descriptors() {
        let providers = vec![
            AsrProvider::LocalWhisper,
            AsrProvider::Api {
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key: String::new(),
                model: "whisper-1".to_string(),
            },
            AsrProvider::AwsTranscribe {
                region: "us-east-1".to_string(),
                language_code: "en-US".to_string(),
                credential_source: AwsCredentialSource::DefaultChain,
                enable_diarization: true,
            },
            AsrProvider::DeepgramStreaming {
                api_key: String::new(),
                model: "nova-3".to_string(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 0,
            },
            AsrProvider::AssemblyAI {
                api_key: String::new(),
                enable_diarization: true,
            },
            AsrProvider::Soniox {
                api_key: String::new(),
                model: crate::asr::soniox::DEFAULT_MODEL.to_string(),
                enable_diarization: true,
                enable_language_identification: true,
                language_hints: vec![],
                max_speakers: 0,
            },
            AsrProvider::SherpaOnnx {
                model_dir: crate::models::SHERPA_ZIPFORMER_20M.to_string(),
                enable_endpoint_detection: true,
            },
            AsrProvider::Moonshine {
                model_dir: "moonshine-small-streaming-en".to_string(),
                enable_speaker_hints: true,
            },
            AsrProvider::OpenAiRealtimeTranscription {
                api_key: String::new(),
                model: crate::asr::openai_realtime::DEFAULT_MODEL.to_string(),
                language: None,
            },
        ];

        let ids: Vec<_> = providers
            .iter()
            .map(|provider| descriptor_for_asr_provider(provider).id)
            .collect();

        assert_eq!(
            ids,
            vec![
                "asr.local_whisper",
                "asr.api",
                "asr.aws_transcribe",
                "asr.deepgram",
                "asr.assemblyai",
                "asr.soniox",
                "asr.sherpa_onnx",
                "asr.moonshine",
                "asr.openai_realtime",
            ]
        );
    }

    #[test]
    fn llm_variants_have_descriptors() {
        let providers = [
            LlmProvider::LocalLlama,
            LlmProvider::Api {
                endpoint: "http://localhost:11434/v1".to_string(),
                api_key: String::new(),
                model: "llama3.2".to_string(),
            },
            LlmProvider::Api {
                endpoint: crate::settings::CEREBRAS_BASE_URL.to_string(),
                api_key: String::new(),
                model: audio_graph_provider_registry::CEREBRAS_DEFAULT_MODEL.to_string(),
            },
            LlmProvider::OpenRouter {
                model: "anthropic/claude-sonnet-4.5".to_string(),
                base_url: crate::llm::openrouter::DEFAULT_BASE_URL.to_string(),
                provider_order: None,
                include_usage_in_stream: true,
                api_key: String::new(),
            },
            LlmProvider::AwsBedrock {
                region: "us-east-1".to_string(),
                model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
                credential_source: AwsCredentialSource::DefaultChain,
            },
            LlmProvider::MistralRs {
                model_id: crate::models::LLM_MODEL_FILENAME.to_string(),
            },
        ];

        let ids: Vec<_> = providers
            .iter()
            .map(|provider| descriptor_for_llm_provider(provider).id)
            .collect();

        assert_eq!(
            ids,
            vec![
                "llm.local_llama",
                "llm.api",
                "llm.cerebras",
                "llm.openrouter",
                "llm.aws_bedrock",
                "llm.mistralrs",
            ]
        );
    }

    #[test]
    fn tts_variants_have_descriptors() {
        let providers = [
            TtsProvider::None,
            TtsProvider::DeepgramAura {
                voice: "aura-asteria-en".to_string(),
                sample_rate: 24_000,
                speed: 1.0,
            },
        ];

        let ids: Vec<_> = providers
            .iter()
            .map(|provider| descriptor_for_tts_provider(provider).id)
            .collect();

        assert_eq!(ids, vec!["tts.none", "tts.deepgram_aura"]);
    }

    #[test]
    fn command_returns_registry_copy() {
        let registry = get_provider_registry_cmd();
        assert_eq!(registry.as_slice(), provider_registry());
    }

    #[test]
    fn generated_provider_registry_ts_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../src/generated/providerRegistry.ts");
        let actual = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "failed to read generated provider registry {}: {e}",
                path.display()
            )
        });
        let expected = provider_registry_typescript_module();
        assert_eq!(
            actual,
            expected,
            "generated provider registry drifted; update {} from provider_registry_typescript_module()",
            path.display()
        );
    }

    #[test]
    fn gemini_credential_modes_are_represented() {
        let api_key_mode = GeminiAuthMode::ApiKey {
            api_key: String::new(),
        };
        let vertex_mode = GeminiAuthMode::VertexAI {
            project_id: "project".to_string(),
            location: "us-central1".to_string(),
            service_account_path: Some("/tmp/sa.json".to_string()),
        };
        let descriptor = descriptor_by_id("realtime_agent.gemini_live");

        assert!(matches!(api_key_mode, GeminiAuthMode::ApiKey { .. }));
        assert!(matches!(vertex_mode, GeminiAuthMode::VertexAI { .. }));
        assert!(descriptor.credential_keys.contains(&"gemini_api_key"));
        assert!(
            descriptor
                .credential_keys
                .contains(&"google_service_account_path")
        );
    }

    #[test]
    fn local_runtime_files_match_model_runtime_constants() {
        let whisper = descriptor_by_id("asr.local_whisper");
        assert_eq!(
            whisper.local_models[0].required_files,
            &[crate::models::WHISPER_MODEL_SMALL_EN]
        );
        assert_eq!(
            whisper.local_models[0].model_id,
            crate::models::WHISPER_MODEL_SMALL_EN
        );

        let sherpa = descriptor_by_id("asr.sherpa_onnx");
        assert_eq!(
            sherpa.local_models[0].required_files,
            crate::models::SHERPA_ZIPFORMER_REQUIRED_FILES
        );
        assert_eq!(
            sherpa.local_models[0].model_id,
            crate::models::SHERPA_ZIPFORMER_20M
        );

        let moonshine = descriptor_by_id("asr.moonshine");
        assert_eq!(
            moonshine.local_models[0].required_files,
            crate::models::MOONSHINE_STREAMING_REQUIRED_FILES
        );
        assert_eq!(
            moonshine.local_models[0].model_id,
            crate::models::MOONSHINE_SMALL_STREAMING_EN
        );
        assert!(
            moonshine
                .local_models
                .iter()
                .any(|model| model.model_id == crate::models::MOONSHINE_MEDIUM_STREAMING_EN)
        );

        let sortformer = descriptor_by_id("diarization.sortformer");
        assert_eq!(
            sortformer.local_models[0].required_files,
            &[crate::models::SORTFORMER_MODEL_FILENAME]
        );
        assert_eq!(
            sortformer.local_models[0].model_id,
            crate::models::SORTFORMER_MODEL_FILENAME
        );

        let clustering = descriptor_by_id("diarization.clustering");
        assert_eq!(
            clustering.local_models[0].model_id,
            crate::models::DIAR_SEG_PYANNOTE_DIR
        );
        assert_eq!(clustering.local_models[0].kind, LocalModelKind::Directory);
        assert_eq!(
            clustering.local_models[0].required_files,
            &["model.onnx", "model.int8.onnx"]
        );
        assert_eq!(
            clustering.local_models[1].model_id,
            crate::models::DIAR_EMB_TITANET_FILENAME
        );
        assert_eq!(clustering.local_models[1].kind, LocalModelKind::File);
        assert_eq!(
            clustering.local_models[1].required_files,
            &[crate::models::DIAR_EMB_TITANET_FILENAME]
        );

        let llama = descriptor_by_id("llm.local_llama");
        let mistralrs = descriptor_by_id("llm.mistralrs");
        assert_eq!(
            llama.local_models[0].required_files,
            &[crate::models::LLM_MODEL_FILENAME]
        );
        assert_eq!(
            llama.local_models[0].required_files,
            mistralrs.local_models[0].required_files
        );
    }

    #[test]
    fn realtime_defaults_match_runtime_constants() {
        assert_eq!(
            descriptor_by_id("asr.openai_realtime").default_model,
            Some(crate::asr::openai_realtime::DEFAULT_MODEL)
        );
    }
}
