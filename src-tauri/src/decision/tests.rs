use super::*;
use audio_graph_decision_contract::OrderedMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn server(status: u16, body: String) -> (String, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let response = format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(bytes).unwrap()
    });
    (format!("http://{address}/v1/systemone"), handle)
}

fn request() -> DecisionRequest {
    DecisionRequest {
        state: audio_graph_decision_contract::DecisionState::Text("A public fixture".to_owned()),
        questions: OrderedMap::from([(
            "team".to_owned(),
            Question::Choice {
                instructions: "Which team?".to_owned(),
                criteria: OrderedMap::from([
                    ("billing".to_owned(), None),
                    ("shipping".to_owned(), None),
                ]),
            },
        )]),
    }
}

#[tokio::test]
async fn shadow_privacy_policy_blocks_cloud_http_before_connect_or_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut engine =
        SystemOneHttpEngine::new(EndpointKind::TypeSafe, Some("fixture-private-token".into()))
            .unwrap()
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::block(
                "local_only",
            ));
    engine.url = Url::parse(&format!(
        "http://{}/v1/systemone",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    engine.client = Client::builder().no_proxy().build().unwrap();
    let error = engine.system_one(&request()).await.unwrap_err();
    assert!(error.to_string().contains("blocked"));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn omitted_served_model_remains_omitted_for_route_identity_source() {
    let (url, served) = server(200, r#"{"answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":0.9,"shipping":0.1},"confidence":0.8}},"usage":{"input_tokens":20,"output_tokens":0}}"#.into()).await;
    let engine = SystemOneHttpEngine::new(
        EndpointKind::Ollama {
            url,
            model: "requested-only".into(),
        },
        None,
    )
    .unwrap();
    let response = engine.system_one(&request()).await.unwrap();
    assert_eq!(response.model, "");
    served.await.unwrap();
}

#[tokio::test]
async fn noul_probability_keys_cannot_echo_content_into_decision_provenance() {
    let (url, served) = server(200, r#"{"answers":{"team":{"type":"noul","noul":0.9,"probabilities":{"private transcript content":0.9,"false":0.1},"confidence":0.9}},"usage":{"input_tokens":20,"output_tokens":0}}"#.into()).await;
    let engine = SystemOneHttpEngine::new(
        EndpointKind::Ollama {
            url,
            model: "fixture".into(),
        },
        None,
    )
    .unwrap();
    let mut request = request();
    request.questions.insert(
        "team".into(),
        Question::Noul {
            instructions: "Answerable?".into(),
            criteria: None,
        },
    );
    let error = engine.system_one(&request).await.unwrap_err();
    // Malformed binary key pairs are rejected at the parse boundary, never
    // echoed into typed decision provenance.
    assert!(error.to_string().contains("Invalid System One response"));
    assert!(!error.to_string().contains("private transcript content"));
    served.await.unwrap();
}

#[tokio::test]
async fn vllm_documented_yes_no_noul_keys_validate_and_preserve_wire_bytes() {
    // https://docs.vllm.ai/en/latest/models/structured_decisions_models/ (2026-10-18).
    let body = r#"{"id":"decision-...","object":"structured_decision","created":1790000000,"model":"Qwen/Qwen3-0.6B","answers":{"team":{"type":"noul","noul":0.02,"probabilities":{"yes":0.02,"no":0.98},"confidence":0.95}},"usage":{"input_tokens":142,"output_tokens":3}}"#;
    let (url, served) = server(200, body.to_owned()).await;
    let engine = SystemOneHttpEngine::new(
        EndpointKind::Vllm {
            url,
            model: "Qwen/Qwen3-0.6B".to_owned(),
        },
        None,
    )
    .unwrap();
    let mut request = request();
    request.questions.insert(
        "team".to_owned(),
        Question::Noul {
            instructions: "The message conveys urgency or time-sensitivity".to_owned(),
            criteria: None,
        },
    );
    // A single request: the fixture server answers one connection, and
    // validate_answers must already accept the normalized yes/no convention.
    let wire = engine.evaluate_wire(&request).await.unwrap();
    assert_eq!(wire.json(), body);
    assert_eq!(serde_json::to_string(&wire).unwrap(), body);
    let Answer::Noul {
        noul,
        probabilities,
        confidence,
    } = &wire.typed().answers["team"]
    else {
        panic!("team is a noul answer");
    };
    assert_eq!(*noul, 0.02);
    assert_eq!(*confidence, Some(0.95));
    let probabilities = probabilities.as_ref().unwrap();
    assert_eq!(
        probabilities.keys().map(String::as_str).collect::<Vec<_>>(),
        ["true", "false"]
    );
    assert_eq!(probabilities["true"], 0.02);
    assert_eq!(probabilities["false"], 0.98);
    served.await.unwrap();
}

#[tokio::test]
async fn mixed_noul_pairs_fail_parsing_and_out_of_range_values_fail_schema_validation() {
    let (url, served) = server(200, r#"{"answers":{"team":{"type":"noul","noul":0.9,"probabilities":{"yes":0.9,"false":0.1},"confidence":0.9}},"usage":{"input_tokens":20,"output_tokens":0}}"#.to_owned()).await;
    let engine = SystemOneHttpEngine::new(
        EndpointKind::Vllm {
            url,
            model: "fixture".to_owned(),
        },
        None,
    )
    .unwrap();
    let mut request = request();
    request.questions.insert(
        "team".to_owned(),
        Question::Noul {
            instructions: "Answerable?".to_owned(),
            criteria: None,
        },
    );
    let error = engine.system_one(&request).await.unwrap_err().to_string();
    assert!(error.contains("Invalid System One response"));
    served.await.unwrap();

    let (url, served) = server(200, r#"{"answers":{"team":{"type":"noul","noul":0.9,"probabilities":{"true":1.4,"false":-0.4},"confidence":0.9}},"usage":{"input_tokens":20,"output_tokens":0}}"#.to_owned()).await;
    let engine = SystemOneHttpEngine::new(
        EndpointKind::Vllm {
            url,
            model: "fixture".to_owned(),
        },
        None,
    )
    .unwrap();
    let error = engine.system_one(&request).await.unwrap_err().to_string();
    assert!(error.contains("schema"));
    served.await.unwrap();
}

#[tokio::test]
async fn transport_rejects_wrong_answer_ids_and_does_not_echo_error_bodies() {
    let (url, served) = server(200, r#"{"model":"nimble","answers":{"wrong":{"type":"choice","choice":"billing","probabilities":{"billing":0.9,"shipping":0.1},"confidence":0.8}},"usage":{"input_tokens":20,"output_tokens":1}}"#.to_owned()).await;
    let engine = SystemOneHttpEngine::new(
        EndpointKind::Ollama {
            url,
            model: "nimble".to_owned(),
        },
        None,
    )
    .unwrap();
    assert!(
        engine
            .system_one(&request())
            .await
            .unwrap_err()
            .to_string()
            .contains("schema")
    );
    served.await.unwrap();
    let secret = "fixture-private-token";
    let (url, served) = server(500, format!("bad token {secret}; private transcript")).await;
    let engine = SystemOneHttpEngine::new(
        EndpointKind::Vllm {
            url,
            model: "test".to_owned(),
        },
        Some(secret.to_owned()),
    )
    .unwrap();
    let error = engine.system_one(&request()).await.unwrap_err().to_string();
    assert!(error.contains("500"));
    assert!(!error.contains(secret));
    assert!(!error.contains("private transcript"));
    served.await.unwrap();
}

#[tokio::test]
async fn local_cardinality_is_checked_before_network_dispatch() {
    let engine = SystemOneHttpEngine::new(
        EndpointKind::Ollama {
            url: "http://127.0.0.1:9/v1/systemone".to_owned(),
            model: "nimble".to_owned(),
        },
        None,
    )
    .unwrap();
    let mut request = request();
    if let Question::Choice { criteria, .. } = request.questions.get_mut("team").unwrap() {
        criteria.truncate(1);
    }
    assert!(matches!(
        engine.system_one(&request).await,
        Err(DecisionError::CardinalityLimit { .. })
    ));
    if let Question::Choice { criteria, .. } = request.questions.get_mut("team").unwrap() {
        *criteria = (0..27).map(|i| (i.to_string(), None)).collect();
    }
    assert!(matches!(
        engine.system_one(&request).await,
        Err(DecisionError::CardinalityLimit { limit: 26, .. })
    ));
    request.questions.insert(
        "grade".to_owned(),
        Question::Score {
            instructions: "rate".to_owned(),
            criteria: vec!["only".to_owned()],
        },
    );
    request.questions.shift_remove("team");
    assert!(matches!(
        engine.system_one(&request).await,
        Err(DecisionError::CardinalityLimit { limit: 10, .. })
    ));
}

#[test]
fn local_egress_cannot_be_remote_or_an_ollama_cloud_relay() {
    for (url, model) in [
        ("http://example.com/v1/systemone", "nimble"),
        ("http://127.0.0.1:11434/v1/systemone", "clef:cloud"),
        ("http://user:pass@127.0.0.1:11434/v1/systemone", "nimble"),
    ] {
        assert!(
            SystemOneHttpEngine::new(
                EndpointKind::Ollama {
                    url: url.to_owned(),
                    model: model.to_owned()
                },
                None
            )
            .is_err()
        );
    }
    assert!(matches!(
        SystemOneHttpEngine::new(EndpointKind::TypeSafe, None),
        Err(DecisionError::Credential)
    ));
}

#[tokio::test]
async fn replay_reports_conditional_grade_and_heuristic_disagreement() {
    let body = r#"{"model":"fixture-model","answers":{"agent_proposal_kind":{"type":"choice","choice":"question","probabilities":{"question":0.75,"graph_suggestion":0.1,"note":0.1,"none":0.05},"confidence":0.6},"agent_signal_grade":{"type":"score","score":1.8,"legend":{"0":"fragment","1":"weak","2":"strong"},"probabilities":{"0":0.05,"1":0.1,"2":0.85},"confidence":0.8}},"usage":{"input_tokens":100,"output_tokens":2}}"#;
    let (url, served) = server(200, body.to_owned()).await;
    let engine = SystemOneHttpEngine::new(
        EndpointKind::Vllm {
            url,
            model: "fixture-model".to_owned(),
        },
        None,
    )
    .unwrap();
    let corpus = eval::ReplayCorpus {
        spans: vec![eval::ReplaySpan {
            id: "s1".to_owned(),
            text: "Consider answering or linking this question: x?".to_owned(),
            confidence: 0.9,
        }],
        persisted_cards: 0,
    };
    let report = eval::run(&engine, &corpus, 0.042).await.unwrap();
    assert_eq!(report.kind_confusion[0][0], 1);
    assert_eq!(report.grade_confusion[2][2], 1);
    assert_eq!(report.grade_compared, 1);
    assert!((report.kind_calibration.ece.unwrap() - 0.25).abs() < 1e-12);
    assert!((report.grade_calibration.ece.unwrap() - 0.15).abs() < 1e-12);
    assert!((report.estimated_cost_usd - 0.0000042).abs() < 1e-12);
    let transmitted = served.await.unwrap();
    assert!(transmitted.starts_with("POST /v1/systemone HTTP/1.1"));

    let (url, served) = server(200, body.to_owned()).await;
    let engine = SystemOneHttpEngine::new(
        EndpointKind::Vllm {
            url,
            model: "fixture-model".to_owned(),
        },
        None,
    )
    .unwrap();
    let corpus = eval::ReplayCorpus {
        spans: vec![eval::ReplaySpan {
            id: "s2".to_owned(),
            text: "Nothing to classify".to_owned(),
            confidence: 0.9,
        }],
        persisted_cards: 0,
    };
    let report = eval::run(&engine, &corpus, 0.0).await.unwrap();
    assert_eq!(report.kind_confusion[3][0], 1);
    assert_eq!(report.kind_agreement, 0.0);
    assert_eq!(report.grade_compared, 0);
    assert_eq!(report.grade_agreement, None);
    assert_eq!(report.grade_calibration.ece, None);
    served.await.unwrap();
}

#[tokio::test]
async fn cloudflare_envelope_preserves_vendor_extensions_and_uses_unified_jev_input() {
    let answer = r#"{"model":"jev-1.13.0","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":0.9,"shipping":0.1},"confidence":0.8}},"usage":{"input_tokens":20,"output_tokens":1},"diagnostics":{"team":{"label_mass":0.95}}}"#;
    let (url, served) = server(
        200,
        format!(r#"{{"result":{answer},"success":true,"errors":[],"messages":[]}}"#),
    )
    .await;
    let mut engine = SystemOneHttpEngine::new(
        EndpointKind::Cloudflare {
            account_id: "fixture".to_owned(),
            model: CloudflareModel::Jev,
        },
        Some("fixture-bearer-token".to_owned()),
    )
    .unwrap();
    engine.url = Url::parse(&url).unwrap();
    engine.client = Client::builder().no_proxy().build().unwrap();
    let wire = engine.evaluate_wire(&request()).await.unwrap();
    assert_eq!(wire.json(), answer);
    assert!(
        matches!(&wire.typed().answers["team"], Answer::Choice { choice, .. } if choice == "billing")
    );
    let transmitted = served.await.unwrap();
    let (headers, body) = transmitted.split_once("\r\n\r\n").unwrap();
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-bearer-token")
    );
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["model"], "typesafe/jev");
    assert_eq!(body["input"]["state"], "A public fixture");
    assert!(body.get("state").is_none());
}

#[tokio::test]
async fn failed_cloudflare_envelope_and_http_errors_do_not_become_answers() {
    let (url, served) = server(200, r#"{"success":false,"result":null,"errors":[{"message":"fixture-private-token and private content"}]}"#.to_owned()).await;
    let mut engine = SystemOneHttpEngine::new(
        EndpointKind::Cloudflare {
            account_id: "fixture".to_owned(),
            model: CloudflareModel::ClefFlash,
        },
        Some("fixture-private-token".to_owned()),
    )
    .unwrap();
    engine.url = Url::parse(&url).unwrap();
    engine.client = Client::builder().no_proxy().build().unwrap();
    let error = engine.system_one(&request()).await.unwrap_err();
    assert!(matches!(error, DecisionError::Transport(_)));
    assert!(!error.to_string().contains("fixture-private-token"));
    assert!(!error.to_string().contains("private content"));
    served.await.unwrap();

    for (status, body) in [
        (401, r#"{"error":"fixture-private-token"}"#),
        (
            400,
            r#"{"error":{"code":"max_tokens_exceeded","message":"private content"}}"#,
        ),
    ] {
        let (url, served) = server(status, body.to_owned()).await;
        let engine = SystemOneHttpEngine::new(
            EndpointKind::Vllm {
                url,
                model: "fixture".to_owned(),
            },
            None,
        )
        .unwrap();
        let error = engine.system_one(&request()).await.unwrap_err();
        match status {
            401 => assert!(matches!(error, DecisionError::Credential)),
            400 => assert!(matches!(error, DecisionError::StateTooLong)),
            _ => unreachable!(),
        }
        served.await.unwrap();
    }
}

#[tokio::test]
async fn endpoint_maxima_are_rejected_before_transport() {
    let mut engine = SystemOneHttpEngine::new(
        EndpointKind::TypeSafe,
        Some("fixture-bearer-token".to_owned()),
    )
    .unwrap();
    engine.url = Url::parse("http://127.0.0.1:9/v1/systemone").unwrap();
    let mut req = request();
    if let Question::Choice { criteria, .. } = req.questions.get_mut("team").unwrap() {
        *criteria = (0..256).map(|i| (i.to_string(), None)).collect();
    }
    assert!(matches!(
        engine.system_one(&req).await,
        Err(DecisionError::CardinalityLimit { limit: 255, .. })
    ));

    let engine = SystemOneHttpEngine::new(
        EndpointKind::Vllm {
            url: "http://127.0.0.1:9/v1/systemone".to_owned(),
            model: "fixture".to_owned(),
        },
        None,
    )
    .unwrap();
    req.questions = (0..65)
        .map(|i| {
            (
                i.to_string(),
                Question::Noul {
                    instructions: "predicate".to_owned(),
                    criteria: None,
                },
            )
        })
        .collect();
    assert!(matches!(
        engine.system_one(&req).await,
        Err(DecisionError::CardinalityLimit { limit: 64, .. })
    ));
    req.questions = OrderedMap::from([(
        "grade".to_owned(),
        Question::Score {
            instructions: "rate".to_owned(),
            criteria: (0..11).map(|i| i.to_string()).collect(),
        },
    )]);
    assert!(matches!(
        engine.system_one(&req).await,
        Err(DecisionError::CardinalityLimit { limit: 10, .. })
    ));
}
