use audio_graph_decision_contract::{
    Answer, DecisionEngine, DecisionResponse, DecisionState, OrderedMap, Usage,
    WireConversionError, WireRequest, WireResponse,
};

const _: Option<std::sync::Arc<dyn DecisionEngine + Send + Sync>> = None;

// https://docs.typesafe.ai/introduction/quickstart.md (2026-10-18), Bash request.
const TYPESAFE: &str = r#"{
  "state": "Our API integration started returning 500 errors on every request about 20 minutes ago, and we can't process any customer orders until this is fixed.",
  "model": "jev-latest",
  "questions": {
    "department": {"type":"choice","instructions":"Which team should handle this","criteria":{"billing":"Payment or subscription issues","technical":"Bugs or integration problems","sales":"Pricing or account questions"}},
    "frustration": {"type":"score","instructions":"How frustrated the customer appears","criteria":["Calm, just stating facts","Frustrated but civil","Very angry, strong language"]},
    "is_urgent": {"type":"noul","instructions":"The message conveys urgency or time-sensitivity"}
  }
}"#;

// https://docs.vllm.ai/en/latest/models/structured_decisions_models/ (2026-10-18).
const VLLM: &str = r#"{
  "id": "decision-...",
  "object": "structured_decision",
  "created": 1790000000,
  "model": "Qwen/Qwen3-0.6B",
  "answers": {
    "team": {"type":"choice","choice":"billing","probabilities":{"billing":0.97,"shipping":0.01,"security":0.02},"confidence":0.90},
    "urgent": {"type":"noul","noul":0.02,"probabilities":{"yes":0.02,"no":0.98},"confidence":0.95},
    "tone": {"type":"score","score":0.6,"legend":{"0":"calm","1":"annoyed","2":"furious"},"probabilities":{"0":0.7,"1":0.2,"2":0.1},"confidence":0.65}
  },
  "usage": {"input_tokens":142,"output_tokens":3},
  "diagnostics": {
    "team":{"label_mass":0.93,"argmax_is_label":true},
    "urgent":{"label_mass":0.98,"argmax_is_label":true},
    "tone":{"label_mass":0.91,"argmax_is_label":true}
  }
}"#;

// https://docs.ollama.com/capabilities/decision (2026-10-18), image response.
const OLLAMA: &str = r#"{
  "model": "clef-flash",
  "answers": {
    "has_ollama": {"type":"noul","noul":0.959},
    "app": {"type":"choice","choice":"vscode","probabilities":{"vscode":0.966,"other":0.017,"browser":0.010,"terminal":0.007},"confidence":0.868}
  },
  "usage": {"input_tokens":677,"output_tokens":0}
}"#;

#[test]
fn vendor_bytes_and_extensions_survive_replay() {
    let request = WireRequest::from_json(TYPESAFE).unwrap();
    assert_eq!(serde_json::to_string(&request).unwrap(), TYPESAFE);
    assert_eq!(
        request
            .typed()
            .questions
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["department", "frustration", "is_urgent"]
    );
    assert!(matches!(request.typed().state, DecisionState::Text(_)));
    for json in [VLLM, OLLAMA] {
        let response: WireResponse = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&response).unwrap(), json);
    }
    let response = WireResponse::from_json(VLLM).unwrap();
    assert!(
        matches!(&response.typed().answers["tone"], Answer::Score { probabilities, .. } if probabilities == &[0.7, 0.2, 0.1])
    );
    assert!(matches!(
        &WireResponse::from_json(OLLAMA).unwrap().typed().answers["has_ollama"],
        Answer::Noul {
            probabilities: None,
            confidence: None,
            ..
        }
    ));
}

#[test]
fn vendor_json_survives_conversion_through_owned_typed_values() {
    let (request, format) = WireRequest::from_json(TYPESAFE).unwrap().into_parts();
    assert!(matches!(request.state, DecisionState::Text(_)));
    let request = WireRequest::from_parts(request, format).unwrap();
    assert_eq!(serde_json::to_string(&request).unwrap(), TYPESAFE);

    for json in [VLLM, OLLAMA] {
        let wire = WireResponse::from_json(json).unwrap();
        let (response, format) = wire.into_parts();
        assert_eq!(response.engine_id, "");
        let restored = WireResponse::from_parts(response, format).unwrap();
        assert_eq!(serde_json::to_string(&restored).unwrap(), json);
    }
}

#[test]
fn native_score_values_encode_probabilities_by_level_index() {
    let response = DecisionResponse {
        answers: OrderedMap::from([(
            "grade".to_owned(),
            Answer::Score {
                score: 0.75,
                probabilities: vec![0.25, 0.75],
                confidence: 0.75,
            },
        )]),
        usage: Usage {
            input_tokens: 12,
            output_tokens: 0,
        },
        engine_id: "native".to_owned(),
        model: "fixture".to_owned(),
    };
    let wire = WireResponse::from_typed(response).unwrap();
    let json: serde_json::Value = serde_json::from_str(wire.json()).unwrap();
    assert_eq!(
        json["answers"]["grade"]["probabilities"],
        serde_json::json!({"0": 0.25, "1": 0.75})
    );
    assert_eq!(
        WireResponse::from_json(wire.json()).unwrap().typed(),
        wire.typed()
    );
}

#[test]
fn reconstruction_rejects_changes_instead_of_emitting_stale_vendor_bytes() {
    let (mut response, format) = WireResponse::from_json(VLLM).unwrap().into_parts();
    response.usage.input_tokens += 1;
    assert!(matches!(
        WireResponse::from_parts(response, format),
        Err(WireConversionError::TypedValueChanged)
    ));
    let (mut request, format) = WireRequest::from_json(TYPESAFE).unwrap().into_parts();
    request.questions.swap_indices(0, 1);
    assert!(matches!(
        WireRequest::from_parts(request, format),
        Err(WireConversionError::TypedValueChanged)
    ));
}

#[test]
fn score_indices_follow_level_index_not_vendor_object_order() {
    let answer: Answer = serde_json::from_str(r#"{"type":"score","score":0.6,"probabilities":{"2":0.1,"0":0.7,"1":0.2},"confidence":0.65}"#).unwrap();
    assert!(
        matches!(answer, Answer::Score { probabilities, .. } if probabilities == [0.7, 0.2, 0.1])
    );
    for probabilities in [r#"{"1":1.0}"#, r#"{"00":1.0}"#, r#"{"-1":1.0}"#] {
        let json = format!(
            r#"{{"type":"score","score":0,"probabilities":{probabilities},"confidence":1}}"#
        );
        assert!(serde_json::from_str::<Answer>(&json).is_err());
    }
}
