//! Projection views are reconstructed from one captured log/evidence generation.
use super::canonical_log::{
    CanonicalBasisHeadVector, CanonicalLogSnapshot, CanonicalRecord, VerifiedTranscriptHead,
    transcript_export::TranscriptRecoveryExport,
};
use crate::projections::{
    MaterializedProjectionState, ProjectionBasisUnavailable, ProjectionKind, ProjectionPatch,
    ProjectionRecoveryStatus, VerifiedProjectionFrame,
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

struct FrameBasis {
    head: VerifiedTranscriptHead,
    sources: CanonicalBasisHeadVector,
    unavailable: bool,
}

impl FrameBasis {
    fn matches<T>(&self, stream: &str, records: &[CanonicalRecord<T>]) -> bool {
        self.sources.get(stream).is_none_or(|head| {
            usize::try_from(head.sequence)
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| records.get(i))
                .is_some_and(|r| {
                    r.sequence == head.sequence
                        && r.event_id == head.event_id
                        && r.record_hash == head.record_hash
                })
        })
    }
}

/// A lane needs repair at every log head from the gap's anchor up to its
/// first causally linked repair. A later repair only counts if it was replayed.
struct LaneGap {
    after: u64,
    repaired_at: Option<u64>,
}

#[derive(Default)]
struct ProjectionGaps {
    notes: BTreeMap<String, LaneGap>,
    graph: BTreeMap<String, LaneGap>,
}

impl ProjectionGaps {
    fn lane(&mut self, kind: &ProjectionKind) -> &mut BTreeMap<String, LaneGap> {
        match kind {
            ProjectionKind::Notes => &mut self.notes,
            ProjectionKind::Graph => &mut self.graph,
        }
    }

    fn plan_at(&self, head: u64) -> ProjectionRepairPlan {
        let pending = |lane: &BTreeMap<String, LaneGap>| {
            lane.iter()
                .filter(|(_, gap)| {
                    gap.after <= head && gap.repaired_at.is_none_or(|seq| head < seq)
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        ProjectionRepairPlan {
            notes: pending(&self.notes),
            graph: pending(&self.graph),
        }
    }
}

pub(crate) struct ProjectionHistory {
    frames: Vec<FrameBasis>,
    replay_transcript_len: Option<usize>,
    replay_speaker_len: Option<usize>,
    pub(crate) basis_unavailable: Option<ProjectionBasisUnavailable>,
    pub(crate) receipt_evidence: super::canonical_log::StreamReceiptEvidence,
    pub(crate) patches: Vec<ProjectionPatch>,
    gaps: ProjectionGaps,
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
        let mut gaps = ProjectionGaps::default();
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
                for kind in [ProjectionKind::Notes, ProjectionKind::Graph] {
                    if gap.projection_lanes.is_empty() || gap.projection_lanes.contains(&kind) {
                        gaps.lane(&kind).insert(
                            format!("projection-gap-{id}"),
                            LaneGap {
                                after: base,
                                repaired_at: None,
                            },
                        );
                    }
                }
            }
        }
        let mut view = Self {
            frames: Vec::with_capacity(records.len()),
            replay_transcript_len: None,
            replay_speaker_len: None,
            basis_unavailable: None,
            receipt_evidence: Default::default(),
            patches: Vec::with_capacity(records.len()),
            gaps,
            present,
            notes_head: None,
            graph_head: None,
        };
        for record in records {
            if record.basis_heads.keys().any(|stream| {
                !matches!(
                    stream.as_str(),
                    "transcript_revisions" | "speaker_revisions"
                )
            }) {
                return Err("Unsupported projection basis stream".into());
            }
            let lane = view.gaps.lane(&record.payload.kind);
            for cause in &record.causal_event_ids {
                if let Some(gap) = lane.get_mut(cause)
                    && record.sequence > gap.after
                {
                    gap.repaired_at.get_or_insert(record.sequence);
                }
            }
            let head = match record.payload.kind {
                ProjectionKind::Notes => &mut view.notes_head,
                ProjectionKind::Graph => &mut view.graph_head,
            };
            let position = VerifiedTranscriptHead {
                sequence: record.sequence,
                record_hash: record.record_hash,
            };
            *head = Some(position.clone());
            view.frames.push(FrameBasis {
                head: position,
                sources: record.basis_heads,
                unavailable: false,
            });
            view.patches.push(record.payload);
        }
        Ok(view)
    }

    fn repair_plan(&self) -> ProjectionRepairPlan {
        self.gaps
            .plan_at(self.frames.last().map_or(0, |row| row.head.sequence))
    }

    /// Compare each referenced physical position to the very source snapshot
    /// used for replay. Equal payloads or equal-height forks are insufficient.
    pub(crate) fn verify_source<T>(
        &self,
        stream: &str,
        records: &[CanonicalRecord<T>],
    ) -> Result<(), String> {
        if self.frames.iter().any(|row| !row.matches(stream, records)) {
            return Err("Projection basis does not match its verified source".into());
        }
        Ok(())
    }

    pub(crate) fn observe_source<T>(&mut self, stream: &str, records: &[CanonicalRecord<T>]) {
        for row in &mut self.frames {
            row.unavailable |= !row.matches(stream, records);
        }
    }

    /// Only a contiguous verified prefix can materialize sequence-dependent
    /// operations. Later, physically proven frames remain accessible as evidence.
    pub(crate) fn select_verified_prefix(
        &mut self,
        speakers: &[CanonicalRecord<super::speaker_log::CompatibleSpeakerRevisionRecord>],
    ) {
        self.observe_source("speaker_revisions", speakers);
        let Some(first) = self.frames.iter().position(|r| r.unavailable) else {
            return;
        };
        let mut withheld = ProjectionBasisUnavailable {
            first_sequence: self.frames[first].head.sequence,
            last_sequence: self.frames.last().unwrap().head.sequence,
            verified_frames: Vec::new(),
        };
        for (row, patch) in self.frames.drain(first..).zip(self.patches.drain(first..)) {
            if !row.unavailable && row.sources.contains_key("transcript_revisions") {
                withheld.verified_frames.push(VerifiedProjectionFrame {
                    sequence: row.head.sequence,
                    basis_heads: row.sources,
                    patch,
                });
            }
        }
        // A valid replacement generation may revise an old span with a timestamp
        // preceding the original patch. Replay against the retained physical
        // source history, while returning the whole new transcript to the reader.
        let last_source = |stream: &str| {
            self.frames
                .iter()
                .filter_map(|row| row.sources.get(stream))
                .map(|head| head.sequence as usize)
                .max()
        };
        self.replay_transcript_len = last_source("transcript_revisions");
        self.replay_speaker_len = last_source("speaker_revisions").map(|len| {
            speakers[..len]
                .iter()
                .map(|row| match &row.payload {
                    super::speaker_log::CompatibleSpeakerRevisionRecord::Versioned(
                        super::speaker_log::SpeakerRevisionV2::RelabelBatch(batch),
                    ) => batch.revisions.len(),
                    _ => 1,
                })
                .sum()
        });
        let last = |kind| {
            self.frames
                .iter()
                .zip(&self.patches)
                .rev()
                .find(|(_, patch)| patch.kind == kind)
                .map(|(row, _)| row.head.clone())
        };
        self.notes_head = last(ProjectionKind::Notes);
        self.graph_head = last(ProjectionKind::Graph);
        self.basis_unavailable = Some(withheld);
        log::warn!("basis_unavailable");
    }

    pub(crate) fn replay_sources(
        &self,
        transcript: &[crate::projections::TranscriptEvent],
        mut speakers: Option<Vec<crate::projections::DiarizationSpanRevision>>,
    ) -> (
        Vec<crate::projections::TranscriptEvent>,
        Option<Vec<crate::projections::DiarizationSpanRevision>>,
    ) {
        if let (Some(len), Some(rows)) = (self.replay_speaker_len, speakers.as_mut()) {
            rows.truncate(len);
        }
        (
            transcript[..self.replay_transcript_len.unwrap_or(transcript.len())].to_vec(),
            speakers,
        )
    }

    pub(crate) fn take_export_loss(&mut self) -> Option<ExportProjectionBasisLoss> {
        let basis_unavailable = self.basis_unavailable.take()?;
        Some(ExportProjectionBasisLoss {
            basis_unavailable,
            projection_notice: ProjectionExportNotice::basis_unavailable(),
        })
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
        ((notes, graph), self.repair_plan())
    }

    pub(crate) fn bind(
        &self,
        ledger: &crate::projections::TranscriptLedger,
        state: &mut MaterializedProjectionState,
    ) {
        state.notes.projection_log_head.clone_from(&self.notes_head);
        state.graph.projection_log_head.clone_from(&self.graph_head);
        let repair = self.repair_plan();
        state.notes.projection_recovery =
            (!repair.notes.is_empty()).then_some(ProjectionRecoveryStatus::RecomputeRequired);
        state.graph.projection_recovery =
            (!repair.graph.is_empty()).then_some(ProjectionRecoveryStatus::RecomputeRequired);
        if let Some(loss) = &self.basis_unavailable {
            let summary = loss.summary();
            state.notes.basis_unavailable = Some(summary.clone());
            state.graph.basis_unavailable = Some(summary);
            return;
        }
        state.notes.transcript_coverage =
            ledger.projection_coverage(self.last_basis(ProjectionKind::Notes));
        state.graph.transcript_coverage =
            ledger.projection_coverage(self.last_basis(ProjectionKind::Graph));
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ExportProjectionBasisLoss {
    pub(crate) basis_unavailable: ProjectionBasisUnavailable,
    projection_notice: ProjectionExportNotice,
}

#[derive(Debug, serde::Serialize)]
struct ProjectionExportNotice {
    en: &'static str,
    pt: &'static str,
}

impl ProjectionExportNotice {
    fn basis_unavailable() -> Self {
        Self {
            en: "Part of the source for these notes and graph changes is unavailable. The views show only changes verified before that point.",
            pt: "Parte da fonte destas alterações nas notas e no grafo está indisponível. As visualizações mostram apenas as alterações verificadas antes desse ponto.",
        }
    }
}
