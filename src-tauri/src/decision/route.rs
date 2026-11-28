//! Named, sealed, single-attempt decision dispatch (ADR-0049). No route walker.
use super::{CloudflareModel, EndpointKind, SystemOneHttpEngine};
use crate::llm::route::{
    LOCAL_MODEL_PLACEHOLDER, ModelIdentitySource, owned_model_id_for_place, sanitize_model_id,
    sanitize_route_metadata,
};
use crate::persistence::{self, LocalMemoryRepository};
use crate::settings::{DecisionRouteSettings, PrivacyMode};
use audio_graph_decision_contract::{
    Answer, DecisionEngine, DecisionRequest, DecisionResponse, Egress, EngineLimits, Usage,
};
use audio_graph_ipc_contract::model_id::{ModelPlacement, stored_for_place};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    HostedJev,
    HostedCloudflare,
    LocalHttp,
    NativeLaya,
    NativeD3,
    NativeClef,
}
impl EngineKind {
    pub const fn egress(self) -> Egress {
        match self {
            Self::HostedJev | Self::HostedCloudflare => Egress::Cloud,
            Self::LocalHttp | Self::NativeLaya | Self::NativeD3 | Self::NativeClef => Egress::Local,
        }
    }
    fn native(self) -> bool {
        matches!(self, Self::NativeLaya | Self::NativeD3 | Self::NativeClef)
    }
}

#[derive(Debug)]
pub struct DecisionRoute {
    pub id: &'static str,
    pub provider_id: &'static str,
    pub engine: EngineKind,
    pub engine_id: &'static str,
    pub model: &'static str,
    pub limits: EngineLimits,
    pub calibrations: &'static [CalibrationRecord],
}
const fn route(
    id: &'static str,
    provider_id: &'static str,
    engine: EngineKind,
    engine_id: &'static str,
    model: &'static str,
    limits: EngineLimits,
) -> DecisionRoute {
    DecisionRoute {
        id,
        provider_id,
        engine,
        engine_id,
        model,
        limits,
        calibrations: &[],
    }
}
const NATIVE: EngineLimits = EngineLimits {
    max_questions: usize::MAX,
    max_choice_options: 255,
    max_score_levels: 10,
    max_state_tokens: None,
};
const LAYA: EngineLimits = EngineLimits {
    max_choice_options: 20,
    max_score_levels: 44,
    max_state_tokens: Some(1024),
    ..NATIVE
};
const LOOPBACK: EngineLimits = EngineLimits {
    max_questions: 64,
    max_choice_options: 26,
    ..NATIVE
};
const JEV: EngineLimits = EngineLimits {
    max_state_tokens: Some(32_768),
    ..NATIVE
};
pub const DECISION_ROUTES: &[DecisionRoute] = &[
    route(
        "route.decision.laya",
        "decision.laya",
        EngineKind::NativeLaya,
        "laya",
        "laya-english",
        LAYA,
    ),
    route(
        "route.decision.d3_edge",
        "decision.d3_edge",
        EngineKind::NativeD3,
        "d3-edge",
        "d3-edge",
        NATIVE,
    ),
    route(
        "route.decision.d3_lite",
        "decision.d3_lite",
        EngineKind::NativeD3,
        "d3-lite",
        "d3-lite",
        NATIVE,
    ),
    route(
        "route.decision.d3_nano",
        "decision.d3_nano",
        EngineKind::NativeD3,
        "d3-nano",
        "d3-nano",
        NATIVE,
    ),
    route(
        "route.decision.d3_mini",
        "decision.d3_mini",
        EngineKind::NativeD3,
        "d3-mini",
        "d3-mini",
        NATIVE,
    ),
    route(
        "route.decision.ollama_loopback",
        "decision.ollama_loopback",
        EngineKind::LocalHttp,
        "local_ollama",
        "",
        LOOPBACK,
    ),
    route(
        "route.decision.vllm_loopback",
        "decision.vllm_loopback",
        EngineKind::LocalHttp,
        "local_vllm",
        "",
        LOOPBACK,
    ),
    route(
        "route.decision.jev_typesafe",
        "decision.jev_typesafe",
        EngineKind::HostedJev,
        "typesafe_jev",
        "jev-latest",
        JEV,
    ),
    route(
        "route.decision.jev_cloudflare",
        "decision.jev_cloudflare",
        EngineKind::HostedCloudflare,
        "cloudflare_workers_ai",
        "typesafe/jev",
        EngineLimits {
            max_state_tokens: Some(32_000),
            ..NATIVE
        },
    ),
    route(
        "route.decision.clef_flash_cloudflare",
        "decision.clef_flash_cloudflare",
        EngineKind::HostedCloudflare,
        "cloudflare_workers_ai",
        "clef-flash",
        EngineLimits {
            max_questions: 64,
            max_state_tokens: Some(24_576),
            ..NATIVE
        },
    ),
];
pub fn route_by_id(id: &str) -> Option<&'static DecisionRoute> {
    DECISION_ROUTES.iter().find(|r| r.id == id)
}

/// Availability selection occurs once, before dispatch. A failed selected engine is never replaced.
pub fn select_shadow_route<'a>(
    settings: &'a DecisionRouteSettings,
    models_dir: &Path,
) -> Option<&'a str> {
    if let Some(id) = settings.live_assist_route_id.as_deref() {
        return Some(id);
    }
    if crate::models::decision_model_cached(models_dir, "d3-edge") {
        Some("route.decision.d3_edge")
    } else if crate::models::decision_model_cached(models_dir, "laya-english") {
        Some("route.decision.laya")
    } else {
        None
    }
}

pub struct AuthorizedDecisionRoute {
    route: &'static DecisionRoute,
    _seal: (),
}
pub fn authorize_decision_route(
    id: &str,
    privacy: PrivacyMode,
) -> Result<AuthorizedDecisionRoute, &'static str> {
    let route = route_by_id(id).ok_or("unknown_route")?;
    // ADR-0049 §2: local shadow admission uses zero-egress evidence, not UI promotion.
    // Hosted content still requires ADR-0033 promotion through the single registry table.
    if route.engine.egress() == Egress::Cloud {
        crate::provider_registry::ensure_provider_id_start_enabled(route.provider_id)
            .map_err(|_| "provider_deferred")?;
    }
    if route.engine == EngineKind::NativeClef {
        return Err("engine_reserved");
    }
    if route.engine.egress() == Egress::Cloud && !privacy.allows_session_cloud_content_transfer() {
        return Err("policy_blocked");
    }
    Ok(AuthorizedDecisionRoute { route, _seal: () })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentPin {
    pub file: String,
    pub sha256: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPin {
    pub requested: String,
    pub repository: Option<String>,
    pub revision: Option<String>,
    pub components: Vec<ComponentPin>,
    pub precision: String,
}
impl DecisionRoute {
    fn pin(&self, settings: &DecisionRouteSettings) -> ModelPin {
        let requested = if self.engine == EngineKind::LocalHttp {
            settings
                .local_http_model
                .as_deref()
                .and_then(sanitize_route_metadata)
                .unwrap_or_default()
        } else {
            self.model.to_owned()
        };
        let mut pin = ModelPin {
            requested,
            repository: None,
            revision: None,
            components: Vec::new(),
            precision: if self.engine.native() {
                "fp32"
            } else {
                "vendor"
            }
            .into(),
        };
        if self.engine.native()
            && let Some((url, files)) = crate::models::decision_model_metadata(self.model)
            && let Some((repo, revision)) = url
                .strip_prefix("https://huggingface.co/")
                .and_then(|url| url.split_once("/resolve/"))
        {
            pin.repository = Some(repo.into());
            pin.revision = Some(revision.into());
            // Missing verified component digests keep native pins in shadow.
            pin.components = files
                .iter()
                .map(|file| ComponentPin {
                    file: (*file).into(),
                    sha256: None,
                })
                .collect();
        }
        pin
    }
    /// `pin` as a record stores it. The pin's digest and the calibrations are
    /// keyed on the pin as configured. A record shows a local model by a
    /// display name (see `ipc_contract::model_id`), never by the path a user
    /// configured for a local server.
    ///
    /// Egress answers where a decision model ran: a hosted engine dials a
    /// fixed public host, and a local HTTP engine only a loopback IP literal
    /// (`EndpointKind::url`).
    fn stored_pin(&self, mut pin: ModelPin, settings: &DecisionRouteSettings) -> ModelPin {
        if self.engine.egress() == Egress::Cloud {
            return pin;
        }
        pin.requested = if self.engine == EngineKind::LocalHttp {
            // From the setting itself: the filter in `pin` has already dropped
            // the separators of a Windows path. A setting that leaves no name
            // to keep is stored as the placeholder a stored record reads as.
            settings
                .local_http_model
                .as_deref()
                .map(|model| {
                    sanitize_model_id(model, false)
                        .unwrap_or_else(|| LOCAL_MODEL_PLACEHOLDER.to_string())
                })
                .unwrap_or_default()
        } else {
            owned_model_id_for_place(pin.requested, false)
        };
        pin
    }
    fn endpoint(&self, settings: &DecisionRouteSettings) -> Result<EndpointKind, &'static str> {
        Ok(match self.engine {
            EngineKind::HostedJev => EndpointKind::TypeSafe,
            EngineKind::HostedCloudflare => EndpointKind::Cloudflare {
                account_id: settings
                    .cloudflare_account_id
                    .clone()
                    .ok_or("missing_account")?,
                model: if self.model == "clef-flash" {
                    CloudflareModel::ClefFlash
                } else {
                    CloudflareModel::Jev
                },
            },
            EngineKind::LocalHttp => {
                let url = settings.local_http_url.clone().ok_or("missing_endpoint")?;
                let model = settings.local_http_model.clone().ok_or("missing_model")?;
                let endpoint = if self.engine_id == "local_ollama" {
                    EndpointKind::Ollama { url, model }
                } else {
                    EndpointKind::Vllm { url, model }
                };
                endpoint.url().map_err(|_| "invalid_loopback_endpoint")?;
                endpoint
            }
            _ => return Err("engine_reserved"),
        })
    }
}

#[derive(Debug)]
pub struct CalibrationRecord {
    pub id: &'static str,
    pub route_id: &'static str,
    pub served_model: &'static str,
    pub model_pin_digest: &'static str,
    pub precision: &'static str,
    pub question_id: &'static str,
    pub schema_version: u32,
    pub temperature: f64,
    pub accept: f64,
    pub abstain: f64,
    pub dataset_id: &'static str,
    pub sample_count: usize,
    pub per_option_counts: &'static [usize],
    pub ece_before: f64,
    pub ece_after: f64,
    pub target_metric: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationSnapshot {
    pub record_id: Option<String>,
    pub temperature: f64,
    pub accept: f64,
    pub abstain: f64,
}
impl CalibrationSnapshot {
    fn identity() -> Self {
        Self {
            record_id: None,
            temperature: 1.0,
            accept: 1.0,
            abstain: 1.0,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    Decided,
    Proposed,
    Abstained,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAuthority {
    Shadow,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    /// Effective answer supplied by a route. No shadow dispatch selects this source.
    Route,
    /// Effective answer supplied by the heuristic, including every shadow observation.
    Heuristic,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionBasis {
    pub span_id: String,
    pub revision: u64,
}
/// Decoded through [`StoredDecisionRecord`], so a stored model id is read by
/// where the record says the model ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "StoredDecisionRecord")]
pub struct DecisionRecord {
    pub id: String,
    pub call_id: String,
    pub route_id: Option<String>,
    pub provider_id: Option<String>,
    pub engine: Option<EngineKind>,
    pub egress: String,
    pub model_pin: Option<ModelPin>,
    pub model_pin_digest: Option<String>,
    pub served_model: Option<String>,
    pub model_identity_source: ModelIdentitySource,
    pub question_id: String,
    pub schema_version: u32,
    pub state_sha256: String,
    pub basis: Vec<DecisionBasis>,
    pub raw: Option<Answer>,
    pub calibrated: Option<Answer>,
    pub calibration: CalibrationSnapshot,
    pub outcome: DecisionOutcome,
    pub authority: DecisionAuthority,
    pub source: DecisionSource,
    pub heuristic_answer: String,
    pub route_answer: Option<String>,
    pub bf16_near_tie: bool,
    pub latency_ms: u64,
    pub usage: Usage,
    pub error_class: Option<String>,
    /// Where the decision model ran, written with the model rule applied.
    /// `None` on records written before it, which are read by `egress`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_placement: Option<ModelPlacement>,
}

/// A [`DecisionRecord`] as a card or a relabel run stores it.
///
/// Records written before the model rule can hold the path a local server
/// loaded or a user configured, and carry no `model_placement`. Building the
/// typed value from this one applies the rule on every read: a marked record
/// is read by its placement, and an unmarked one that does not say `cloud` is
/// a local model's and shows a display name. The stored record is not
/// rewritten.
#[derive(Deserialize)]
struct StoredDecisionRecord {
    id: String,
    call_id: String,
    route_id: Option<String>,
    provider_id: Option<String>,
    engine: Option<EngineKind>,
    egress: String,
    model_pin: Option<ModelPin>,
    model_pin_digest: Option<String>,
    served_model: Option<String>,
    model_identity_source: ModelIdentitySource,
    question_id: String,
    schema_version: u32,
    state_sha256: String,
    basis: Vec<DecisionBasis>,
    raw: Option<Answer>,
    calibrated: Option<Answer>,
    calibration: CalibrationSnapshot,
    outcome: DecisionOutcome,
    authority: DecisionAuthority,
    source: DecisionSource,
    heuristic_answer: String,
    route_answer: Option<String>,
    bf16_near_tie: bool,
    latency_ms: u64,
    usage: Usage,
    error_class: Option<String>,
    #[serde(
        default,
        deserialize_with = "audio_graph_ipc_contract::model_id::lenient_placement"
    )]
    model_placement: Option<ModelPlacement>,
}

impl From<StoredDecisionRecord> for DecisionRecord {
    fn from(stored: StoredDecisionRecord) -> Self {
        let legacy_remote = stored.egress == "cloud";
        let placed = |id| stored_for_place(id, stored.model_placement, legacy_remote);
        Self {
            id: stored.id,
            call_id: stored.call_id,
            route_id: stored.route_id,
            provider_id: stored.provider_id,
            engine: stored.engine,
            egress: stored.egress,
            model_pin: stored.model_pin.map(|pin| ModelPin {
                requested: placed(pin.requested),
                ..pin
            }),
            model_pin_digest: stored.model_pin_digest,
            served_model: stored.served_model.map(placed),
            model_identity_source: stored.model_identity_source,
            question_id: stored.question_id,
            schema_version: stored.schema_version,
            state_sha256: stored.state_sha256,
            basis: stored.basis,
            raw: stored.raw,
            calibrated: stored.calibrated,
            calibration: stored.calibration,
            outcome: stored.outcome,
            authority: stored.authority,
            source: stored.source,
            heuristic_answer: stored.heuristic_answer,
            route_answer: stored.route_answer,
            bf16_near_tie: stored.bf16_near_tie,
            latency_ms: stored.latency_ms,
            usage: stored.usage,
            error_class: stored.error_class,
            model_placement: stored.model_placement,
        }
    }
}

struct CachedNativeEngine {
    route_id: &'static str,
    models_dir: PathBuf,
    engine: Arc<dyn DecisionEngine>,
}
#[derive(Default)]
pub struct EngineCache {
    cached: Mutex<Option<CachedNativeEngine>>,
    #[cfg(test)]
    fixture: Mutex<Option<Arc<dyn DecisionEngine>>>,
}
impl EngineCache {
    #[cfg(test)]
    pub(crate) fn set_fixture(&self, engine: Arc<dyn DecisionEngine>) {
        assert_eq!(engine.egress(), Egress::Local);
        *self.fixture.lock().unwrap() = Some(engine);
    }
    fn native(
        &self,
        authorized: &AuthorizedDecisionRoute,
        models_dir: &Path,
    ) -> Result<Arc<dyn DecisionEngine>, audio_graph_decision_contract::DecisionError> {
        #[cfg(test)]
        if let Some(engine) = self.fixture.lock().unwrap().clone() {
            return Ok(engine);
        }
        let mut cached = self
            .cached
            .lock()
            .map_err(|_| audio_graph_decision_contract::DecisionError::Internal)?;
        if let Some(slot) = cached
            .as_ref()
            .filter(|slot| slot.route_id == authorized.route.id && slot.models_dir == models_dir)
        {
            return Ok(slot.engine.clone());
        }
        let config = audio_graph_decision_engine::EngineConfig::new(models_dir.to_path_buf());
        #[cfg_attr(not(feature = "decision-d3"), allow(unused_mut))]
        let mut registry = audio_graph_decision_engine::EngineRegistry::default();
        #[cfg(feature = "decision-d3")]
        audio_graph_decision_engine::d3::register(&mut registry);
        let engine = registry.create(authorized.route.engine_id, &config)?;
        *cached = Some(CachedNativeEngine {
            route_id: authorized.route.id,
            models_dir: models_dir.to_path_buf(),
            engine: engine.clone(),
        });
        Ok(engine)
    }
}

pub struct DispatchContext<'a> {
    pub session_id: &'a str,
    pub settings: &'a DecisionRouteSettings,
    pub privacy: PrivacyMode,
    pub models_dir: &'a Path,
    pub schema_version: u32,
    pub basis: &'a [DecisionBasis],
    pub repository: &'a dyn LocalMemoryRepository,
    pub engines: &'a EngineCache,
}

struct DigestWriter(Sha256);
impl std::io::Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn canonical_json(value: &serde_json::Value, writer: &mut DigestWriter) {
    match value {
        serde_json::Value::Object(object) => {
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
            writer.0.update(b"{");
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    writer.0.update(b",");
                }
                serde_json::to_writer(&mut *writer, key).expect("JSON key serializes");
                writer.0.update(b":");
                canonical_json(value, writer);
            }
            writer.0.update(b"}");
        }
        serde_json::Value::Array(array) => {
            writer.0.update(b"[");
            for (index, value) in array.iter().enumerate() {
                if index != 0 {
                    writer.0.update(b",");
                }
                canonical_json(value, writer);
            }
            writer.0.update(b"]");
        }
        _ => serde_json::to_writer(writer, value).expect("JSON value serializes"),
    }
}
pub fn state_digest(state: &audio_graph_decision_contract::DecisionState) -> String {
    let mut writer = DigestWriter(Sha256::new());
    match state {
        audio_graph_decision_contract::DecisionState::Text(text) => {
            serde_json::to_writer(&mut writer, text).expect("state serializes")
        }
        audio_graph_decision_contract::DecisionState::Json(value) => {
            canonical_json(value, &mut writer)
        }
    }
    format!("sha256:{}", hex::encode(writer.0.finalize()))
}
fn digest_pin(pin: &ModelPin) -> String {
    let mut writer = DigestWriter(Sha256::new());
    serde_json::to_writer(&mut writer, pin).expect("pin serializes");
    format!("sha256:{}", hex::encode(writer.0.finalize()))
}
fn records(
    route: Option<&DecisionRoute>,
    request: &DecisionRequest,
    context: &DispatchContext<'_>,
    heuristics: &[(&str, &str)],
) -> Vec<DecisionRecord> {
    let digest = state_digest(&request.state);
    let call_id = uuid::Uuid::new_v4().to_string();
    // The digest is of the pin as configured, which calibrations are keyed
    // on. The records store it as `stored_pin` gives it.
    let pin = route.map(|route| route.pin(context.settings));
    let pin_digest = pin.as_ref().map(digest_pin);
    let pin = route
        .zip(pin)
        .map(|(route, pin)| route.stored_pin(pin, context.settings));
    request
        .questions
        .keys()
        .map(|id| DecisionRecord {
            id: uuid::Uuid::new_v4().to_string(),
            call_id: call_id.clone(),
            route_id: route.map(|r| r.id.into()),
            provider_id: route.map(|r| r.provider_id.into()),
            engine: route.map(|r| r.engine),
            egress: if route.is_some_and(|r| r.engine.egress() == Egress::Cloud) {
                "cloud"
            } else {
                "local"
            }
            .into(),
            model_pin: pin.clone(),
            model_pin_digest: pin_digest.clone(),
            served_model: None,
            model_identity_source: ModelIdentitySource::Requested,
            question_id: id.clone(),
            schema_version: context.schema_version,
            state_sha256: digest.clone(),
            basis: context.basis.to_vec(),
            raw: None,
            calibrated: None,
            calibration: CalibrationSnapshot::identity(),
            outcome: DecisionOutcome::Unavailable,
            authority: DecisionAuthority::Shadow,
            source: DecisionSource::Heuristic,
            heuristic_answer: heuristics
                .iter()
                .find(|(key, _)| *key == id)
                .map_or("none", |(_, answer)| *answer)
                .into(),
            route_answer: None,
            bf16_near_tie: false,
            latency_ms: 0,
            usage: Usage::default(),
            error_class: None,
            model_placement: route.map(|r| {
                if r.engine.egress() == Egress::Cloud {
                    ModelPlacement::Remote
                } else {
                    ModelPlacement::Local
                }
            }),
        })
        .collect()
}
fn fail(records: &mut [DecisionRecord], code: &str, start: Instant) {
    for record in records {
        record.outcome = DecisionOutcome::Unavailable;
        record.error_class = Some(code.into());
        record.latency_ms = start.elapsed().as_millis() as u64;
    }
}
fn movement(
    context: &DispatchContext<'_>,
    record: &DecisionRecord,
    event_type: persistence::DataMovementEventType,
    sent: bool,
) -> Result<(), String> {
    use persistence::*;
    let remote = sent && record.egress == "cloud";
    let destination = if remote {
        DataMovementDestination::provider(
            record
                .provider_id
                .as_deref()
                .unwrap_or("decision.unavailable"),
            "system_one",
        )
    } else {
        DataMovementDestination::local()
    };
    let policy = MovementPolicy {
        privacy_mode: match context.privacy {
            crate::settings::PrivacyMode::LocalOnly => persistence::PrivacyMode::LocalOnly,
            crate::settings::PrivacyMode::ByokCloud => persistence::PrivacyMode::ByokCloud,
            crate::settings::PrivacyMode::CloudDisabledReadinessOnly => {
                persistence::PrivacyMode::CloudDisabledReadinessOnly
            }
            crate::settings::PrivacyMode::OrgPromotion => persistence::PrivacyMode::OrgSync,
        },
        user_visible: true,
        retention_class: RetentionClass::Transient,
    };
    let mut builder = DataMovementLedgerBuilder::new(
        context.session_id,
        DataMovementActor::System,
        event_type,
        policy,
        destination,
    )
    .source(DataMovementSource {
        kind: record
            .route_id
            .clone()
            .unwrap_or_else(|| "decision.unavailable".into()),
        source_id: Some(record.call_id.clone()),
        source_label: None,
    })
    .model(MovementModel {
        provider_id: record.provider_id.clone(),
        model_id: record
            .served_model
            .clone()
            .or_else(|| record.model_pin.as_ref().map(|pin| pin.requested.clone())),
        model_placement: record.model_placement,
    });
    if sent {
        builder = builder.data_classes([DataClass::TranscriptText, DataClass::UsageMetadata]);
    }
    if event_type == DataMovementEventType::ProviderCallStarted {
        builder = builder.result(DataMovementResult::started());
    }
    if let Some(code) = &record.error_class {
        builder = builder.result(DataMovementResult::failed(
            code,
            "Decision unavailable; heuristic retained",
        ));
    }
    builder = builder.counts(MovementCounts {
        audio_ms: None,
        text_chars: None,
        tokens_in: Some(record.usage.input_tokens),
        tokens_out: Some(record.usage.output_tokens),
        bytes: None,
    });
    context
        .repository
        .append_data_movement_event(context.session_id, &builder.build())
}

pub async fn dispatch(
    id: Option<&str>,
    request: &DecisionRequest,
    context: &DispatchContext<'_>,
    heuristics: &[(&str, &str)],
) -> Vec<DecisionRecord> {
    let start = Instant::now();
    let route = id.and_then(route_by_id);
    let mut records = records(route, request, context, heuristics);
    let authorized = match id
        .ok_or("no_cached_route")
        .and_then(|id| authorize_decision_route(id, context.privacy))
    {
        Ok(route) => route,
        Err(code) => {
            fail(&mut records, code, start);
            if let Some(record) = records.first() {
                let _ = movement(
                    context,
                    record,
                    persistence::DataMovementEventType::ProviderCallFailed,
                    false,
                );
            }
            return records;
        }
    };
    let engine = if authorized.route.engine.native() {
        context.engines.native(&authorized, context.models_dir)
    } else {
        authorized
            .route
            .endpoint(context.settings)
            .map_err(|_| audio_graph_decision_contract::DecisionError::Internal)
            .and_then(SystemOneHttpEngine::from_saved)
            .map(|engine| engine.with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::from_privacy_mode_and_transfer_requirement(context.privacy, authorized.route.engine.egress() == Egress::Cloud)))
            .map(|engine| Arc::new(engine) as Arc<dyn DecisionEngine>)
    };
    match engine {
        Ok(engine) => {
            dispatch_authorized(&authorized, engine.as_ref(), request, context, records).await
        }
        Err(_) => {
            fail(&mut records, "engine_unavailable", start);
            if let Some(record) = records.first() {
                let _ = movement(
                    context,
                    record,
                    persistence::DataMovementEventType::ProviderCallFailed,
                    false,
                );
            }
            records
        }
    }
}

fn native_request_admitted(
    route: &DecisionRoute,
    request: &DecisionRequest,
) -> Result<(), &'static str> {
    #[cfg(feature = "decision-laya")]
    if route.engine == EngineKind::NativeLaya
        && audio_graph_decision_engine::laya::checkpoint_for_request(request)
            == audio_graph_decision_engine::laya::Checkpoint::Multilingual
    {
        return Err("multilingual_distribution_deferred");
    }
    let _ = (route, request);
    Ok(())
}
async fn dispatch_authorized(
    authorized: &AuthorizedDecisionRoute,
    engine: &dyn DecisionEngine,
    request: &DecisionRequest,
    context: &DispatchContext<'_>,
    mut records: Vec<DecisionRecord>,
) -> Vec<DecisionRecord> {
    let start = Instant::now();
    let route = authorized.route;
    let preflight = if engine.egress() != route.engine.egress() || engine.id() != route.engine_id {
        Err("engine_identity_mismatch")
    } else if route
        .limits
        .validate(request)
        .and_then(|()| engine.limits().validate(request))
        .is_err()
    {
        Err("limit_exceeded")
    } else {
        native_request_admitted(route, request)
    };
    if let Err(code) = preflight {
        fail(&mut records, code, start);
        if let Some(record) = records.first() {
            let _ = movement(
                context,
                record,
                persistence::DataMovementEventType::ProviderCallFailed,
                false,
            );
        }
        return records;
    }
    if records.first().is_none_or(|record| {
        movement(
            context,
            record,
            persistence::DataMovementEventType::ProviderCallStarted,
            true,
        )
        .is_err()
    }) {
        fail(&mut records, "ledger_unavailable", start);
        return records;
    }
    match engine.system_one(request).await {
        Ok(response) if super::validate_answers(request, &response).is_ok() => {
            apply_response(route, request, &response, &mut records, start)
        }
        Ok(_) => fail(&mut records, "invalid_response", start),
        Err(_) => fail(&mut records, "engine_error", start),
    }
    if let Some(record) = records.first() {
        let event_type = if record.error_class.is_some() {
            persistence::DataMovementEventType::ProviderCallFailed
        } else {
            persistence::DataMovementEventType::ProviderCallSucceeded
        };
        if movement(context, record, event_type, true).is_err() {
            fail(&mut records, "ledger_unavailable", start);
        }
    }
    records
}

fn apply_response(
    route: &DecisionRoute,
    request: &DecisionRequest,
    response: &DecisionResponse,
    records: &mut [DecisionRecord],
    start: Instant,
) {
    for record in records {
        let Some(raw) = response.answers.get(&record.question_id) else {
            continue;
        };
        // Calibrations are keyed on the id as the engine reported it, so the
        // fit is matched on that. The record stores the id by where the model
        // ran: a display name for a local engine.
        let reported = sanitize_route_metadata(&response.model);
        let served = if route.engine.egress() == Egress::Cloud {
            reported.clone()
        } else {
            sanitize_model_id(&response.model, false)
        };
        record.model_identity_source = if served.is_some() {
            ModelIdentitySource::Served
        } else {
            ModelIdentitySource::Requested
        };
        record.served_model =
            served.or_else(|| record.model_pin.as_ref().map(|pin| pin.requested.clone()));
        let calibration = route.calibrations.iter().find(|fit| {
            record.model_identity_source == ModelIdentitySource::Served
                && fit.route_id == route.id
                && Some(fit.served_model) == reported.as_deref()
                && Some(fit.model_pin_digest) == record.model_pin_digest.as_deref()
                && record
                    .model_pin
                    .as_ref()
                    .is_some_and(|pin| pin.precision == fit.precision)
                && fit.question_id == record.question_id
                && fit.schema_version == record.schema_version
                && fit.sample_count >= 200
                && !fit.dataset_id.is_empty()
                && fit.per_option_counts.len()
                    == option_count(&request.questions[&record.question_id])
                && fit.per_option_counts.iter().all(|count| *count >= 20)
                && fit.per_option_counts.iter().sum::<usize>() == fit.sample_count
                && fit.temperature.is_finite()
                && fit.temperature > 0.0
                && (route.engine.native() || fit.temperature == 1.0)
                && (0.0..=1.0).contains(&fit.abstain)
                && (fit.abstain..=1.0).contains(&fit.accept)
                && [fit.ece_before, fit.ece_after, fit.target_metric]
                    .iter()
                    .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        });
        record.calibration =
            calibration.map_or_else(CalibrationSnapshot::identity, |fit| CalibrationSnapshot {
                record_id: Some(fit.id.into()),
                temperature: fit.temperature,
                accept: fit.accept,
                abstain: fit.abstain,
            });
        let calibrated = calibrate(raw, record.calibration.temperature, route.engine.native());
        let confidence = confidence(&calibrated);
        record.outcome = match confidence {
            Some(c) if c >= record.calibration.accept => DecisionOutcome::Decided,
            Some(c) if c >= record.calibration.abstain => DecisionOutcome::Proposed,
            _ => DecisionOutcome::Abstained,
        };
        record.route_answer = Some(answer_key(
            &calibrated,
            &request.questions[&record.question_id],
        ));
        record.bf16_near_tie = record
            .model_pin
            .as_ref()
            .is_some_and(|pin| pin.precision == "bf16")
            && near_tie(&calibrated);
        record.raw = Some(raw.clone());
        record.calibrated = Some(calibrated);
        record.usage = response.usage.clone();
        record.latency_ms = start.elapsed().as_millis() as u64;
    }
}
fn option_count(question: &audio_graph_decision_contract::Question) -> usize {
    use audio_graph_decision_contract::Question;
    match question {
        Question::Choice { criteria, .. } => criteria.len(),
        Question::Noul { .. } => 2,
        Question::Score { criteria, .. } => criteria.len(),
    }
}
fn confidence(answer: &Answer) -> Option<f64> {
    match answer {
        Answer::Choice { confidence, .. } | Answer::Score { confidence, .. } => Some(*confidence),
        Answer::Noul { confidence, .. } => *confidence,
    }
}
fn temper(values: &mut [f64], temperature: f64) {
    let max = values.iter().copied().fold(0.0, f64::max);
    if max == 0.0 {
        return;
    }
    for value in values.iter_mut() {
        *value = (*value / max).powf(1.0 / temperature);
    }
    let sum: f64 = values.iter().sum();
    for value in values {
        *value /= sum;
    }
}
fn calibrate(raw: &Answer, temperature: f64, native: bool) -> Answer {
    if !native || temperature == 1.0 {
        return raw.clone();
    }
    match raw {
        Answer::Choice {
            choice,
            probabilities,
            ..
        } => {
            let mut probabilities = probabilities.clone();
            let max = probabilities.values().copied().fold(0.0, f64::max);
            if max > 0.0 {
                for value in probabilities.values_mut() {
                    *value = (*value / max).powf(1.0 / temperature);
                }
                let sum: f64 = probabilities.values().sum();
                for value in probabilities.values_mut() {
                    *value /= sum;
                }
            }
            let confidence = probabilities.values().copied().fold(0.0, f64::max);
            Answer::Choice {
                choice: choice.clone(),
                probabilities,
                confidence,
            }
        }
        Answer::Score { probabilities, .. } => {
            let mut probabilities = probabilities.clone();
            temper(&mut probabilities, temperature);
            let confidence = probabilities.iter().copied().fold(0.0, f64::max);
            Answer::Score {
                score: probabilities
                    .iter()
                    .enumerate()
                    .map(|(index, p)| index as f64 * p)
                    .sum(),
                probabilities,
                confidence,
            }
        }
        Answer::Noul {
            noul, confidence, ..
        } => {
            let mut p = [1.0 - noul, *noul];
            temper(&mut p, temperature);
            Answer::Noul {
                noul: p[1],
                probabilities: Some([("false".into(), p[0]), ("true".into(), p[1])].into()),
                confidence: confidence.map(|_| p[0].max(p[1])),
            }
        }
    }
}
fn answer_key(answer: &Answer, question: &audio_graph_decision_contract::Question) -> String {
    match answer {
        Answer::Choice { choice, .. } => choice.clone(),
        Answer::Noul { noul, .. } => (*noul > 0.5).to_string(),
        Answer::Score { probabilities, .. } => {
            let index = probabilities.iter().enumerate().fold(0, |best, (i, p)| {
                if *p > probabilities[best] { i } else { best }
            });
            match question {
                audio_graph_decision_contract::Question::Score { criteria, .. } => {
                    criteria[index].clone()
                }
                _ => index.to_string(),
            }
        }
    }
}
fn near_tie(answer: &Answer) -> bool {
    match answer {
        Answer::Choice { probabilities, .. } => {
            probability_near_tie(probabilities.values().copied())
        }
        Answer::Score { probabilities, .. } => probability_near_tie(probabilities.iter().copied()),
        Answer::Noul { noul, .. } => probability_near_tie([*noul, 1.0 - noul]),
    }
}
fn probability_near_tie(values: impl IntoIterator<Item = f64>) -> bool {
    let (mut first, mut second) = (0.0_f64, 0.0_f64);
    for value in values {
        if value > first {
            second = first;
            first = value;
        } else if value > second {
            second = value;
        }
    }
    first - second < 0.05
}

#[cfg(test)]
pub(crate) async fn dispatch_fixture(
    route: &'static DecisionRoute,
    engine: &dyn DecisionEngine,
    request: &DecisionRequest,
    context: &DispatchContext<'_>,
    heuristics: &[(&str, &str)],
) -> Vec<DecisionRecord> {
    assert_eq!(engine.egress(), Egress::Local);
    let mut authorized = authorize_decision_route(route.id, context.privacy)
        .expect("fixture uses normal route admission");
    assert_eq!(authorized.route.provider_id, route.provider_id);
    assert_eq!(authorized.route.engine, route.engine);
    authorized.route = route;
    let records = records(Some(route), request, context, heuristics);
    dispatch_authorized(&authorized, engine, request, context, records).await
}

#[cfg(test)]
mod tests;
