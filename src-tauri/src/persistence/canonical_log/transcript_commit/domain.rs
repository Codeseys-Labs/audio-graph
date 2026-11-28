//! Payload rules for the shared floor-v1 commit protocol. Patch sequences are
//! monotone within notes or graph; frame sequences order the combined log.
use super::*;
use crate::projections::{ProjectionKind, ProjectionOperation};

#[cfg(test)]
thread_local! {
    pub(super) static AFTER_BASIS_CAPTURE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {std::cell::RefCell::new(None)};
}

pub(crate) trait CommitPayload:
    Serialize + DeserializeOwned + PartialEq + Clone + Send + 'static
{
    fn attempts() -> &'static Attempts<Self>;
    fn admit_basis(
        &self,
        _source: &Path,
        _session: &str,
        _through: Option<u64>,
    ) -> Result<CanonicalBasisHeadVector, TranscriptCommitError> {
        Ok(BTreeMap::new())
    }
    fn recovery_basis(
        &self,
        _source: &Path,
        _session: &str,
        _heads: &CanonicalBasisHeadVector,
    ) -> Result<bool, TranscriptCommitError> {
        Ok(true)
    }
    const STREAM: GapStream;
    fn lane(&self) -> &str;
    fn projection_kind(&self) -> Option<&crate::projections::ProjectionKind> {
        None
    }
    fn revision(&self) -> u64;
    fn event_id(&self) -> Result<String, TranscriptCommitError>;
    fn valid_for_commit(&self) -> bool;
}

impl CommitPayload for TranscriptEvent {
    fn attempts() -> &'static Attempts<Self> {
        static LIVE: OnceLock<Attempts<TranscriptEvent>> = OnceLock::new();
        LIVE.get_or_init(Default::default)
    }
    const STREAM: GapStream = GapStream::TranscriptRevisions;
    fn lane(&self) -> &str {
        &self.span_id
    }
    fn revision(&self) -> u64 {
        self.revision_number
    }
    fn event_id(&self) -> Result<String, TranscriptCommitError> {
        revision_id(self)
    }
    fn valid_for_commit(&self) -> bool {
        self.start_time.is_finite() && self.end_time.is_finite() && self.confidence.is_finite()
    }
}

impl CommitPayload for ProjectionPatch {
    fn attempts() -> &'static Attempts<Self> {
        static LIVE: OnceLock<Attempts<ProjectionPatch>> = OnceLock::new();
        LIVE.get_or_init(Default::default)
    }
    fn admit_basis(
        &self,
        source: &Path,
        session: &str,
        through: Option<u64>,
    ) -> Result<CanonicalBasisHeadVector, TranscriptCommitError> {
        projection_basis(self, source, session, through, None)?
            .map(|(heads, _)| heads)
            .ok_or(TranscriptCommitError::InvalidRevision)
    }
    fn recovery_basis(
        &self,
        source: &Path,
        session: &str,
        heads: &CanonicalBasisHeadVector,
    ) -> Result<bool, TranscriptCommitError> {
        let through = heads.get(STREAM).map_or(0, |head| head.sequence);
        Ok(projection_basis(self, source, session, Some(through), Some(heads))?.is_some())
    }
    const STREAM: GapStream = GapStream::ProjectionPatches;
    fn projection_kind(&self) -> Option<&crate::projections::ProjectionKind> {
        Some(&self.kind)
    }
    fn lane(&self) -> &str {
        match self.kind {
            ProjectionKind::Notes => "notes",
            ProjectionKind::Graph => "graph",
        }
    }
    fn revision(&self) -> u64 {
        self.sequence
    }
    fn event_id(&self) -> Result<String, TranscriptCommitError> {
        Ok(format!("projection-{}-{}", self.lane(), self.sequence))
    }
    fn valid_for_commit(&self) -> bool {
        self.sequence != 0
            && self.confidence.is_finite()
            && self.operations.iter().all(|op| match op {
                ProjectionOperation::UpsertGraphEdge { weight, .. } => weight.is_finite(),
                ProjectionOperation::StrengthenGraphEdge { weight_delta, .. }
                | ProjectionOperation::WeakenGraphEdge { weight_delta, .. } => {
                    weight_delta.is_finite()
                }
                _ => true,
            })
    }
}

impl GapStream {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::TranscriptRevisions => "transcript_revisions",
            Self::ProjectionPatches => "projection_patches",
        }
    }
    pub(crate) fn directory(self) -> &'static str {
        match self {
            Self::TranscriptRevisions => "transcripts",
            Self::ProjectionPatches => "projections",
        }
    }
}

fn pin<T>(
    records: &[CanonicalRecord<T>],
    count: usize,
    stream: &str,
    heads: &mut CanonicalBasisHeadVector,
) -> bool {
    if count == 0 {
        return true;
    }
    let Some(record) = records.get(count - 1) else {
        return false;
    };
    heads.insert(
        stream.to_owned(),
        CanonicalBasisHead {
            sequence: record.sequence,
            record_hash: record.record_hash.clone(),
            event_id: record.event_id.clone(),
        },
    );
    true
}

/// Accept an appended transcript tail while proving the semantic hash/version
/// and covered revisions at the physical prefix the materializer observed.
/// During recovery use the *recorded* positions, even if either stream grew.
pub(super) fn projection_basis(
    patch: &ProjectionPatch,
    source: &Path,
    session: &str,
    through: Option<u64>,
    expected: Option<&CanonicalBasisHeadVector>,
) -> Result<
    Option<(
        CanonicalBasisHeadVector,
        crate::projections::AppliedBasisCurrency,
    )>,
    TranscriptCommitError,
> {
    use crate::persistence::canonical_reader::{StrictCanonicalRead, load_speaker_revisions};
    use crate::projections::{
        BasisCurrency, SpeakerTimeline, TranscriptLedger, TranscriptLiveIndex,
    };
    let dir = source
        .parent()
        .and_then(Path::parent)
        .ok_or(TranscriptCommitError::InvalidAddress)?
        .join("transcripts");
    let path = dir.join(format!("{session}.events.jsonl"));
    // A hidden transcript suffix can still produce more kept frames. Do not
    // make an irreversible gap decision on a dependent stream in that state.
    let captured = capture_with_hidden_prefix(&path, session, GapStream::TranscriptRevisions, None)
        .map_err(|_| TranscriptCommitError::InvalidHistory)?;
    #[cfg(test)]
    if let Some(after) = AFTER_BASIS_CAPTURE.take() {
        after();
    }
    let unresolved = captured.as_ref().is_some_and(|copy| copy.intent.is_some());
    let mut records = captured
        .map(|s| s.verify::<TranscriptEvent>(session))
        .transpose()
        .map_err(|_| TranscriptCommitError::InvalidHistory)?
        .map(|s| s.records)
        .unwrap_or_default();
    let count = usize::try_from(through.unwrap_or(records.len() as u64))
        .map_err(|_| TranscriptCommitError::InvalidHistory)?;
    let mut heads = BTreeMap::new();
    if !pin(&records, count, STREAM, &mut heads) {
        // A descriptor hides only its suffix. The verified base can still
        // support a live patch; a missing head may be confirmed on retry.
        return if unresolved {
            Err(TranscriptCommitError::ConflictingAttempt)
        } else {
            Ok(None)
        };
    }
    records.truncate(count);
    let mut ledger = TranscriptLedger::new(session);
    let mut index = TranscriptLiveIndex::default();
    for row in records {
        index
            .apply(&mut ledger, row.payload)
            .map_err(|_| TranscriptCommitError::InvalidHistory)?;
    }
    let speaker = if patch.basis.diarization_span_revisions.is_empty() {
        None
    } else {
        let speaker_path = dir.join(format!("{session}.speaker.jsonl"));
        let StrictCanonicalRead::Present(mut snapshot) =
            load_speaker_revisions(&speaker_path, session)
                .map_err(|_| TranscriptCommitError::InvalidHistory)?
        else {
            return Ok(None);
        };
        let count = match expected {
            Some(heads) => {
                let Some(head) = heads.get("speaker_revisions") else {
                    return Ok(None);
                };
                usize::try_from(head.sequence).map_err(|_| TranscriptCommitError::InvalidHistory)?
            }
            None => snapshot.records.len(),
        };
        if !pin(&snapshot.records, count, "speaker_revisions", &mut heads) {
            return Ok(None);
        }
        snapshot.records.truncate(count);
        let events = StrictCanonicalRead::Present(snapshot)
            .speaker_history(session)
            .map_err(|_| TranscriptCommitError::InvalidHistory)?
            .unwrap_or_default();
        Some(
            SpeakerTimeline::replay(session, events)
                .map_err(|_| TranscriptCommitError::InvalidHistory)?,
        )
    };
    if expected.is_some_and(|original| original != &heads) {
        return Ok(None);
    }
    let currency = match ledger.classify_basis_currency(&patch.basis, speaker.as_ref()) {
        BasisCurrency::Current => crate::projections::AppliedBasisCurrency::Current,
        BasisCurrency::AppendOnlyStale(staleness) => {
            crate::projections::AppliedBasisCurrency::AppendedTail { staleness }
        }
        BasisCurrency::Revised(_) => return Ok(None),
    };
    Ok(Some((heads, currency)))
}
