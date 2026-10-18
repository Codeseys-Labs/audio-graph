#![cfg(feature = "decision-laya")]

use audio_graph_decision_engine::{
    Answer, DecisionRequest, EngineConfig, EngineRegistry, ExecutionProvider,
};

#[tokio::test]
#[ignore = "requires AUDIOGRAPH_MODELS_DIR and an ONNX Runtime shared library"]
async fn python_reference_probabilities_match_native_inference() {
    let root = std::env::var_os("AUDIOGRAPH_MODELS_DIR")
        .expect("set AUDIOGRAPH_MODELS_DIR to the existing app-data models directory");
    let mut config = EngineConfig::new(root.into());
    config.execution_provider = match std::env::var("AUDIOGRAPH_LAYA_EP").as_deref() {
        Ok("directml") => ExecutionProvider::DirectMl,
        Ok("cuda") => ExecutionProvider::Cuda,
        Ok("coreml") => ExecutionProvider::CoreMl,
        _ => ExecutionProvider::Cpu,
    };
    let engine = EngineRegistry::default().create("laya", &config).unwrap();
    for source in [
        include_str!("../fixtures/python-reference.json"),
        include_str!("../fixtures/python-reference-multilingual.json"),
    ] {
        let fixture: serde_json::Value = serde_json::from_str(source).unwrap();
        let mut max_error = 0_f64;
        for case in fixture["cases"].as_array().unwrap() {
            let request: DecisionRequest = serde_json::from_value(case["request"].clone()).unwrap();
            let response = engine.system_one(&request).await.unwrap();
            assert_eq!(response.engine_id, "laya");
            assert_eq!(
                response.usage.input_tokens,
                case["reference"]["usage"]["input_tokens"].as_u64().unwrap()
            );
            for (id, answer) in response.answers {
                let expected = &case["reference"]["answers"][&id];
                match answer {
                    Answer::Choice {
                        choice,
                        probabilities,
                        confidence,
                    } => {
                        assert_eq!(choice, expected["choice"].as_str().unwrap());
                        assert_eq!(
                            confidence,
                            probabilities.values().copied().fold(0_f64, f64::max)
                        );
                        for (label, p) in probabilities {
                            max_error = max_error.max(
                                (p - expected["probabilities"][label].as_f64().unwrap()).abs(),
                            );
                        }
                    }
                    Answer::Score {
                        score,
                        probabilities,
                        confidence,
                    } => {
                        max_error =
                            max_error.max((score - expected["score"].as_f64().unwrap()).abs());
                        assert_eq!(
                            confidence,
                            probabilities.iter().copied().fold(0_f64, f64::max)
                        );
                        for (i, p) in probabilities.into_iter().enumerate() {
                            max_error = max_error.max(
                                (p - expected["probabilities"][i.to_string()].as_f64().unwrap())
                                    .abs(),
                            );
                        }
                    }
                    Answer::Noul {
                        noul,
                        probabilities,
                        confidence,
                    } => {
                        max_error =
                            max_error.max((noul - expected["noul"].as_f64().unwrap()).abs());
                        let probabilities = probabilities.unwrap();
                        assert_eq!(noul, probabilities["true"]);
                        assert_eq!(
                            confidence.unwrap(),
                            probabilities.values().copied().fold(0_f64, f64::max)
                        );
                    }
                }
            }
        }
        println!("maximum Python/native absolute error: {max_error:.8}");
        assert!(max_error <= fixture["tolerance"].as_f64().unwrap());
    }
}
