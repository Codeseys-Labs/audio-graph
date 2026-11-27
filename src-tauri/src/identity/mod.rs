//! Session-local identity is derived from accepted speaker revisions, never a second log.
pub mod channel;
pub mod relabel;

use crate::projections::{DiarizationSpanRevision, SpeakerTimeline};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClusterRef {
    pub provider: String,
    pub timeline_id: String,
    pub speaker_id: String,
}

impl ClusterRef {
    pub fn origin(span: &DiarizationSpanRevision) -> Option<Self> {
        Some(Self {
            speaker_id: span.speaker_id.clone()?,
            provider: span.provider.clone(),
            timeline_id: span.timeline_id.clone(),
        })
    }
    pub fn matches_origin(&self, span: &DiarizationSpanRevision) -> bool {
        span.speaker_id.as_ref() == Some(&self.speaker_id)
            && span.provider == self.provider
            && span.timeline_id == self.timeline_id
    }
    pub fn contains(&self, span: &DiarizationSpanRevision) -> bool {
        span.identity.as_ref().map_or_else(
            || self.matches_origin(span),
            |stamp| &stamp.cluster_ref == self,
        )
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BindingState {
    Anonymous,
    Suggested,
    Bound,
    UserConfirmed,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LabelSource {
    Diarizer,
    Provider,
    IdentityBinding,
    User,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    UserManual,
    UserSelf,
    MicSelf,
    Decision,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct IdentityEvidenceRef {
    pub kind: EvidenceKind,
    pub weight: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_record_id: Option<String>,
    pub basis_span_ids: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakerIdentityStamp {
    pub schema_version: u32,
    pub cluster_ref: ClusterRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_ref: Option<String>,
    pub state: BindingState,
    pub label_source: LabelSource,
    pub binding_id: String,
    pub binding_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub evidence: Vec<IdentityEvidenceRef>,
    // Forward-resolved singleton revisions inherit the binding, not batch membership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<relabel::BatchMember>,
}
impl fmt::Debug for SpeakerIdentityStamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpeakerIdentityStamp")
            .field("state", &self.state)
            .field("binding_revision", &self.binding_revision)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
pub struct ClusterBinding<'a> {
    pub cluster: ClusterRef,
    pub label: &'a str,
    pub span_count: usize,
    pub state: BindingState,
    pub identity: Option<&'a SpeakerIdentityStamp>,
}

pub struct SessionIdentityLedger<'a> {
    pub clusters: BTreeMap<ClusterRef, ClusterBinding<'a>>,
    origins: BTreeMap<ClusterRef, &'a DiarizationSpanRevision>,
}
impl<'a> SessionIdentityLedger<'a> {
    pub fn from_timeline(timeline: &'a SpeakerTimeline) -> Self {
        let mut result = Self {
            clusters: BTreeMap::new(),
            origins: BTreeMap::new(),
        };
        for span in &timeline.latest_spans {
            let Some(origin) = ClusterRef::origin(span) else {
                continue;
            };
            let stamp = span.identity.as_ref();
            let canonical = stamp.map_or(&origin, |s| &s.cluster_ref);
            let binding =
                result
                    .clusters
                    .entry(canonical.clone())
                    .or_insert_with(|| ClusterBinding {
                        cluster: canonical.clone(),
                        label: "",
                        span_count: 0,
                        state: BindingState::Anonymous,
                        identity: None,
                    });
            binding.span_count += 1;
            if binding.identity.map(|s| s.binding_revision) <= stamp.map(|s| s.binding_revision) {
                binding.label = span
                    .speaker_label
                    .as_deref()
                    .or(span.speaker_id.as_deref())
                    .unwrap_or_default();
                binding.identity = stamp;
                binding.state = stamp.map_or(BindingState::Anonymous, |s| s.state);
            }
            if let Some(stamp) = stamp
                && stamp
                    .batch
                    .as_ref()
                    .is_none_or(|b| b.kind != relabel::RelabelKind::Split)
            {
                let row = result.origins.entry(origin).or_insert(span);
                if row
                    .identity
                    .as_ref()
                    .is_none_or(|s| s.binding_revision <= stamp.binding_revision)
                {
                    *row = span;
                }
            }
        }
        result
    }
    pub fn resolve_forward_label(
        &self,
        origin: &ClusterRef,
    ) -> Option<(&'a str, &'a SpeakerIdentityStamp)> {
        let span = self.origins.get(origin)?;
        Some((span.speaker_label.as_deref()?, span.identity.as_ref()?))
    }
}

/// The cache owns no durable state. Rebuild lazily on reload or when a binding changes;
/// ordinary new windows do not rebuild a map of every span in the session.
#[derive(Default)]
pub(crate) struct ForwardBindings(
    std::sync::OnceLock<BTreeMap<ClusterRef, (String, SpeakerIdentityStamp)>>,
);
impl Clone for ForwardBindings {
    fn clone(&self) -> Self {
        Self::default()
    }
}
impl fmt::Debug for ForwardBindings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ForwardBindings { .. }")
    }
}
impl ForwardBindings {
    pub(crate) fn invalidate(&mut self) {
        self.0.take();
    }
    pub(crate) fn resolve<'a>(
        &'a self,
        timeline: &SpeakerTimeline,
        provider: &str,
        lane: &str,
        id: &str,
    ) -> Option<(&'a str, &'a SpeakerIdentityStamp)> {
        let bindings = self.0.get_or_init(|| {
            if !timeline.latest_spans.iter().any(|s| s.identity.is_some()) {
                return BTreeMap::new();
            }
            SessionIdentityLedger::from_timeline(timeline)
                .origins
                .into_iter()
                .filter_map(|(key, s)| Some((key, (s.speaker_label.clone()?, s.identity.clone()?))))
                .collect()
        });
        bindings
            .iter()
            .find(|(k, _)| k.provider == provider && k.timeline_id == lane && k.speaker_id == id)
            .map(|(_, v)| (v.0.as_str(), &v.1))
    }
}

/// ASCII encoding makes source IDs distinct even in the case-insensitive graph name index.
pub fn local_cluster_key(source: &str, global: u32) -> (String, String) {
    let lane = hex::encode(source);
    (
        format!("local-cluster:{lane}:{global}"),
        format!(
            "Speaker {} [{}]",
            u64::from(global) + 1,
            if lane.is_empty() { "session" } else { &lane }
        ),
    )
}
pub fn forward_local_label<'a>(
    timeline: &'a SpeakerTimeline,
    source: &str,
    id: &str,
) -> Option<&'a str> {
    timeline
        .resolve_forward_label(
            "local_clustering",
            if source.is_empty() { "session" } else { source },
            id,
        )
        .map(|(label, _)| label)
}

#[derive(Serialize)]
pub struct SessionSpeakerCluster {
    pub cluster: ClusterRef,
    pub label: String,
    pub span_count: usize,
    pub state: BindingState,
    pub suggested_self: bool,
}

pub enum ManualEdit {
    Name(String),
    MarkSelf(String),
    Merge(ClusterRef),
    Unbind,
}

pub fn manual_batch(
    timeline: &SpeakerTimeline,
    cluster: &ClusterRef,
    edit: ManualEdit,
    effective_at_ms: u64,
    original_label: impl FnOnce() -> Result<String, String>,
) -> Result<relabel::SpeakerRelabelBatch, String> {
    use relabel::RelabelKind;
    use sha2::{Digest, Sha256};
    let ledger = SessionIdentityLedger::from_timeline(timeline);
    let source = ledger
        .clusters
        .get(cluster)
        .ok_or("unknown speaker cluster")?;
    let mut target = source;
    let mut person = source.identity.and_then(|s| s.person_ref.clone());
    let marking_self = matches!(&edit, ManualEdit::MarkSelf(_));
    let (kind, label) = match edit {
        ManualEdit::Name(name) | ManualEdit::MarkSelf(name) => {
            let name = name.trim();
            let mut words = name.split_whitespace();
            let reserved = words
                .next()
                .is_some_and(|s| s.eq_ignore_ascii_case("speaker"))
                && words
                    .next()
                    .is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()));
            if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) || reserved
            {
                return Err("invalid or reserved speaker name".into());
            }
            let mut label = name.to_owned();
            let mut suffix = 2;
            while ledger
                .clusters
                .values()
                .any(|b| b.cluster != *cluster && b.label.to_lowercase() == label.to_lowercase())
            {
                label = format!("{name} ({suffix})");
                suffix += 1;
            }
            let kind = if person.is_some() {
                RelabelKind::Rename
            } else {
                RelabelKind::Bind
            };
            person.get_or_insert_with(|| format!("person:{}", uuid::Uuid::new_v4()));
            (kind, label)
        }
        ManualEdit::Merge(into) => {
            if into == *cluster {
                return Err("cannot merge a cluster into itself".into());
            }
            target = ledger
                .clusters
                .get(&into)
                .ok_or("unknown surviving speaker cluster")?;
            person = target.identity.and_then(|s| s.person_ref.clone());
            (RelabelKind::Merge, target.label.to_owned())
        }
        ManualEdit::Unbind => {
            if person.is_none() {
                return Err("speaker is already anonymous".into());
            }
            person = None;
            (RelabelKind::Unbind, original_label()?)
        }
    };
    let binding_revision = source
        .identity
        .into_iter()
        .chain(target.identity)
        .map(|s| s.binding_revision)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("binding revision exhausted")?;
    let binding_id = hex::encode(Sha256::digest(
        serde_json::to_vec(&(&timeline.session_id, &target.cluster))
            .map_err(|_| "invalid cluster key")?,
    ));
    let evidence_kind = if marking_self
        || (person.is_some()
            && target
                .identity
                .is_some_and(|s| s.evidence.iter().any(|e| e.kind == EvidenceKind::UserSelf)))
    {
        EvidenceKind::UserSelf
    } else {
        EvidenceKind::UserManual
    };
    let stamp = SpeakerIdentityStamp {
        schema_version: 1,
        cluster_ref: target.cluster.clone(),
        state: if person.is_some() {
            BindingState::UserConfirmed
        } else {
            BindingState::Anonymous
        },
        person_ref: person,
        label_source: LabelSource::User,
        binding_id,
        binding_revision,
        confidence: None,
        evidence: vec![IdentityEvidenceRef {
            kind: evidence_kind,
            weight: 1.0,
            decision_record_id: None,
            basis_span_ids: vec![],
        }],
        batch: None,
    };
    relabel::build_relabel_batch(
        timeline.latest_spans.iter().filter(|s| {
            cluster.contains(s) || (kind == RelabelKind::Merge && target.cluster.contains(s))
        }),
        kind,
        &label,
        &stamp,
        effective_at_ms,
        vec![],
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn span(id: &str, source: &str) -> DiarizationSpanRevision {
        let (speaker_id, label) = local_cluster_key(source, 0);
        serde_json::from_value(serde_json::json!({"span_id":id,"provider":"local_clustering","timeline_id":source,"source_id":source,"speaker_id":speaker_id,"speaker_label":label,"start_time":0.0,"end_time":1.0,"is_final":false,"stability":"provisional","revision_number":1,"basis_asr_span_ids":[format!("asr:{id}")],"basis_transcript_segment_ids":[format!("seg:{id}")],"received_at_ms":1})).unwrap()
    }
    #[test]
    fn bind_is_atomic_replayable_and_source_local() {
        let a = span("a", "mic");
        let b = span("b", "mic");
        let c = span("c", "system");
        let origin = ClusterRef::origin(&a).unwrap();
        let other = ClusterRef::origin(&c).unwrap();
        let initial = SpeakerTimeline::replay("s", [a.clone(), b, c.clone()]).unwrap();
        let batch = manual_batch(
            &initial,
            &origin,
            ManualEdit::Name("Private Name".into()),
            10,
            || unreachable!(),
        )
        .unwrap();
        assert_eq!(batch.revision_count, 2);
        assert!(batch.decision_records.is_empty());
        let mut live = initial.clone();
        for invalid in [0, 1, 2, 3, 4, 5, 6] {
            let mut broken = batch.clone();
            let row = &mut broken.revisions[1];
            match invalid {
                0 => row.span_id = a.span_id.clone(),
                1 => row.revision_number += 1,
                2 => row.supersedes = None,
                3 => row.basis_asr_span_ids.clear(),
                4 => row.received_at_ms += 1,
                5 => row.identity.as_mut().unwrap().batch.as_mut().unwrap().index = 0,
                _ => broken.revision_count = 1,
            }
            assert!(
                live.admit_batch(&broken, || panic!("invalid batch reached disk"))
                    .is_err()
            );
            assert_eq!(live, initial);
        }
        assert!(
            live.admit_batch(&batch, || Err("disk full".into()))
                .is_err()
        );
        assert_eq!(live, initial);
        let remaps = live.apply_batch(&batch).unwrap();
        assert_eq!(remaps.len(), 1);
        assert_eq!(remaps[0].scope, relabel::RemapScope::Entity);
        assert_eq!(
            live.latest_spans.iter().find(|s| s.span_id == "c"),
            Some(&c)
        );
        let ledger = SessionIdentityLedger::from_timeline(&live);
        let (label, stamp) = ledger.resolve_forward_label(&origin).unwrap();
        assert_eq!(label, "Private Name");
        assert_eq!(stamp.state, BindingState::UserConfirmed);
        assert!(ledger.resolve_forward_label(&other).is_none());
        assert!(!format!("{stamp:?}").contains(stamp.person_ref.as_ref().unwrap()));
        assert_ne!(a.speaker_label, c.speaker_label);
        let merge = manual_batch(
            &live,
            &other,
            ManualEdit::Merge(origin.clone()),
            11,
            || unreachable!(),
        )
        .unwrap();
        live.apply_batch(&merge).unwrap();
        let ledger = SessionIdentityLedger::from_timeline(&live);
        assert_eq!(ledger.clusters.len(), 1);
        assert_eq!(
            ledger.resolve_forward_label(&other).unwrap().0,
            "Private Name"
        );
        let unbind = manual_batch(&live, &origin, ManualEdit::Unbind, 12, || {
            Ok(a.speaker_label.clone().unwrap())
        })
        .unwrap();
        assert_eq!(unbind.scope, relabel::RemapScope::Segments);
        live.apply_batch(&unbind).unwrap();
        assert_eq!(
            SessionIdentityLedger::from_timeline(&live)
                .resolve_forward_label(&other)
                .unwrap()
                .1
                .state,
            BindingState::Anonymous
        );
    }
}
