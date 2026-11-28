use super::*;
use crate::persistence::{DataMovementEventType, DestinationBoundary, FileMemoryRepository};
use audio_graph_decision_contract::{
    DecisionError, DecisionState, OrderedMap, Question, async_trait,
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn request() -> DecisionRequest {
    DecisionRequest {
        state: DecisionState::Json(
            serde_json::json!({"text":"private transcript must never appear in provenance", "asr_confidence":0.9}),
        ),
        questions: OrderedMap::from([(
            "agent_proposal_kind".into(),
            Question::Choice {
                instructions: "Classify utterance".into(),
                criteria: ["question", "graph_suggestion", "note", "none"]
                    .into_iter()
                    .map(|key| (key.into(), None))
                    .collect(),
            },
        )]),
    }
}
struct Fake {
    probability: f64,
    fail: bool,
    calls: AtomicUsize,
    limit: usize,
}
#[async_trait]
impl DecisionEngine for Fake {
    fn id(&self) -> &str {
        "d3-edge"
    }
    fn egress(&self) -> Egress {
        Egress::Local
    }
    fn limits(&self) -> EngineLimits {
        EngineLimits {
            max_choice_options: self.limit,
            ..NATIVE
        }
    }
    async fn system_one(&self, _: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(DecisionError::Transport("private provider error".into()));
        }
        Ok(DecisionResponse {
            answers: OrderedMap::from([(
                "agent_proposal_kind".into(),
                Answer::Choice {
                    choice: "note".into(),
                    probabilities: [
                        ("question".into(), 1.0 - self.probability),
                        ("graph_suggestion".into(), 0.0),
                        ("note".into(), self.probability),
                        ("none".into(), 0.0),
                    ]
                    .into(),
                    confidence: self.probability,
                },
            )]),
            usage: Usage {
                input_tokens: 12,
                output_tokens: 0,
            },
            engine_id: "untrusted-id".into(),
            model: "vllm-sr/d3-edge".into(),
        })
    }
}
fn fitted_route(_: &DecisionRouteSettings) -> &'static DecisionRoute {
    use std::sync::LazyLock;
    static PIN: LazyLock<String> = LazyLock::new(|| {
        digest_pin(
            &route_by_id("route.decision.d3_edge")
                .unwrap()
                .pin(&DecisionRouteSettings::default()),
        )
    });
    static FIT: LazyLock<[CalibrationRecord; 1]> = LazyLock::new(|| {
        [CalibrationRecord {
            id: "fit-human-200",
            route_id: "route.decision.d3_edge",
            served_model: "vllm-sr/d3-edge",
            model_pin_digest: &PIN,
            precision: "fp32",
            question_id: "agent_proposal_kind",
            schema_version: 1,
            temperature: 1.0,
            accept: 0.9,
            abstain: 0.7,
            dataset_id: "held-out-human-labels",
            sample_count: 200,
            per_option_counts: &[50, 50, 50, 50],
            ece_before: 0.2,
            ece_after: 0.1,
            target_metric: 0.95,
        }]
    });
    static FITTED: LazyLock<DecisionRoute> = LazyLock::new(|| {
        let base = route_by_id("route.decision.d3_edge").unwrap();
        DecisionRoute {
            id: base.id,
            provider_id: base.provider_id,
            engine: base.engine,
            engine_id: base.engine_id,
            model: base.model,
            limits: base.limits,
            calibrations: &*FIT,
        }
    });
    &FITTED
}
#[test]
fn table_egress_and_registry_boundaries_are_pinned() {
    for (kind, egress) in [
        (EngineKind::HostedJev, Egress::Cloud),
        (EngineKind::HostedCloudflare, Egress::Cloud),
        (EngineKind::LocalHttp, Egress::Local),
        (EngineKind::NativeLaya, Egress::Local),
        (EngineKind::NativeD3, Egress::Local),
        (EngineKind::NativeClef, Egress::Local),
    ] {
        assert_eq!(kind.egress(), egress);
    }
    let mut ids = std::collections::HashSet::new();
    for route in DECISION_ROUTES {
        assert!(ids.insert(route.id));
        let descriptor = crate::provider_registry::descriptor_by_id(route.provider_id);
        assert_eq!(
            descriptor.stage,
            crate::provider_registry::ProviderStage::Decision
        );
        assert!(!descriptor.ui_selectable);
        let boundary = descriptor.privacy.data_boundary;
        assert_eq!(
            boundary == crate::provider_registry::ProviderDataBoundary::LocalOnly,
            route.engine.native()
        );
        if route.engine.native() {
            let pin = route.pin(&DecisionRouteSettings::default());
            assert_eq!(pin.revision.as_ref().unwrap().len(), 40);
            assert_eq!(pin.precision, "fp32");
            assert!(
                pin.components
                    .iter()
                    .all(|component| component.sha256.is_none())
            );
        }
    }
}
#[test]
fn local_http_routes_cannot_construct_non_loopback_or_cloud_relays() {
    for id in [
        "route.decision.ollama_loopback",
        "route.decision.vllm_loopback",
    ] {
        let route = route_by_id(id).unwrap();
        for url in [
            "http://example.com/v1/systemone",
            "http://localhost/v1/systemone",
            "https://8.8.8.8/v1/systemone",
            "file:///v1/systemone",
            "http://127.0.0.1/v1/systemone?secret=1",
        ] {
            let settings = DecisionRouteSettings {
                local_http_url: Some(url.into()),
                local_http_model: Some("jev".into()),
                ..Default::default()
            };
            assert!(route.endpoint(&settings).is_err(), "{id}: {url}");
        }
        for url in [
            "http://127.0.0.1:11434/v1/systemone",
            "http://[::1]:8000/v1/systemone",
        ] {
            let mut settings = DecisionRouteSettings {
                local_http_url: Some(url.into()),
                local_http_model: Some("jev".into()),
                ..Default::default()
            };
            assert!(route.endpoint(&settings).is_ok());
            settings.local_http_model = Some("jev:cloud".into());
            assert!(route.endpoint(&settings).is_err());
        }
    }
}
#[test]
fn canonical_state_digest_is_order_independent_and_sensitive_to_revisions() {
    let a = DecisionState::Json(serde_json::from_str(r#"{"b":{"z":2,"a":1},"a":3}"#).unwrap());
    let b = DecisionState::Json(serde_json::from_str(r#"{"a":3,"b":{"a":1,"z":2}}"#).unwrap());
    assert_eq!(state_digest(&a), state_digest(&b));
    assert_ne!(
        state_digest(&a),
        state_digest(&DecisionState::Text("changed".into()))
    );
    assert_eq!(state_digest(&a).len(), 71);
}
#[tokio::test]
async fn dispatch_outcomes_stamp_raw_calibrated_identity_usage_and_redacted_ledger() {
    for (probability, failure, expected) in [
        (0.95, false, DecisionOutcome::Decided),
        (0.8, false, DecisionOutcome::Proposed),
        (0.6, false, DecisionOutcome::Abstained),
        (0.95, true, DecisionOutcome::Unavailable),
    ] {
        let dir = std::env::temp_dir().join(format!("decision-route-{}", uuid::Uuid::new_v4()));
        let repo = FileMemoryRepository::with_data_root(&dir);
        let settings = DecisionRouteSettings::default();
        let basis = [DecisionBasis {
            span_id: "span-1".into(),
            revision: 3,
        }];
        let context = DispatchContext {
            session_id: "session-fixture",
            settings: &settings,
            privacy: PrivacyMode::LocalOnly,
            models_dir: &dir,
            schema_version: crate::decision::shadow::LIVE_ASSIST_SCHEMA_VERSION,
            basis: &basis,
            repository: &repo,
            engines: &EngineCache::default(),
        };
        let fake = Fake {
            probability,
            fail: failure,
            calls: AtomicUsize::new(0),
            limit: 255,
        };
        let records = dispatch_fixture(
            fitted_route(&settings),
            &fake,
            &request(),
            &context,
            &[("agent_proposal_kind", "question")],
        )
        .await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        let record = &records[0];
        assert_eq!(record.outcome, expected);
        assert_eq!(record.authority, DecisionAuthority::Shadow);
        assert_eq!(record.source, DecisionSource::Heuristic);
        assert_eq!(record.heuristic_answer, "question");
        assert_eq!(record.basis, basis);
        assert_eq!(record.schema_version, 1);
        assert_eq!(record.route_id.as_deref(), Some("route.decision.d3_edge"));
        assert_eq!(record.provider_id.as_deref(), Some("decision.d3_edge"));
        assert_eq!(record.state_sha256, state_digest(&request().state));
        if !failure {
            assert_eq!(record.served_model.as_deref(), Some("vllm-sr/d3-edge"));
            assert_eq!(record.model_identity_source, ModelIdentitySource::Served);
            assert_eq!(record.route_answer.as_deref(), Some("note"));
            assert_eq!(
                record.calibration.record_id.as_deref(),
                Some("fit-human-200")
            );
            assert_eq!(record.usage.input_tokens, 12);
            assert!(
                matches!(&record.raw,Some(Answer::Choice { confidence, .. }) if *confidence == probability)
            );
        }
        let ledger = repo.load_data_movement_events(context.session_id).unwrap();
        assert_eq!(
            ledger[0].event_type,
            DataMovementEventType::ProviderCallStarted
        );
        assert_eq!(
            ledger[1].event_type,
            if failure {
                DataMovementEventType::ProviderCallFailed
            } else {
                DataMovementEventType::ProviderCallSucceeded
            }
        );
        assert!(
            ledger
                .iter()
                .all(|event| event.destination.boundary == DestinationBoundary::Local)
        );
        assert_eq!(
            ledger[0].source.as_ref().unwrap().source_id.as_deref(),
            Some(record.call_id.as_str())
        );
        let serialized = serde_json::to_string(&(&records, &ledger)).unwrap();
        assert!(!serialized.contains("private transcript"));
        assert!(!serialized.contains("private provider error"));
        assert!(!serialized.contains("untrusted-id"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[tokio::test]
async fn preflight_and_product_gate_refuse_before_engine_call() {
    let dir = std::env::temp_dir().join(format!("decision-gates-{}", uuid::Uuid::new_v4()));
    let repo = FileMemoryRepository::with_data_root(&dir);
    let settings = DecisionRouteSettings::default();
    let context = DispatchContext {
        session_id: "session-gates",
        settings: &settings,
        privacy: PrivacyMode::LocalOnly,
        models_dir: &dir,
        schema_version: crate::decision::shadow::LIVE_ASSIST_SCHEMA_VERSION,
        basis: &[],
        repository: &repo,
        engines: &EngineCache::default(),
    };
    let fake = Fake {
        probability: 1.0,
        fail: false,
        calls: AtomicUsize::new(0),
        limit: 2,
    };
    let records = dispatch_fixture(
        route_by_id("route.decision.d3_edge").unwrap(),
        &fake,
        &request(),
        &context,
        &[],
    )
    .await;
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert_eq!(records[0].error_class.as_deref(), Some("limit_exceeded"));
    for privacy in [PrivacyMode::LocalOnly, PrivacyMode::ByokCloud] {
        assert!(authorize_decision_route("route.decision.jev_typesafe", privacy).is_err());
    }
    let records = dispatch(
        Some("route.decision.jev_typesafe"),
        &request(),
        &context,
        &[],
    )
    .await;
    assert_eq!(records[0].outcome, DecisionOutcome::Unavailable);
    let ledger = repo.load_data_movement_events(context.session_id).unwrap();
    assert!(ledger.iter().all(|event| event.event_type
        == DataMovementEventType::ProviderCallFailed
        && event.destination.boundary == DestinationBoundary::Local
        && event.data_classes.is_empty()));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn temperature_preserves_raw_and_http_confidence_and_score_ties_choose_lower() {
    let raw = Answer::Noul {
        noul: 0.8,
        probabilities: None,
        confidence: Some(0.8),
    };
    let calibrated = calibrate(&raw, 2.0, true);
    assert!(
        matches!(calibrated,Answer::Noul { noul, confidence:Some(confidence), .. } if (noul-2.0/3.0).abs()<1e-12 && (confidence-noul).abs()<1e-12)
    );
    assert_eq!(calibrate(&raw, 2.0, false), raw);
    assert_eq!(
        confidence(&calibrate(
            &Answer::Noul {
                noul: 0.8,
                probabilities: None,
                confidence: None
            },
            1.0,
            true
        )),
        None
    );
    let score = Answer::Score {
        score: 1.5,
        probabilities: vec![0.1, 0.45, 0.45],
        confidence: 0.45,
    };
    let question = Question::Score {
        instructions: "grade".into(),
        criteria: vec!["fragment".into(), "weak".into(), "strong".into()],
    };
    assert_eq!(answer_key(&score, &question), "weak");
}

#[tokio::test]
async fn normal_shadow_dispatch_admits_native_without_ui_promotion_and_never_grants_authority() {
    let dir = std::env::temp_dir().join(format!("decision-normal-{}", uuid::Uuid::new_v4()));
    let repo = FileMemoryRepository::with_data_root(&dir);
    let settings = DecisionRouteSettings::default();
    let engines = EngineCache::default();
    engines.set_fixture(Arc::new(Fake {
        probability: 0.95,
        fail: false,
        calls: AtomicUsize::new(0),
        limit: 255,
    }));
    let context = DispatchContext {
        session_id: "normal-shadow",
        settings: &settings,
        privacy: PrivacyMode::LocalOnly,
        models_dir: &dir,
        schema_version: crate::decision::shadow::LIVE_ASSIST_SCHEMA_VERSION,
        basis: &[],
        repository: &repo,
        engines: &engines,
    };
    let records = dispatch(
        Some("route.decision.d3_edge"),
        &request(),
        &context,
        &[("agent_proposal_kind", "question")],
    )
    .await;
    assert_eq!(records[0].error_class, None);
    assert_eq!(records[0].outcome, DecisionOutcome::Abstained);
    assert_eq!(records[0].calibration, CalibrationSnapshot::identity());
    assert_eq!(records[0].authority, DecisionAuthority::Shadow);
    assert_eq!(records[0].raw.as_ref(), records[0].calibrated.as_ref());
    assert_eq!(records[0].route_answer.as_deref(), Some("note"));
    let ledger = repo.load_data_movement_events(context.session_id).unwrap();
    assert_eq!(
        ledger[1].event_type,
        DataMovementEventType::ProviderCallSucceeded
    );
    assert!(!crate::provider_registry::descriptor_by_id("decision.d3_edge").ui_selectable);
    let records = dispatch(
        Some("route.decision.jev_typesafe"),
        &request(),
        &context,
        &[],
    )
    .await;
    assert_eq!(records[0].error_class.as_deref(), Some("provider_deferred"));
    let records = dispatch(Some("route.decision.unknown"), &request(), &context, &[]).await;
    assert_eq!(records[0].error_class.as_deref(), Some("unknown_route"));
    assert_eq!(records[0].route_id, None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn native_selection_uses_complete_cache_then_one_route_without_cloud_default() {
    let dir = std::env::temp_dir().join(format!("decision-cache-{}", uuid::Uuid::new_v4()));
    let settings = DecisionRouteSettings::default();
    assert_eq!(select_shadow_route(&settings, &dir), None);
    for id in ["laya-english", "d3-edge"] {
        let (_, files) = crate::models::decision_model_metadata(id).unwrap();
        for file in files {
            let path = dir.join(id).join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, [1u8]).unwrap();
        }
        assert_eq!(
            select_shadow_route(&settings, &dir),
            Some(if id == "d3-edge" {
                "route.decision.d3_edge"
            } else {
                "route.decision.laya"
            })
        );
    }
    let explicit = DecisionRouteSettings {
        live_assist_route_id: Some("route.decision.jev_typesafe".into()),
        ..Default::default()
    };
    assert_eq!(
        select_shadow_route(&explicit, &dir),
        Some("route.decision.jev_typesafe")
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn non_live_assist_question_family_preserves_parameterized_schema_version() {
    struct SpeakerIdentityFixture;
    #[async_trait]
    impl DecisionEngine for SpeakerIdentityFixture {
        fn id(&self) -> &str {
            "d3-edge"
        }
        fn egress(&self) -> Egress {
            Egress::Local
        }
        fn limits(&self) -> EngineLimits {
            NATIVE
        }
        async fn system_one(&self, _: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
            Ok(DecisionResponse {
                answers: OrderedMap::from([(
                    "speaker_identity_same_person".into(),
                    Answer::Choice {
                        choice: "same".into(),
                        probabilities: [
                            ("same".into(), 0.94),
                            ("different".into(), 0.04),
                            ("unsure".into(), 0.02),
                        ]
                        .into(),
                        confidence: 0.94,
                    },
                )]),
                usage: Usage {
                    input_tokens: 18,
                    output_tokens: 0,
                },
                engine_id: "d3-edge".into(),
                model: "vllm-sr/d3-edge".into(),
            })
        }
    }

    const SPEAKER_IDENTITY_SCHEMA_VERSION: u32 = 2;
    let request = DecisionRequest {
        state: DecisionState::Json(serde_json::json!({
            "cluster_id": "speaker-c-3",
            "candidate": "person-1"
        })),
        questions: OrderedMap::from([(
            "speaker_identity_same_person".into(),
            Question::Choice {
                instructions: "Determine whether the cluster refers to the candidate.".into(),
                criteria: ["same", "different", "unsure"]
                    .into_iter()
                    .map(|key| (key.into(), None))
                    .collect(),
            },
        )]),
    };

    use std::sync::LazyLock;
    static PIN: LazyLock<String> = LazyLock::new(|| {
        digest_pin(
            &route_by_id("route.decision.d3_edge")
                .unwrap()
                .pin(&DecisionRouteSettings::default()),
        )
    });
    static FIT: LazyLock<[CalibrationRecord; 1]> = LazyLock::new(|| {
        [CalibrationRecord {
            id: "fit-speaker-identity-v2",
            route_id: "route.decision.d3_edge",
            served_model: "vllm-sr/d3-edge",
            model_pin_digest: &PIN,
            precision: "fp32",
            question_id: "speaker_identity_same_person",
            schema_version: SPEAKER_IDENTITY_SCHEMA_VERSION,
            temperature: 1.0,
            accept: 0.9,
            abstain: 0.7,
            dataset_id: "held-out-speaker-identity-v2",
            sample_count: 210,
            per_option_counts: &[70, 70, 70],
            ece_before: 0.12,
            ece_after: 0.06,
            target_metric: 0.96,
        }]
    });
    static FITTED: LazyLock<DecisionRoute> = LazyLock::new(|| {
        let base = route_by_id("route.decision.d3_edge").unwrap();
        DecisionRoute {
            id: base.id,
            provider_id: base.provider_id,
            engine: base.engine,
            engine_id: base.engine_id,
            model: base.model,
            limits: base.limits,
            calibrations: &*FIT,
        }
    });

    let dir = std::env::temp_dir().join(format!("decision-schema-ver-{}", uuid::Uuid::new_v4()));
    let repo = FileMemoryRepository::with_data_root(&dir);
    let settings = DecisionRouteSettings::default();
    let engines = EngineCache::default();
    engines.set_fixture(Arc::new(SpeakerIdentityFixture));
    let context_v2 = DispatchContext {
        session_id: "session-speaker-identity",
        settings: &settings,
        privacy: PrivacyMode::LocalOnly,
        models_dir: &dir,
        schema_version: SPEAKER_IDENTITY_SCHEMA_VERSION,
        basis: &[],
        repository: &repo,
        engines: &engines,
    };

    // Normal dispatch preserves non-live-assist schema_version on both success and preflight error.
    let ok_records = dispatch(
        Some("route.decision.d3_edge"),
        &request,
        &context_v2,
        &[("speaker_identity_same_person", "unsure")],
    )
    .await;
    assert_eq!(ok_records.len(), 1);
    assert_eq!(
        ok_records[0].schema_version,
        SPEAKER_IDENTITY_SCHEMA_VERSION
    );
    assert_ne!(
        ok_records[0].schema_version,
        crate::decision::shadow::LIVE_ASSIST_SCHEMA_VERSION
    );

    let err_records = dispatch(
        Some("route.decision.unknown"),
        &request,
        &context_v2,
        &[("speaker_identity_same_person", "unsure")],
    )
    .await;
    assert_eq!(err_records[0].outcome, DecisionOutcome::Unavailable);
    assert_eq!(
        err_records[0].schema_version,
        SPEAKER_IDENTITY_SCHEMA_VERSION
    );

    // Calibration matching honors the parameterized schema_version and drops to identity on mismatch.
    let matched = dispatch_fixture(
        &FITTED,
        &SpeakerIdentityFixture,
        &request,
        &context_v2,
        &[("speaker_identity_same_person", "unsure")],
    )
    .await;
    assert_eq!(matched[0].schema_version, SPEAKER_IDENTITY_SCHEMA_VERSION);
    assert_eq!(
        matched[0].calibration.record_id.as_deref(),
        Some("fit-speaker-identity-v2")
    );
    assert_eq!(matched[0].outcome, DecisionOutcome::Decided);

    let context_v3 = DispatchContext {
        schema_version: SPEAKER_IDENTITY_SCHEMA_VERSION + 1,
        ..context_v2
    };
    let mismatched = dispatch_fixture(
        &FITTED,
        &SpeakerIdentityFixture,
        &request,
        &context_v3,
        &[("speaker_identity_same_person", "unsure")],
    )
    .await;
    assert_eq!(
        mismatched[0].schema_version,
        SPEAKER_IDENTITY_SCHEMA_VERSION + 1
    );
    assert_eq!(mismatched[0].calibration, CalibrationSnapshot::identity());
    assert_eq!(mismatched[0].outcome, DecisionOutcome::Abstained);

    std::fs::remove_dir_all(dir).unwrap();
}
