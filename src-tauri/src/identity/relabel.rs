use super::{BindingState, SpeakerIdentityStamp};
use crate::decision::route::DecisionRecord;
use crate::projections::DiarizationSpanRevision;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelabelKind {
    Bind,
    Rename,
    Unbind,
    Merge,
    Split,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemapScope {
    Entity,
    Segments,
}
impl RelabelKind {
    pub fn scope(self) -> RemapScope {
        match self {
            Self::Split | Self::Unbind => RemapScope::Segments,
            _ => RemapScope::Entity,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchMember {
    pub batch_id: String,
    pub kind: RelabelKind,
    pub scope: RemapScope,
    pub index: usize,
    pub len: usize,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakerRelabelBatch {
    pub batch_id: String,
    pub kind: RelabelKind,
    pub scope: RemapScope,
    pub effective_at_ms: u64,
    pub revision_count: usize,
    pub revisions: Vec<DiarizationSpanRevision>,
    pub decision_records: Vec<DecisionRecord>,
}

pub fn build_relabel_batch<'a>(
    spans: impl IntoIterator<Item = &'a DiarizationSpanRevision>,
    kind: RelabelKind,
    label: &str,
    stamp: &SpeakerIdentityStamp,
    effective_at_ms: u64,
    decision_records: Vec<DecisionRecord>,
) -> Result<SpeakerRelabelBatch, String> {
    let batch_id = uuid::Uuid::new_v4().to_string();
    let mut revisions = Vec::new();
    for current in spans {
        let mut next = current.clone();
        next.provider_revision_number = Some(
            current
                .provider_revision_number
                .unwrap_or(current.revision_number),
        );
        next.revision_number = current
            .revision_number
            .checked_add(1)
            .ok_or("speaker revision exhausted")?;
        next.supersedes = Some(format!(
            "{}@rev{}",
            current.span_id, current.revision_number
        ));
        next.received_at_ms = effective_at_ms;
        next.speaker_label = Some(label.to_owned());
        next.identity = Some(stamp.clone());
        revisions.push(next);
    }
    let len = revisions.len();
    for (index, span) in revisions.iter_mut().enumerate() {
        span.identity.as_mut().expect("stamp assigned above").batch = Some(BatchMember {
            batch_id: batch_id.clone(),
            kind,
            scope: kind.scope(),
            index,
            len,
        });
    }
    let batch = SpeakerRelabelBatch {
        batch_id,
        kind,
        scope: kind.scope(),
        effective_at_ms,
        revision_count: len,
        revisions,
        decision_records,
    };
    batch.validate_header()?;
    Ok(batch)
}
impl SpeakerRelabelBatch {
    pub(crate) fn validate_header(&self) -> Result<(), String> {
        let len = self.revisions.len();
        if len == 0
            || len != self.revision_count
            || self.batch_id.is_empty()
            || self.scope != self.kind.scope()
        {
            return Err("invalid relabel batch header".into());
        }
        let first = &self.revisions[0];
        let base = first.identity.as_ref().ok_or("missing identity stamp")?;
        if base.schema_version != 1
            || base.binding_revision == 0
            || base.state == BindingState::Suggested
            || (base.state == BindingState::Anonymous) != base.person_ref.is_none()
            || base.binding_id.is_empty()
            || base.cluster_ref.speaker_id.is_empty()
            || first
                .speaker_label
                .as_deref()
                .is_none_or(|s| s.trim().is_empty())
        {
            return Err("invalid durable binding".into());
        }
        if self.kind == RelabelKind::Unbind && base.state != BindingState::Anonymous {
            return Err("unbind must restore an anonymous identity".into());
        }
        let decisions: HashSet<_> = self.decision_records.iter().map(|r| &r.id).collect();
        if decisions.len() != self.decision_records.len()
            || base
                .evidence
                .iter()
                .filter_map(|e| e.decision_record_id.as_ref())
                .any(|id| !decisions.contains(id))
        {
            return Err("invalid identity decision references".into());
        }
        let mut span_ids = HashSet::with_capacity(len);
        for (index, span) in self.revisions.iter().enumerate() {
            let stamp = span.identity.as_ref().ok_or("missing identity stamp")?;
            let member = stamp
                .batch
                .as_ref()
                .ok_or("missing relabel batch membership")?;
            if !span_ids.insert(span.span_id.as_str())
                || member.batch_id != self.batch_id
                || member.kind != self.kind
                || member.scope != self.scope
                || member.index != index
                || member.len != len
                || span.received_at_ms != self.effective_at_ms
                || span.speaker_label != first.speaker_label
                || stamp.cluster_ref != base.cluster_ref
                || stamp.binding_id != base.binding_id
                || stamp.binding_revision != base.binding_revision
                || stamp.state != base.state
                || stamp.person_ref != base.person_ref
                || stamp.label_source != base.label_source
                || stamp.evidence != base.evidence
                || stamp.confidence != base.confidence
                || stamp.schema_version != base.schema_version
            {
                return Err("inconsistent relabel batch member".into());
            }
        }
        Ok(())
    }
}
