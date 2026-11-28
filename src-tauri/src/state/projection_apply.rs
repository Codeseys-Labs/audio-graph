//! The session's projection writer serializes prepare, append and publication.
//! A waiter may time out. The actor still owns its candidate until the storage
//! result settles; no other patch can be based on an unresolved candidate.
use super::*;
use crate::persistence::canonical_log::{
    PreparedProjectionStorage, ProjectionReceipt, TranscriptCommitError,
};
use std::sync::{Weak, mpsc};
type Save<T> = Box<dyn FnOnce(&str, &T) -> Result<(), String> + Send>;
pub(super) struct Savers {
    notes: Save<MaterializedNotes>,
    graph: Save<MaterializedGraph>,
}
impl Savers {
    pub(super) fn new<N, G>(notes: N, graph: G) -> Self
    where
        N: FnOnce(&str, &MaterializedNotes) -> Result<(), String> + Send + 'static,
        G: FnOnce(&str, &MaterializedGraph) -> Result<(), String> + Send + 'static,
    {
        Self {
            notes: Box::new(notes),
            graph: Box::new(graph),
        }
    }
}

pub(crate) type ApplyResult = Result<ProjectionRuntimeApplyResult, ProjectionRuntimeApplyError>;
pub(super) fn pending() -> ProjectionRuntimeApplyError {
    ProjectionRuntimeApplyError::ProjectionStorage {
        reason: ProjectionSaveError::ConfirmationPending,
    }
}
fn failed(error: TranscriptCommitError) -> ProjectionRuntimeApplyError {
    use TranscriptCommitError as E;
    let reason = if error.is_storage_full() {
        ProjectionSaveError::StorageFull
    } else {
        match error {
            E::InvalidRevision | E::Frame(_) | E::SessionFloorUnavailable | E::LiveHeadChanged => {
                ProjectionSaveError::Refused
            }
            E::InvalidHistory => ProjectionSaveError::InvalidHistory,
            E::UncommittedAttempt => ProjectionSaveError::Uncommitted,
            E::ConflictingAttempt | E::ConfirmationPending | E::AckTimeout | E::ReplyLost => {
                ProjectionSaveError::ConfirmationPending
            }
            _ => ProjectionSaveError::WriteFailed,
        }
    };
    ProjectionRuntimeApplyError::ProjectionStorage { reason }
}

#[derive(Clone, Debug)]
pub(super) struct ProjectionOwner(Arc<AtomicBool>);
impl PartialEq for ProjectionOwner {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ProjectionOwner {}

impl ProjectionRuntimeApplyResult {
    /// Called under the short session publication guard by the event sink.
    /// Matching both distinguishes a rotation away and back to the same ID.
    pub(crate) fn may_emit(&self, runtime: &ProjectionRuntimeHandle) -> bool {
        !self.already_accepted
            && runtime.current_session_id() == self.session_id
            && self.publication_owner.as_ref().is_some_and(|token| {
                runtime
                    .projection_event_writer
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .is_some_and(|writer| writer.owns_projection_token(&token.0))
            })
    }
}
struct Target {
    session: String,
    active: Arc<RwLock<String>>,
    ledger: Arc<Mutex<TranscriptLedger>>,
    publication: Arc<RwLock<()>>,
    slot: Weak<Mutex<Option<ProjectionEventWriter>>>,
    token: Option<Arc<AtomicBool>>,
    state: Arc<Mutex<MaterializedProjectionState>>,
}
impl Target {
    // Caller holds publication.read(). The writer token distinguishes two
    // visits to the same session id across rotation.
    fn current(&self) -> bool {
        *self.active.read().unwrap_or_else(|p| p.into_inner()) == self.session
            && self.slot.upgrade().is_some_and(|slot| {
                let owner = slot.lock().unwrap_or_else(|p| p.into_inner());
                owner
                    .as_ref()
                    .zip(self.token.as_ref())
                    .is_some_and(|(writer, token)| writer.owns_projection_token(token))
            })
    }
}

pub(crate) struct ProjectionApplyCommand {
    target: Target,
    patch: ProjectionPatch,
    repair_ids: Vec<String>,
    savers: Savers,
    reply: mpsc::Sender<ApplyResult>,
}
impl ProjectionApplyCommand {
    pub(super) fn new(
        handle: &ProjectionRuntimeHandle,
        patch: ProjectionPatch,
        session: &str,
        repair_ids: Vec<String>,
        savers: Savers,
    ) -> (Self, mpsc::Receiver<ApplyResult>) {
        let (reply, rx) = mpsc::channel();
        let target = Target {
            session: session.to_owned(),
            ledger: handle.transcript_ledger.clone(),
            active: handle.session_id.clone(),
            publication: handle.transcript_publication.clone(),
            slot: Arc::downgrade(&handle.projection_event_writer),
            token: None,
            state: handle.materialized_projection_state.clone(),
        };
        (
            Self {
                target,
                patch,
                repair_ids,
                savers,
                reply,
            },
            rx,
        )
    }
    pub(crate) fn bind(&mut self, token: Arc<AtomicBool>) {
        self.target.token = Some(token);
    }
    pub(crate) fn refuse_pending(self) {
        let _ = self.reply.send(Err(pending()));
    }

    pub(crate) fn execute(
        self,
        storage: &PreparedProjectionStorage,
    ) -> Option<PendingProjectionApply> {
        let Self {
            target,
            mut patch,
            repair_ids,
            savers,
            reply,
        } = self;
        // Use the same projection in the live view, pending receipt and replay,
        // including the model interpretation of records without placement.
        patch.provenance.normalize_stored_model();
        let (mut candidate, observed_count) = {
            let _epoch = target.publication.read().unwrap_or_else(|p| p.into_inner());
            if !target.current() {
                let actual = target
                    .active
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                let _ = reply.send(Err(
                    ProjectionRuntimeApplyError::SessionChangedDuringApply {
                        expected_session_id: target.session.clone(),
                        current_session_id: actual,
                    },
                ));
                return None;
            }
            let candidate = target
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let ledger = target.ledger.lock().unwrap_or_else(|p| p.into_inner());
            if ledger.session_id != target.session || candidate.session_id != target.session {
                let _ = reply.send(Err(ProjectionRuntimeApplyError::SessionMismatch {
                    expected_session_id: target.session.clone(),
                    current_session_id: target.session.clone(),
                    ledger_session_id: ledger.session_id.clone(),
                    materialized_session_id: candidate.session_id,
                }));
                return None;
            }
            (candidate, ledger.accepted_event_count)
        };
        // Exact retries identify the physical record even when its generated
        // timing differed. They publish neither a second event nor a new row.
        let last = match patch.kind {
            ProjectionKind::Notes => candidate.notes.last_sequence,
            ProjectionKind::Graph => candidate.graph.last_sequence,
        };
        if patch.sequence <= last {
            let result = storage
                .accepted_patch(&patch, &repair_ids, None)
                .map_err(failed)
                .and_then(|accepted| {
                    let (_, currency) = accepted.ok_or(ProjectionRuntimeApplyError::Apply {
                        error: ProjectionApplyError::StaleSequence {
                            current: last,
                            incoming: patch.sequence,
                        },
                    })?;
                    let outcome = match patch.kind {
                        ProjectionKind::Notes => MaterializedProjectionApplyOutcome::Notes {
                            last_sequence: last,
                            note_count: candidate.notes.notes.len(),
                        },
                        ProjectionKind::Graph => MaterializedProjectionApplyOutcome::Graph {
                            last_sequence: last,
                            node_count: candidate.graph.nodes.len(),
                            edge_count: candidate.graph.edges.len(),
                        },
                    };
                    Ok(ProjectionRuntimeApplyResult {
                        session_id: target.session.clone(),
                        outcome,
                        already_accepted: true,
                        publication_owner: None,
                        projection_event_enqueued: true,
                        materialized_snapshot_saved: false,
                        basis_currency_at_apply: currency,
                    })
                });
            let _ = reply.send(result);
            return None;
        }
        let started = std::time::Instant::now();
        let original = patch.clone();
        // The receipt-backed live ledger contributes its observed position.
        // Read and validate that physical prefix on the actor, outside ASR locks.
        if !patch.basis.diarization_span_revisions.is_empty() {
            let error = ProjectionApplyError::StaleBasis {
                staleness:
                    crate::projections::ProjectionBasisStaleness::DiarizationBasisUnavailable {
                        count: patch.basis.diarization_span_revisions.len(),
                    },
            };
            let _ = reply.send(Err(ProjectionRuntimeApplyError::Apply { error }));
            return None;
        }
        #[cfg(all(test, not(debug_assertions)))]
        crate::persistence::projection_profile::reset();
        let proof = match storage.live_basis(&patch, observed_count) {
            Ok(proof) => proof,
            Err(error) => {
                let _ = reply.send(Err(failed(error)));
                return None;
            }
        };
        #[cfg(all(test, not(debug_assertions)))]
        crate::persistence::projection_profile::record(|s| s.basis = started.elapsed());
        let currency = match proof.currency() {
            Ok(currency) => currency,
            Err(error) => {
                let _ = reply.send(Err(ProjectionRuntimeApplyError::Apply { error }));
                return None;
            }
        };
        let outcome =
            match candidate.apply_replayed_patch(&patch, Some(&proof.evidence_for(&patch))) {
                Ok(outcome) => outcome,
                Err(error) => {
                    let _ = reply.send(Err(ProjectionRuntimeApplyError::Apply { error }));
                    return None;
                }
            };
        patch
            .apply_latency_ms
            .get_or_insert(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
        #[cfg(all(test, not(debug_assertions)))]
        let commit_start = std::time::Instant::now();
        let mut result = storage.commit_at_lane(
            &patch,
            Some(observed_count),
            &repair_ids,
            Some(last),
            Some(proof.heads()),
        );
        #[cfg(all(test, not(debug_assertions)))]
        crate::persistence::projection_profile::record(|s| s.commit = commit_start.elapsed());
        if matches!(
            result,
            Err(TranscriptCommitError::Frame(
                crate::persistence::canonical_log::CanonicalAppendRejection::EventIdConflict
            ))
        ) && original.apply_latency_ms.is_none()
        {
            // A prior process may have already committed while the cache still
            // lags. Compare the entire original payload except generated timing.
            if let Ok(Some((verified, _))) =
                storage.accepted_patch(&original, &repair_ids, Some(last))
            {
                result = Ok(verified);
            }
        }
        let mut waiting = PendingProjectionApply {
            target,
            candidate,
            observed_count,
            patch,
            outcome,
            currency,
            savers: Some(savers),
            reply: Some(reply),
        };
        match result {
            Ok(receipt) => {
                waiting.publish(receipt);
                None
            }
            Err(error) if storage.has_pending_attempt() => {
                if error == TranscriptCommitError::FrameWriteStorageFull {
                    // The first write accepted zero bytes. Settle the caller
                    // now; the actor still owns the empty descriptor and gap.
                    waiting.respond(Err(failed(error)));
                }
                Some(waiting)
            }
            Err(error) => {
                waiting.respond(Err(failed(error)));
                None
            }
        }
    }
}

pub(crate) struct PendingProjectionApply {
    target: Target,
    candidate: MaterializedProjectionState,
    observed_count: u64,
    patch: ProjectionPatch,
    outcome: MaterializedProjectionApplyOutcome,
    currency: AppliedBasisCurrency,
    savers: Option<Savers>,
    reply: Option<mpsc::Sender<ApplyResult>>,
}
impl PendingProjectionApply {
    fn respond(&mut self, result: ApplyResult) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(result);
        }
    }

    pub(crate) fn resolve(&mut self, storage: &PreparedProjectionStorage) -> bool {
        if storage.resolve_pending().is_err() {
            return false;
        }
        if let Some(frame) = storage
            .take_recovered()
            .into_iter()
            .find(|r| r.event == self.patch)
        {
            if self.reply.is_none() {
                // Only the proven zero-byte storage-full path replies early.
                log::warn!("projection_recovery.published_after_storage_full");
            }
            self.publish(ProjectionReceipt {
                head: crate::persistence::canonical_log::VerifiedTranscriptHead {
                    sequence: frame.receipt.sequence,
                    record_hash: frame.record_hash,
                },
                already_accepted: false,
                barrier: frame.receipt.barrier,
            });
        } else {
            self.respond(Err(failed(TranscriptCommitError::UncommittedAttempt)));
        }
        true
    }

    fn publish(&mut self, receipt: ProjectionReceipt) {
        self.candidate
            .bind_projection_head(&self.patch.kind, receipt.head);
        let installed = {
            let _epoch = self
                .target
                .publication
                .read()
                .unwrap_or_else(|p| p.into_inner());
            if self.target.current() {
                let count = self
                    .target
                    .ledger
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .accepted_event_count;
                let coverage = (count != self.observed_count
                    || matches!(self.currency, AppliedBasisCurrency::AppendedTail { .. }))
                .then_some(crate::projections::ProjectionCoverageStatus::BehindTranscript);
                match self.patch.kind {
                    ProjectionKind::Notes => self.candidate.notes.transcript_coverage = coverage,
                    ProjectionKind::Graph => self.candidate.graph.transcript_coverage = coverage,
                }
                let mut live = self.target.state.lock().unwrap_or_else(|p| p.into_inner());
                match self.patch.kind {
                    ProjectionKind::Notes => live.notes.clone_from(&self.candidate.notes),
                    ProjectionKind::Graph => live.graph.clone_from(&self.candidate.graph),
                }
                true
            } else {
                false
            }
        };
        // Complete the exact session's derived cache after the canonical
        // receipt. A rotated session relies on verified replay on next load.
        let saved = if installed {
            let savers = self
                .savers
                .take()
                .expect("one projection receipt per transaction");
            let result = match self.patch.kind {
                ProjectionKind::Notes => {
                    (savers.notes)(&self.target.session, &self.candidate.notes)
                }
                ProjectionKind::Graph => {
                    (savers.graph)(&self.target.session, &self.candidate.graph)
                }
            };
            if result.is_err() {
                log::warn!("projection snapshot refresh pending");
            }
            result.is_ok()
        } else {
            false
        };
        self.respond(Ok(ProjectionRuntimeApplyResult {
            session_id: self.target.session.clone(),
            outcome: self.outcome.clone(),
            already_accepted: false,
            publication_owner: self
                .target
                .token
                .clone()
                .filter(|_| installed)
                .map(ProjectionOwner),
            projection_event_enqueued: true,
            materialized_snapshot_saved: saved,
            basis_currency_at_apply: self.currency.clone(),
        }));
    }
}
