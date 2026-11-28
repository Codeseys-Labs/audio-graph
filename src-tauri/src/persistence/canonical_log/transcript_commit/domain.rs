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
        _expected: Option<&CanonicalBasisHeadVector>,
        _cache: &std::cell::RefCell<BasisCache>,
    ) -> Result<CanonicalBasisHeadVector, TranscriptCommitError> {
        Ok(BTreeMap::new())
    }
    fn recovery_basis(
        &self,
        _source: &Path,
        _session: &str,
        _heads: &CanonicalBasisHeadVector,
        _cache: &std::cell::RefCell<BasisCache>,
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
        expected: Option<&CanonicalBasisHeadVector>,
        cache: &std::cell::RefCell<BasisCache>,
    ) -> Result<CanonicalBasisHeadVector, TranscriptCommitError> {
        projection_basis(self, source, session, through, expected, cache)?
            .map(|(heads, _)| heads)
            .ok_or(TranscriptCommitError::InvalidRevision)
    }
    fn recovery_basis(
        &self,
        source: &Path,
        session: &str,
        heads: &CanonicalBasisHeadVector,
        cache: &std::cell::RefCell<BasisCache>,
    ) -> Result<bool, TranscriptCommitError> {
        let through = heads.get(STREAM).map_or(0, |head| head.sequence);
        Ok(projection_basis(self, source, session, Some(through), Some(heads), cache)?.is_some())
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

mod basis_cache;
pub(crate) use basis_cache::BasisCache;
pub(super) use basis_cache::{projection_basis, remember_head};
