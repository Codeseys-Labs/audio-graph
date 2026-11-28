//! Session writer's verified dependency prefixes. Only a writer-verified new head
//! certifies that extra bytes are an append to the previous hash chain.
use super::*;
use crate::persistence::{
    canonical_durability::{transcript_coordination_path, try_shared_transcript_lock},
    speaker_log::CompatibleSpeakerRevisionRecord,
};
use crate::projections::{
    AppliedBasisCurrency, BasisCurrency, ProjectionApplyError, ProjectionBasis, SpeakerTimeline,
    TranscriptLedger, TranscriptLiveIndex,
};
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

type Error = TranscriptCommitError;
type RecentHeads = VecDeque<(PathBuf, SourceStamp, CanonicalStreamHead)>;
fn published() -> &'static Mutex<RecentHeads> {
    static HEADS: OnceLock<Mutex<RecentHeads>> = OnceLock::new();
    HEADS.get_or_init(Default::default)
}
/// An evicted or externally written generation takes the full verification path.
pub(in super::super) fn remember_head(
    path: &Path,
    stamp: Option<&SourceStamp>,
    head: Option<&CanonicalStreamHead>,
) {
    let (Some(stamp), Some(head)) = (stamp, head) else {
        return;
    };
    let mut heads = published().lock().unwrap_or_else(|p| p.into_inner());
    heads.retain(|(p, _, _)| p != path);
    if heads.len() >= 64 {
        heads.pop_front();
    }
    heads.push_back((path.to_owned(), stamp.clone(), head.clone()));
}
fn published_head(path: &Path, stamp: &SourceStamp) -> Option<CanonicalStreamHead> {
    published()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|(p, s, _)| p == path && s == stamp)
        .map(|(_, _, h)| h.clone())
}

struct SourceRows<T> {
    stamp: Option<SourceStamp>,
    rows: Vec<CanonicalRecord<T>>,
    ids: HashSet<String>,
    #[cfg(test)]
    scans: usize,
    #[cfg(test)]
    appended: usize,
}
impl<T> Default for SourceRows<T> {
    fn default() -> Self {
        Self {
            stamp: None,
            rows: vec![],
            ids: HashSet::new(),
            #[cfg(test)]
            scans: 0,
            #[cfg(test)]
            appended: 0,
        }
    }
}
impl<T: DeserializeOwned> SourceRows<T> {
    fn head(&self, count: usize) -> Option<CanonicalBasisHead> {
        let r = self.rows.get(count.checked_sub(1)?)?;
        Some(CanonicalBasisHead {
            sequence: r.sequence,
            event_id: r.event_id.clone(),
            record_hash: r.record_hash.clone(),
        })
    }
    fn replace(&mut self, rows: Vec<CanonicalRecord<T>>, stamp: Option<SourceStamp>) {
        #[cfg(test)]
        {
            self.scans += 1;
        }
        self.ids = rows.iter().map(|r| r.event_id.clone()).collect();
        self.rows = rows;
        self.stamp = stamp;
    }

    /// The caller took a shared lock and ruled out an attempt. Read through
    /// this open handle after releasing that lock; a changed stamp requests
    /// a fresh coordinated snapshot. Return whether the prefix was replaced.
    fn read(
        &mut self,
        path: &Path,
        session: &str,
        stream: &str,
        version: u32,
        mut file: File,
        stamp: SourceStamp,
    ) -> Result<Option<bool>, Error> {
        if stamp.change_time.is_some() && self.stamp.as_ref() == Some(&stamp) {
            return Ok(Some(false));
        }
        let append = self
            .stamp
            .as_ref()
            .filter(|prev| {
                stamp.change_time.is_some()
                    && prev.identity == stamp.identity
                    && prev.length < stamp.length
            })
            .and_then(|prev| Some((prev.length, published_head(path, &stamp)?)));
        let start = append.as_ref().map_or(0, |(offset, _)| *offset);
        file.seek(SeekFrom::Start(start))
            .map_err(|_| Error::InvalidHistory)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(stamp.length - start)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::InvalidHistory)?;
        if bytes.len() as u64 != stamp.length - start
            || SourceStamp::read(&file).ok().as_ref() != Some(&stamp)
        {
            return Ok(None);
        }
        let anchor = append.as_ref().and_then(|_| self.rows.last()).map(|r| {
            (
                self.rows.len(),
                r.record_hash.as_str(),
                r.encoding,
                r.domain_schema_version,
            )
        });
        let has_speaker_tail =
            stream == "speaker_revisions" && !bytes.is_empty() && !bytes.ends_with(b"\n");
        if has_speaker_tail {
            if append.is_some() {
                return Ok(None);
            }
            let end = bytes
                .iter()
                .rposition(|&byte| byte == b'\n')
                .map_or(0, |at| at + 1);
            bytes.truncate(end);
        }
        let parsed = parse_structural_suffix(&bytes, session, stream, version, anchor)
            .ok()
            .and_then(|raw| raw_to_typed::<T>(raw).ok());
        if let Some((_, head)) = append {
            let Some(parsed) = parsed.filter(|s| s.head.as_ref() == Some(&head)) else {
                // A valid replacement followed by a legitimate append may
                // have the same file identity. Rebuild from the physical bytes.
                return Ok(None);
            };
            if parsed
                .records
                .iter()
                .any(|r| self.ids.contains(&r.event_id))
            {
                return Err(Error::InvalidHistory);
            }
            self.ids
                .extend(parsed.records.iter().map(|r| r.event_id.clone()));
            #[cfg(test)]
            {
                self.appended += parsed.records.len();
            }
            self.rows.extend(parsed.records);
            self.stamp = Some(stamp);
            Ok(Some(false))
        } else {
            self.replace(
                parsed.ok_or(Error::InvalidHistory)?.records,
                (!has_speaker_tail).then_some(stamp),
            );
            Ok(Some(true))
        }
    }
}

use crate::projections::{
    ProjectionBasisStaleness, TRANSCRIPT_HASH_V1_INITIAL, projection_event_is_eligible,
    update_transcript_hash_v1,
};

struct PrefixPoint {
    position: usize,
    hash: u64,
    max_revision: u64,
}
#[derive(Default)]
struct BasisIndex {
    all: Vec<PrefixPoint>,
    eligible: Vec<PrefixPoint>,
}

impl BasisIndex {
    fn update(
        points: &mut Vec<PrefixPoint>,
        ledger: &TranscriptLedger,
        from: usize,
        eligible_only: bool,
    ) {
        points.truncate(points.partition_point(|point| point.position < from));
        let mut hash = points.last().map_or(TRANSCRIPT_HASH_V1_INITIAL, |p| p.hash);
        let mut max_revision = points.last().map_or(0, |p| p.max_revision);
        for (offset, row) in ledger.latest_spans[from..].iter().enumerate() {
            if eligible_only && !projection_event_is_eligible(row) {
                continue;
            }
            update_transcript_hash_v1(&mut hash, row);
            max_revision = max_revision.max(row.revision_number);
            points.push(PrefixPoint {
                position: from + offset,
                hash,
                max_revision,
            });
        }
    }

    // The common scheduler bases name a chronological prefix of the eligible
    // transcript; approved manual patches can include partials. Prove both
    // digests and the exact exposed tail. Other shapes use the general gate.
    fn classify(
        &self,
        ledger: &TranscriptLedger,
        basis: &ProjectionBasis,
    ) -> Option<(BasisCurrency, usize, bool)> {
        let count = basis.covered_span_count();
        for (points, eligible) in [(&self.eligible, true), (&self.all, false)] {
            if count > points.len() || (eligible && count == 0) {
                continue;
            }
            let hash = count
                .checked_sub(1)
                .map_or(TRANSCRIPT_HASH_V1_INITIAL, |n| points[n].hash);
            if basis.transcript_hash != format!("fnv1a64:{hash:016x}") {
                continue;
            }
            let mut start = 0;
            if let Some(prefix) = &basis.covered_prefix {
                if prefix.span_count == 0 || prefix.span_count >= count {
                    continue;
                }
                let prefix_hash = points[prefix.span_count - 1].hash;
                if prefix.content_hash != format!("fnv1a64:{prefix_hash:016x}") {
                    continue;
                }
                start = prefix.span_count;
            }
            if let Some(revision) = basis.summarized_through_revision
                && count > crate::projections::ROLLING_SUMMARY_HOT_WINDOW_TURNS
                && points[count - crate::projections::ROLLING_SUMMARY_HOT_WINDOW_TURNS - 1]
                    .max_revision
                    != revision
            {
                continue;
            }
            let mut tail: Vec<_> = points[start..count]
                .iter()
                .map(|p| &ledger.latest_spans[p.position])
                .collect();
            tail.sort_unstable_by(|a, b| a.span_id.cmp(&b.span_id));
            if !tail.iter().zip(&basis.span_revisions).all(|(row, span)| {
                row.span_id == span.span_id && row.revision_number == span.revision_number
            }) {
                continue;
            }
            let currency = if let Some(next) = points.get(count) {
                let row = &ledger.latest_spans[next.position];
                BasisCurrency::AppendOnlyStale(ProjectionBasisStaleness::MissingCurrentSpan {
                    span_id: row.span_id.clone(),
                    current_revision: row.revision_number,
                })
            } else {
                BasisCurrency::Current
            };
            return Some((
                currency,
                points
                    .get(count.saturating_sub(1))
                    .map_or(0, |p| if count == 0 { 0 } else { p.position + 1 }),
                eligible,
            ));
        }
        None
    }
}
fn anchors(patch: &ProjectionPatch) -> impl Iterator<Item = &str> {
    patch
        .operations
        .iter()
        .filter_map(|op| match op {
            ProjectionOperation::UpsertNote { evidence, .. }
            | ProjectionOperation::UpsertGraphNode { evidence, .. }
            | ProjectionOperation::UpsertGraphEdge { evidence, .. } => evidence.span_id.as_deref(),
            _ => None,
        })
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

#[derive(Default)]
pub(crate) struct BasisCache {
    transcript: SourceRows<TranscriptEvent>,
    speaker: SourceRows<CompatibleSpeakerRevisionRecord>,
    ledger: Option<(TranscriptLedger, TranscriptLiveIndex, usize)>,
    timeline: Option<(SpeakerTimeline, usize)>,
    semantics: BasisIndex,
    proof: Option<Arc<Proof>>,
}
pub(crate) struct Proof {
    basis: ProjectionBasis,
    heads: CanonicalBasisHeadVector,
    currency: BasisCurrency,
    evidence: HashMap<String, Option<TranscriptEvent>>,
}
impl Proof {
    pub(crate) fn heads(&self) -> &CanonicalBasisHeadVector {
        &self.heads
    }
    pub(crate) fn currency(&self) -> Result<AppliedBasisCurrency, ProjectionApplyError> {
        match &self.currency {
            BasisCurrency::Current => Ok(AppliedBasisCurrency::Current),
            BasisCurrency::AppendOnlyStale(staleness) => Ok(AppliedBasisCurrency::AppendedTail {
                staleness: staleness.clone(),
            }),
            BasisCurrency::Revised(staleness) => Err(ProjectionApplyError::StaleBasis {
                staleness: staleness.clone(),
            }),
        }
    }
    pub(crate) fn evidence_for<'a>(
        &'a self,
        patch: &ProjectionPatch,
    ) -> BTreeMap<&'a str, &'a TranscriptEvent> {
        anchors(patch)
            .filter_map(|id| {
                self.evidence
                    .get(id)?
                    .as_ref()
                    .map(|e| (e.span_id.as_str(), e))
            })
            .collect()
    }
}
impl BasisCache {
    #[cfg(test)]
    pub(in super::super) fn source_work(&self) -> [(usize, usize); 2] {
        [
            (self.transcript.scans, self.transcript.appended),
            (self.speaker.scans, self.speaker.appended),
        ]
    }
    fn transcript(&mut self, path: &Path, session: &str) -> Result<bool, Error> {
        // Missing locks (including legacy roots) and active intents use the
        // same two-descriptor capture as strict historical reads.
        let lock_path = transcript_coordination_path(path);
        let attempt = path.with_added_extension("attempt");
        let has_attempt = || attempt.try_exists().map_err(|_| Error::InvalidHistory);
        // Ordinary appends keep the incremental path even when a flush lasts
        // longer than a scheduling slice. Only a truly stalled writer with a
        // published intent needs the strict, double-checked base snapshot.
        let guard = match try_shared_transcript_lock(&lock_path) {
            Err(CanonicalCoordinationError::Contended) => {
                let result = crate::persistence::canonical_durability::wait_for_transcript_lock(
                    &lock_path, true, false,
                );
                if matches!(result, Err(CanonicalCoordinationError::Contended)) && !has_attempt()? {
                    return Err(Error::ConflictingAttempt);
                }
                result
            }
            result => result,
        };
        if let Ok(guard) = guard
            && !has_attempt()?
            && let Ok(file) = File::open(path)
            && let Ok(stamp) = SourceStamp::read(&file)
        {
            drop(guard);
            if let Some(reset) = self
                .transcript
                .read(path, session, STREAM, 1, file, stamp)?
            {
                if reset {
                    self.ledger = None;
                }
                return Ok(false);
            }
        }
        let captured =
            capture_with_hidden_prefix(path, session, GapStream::TranscriptRevisions, None)
                .map_err(|error| {
                    match error {
            crate::persistence::canonical_reader::CanonicalReaderError::TranscriptLockContended => {
                Error::ConflictingAttempt
            }
            _ => Error::InvalidHistory,
        }
                })?;
        let hidden = captured.as_ref().is_some_and(|c| c.intent.is_some());
        let rows = captured
            .map(|c| c.verify(session))
            .transpose()
            .map_err(|_| Error::InvalidHistory)?
            .map(|s| s.records)
            .unwrap_or_default();
        self.transcript.replace(rows, None);
        self.ledger = None;
        Ok(hidden)
    }
    fn speaker(&mut self, path: &Path, session: &str) -> Result<(), Error> {
        for _ in 0..2 {
            let mut file = match File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.speaker.replace(Vec::new(), None);
                    self.timeline = None;
                    return Ok(());
                }
                Err(_) => return Err(Error::InvalidHistory),
            };
            file.try_lock_shared().map_err(|error| match error {
                std::fs::TryLockError::WouldBlock => Error::ConflictingAttempt,
                std::fs::TryLockError::Error(_) => Error::InvalidHistory,
            })?;
            if let Ok(stamp) = SourceStamp::read(&file) {
                file.unlock().map_err(|_| Error::InvalidHistory)?;
                if let Some(reset) =
                    self.speaker
                        .read(path, session, "speaker_revisions", 2, file, stamp)?
                {
                    if reset {
                        self.timeline = None;
                    }
                    return Ok(());
                }
                // Reacquire and cold-verify if the file changed during capture
                // or the verified writer head no longer extends our prefix.
                self.speaker.stamp = None;
            } else {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|_| Error::InvalidHistory)?;
                drop(file);
                let end = bytes
                    .iter()
                    .rposition(|&byte| byte == b'\n')
                    .map_or(0, |at| at + 1);
                let raw = parse_structural_records(&bytes[..end], session, "speaker_revisions", 2)
                    .map_err(|_| Error::InvalidHistory)?;
                let snapshot = raw_to_typed(raw).map_err(|_| Error::InvalidHistory)?;
                self.speaker.replace(snapshot.records, None);
                self.timeline = None;
                return Ok(());
            }
        }
        Err(Error::ConflictingAttempt)
    }
    fn fold_transcript(&mut self, session: &str, count: usize) -> Result<(), Error> {
        if self.ledger.as_ref().is_some_and(|(_, _, n)| *n > count) {
            self.ledger = None;
        }
        let (ledger, index, done) = self.ledger.get_or_insert_with(|| {
            (
                TranscriptLedger::new(session),
                TranscriptLiveIndex::default(),
                0,
            )
        });
        let mut changed = if *done == 0 {
            0
        } else {
            ledger.latest_spans.len()
        };
        for r in &self.transcript.rows[*done..count] {
            let id = r.payload.span_id.as_str();
            changed = changed.min(index.position(id).unwrap_or(ledger.latest_spans.len()));
            if index.apply(ledger, r.payload.clone()).is_err() {
                // A later projection may ask for an earlier, still-valid
                // prefix. Restart its fold rather than retaining half a delta.
                self.ledger = None;
                return Err(Error::InvalidHistory);
            }
            changed = changed.min(index.position(id).expect("applied span exists"));
            *done += 1;
        }
        BasisIndex::update(&mut self.semantics.all, ledger, changed, false);
        BasisIndex::update(&mut self.semantics.eligible, ledger, changed, true);
        Ok(())
    }

    fn verified(
        &mut self,
        patch: &ProjectionPatch,
        source: &Path,
        session: &str,
        through: Option<u64>,
        expected: Option<&CanonicalBasisHeadVector>,
    ) -> Result<Option<Arc<Proof>>, Error> {
        let dir = source
            .parent()
            .and_then(Path::parent)
            .ok_or(Error::InvalidAddress)?
            .join("transcripts");
        let hidden = self.transcript(&dir.join(format!("{session}.events.jsonl")), session)?;
        #[cfg(test)]
        if let Some(after) = AFTER_BASIS_CAPTURE.take() {
            after();
        }
        let count = usize::try_from(through.unwrap_or(self.transcript.rows.len() as u64))
            .map_err(|_| Error::InvalidHistory)?;
        if count > self.transcript.rows.len() {
            return if hidden {
                Err(Error::ConflictingAttempt)
            } else {
                Ok(None)
            };
        }
        let mut heads = CanonicalBasisHeadVector::new();
        if let Some(h) = self.transcript.head(count) {
            heads.insert(STREAM.into(), h);
        }
        let mut sp_count = None;
        if !patch.basis.diarization_span_revisions.is_empty() {
            self.speaker(&dir.join(format!("{session}.speaker.jsonl")), session)?;
            let n = match expected {
                Some(heads) => {
                    let Some(h) = heads.get("speaker_revisions") else {
                        return Ok(None);
                    };
                    usize::try_from(h.sequence).map_err(|_| Error::InvalidHistory)?
                }
                None => self.speaker.rows.len(),
            };
            let Some(h) = self.speaker.head(n) else {
                return Ok(None);
            };
            heads.insert("speaker_revisions".into(), h);
            sp_count = Some(n);
        }
        if expected.is_some_and(|e| e != &heads) {
            return Ok(None);
        }
        if let Some(proof) = &self.proof
            && proof.heads == heads
            && proof.basis == patch.basis
            && anchors(patch).all(|id| proof.evidence.contains_key(id))
        {
            return Ok(Some(proof.clone()));
        }
        self.fold_transcript(session, count)?;
        let (ledger, index, _) = self.ledger.as_ref().expect("verified transcript folded");
        let timeline = if let Some(n) = sp_count {
            if self.timeline.as_ref().is_some_and(|(_, done)| *done > n) {
                self.timeline = None;
            }
            let (t, done) = self
                .timeline
                .get_or_insert_with(|| (SpeakerTimeline::new(session), 0));
            for r in &self.speaker.rows[*done..n] {
                r.payload
                    .clone()
                    .fold(r.domain_schema_version, &r.event_id, Some(t), None)
                    .map_err(|_| Error::InvalidHistory)?;
                *done += 1;
            }
            Some(&*t)
        } else {
            None
        };
        let (currency, evidence) = if let Some(stale) =
            timeline.and_then(|t| t.validate_diarization_basis(&patch.basis).err())
        {
            (BasisCurrency::Revised(stale), HashMap::new())
        } else if let Some((currency, end, eligible)) =
            self.semantics.classify(ledger, &patch.basis)
        {
            let winners = timeline
                .filter(|_| anchors(patch).next().is_some())
                .map(crate::timeline::speaker_attribution_index);
            let evidence = anchors(patch)
                .map(|id| {
                    let row = index
                        .position(id)
                        .filter(|pos| *pos < end)
                        .map(|pos| &ledger.latest_spans[pos])
                        .filter(|r| !eligible || projection_event_is_eligible(r));
                    let event = row.cloned().map(|row| {
                        crate::projections::overlay_evidence_speaker(row, winners.as_ref())
                    });
                    (id.to_owned(), event)
                })
                .collect();
            (currency, evidence)
        } else {
            let currency = ledger.classify_basis_currency(&patch.basis, timeline);
            let mut evidence: HashMap<String, Option<TranscriptEvent>> =
                anchors(patch).map(|id| (id.to_owned(), None)).collect();
            if !matches!(currency, BasisCurrency::Revised(_)) && !evidence.is_empty() {
                for event in crate::projections::resolve_claim_evidence_basis_events(
                    &patch.basis,
                    ledger,
                    timeline,
                ) {
                    if let Some(slot) = evidence.get_mut(&event.span_id) {
                        *slot = Some(event)
                    }
                }
            }
            (currency, evidence)
        };
        let proof = Arc::new(Proof {
            basis: patch.basis.clone(),
            heads,
            currency,
            evidence,
        });
        self.proof = Some(proof.clone());
        Ok(Some(proof))
    }
}
pub(in super::super) fn projection_basis(
    patch: &ProjectionPatch,
    source: &Path,
    session: &str,
    through: Option<u64>,
    expected: Option<&CanonicalBasisHeadVector>,
    cache: &RefCell<BasisCache>,
) -> Result<Option<(CanonicalBasisHeadVector, AppliedBasisCurrency)>, Error> {
    Ok(cache
        .borrow_mut()
        .verified(patch, source, session, through, expected)?
        .and_then(|p| Some((p.heads.clone(), p.currency().ok()?))))
}
impl PreparedProjectionStorage {
    pub(crate) fn live_basis(
        &self,
        patch: &ProjectionPatch,
        count: u64,
    ) -> Result<Arc<Proof>, Error> {
        self.source_basis
            .borrow_mut()
            .verified(patch, &self.path, &self.session, Some(count), None)?
            .ok_or(Error::InvalidRevision)
    }
}
