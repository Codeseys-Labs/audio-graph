//! Projection views are reconstructed from one captured log/evidence generation.
use super::canonical_log::{
    CanonicalBasisHead, CanonicalLogSnapshot, CanonicalRecord, VerifiedTranscriptHead,
    transcript_export::TranscriptRecoveryExport,
};
use crate::projections::{
    MaterializedProjectionState, ProjectionKind, ProjectionPatch, ProjectionRecoveryStatus,
};
use std::collections::BTreeMap;

pub(crate) const MAX_PROJECTION_EVENTS_BYTES: u64 = 12 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct ProjectionRepairPlan {
    notes: Vec<String>,
    graph: Vec<String>,
}

impl ProjectionRepairPlan {
    pub(crate) fn into_lanes(self) -> [Vec<String>; 2] {
        [self.notes, self.graph]
    }

    /// Exact gap identities observed when this lane's job starts. Only an
    /// accepted repair carries these as causal references in its frame.
    pub(crate) fn for_kind(&self, kind: &ProjectionKind) -> &[String] {
        match kind {
            ProjectionKind::Notes => &self.notes,
            ProjectionKind::Graph => &self.graph,
        }
    }
}

pub(crate) type ProjectionCaptureCoverage = (
    (
        Option<crate::projections::ProjectionBasis>,
        Option<crate::projections::ProjectionBasis>,
    ),
    ProjectionRepairPlan,
);

pub(crate) struct ProjectionHistory {
    sources: BTreeMap<String, BTreeMap<u64, CanonicalBasisHead>>,
    pub(crate) receipt_evidence: super::canonical_log::StreamReceiptEvidence,
    pub(crate) patches: Vec<ProjectionPatch>,
    pub(crate) repair: ProjectionRepairPlan,
    pub(crate) present: bool,
    notes_head: Option<VerifiedTranscriptHead>,
    graph_head: Option<VerifiedTranscriptHead>,
}

impl ProjectionHistory {
    pub(super) fn from_snapshot(
        snapshot: Option<CanonicalLogSnapshot<ProjectionPatch>>,
        recovery: Option<&TranscriptRecoveryExport>,
    ) -> Result<Self, String> {
        let present = snapshot.is_some() || recovery.is_some();
        let records = snapshot.map(|s| s.records).unwrap_or_default();
        let mut gaps = BTreeMap::new();
        if let Some(recovery) = recovery {
            for (id, gap) in &recovery.gap_positions {
                let base = match &gap.after {
                    None => 0,
                    Some(head) => {
                        let record = usize::try_from(head.sequence)
                            .ok()
                            .and_then(|s| s.checked_sub(1))
                            .and_then(|i| records.get(i));
                        if !record.is_some_and(|r| {
                            r.sequence == head.sequence && r.record_hash == head.record_hash
                        }) {
                            return Err("Projection gap has no verified preceding head".into());
                        }
                        head.sequence
                    }
                };
                let affected =
                    |kind| gap.projection_lanes.is_empty() || gap.projection_lanes.contains(&kind);
                gaps.insert(
                    format!("projection-gap-{id}"),
                    (
                        base,
                        affected(ProjectionKind::Notes),
                        affected(ProjectionKind::Graph),
                    ),
                );
            }
        }
        let mut view = Self {
            sources: BTreeMap::new(),
            receipt_evidence: Default::default(),
            patches: Vec::with_capacity(records.len()),
            repair: ProjectionRepairPlan::default(),
            present,
            notes_head: None,
            graph_head: None,
        };
        for record in records {
            for (stream, head) in record.basis_heads {
                if !matches!(
                    stream.as_str(),
                    "transcript_revisions" | "speaker_revisions"
                ) {
                    return Err("Unsupported projection basis stream".into());
                }
                let positions = view.sources.entry(stream).or_default();
                if positions
                    .insert(head.sequence, head.clone())
                    .is_some_and(|old| old != head)
                {
                    return Err("Projection basis has conflicting source heads".into());
                }
            }
            for cause in &record.causal_event_ids {
                if let Some((base, notes, graph)) = gaps.get_mut(cause)
                    && record.sequence > *base
                {
                    match record.payload.kind {
                        ProjectionKind::Notes => *notes = false,
                        ProjectionKind::Graph => *graph = false,
                    }
                }
            }
            let head = match record.payload.kind {
                ProjectionKind::Notes => &mut view.notes_head,
                ProjectionKind::Graph => &mut view.graph_head,
            };
            *head = Some(VerifiedTranscriptHead {
                sequence: record.sequence,
                record_hash: record.record_hash,
            });
            view.patches.push(record.payload);
        }
        for (id, (_, notes, graph)) in gaps {
            if notes {
                view.repair.notes.push(id.clone());
            }
            if graph {
                view.repair.graph.push(id);
            }
        }
        Ok(view)
    }

    /// Compare each referenced physical position to the very source snapshot
    /// used for replay. Equal payloads or equal-height forks are insufficient.
    pub(crate) fn verify_source<T>(
        &self,
        stream: &str,
        records: &[CanonicalRecord<T>],
    ) -> Result<(), String> {
        if let Some(positions) = self.sources.get(stream) {
            for head in positions.values() {
                let record = usize::try_from(head.sequence)
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|i| records.get(i));
                if !record.is_some_and(|r| {
                    r.sequence == head.sequence
                        && r.event_id == head.event_id
                        && r.record_hash == head.record_hash
                }) {
                    return Err("Projection basis does not match its verified source".into());
                }
            }
        }
        Ok(())
    }

    fn last_basis(&self, kind: ProjectionKind) -> Option<&crate::projections::ProjectionBasis> {
        self.patches
            .iter()
            .rev()
            .find(|p| p.kind == kind)
            .map(|p| &p.basis)
    }

    pub(super) fn into_capture_coverage(self) -> ProjectionCaptureCoverage {
        let notes = self.last_basis(ProjectionKind::Notes).cloned();
        let graph = self.last_basis(ProjectionKind::Graph).cloned();
        ((notes, graph), self.repair)
    }

    pub(crate) fn bind(
        &self,
        ledger: &crate::projections::TranscriptLedger,
        state: &mut MaterializedProjectionState,
    ) {
        state.notes.transcript_coverage =
            ledger.projection_coverage(self.last_basis(ProjectionKind::Notes));
        state.graph.transcript_coverage =
            ledger.projection_coverage(self.last_basis(ProjectionKind::Graph));
        state.notes.projection_log_head.clone_from(&self.notes_head);
        state.graph.projection_log_head.clone_from(&self.graph_head);
        state.notes.projection_recovery =
            (!self.repair.notes.is_empty()).then_some(ProjectionRecoveryStatus::RecomputeRequired);
        state.graph.projection_recovery =
            (!self.repair.graph.is_empty()).then_some(ProjectionRecoveryStatus::RecomputeRequired);
    }
}
