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
