//! One head-bound witness for each receipt class seen in a stream.
//! Historical and unobserved sequences never inherit a neighbouring tier.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptTierWitness {
    pub sequence: u64,
    pub record_hash: String,
    // Canonical identity (hash-derived transcript key or projection lane/sequence).
    pub event_id: String,
}

impl ReceiptTierWitness {
    pub(super) fn from_frame(frame: &PendingAppend) -> Self {
        Self {
            sequence: frame.sequence,
            record_hash: frame.record_hash.clone(),
            event_id: frame.event_id.clone(),
        }
    }

    fn valid(&self) -> bool {
        self.sequence != 0 && valid_hash(&self.record_hash) && valid_identifier(&self.event_id)
    }

    fn matches_cache(&self, cache: &CachedStreamState) -> bool {
        cache.events.get(&self.event_id).is_some_and(|record| {
            record.sequence == self.sequence && record.record_hash == self.record_hash
        })
    }

    fn matches_records<T>(&self, records: &[CanonicalRecord<T>]) -> bool {
        self.sequence
            .checked_sub(1)
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|i| records.get(i))
            .is_some_and(|row| row.sequence == self.sequence && row.record_hash == self.record_hash)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    Qualified,
    SyncedUnqualified,
    RecoveredFromUnqualifiedAttempt,
}

/// At most one observation per tier. A process exit can lose evidence about a
/// tier without changing which canonical frames are committed.
pub(super) type DeferredWitnesses = std::cell::RefCell<Vec<(Class, ReceiptTierWitness)>>;

impl Class {
    fn path(self, log: &Path) -> PathBuf {
        log.with_added_extension(match self {
            Self::Qualified => "qualified-receipt",
            Self::SyncedUnqualified => "unqualified-receipt",
            Self::RecoveredFromUnqualifiedAttempt => "recovered-unqualified-attempt",
        })
    }

    fn read(self, log: &Path) -> Result<Option<ReceiptTierWitness>, TranscriptCommitError> {
        checked_bytes_bounded(&self.path(log), 1024)?
            .map(|bytes| {
                let witness: ReceiptTierWitness = serde_json::from_slice(&bytes)
                    .map_err(|_| TranscriptCommitError::InvalidHistory)?;
                witness
                    .valid()
                    .then_some(witness)
                    .ok_or(TranscriptCommitError::InvalidHistory)
            })
            .transpose()
    }
}

/// Observations at named verified heads. Empty evidence says nothing about
/// the device that wrote earlier revisions. A later reopen never upgrades a
/// weak witness.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StreamReceiptEvidence {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified: Option<VerifiedTranscriptHead>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced_unqualified: Option<VerifiedTranscriptHead>,
    /// The interrupted descriptor originated on unqualified storage. Its matching
    /// members were kept on qualified storage; the original sync is unproven.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovered_from_unqualified_attempt: Option<VerifiedTranscriptHead>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub evidence_unavailable: bool,
}

impl StreamReceiptEvidence {
    pub fn is_empty(&self) -> bool {
        self.qualified.is_none()
            && self.synced_unqualified.is_none()
            && self.recovered_from_unqualified_attempt.is_none()
            && !self.evidence_unavailable
    }
}

/// Call with the *same* verified records as the returned load/export payload.
/// A concurrent writer may already have published a candidate beyond this
/// reader's prefix. It becomes evidence only once its head is verified.
pub(crate) fn receipt_evidence<T>(
    path: &Path,
    records: &[CanonicalRecord<T>],
) -> StreamReceiptEvidence {
    let mut evidence_unavailable = false;
    let mut observed = |tier: Class| match tier.read(path) {
        Ok(w) => w
            .filter(|w| w.matches_records(records))
            .map(|w| VerifiedTranscriptHead {
                sequence: w.sequence,
                record_hash: w.record_hash,
            }),
        Err(_) => {
            evidence_unavailable = true;
            None
        }
    };
    StreamReceiptEvidence {
        qualified: observed(Class::Qualified),
        synced_unqualified: observed(Class::SyncedUnqualified),
        recovered_from_unqualified_attempt: observed(Class::RecoveredFromUnqualifiedAttempt),
        evidence_unavailable,
    }
}

impl<E: CommitPayload> Commit<'_, E> {
    fn witness(
        &self,
        class: Class,
        next: &ReceiptTierWitness,
        prefix: &CachedStreamState,
    ) -> Result<(), TranscriptCommitError> {
        let path = class.path(self.path);
        if let Some(prior) = class.read(self.path)? {
            if &prior == next {
                // A failed rename barrier can leave exactly this witness visible.
                self.sync_entry(&path)?;
                return Ok(());
            }
            if prior.matches_cache(prefix) {
                return Ok(());
            }
            // A replaced source no longer contains the old observation.
            self.unlink(&path)?;
        }
        let contents =
            serde_json::to_vec(next).map_err(|_| TranscriptCommitError::InvalidHistory)?;
        self.publish(&path, &contents)
    }

    /// Keep the existing floor-v1 descriptor readable by its predecessor. The
    /// optional companion belongs to a fresh attempt UUID, never to a payload.
    /// It is needed only until the stream has a confirmed weak observation.
    pub(super) fn record_attempt_origin(&self, intent: &Intent, prefix: &CachedStreamState) {
        if self.try_record_attempt_origin(intent, prefix).is_err() {
            log::warn!("receipt_evidence.origin_unavailable");
        }
    }

    fn try_record_attempt_origin(
        &self,
        intent: &Intent,
        prefix: &CachedStreamState,
    ) -> Result<(), TranscriptCommitError> {
        if matches!(self.guard, TranscriptGuard::Unqualified(_))
            && !Class::SyncedUnqualified
                .read(self.path)?
                .is_some_and(|w| w.matches_cache(prefix))
        {
            self.publish(
                &self.path.with_added_extension("attempt-origin"),
                intent.attempt.as_bytes(),
            )?;
        }
        Ok(())
    }

    pub(super) fn clear_attempt_origin(&self) {
        let origin = self.path.with_added_extension("attempt-origin");
        let result = match fs::symlink_metadata(&origin) {
            Ok(_) => self.unlink(&origin),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_failure(CanonicalAppendPhase::RecoveryRead, error)),
        };
        if result.is_err() {
            log::warn!("receipt_evidence.origin_cleanup_deferred");
        }
    }

    /// Call only after the actual source barrier. For a matching interrupted
    /// weak attempt that is rewritten on a qualified volume, record the prior
    /// origin separately from this confirmed receipt.
    pub(super) fn record_barrier(
        &self,
        next: &ReceiptTierWitness,
        prefix: &CachedStreamState,
        interrupted: Option<&Intent>,
    ) {
        let mut observations = self.deferred_witnesses.take();
        observations.retain(|(_, w)| w == next || w.matches_cache(prefix));
        let mut observe = |class| {
            if !observations.iter().any(|(c, _)| *c == class) {
                observations.push((class, next.clone()));
            }
        };
        let class = match self.guard {
            TranscriptGuard::Unqualified(_) => Class::SyncedUnqualified,
            TranscriptGuard::Qualified(..) => {
                if let Some(intent) = interrupted {
                    let origin = checked_bytes_bounded(
                        &self.path.with_added_extension("attempt-origin"),
                        36,
                    );
                    match origin {
                        Ok(Some(bytes)) if bytes == intent.attempt.as_bytes() => {
                            observe(Class::RecoveredFromUnqualifiedAttempt)
                        }
                        Err(_) => log::warn!("receipt_evidence.origin_unavailable"),
                        _ => {}
                    }
                }
                Class::Qualified
            }
        };
        observe(class);
        observations.retain(|(class, witness)| self.witness(*class, witness, prefix).is_err());
        if !observations.is_empty() {
            log::warn!("receipt_evidence.write_deferred");
            self.deferred_witnesses.replace(observations);
        }
    }
}
