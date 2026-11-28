//! Receipt-bearing commits for the legacy-v1 transcript payload.
use super::*;
use crate::persistence::canonical_durability::{
    CanonicalCoordinationError, CanonicalDurabilityBarrier, CanonicalExclusiveGuard,
    CanonicalFilesystemQualification, CanonicalFilesystemQualificationError,
    CanonicalSnapshotExpectation, CanonicalUnlinkOutcome,
};
use crate::projections::{TranscriptEvent, TranscriptLedger};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use uuid::Uuid;

const STREAM: &str = "transcript_revisions";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptCommitError {
    WriterAbsent,
    QueueFull,
    ReplyLost,
    UnsupportedRepository,
    InvalidAddress,
    SessionFloorUnavailable,
    InvalidHistory,
    LiveHeadChanged,
    ConflictingAttempt,
    InvalidRevision,
    Qualification(CanonicalFilesystemQualificationError),
    Coordination(CanonicalCoordinationError),
    Rejected(CanonicalDurabilityRejection),
    Indeterminate(CanonicalDurabilityIndeterminate),
    Frame(CanonicalAppendRejection),
    Io {
        phase: CanonicalAppendPhase,
        kind: io::ErrorKind,
        code: Option<i32>,
    },
}

impl TranscriptCommitError {
    pub(crate) fn is_storage_full(&self) -> bool {
        let (kind, code) = match self {
            Self::Io { kind, code, .. } => (*kind, *code),
            Self::Indeterminate(e) => (e.kind, e.raw_os_error),
            Self::Rejected(CanonicalDurabilityRejection::IoFailedBeforeMutation {
                kind,
                raw_os_error,
                ..
            })
            | Self::Qualification(CanonicalFilesystemQualificationError::Io {
                kind,
                raw_os_error,
                ..
            }) => (*kind, *raw_os_error),
            _ => return false,
        };
        kind == io::ErrorKind::StorageFull
            || code
                .is_some_and(|n| crate::events::is_storage_full(&io::Error::from_raw_os_error(n)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptReceipt {
    pub sequence: u64,
    pub already_accepted: bool,
    pub barrier: CanonicalDurabilityBarrier,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerifiedTranscriptHead {
    pub sequence: u64,
    pub record_hash: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GapStream {
    TranscriptRevisions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TranscriptGap {
    pub stream: GapStream,
    pub after: Option<VerifiedTranscriptHead>,
    pub suffix_quarantined: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    attempt: String,
    owner: String,
    after: Option<VerifiedTranscriptHead>,
    base_len: u64,
    source_was_absent: bool,
    frame_len: u64,
    frame_hash: String,
}

fn process_owner() -> &'static str {
    static OWNER: OnceLock<String> = OnceLock::new();
    OWNER.get_or_init(|| Uuid::new_v4().to_string())
}
fn io_failure(phase: CanonicalAppendPhase, error: io::Error) -> TranscriptCommitError {
    TranscriptCommitError::Io {
        phase,
        kind: error.kind(),
        code: error.raw_os_error(),
    }
}
fn head(cache: &CachedStreamState) -> Option<VerifiedTranscriptHead> {
    cache.head.as_ref().map(|h| VerifiedTranscriptHead {
        sequence: h.sequence,
        record_hash: h.record_hash.clone(),
    })
}
fn checked_bytes(path: &Path) -> Result<Option<Vec<u8>>, TranscriptCommitError> {
    checked_bytes_bounded(path, u64::MAX)
}

fn checked_bytes_bounded(
    path: &Path,
    limit: u64,
) -> Result<Option<Vec<u8>>, TranscriptCommitError> {
    match fs::symlink_metadata(path) {
        Ok(meta)
            if meta.len() <= limit
                && meta.is_file()
                && !crate::sessions::is_reparse_point(&meta) => {}
        Ok(_) => return Err(TranscriptCommitError::InvalidHistory),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_failure(CanonicalAppendPhase::RecoveryRead, e)),
    }
    let mut bytes = Vec::new();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_failure(CanonicalAppendPhase::RecoveryRead, error)),
    };
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?;
    if bytes.len() as u64 > limit {
        return Err(TranscriptCommitError::InvalidHistory);
    }
    Ok(Some(bytes))
}

// Keep the owner lock and the exact attempted bytes across an uncertain
// return. A different live process must not treat a temporarily idle writer
// as a crashed writer. Kernel lock release on exit is the restart witness.
struct LiveAttempt {
    file: File,
    frame: Vec<u8>,
    payload: String,
}
type Attempts = std::sync::Mutex<HashMap<PathBuf, LiveAttempt>>;
fn attempts() -> &'static Attempts {
    static LIVE: OnceLock<Attempts> = OnceLock::new();
    LIVE.get_or_init(Default::default)
}
struct Lease {
    path: PathBuf,
    file: Option<File>,
    pending: Option<(Vec<u8>, String)>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let (Some(file), Some((frame, payload))) = (self.file.take(), self.pending.take()) {
            attempts().lock().unwrap_or_else(|p| p.into_inner()).insert(
                self.path.clone(),
                LiveAttempt {
                    file,
                    frame,
                    payload,
                },
            );
        }
    }
}
struct Commit<'a> {
    path: &'a Path,
    session: &'a str,
    guard: CanonicalExclusiveGuard,
    qualification: CanonicalFilesystemQualification,
    key: CanonicalRecoveryKey,
}

fn barrier(
    outcome: NamespaceDurabilityOutcome,
) -> Result<CanonicalDurabilityBarrier, TranscriptCommitError> {
    match outcome {
        NamespaceDurabilityOutcome::Accepted(r) => Ok(r.barrier),
        NamespaceDurabilityOutcome::Rejected(r) => Err(TranscriptCommitError::Rejected(r)),
        NamespaceDurabilityOutcome::DurabilityIndeterminate(r) => {
            Err(TranscriptCommitError::Indeterminate(r))
        }
    }
}

impl<'a> Commit<'a> {
    fn open(path: &'a Path, session: &'a str) -> Result<Self, TranscriptCommitError> {
        let dir = path.parent().ok_or(TranscriptCommitError::InvalidAddress)?;
        let (qualification, durability) =
            CanonicalFilesystemQualification::for_existing_managed_root(dir)
                .map_err(TranscriptCommitError::Qualification)?;
        let guard = durability
            .try_lock_exclusive_qualified(dir, &qualification)
            .map_err(TranscriptCommitError::Coordination)?;
        Ok(Self {
            path,
            session,
            guard,
            qualification,
            key: CanonicalRecoveryKey::from_opaque_bytes(*Uuid::new_v4().as_bytes()),
        })
    }
    fn unlink(&self, path: &Path) -> Result<(), TranscriptCommitError> {
        match self
            .guard
            .unlink_canonical_entry(path, Some(&self.qualification), self.key)
        {
            CanonicalUnlinkOutcome::Unlinked(_) | CanonicalUnlinkOutcome::AlreadyAbsent(_) => {
                Ok(())
            }
            CanonicalUnlinkOutcome::Rejected(e) => Err(TranscriptCommitError::Rejected(e)),
            CanonicalUnlinkOutcome::DurabilityIndeterminate(e) => {
                Err(TranscriptCommitError::Indeterminate(e))
            }
        }
    }
    fn publish(&self, path: &Path, bytes: &[u8]) -> Result<(), TranscriptCommitError> {
        let staging = self.path.with_added_extension("attempt-stage");
        // A staged file alone never authorized a source mutation. It may be
        // left by a killed publisher, including a partially written staging file.
        if staging
            .try_exists()
            .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
        {
            self.unlink(&staging)?;
        }
        if let Some(existing) = checked_bytes_bounded(path, bytes.len() as u64)? {
            if existing != bytes {
                return Err(TranscriptCommitError::InvalidHistory);
            }
            barrier(self.guard.sync_recovery_namespace_entry(
                path,
                Some(&self.qualification),
                self.key,
            ))?;
        } else {
            barrier(self.guard.install_manifest_snapshot(
                &staging,
                path,
                bytes,
                &mut CanonicalSnapshotExpectation::Absent,
                Some(&self.qualification),
                self.key,
                false,
                None,
            ))?;
        }
        Ok(())
    }
    fn cache(
        &self,
        bytes: &[u8],
    ) -> Result<(CachedStreamState, TranscriptLedger), TranscriptCommitError> {
        let raw = parse_structural_records(bytes, self.session, STREAM, 1)
            .map_err(|_| TranscriptCommitError::InvalidHistory)?;
        let mut cache = cache_from_raw::<TranscriptEvent>(&raw)
            .map_err(|_| TranscriptCommitError::InvalidHistory)?;
        let mut ledger = TranscriptLedger::new(self.session);
        for row in raw.records {
            let event: TranscriptEvent = serde_json::from_value(row.payload)
                .map_err(|_| TranscriptCommitError::InvalidHistory)?;
            // Historical JSONL has positional IDs. A redelivery must resolve
            // to that existing sequence, even after a later revision.
            let id = revision_id(&event)?;
            if id != row.event_id {
                let payload = canonicalize_json_value(
                    serde_json::to_value(&event)
                        .map_err(|_| TranscriptCommitError::InvalidHistory)?,
                );
                let hash =
                    payload_digest(&payload).map_err(|_| TranscriptCommitError::InvalidHistory)?;
                let commitment = event_commitment_hash(
                    self.session,
                    STREAM,
                    1,
                    &id,
                    &[],
                    &Default::default(),
                    &hash,
                )
                .map_err(|_| TranscriptCommitError::InvalidHistory)?;
                cache.events.entry(id).or_insert_with(|| CachedEvent {
                    sequence: row.sequence,
                    record_hash: row.record_hash,
                    payload_hash: hash,
                    commitment_hash: commitment,
                });
            }
            ledger
                .apply_event(event)
                .map_err(|_| TranscriptCommitError::InvalidHistory)?;
        }
        Ok((cache, ledger))
    }
    fn run(
        &self,
        event: Option<&TranscriptEvent>,
        expected_head: Option<u64>,
        owner: &str,
    ) -> Result<Option<TranscriptReceipt>, TranscriptCommitError> {
        let lock = self.path.with_added_extension("attempt-lock");
        let lock_key =
            fs::canonicalize(lock.parent().ok_or(TranscriptCommitError::InvalidAddress)?)
                .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
                .join(
                    lock.file_name()
                        .ok_or(TranscriptCommitError::InvalidAddress)?,
                );
        let retained = attempts()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&lock_key);
        let mut lease = if let Some(attempt) = retained {
            Lease {
                path: lock_key,
                file: Some(attempt.file),
                pending: Some((attempt.frame, attempt.payload)),
            }
        } else {
            if !lock
                .try_exists()
                .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
            {
                barrier(
                    self.guard
                        .append(&lock, b"", Some(&self.qualification), self.key),
                )?;
            }
            let file = self
                .guard
                .open_recovery_source(&lock)
                .map_err(TranscriptCommitError::Rejected)?;
            file.try_lock().map_err(|e| match e {
                TryLockError::WouldBlock => TranscriptCommitError::ConflictingAttempt,
                TryLockError::Error(e) => io_failure(CanonicalAppendPhase::RecoveryRead, e),
            })?;
            Lease {
                path: lock_key,
                file: Some(file),
                pending: None,
            }
        };
        let payload_digest = event
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| TranscriptCommitError::InvalidHistory)?
            .map(|bytes| digest(&bytes));
        if let Some((_, previous)) = &lease.pending
            && Some(previous) != payload_digest.as_ref()
        {
            return Err(TranscriptCommitError::ConflictingAttempt);
        }
        let intent_path = self.path.with_added_extension("attempt");
        let intent: Option<Intent> = checked_bytes_bounded(&intent_path, 4096)?
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()
            .map_err(|_| TranscriptCommitError::InvalidHistory)?;
        let mut source = match self.guard.open_recovery_source(self.path) {
            Ok(file) => Some(file),
            Err(CanonicalDurabilityRejection::IoFailedBeforeMutation {
                kind: io::ErrorKind::NotFound,
                ..
            }) => None,
            Err(e) => return Err(TranscriptCommitError::Rejected(e)),
        };
        let mut bytes = Vec::new();
        if let Some(file) = &mut source {
            file.read_to_end(&mut bytes)
                .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?;
        }
        let mut live_attempt = None;
        if let Some(intent) = intent {
            let index = usize::try_from(intent.base_len)
                .map_err(|_| TranscriptCommitError::InvalidHistory)?;
            let (prefix, suffix) = bytes
                .split_at_checked(index)
                .ok_or(TranscriptCommitError::InvalidHistory)?;
            let (base, _) = self.cache(prefix)?;
            if (!intent.source_was_absent && source.is_none())
                || (intent.source_was_absent && intent.base_len != 0)
                || head(&base) != intent.after
                || !valid_attempt(&intent.attempt)
                || !valid_attempt(&intent.owner)
                || !valid_hash(&intent.frame_hash)
                || intent.frame_len == 0
            {
                return Err(TranscriptCommitError::InvalidHistory);
            }
            if suffix.len() as u64 == intent.frame_len && digest(suffix) == intent.frame_hash {
                let (end, _) = self.cache(&bytes)?;
                if end.head.as_ref().map(|h| h.sequence)
                    != Some(
                        base.head
                            .as_ref()
                            .map_or(0, |h| h.sequence)
                            .checked_add(1)
                            .ok_or(TranscriptCommitError::InvalidHistory)?,
                    )
                {
                    return Err(TranscriptCommitError::InvalidHistory);
                }

                #[cfg(test)]
                if CUT.get() == Some(Cut::RecoverySync) {
                    CUT.set(None);
                    return Err(io_failure(
                        CanonicalAppendPhase::RecoverySync,
                        io::ErrorKind::Other.into(),
                    ));
                }
                self.sync_source(
                    source
                        .as_ref()
                        .ok_or(TranscriptCommitError::InvalidHistory)?,
                )?;
                self.unlink(&intent_path)?;
                lease.pending = None;
            } else if suffix.len() as u64 >= intent.frame_len
                || suffix
                    .strip_prefix(b"\n")
                    .unwrap_or(suffix)
                    .contains(&b'\n')
            {
                return Err(TranscriptCommitError::InvalidHistory);
            } else if intent.owner == owner {
                if event.is_none() {
                    return Err(TranscriptCommitError::ConflictingAttempt);
                }
                live_attempt = Some(intent);
            } else {
                self.recover_gap(&intent, suffix, source.as_ref())?;
                if intent.source_was_absent {
                    drop(source.take());
                    self.unlink(self.path)?;
                }
                bytes.truncate(index);
                self.unlink(&intent_path)?;
            }
        }
        let base_len = live_attempt
            .as_ref()
            .map_or(bytes.len(), |i| i.base_len as usize);
        let (cache, mut ledger) = self.cache(&bytes[..base_len])?;
        let Some(event) = event else {
            return Ok(None);
        };
        // JSON encodes non-finite floats as null. Required numeric fields must
        // round-trip through the replay decoder before a frame can commit.
        if !event.start_time.is_finite()
            || !event.end_time.is_finite()
            || !event.confidence.is_finite()
        {
            return Err(TranscriptCommitError::InvalidRevision);
        }
        let metadata = CanonicalEventMetadata::new(revision_id(event)?);
        let basis = FrameBasis {
            session_id: self.session,
            stream_id: STREAM,
            domain_schema_version: 1,
            head: cache.head.as_ref(),
            events: &cache.events,
            byte_len: cache.byte_len,
            ends_with_newline: cache.ends_with_newline,
            poisoned: None,
        };
        let frame = match basis.plan(&metadata, event) {
            PlannedAppend::New(frame) => {
                if expected_head
                    .is_some_and(|previous| previous.checked_add(1) != Some(frame.sequence))
                {
                    return Err(TranscriptCommitError::LiveHeadChanged);
                }
                ledger
                    .apply_event(event.clone())
                    .map_err(|_| TranscriptCommitError::InvalidRevision)?;
                frame
            }
            PlannedAppend::Already(r) if live_attempt.is_none() => {
                if expected_head.is_some_and(|previous| r.sequence > previous.saturating_add(1)) {
                    return Err(TranscriptCommitError::LiveHeadChanged);
                }
                let namespace = self.sync_source(
                    source
                        .as_ref()
                        .ok_or(TranscriptCommitError::InvalidHistory)?,
                )?;
                lease.pending = None;
                return Ok(Some(TranscriptReceipt {
                    sequence: r.sequence,
                    already_accepted: true,
                    barrier: namespace,
                }));
            }
            PlannedAppend::Rejected(e) => return Err(TranscriptCommitError::Frame(e)),
            _ => return Err(TranscriptCommitError::ConflictingAttempt),
        };
        let frame_hash = digest(&frame.frame);
        let suffix = &bytes[base_len..];
        if let Some(intent) = live_attempt {
            if intent.frame_hash != frame_hash
                || !frame.frame.starts_with(suffix)
                || lease
                    .pending
                    .as_ref()
                    .is_some_and(|(original, _)| original != &frame.frame)
            {
                return Err(TranscriptCommitError::ConflictingAttempt);
            }
        } else {
            let intent = Intent {
                attempt: Uuid::new_v4().to_string(),
                owner: owner.to_owned(),
                after: head(&cache),
                source_was_absent: source.is_none(),
                base_len: cache.byte_len,
                frame_len: frame.frame.len() as u64,
                frame_hash,
            };
            let descriptor =
                serde_json::to_vec(&intent).map_err(|_| TranscriptCommitError::InvalidHistory)?;
            lease.pending = Some((
                frame.frame.clone(),
                payload_digest.ok_or(TranscriptCommitError::InvalidHistory)?,
            ));
            self.publish(&intent_path, &descriptor)?;
        }
        let mut file = match source {
            Some(file) => file,
            None => {
                self.publish(self.path, b"")?;
                self.guard
                    .open_recovery_source(self.path)
                    .map_err(TranscriptCommitError::Rejected)?
            }
        };
        self.guard
            .revalidate_recovery_source(self.path, &file)
            .map_err(TranscriptCommitError::Rejected)?;
        let remaining = &frame.frame[suffix.len()..];
        file.seek(SeekFrom::End(0))
            .map_err(|e| io_failure(CanonicalAppendPhase::Write, e))?;
        write_frame(&mut file, remaining)?;
        let namespace = self.sync_namespace()?;
        // Readers fence an outstanding intent. Report success only once the
        // verified frame can also be observed by a fresh reader.
        self.unlink(&intent_path)?;
        lease.pending = None;
        Ok(Some(TranscriptReceipt {
            sequence: frame.sequence,
            already_accepted: false,
            barrier: namespace,
        }))
    }

    fn sync_source(
        &self,
        file: &File,
    ) -> Result<CanonicalDurabilityBarrier, TranscriptCommitError> {
        file.sync_all()
            .map_err(|e| io_failure(CanonicalAppendPhase::RecoverySync, e))?;
        self.sync_namespace()
    }

    fn sync_namespace(&self) -> Result<CanonicalDurabilityBarrier, TranscriptCommitError> {
        barrier(self.guard.sync_recovery_namespace_entry(
            self.path,
            Some(&self.qualification),
            self.key,
        ))
    }

    fn recover_gap(
        &self,
        intent: &Intent,
        suffix: &[u8],
        file: Option<&File>,
    ) -> Result<(), TranscriptCommitError> {
        let evidence = self
            .path
            .with_added_extension(format!("{}.quarantine", intent.attempt));
        let gap = self
            .path
            .with_added_extension(format!("{}.gap", intent.attempt));
        let quarantined = if suffix.is_empty() {
            checked_bytes(&evidence)?.is_some()
        } else {
            self.publish(&evidence, suffix)?;
            let file = file.ok_or(TranscriptCommitError::InvalidHistory)?;
            self.guard
                .revalidate_recovery_source(self.path, file)
                .map_err(TranscriptCommitError::Rejected)?;
            file.set_len(intent.base_len)
                .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryTruncate, e))?;
            true
        };
        if let Some(file) = file {
            file.sync_all()
                .map_err(|e| io_failure(CanonicalAppendPhase::RecoverySync, e))?;
        }
        let record = TranscriptGap {
            stream: GapStream::TranscriptRevisions,
            after: intent.after.clone(),
            suffix_quarantined: quarantined,
        };
        self.publish(
            &gap,
            &serde_json::to_vec(&record).map_err(|_| TranscriptCommitError::InvalidHistory)?,
        )
    }
}

fn revision_id(event: &TranscriptEvent) -> Result<String, TranscriptCommitError> {
    let key = serde_json::to_vec(&(&event.span_id, event.revision_number))
        .map_err(|_| TranscriptCommitError::InvalidHistory)?;
    Ok(format!("transcript-{}", digest(&key)))
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn valid_attempt(s: &str) -> bool {
    Uuid::parse_str(s).is_ok_and(|id| id.to_string() == s)
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cut {
    Full,
    Short,
    Flush,
    Sync,
    RecoverySync,
    KillPartial,
    KillComplete,
}
#[cfg(test)]
thread_local! { static CUT: std::cell::Cell<Option<Cut>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
fn wait_for_kill() -> ! {
    println!("TRANSCRIPT_WRITER_CUT");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
fn write_frame(file: &mut File, bytes: &[u8]) -> Result<(), TranscriptCommitError> {
    #[cfg(test)]
    let cut = CUT.take();
    #[cfg(test)]
    match cut {
        Some(Cut::Full) => {
            return Err(io_failure(
                CanonicalAppendPhase::Write,
                io::ErrorKind::StorageFull.into(),
            ));
        }
        Some(Cut::Short | Cut::KillPartial) => {
            file.write_all(&bytes[..bytes.len() / 2])
                .map_err(|e| io_failure(CanonicalAppendPhase::Write, e))?;
            if cut == Some(Cut::KillPartial) {
                wait_for_kill()
            }
            return Err(io_failure(
                CanonicalAppendPhase::Write,
                io::ErrorKind::WriteZero.into(),
            ));
        }
        _ => {}
    }
    let n = file
        .write(bytes)
        .map_err(|e| io_failure(CanonicalAppendPhase::Write, e))?;
    if n != bytes.len() {
        return Err(io_failure(
            CanonicalAppendPhase::Write,
            io::ErrorKind::WriteZero.into(),
        ));
    }
    #[cfg(test)]
    match cut {
        Some(Cut::KillComplete) => wait_for_kill(),
        Some(Cut::Flush) => {
            return Err(io_failure(
                CanonicalAppendPhase::Flush,
                io::ErrorKind::Other.into(),
            ));
        }
        _ => {}
    }
    file.flush()
        .map_err(|e| io_failure(CanonicalAppendPhase::Flush, e))?;
    #[cfg(test)]
    if cut == Some(Cut::Sync) {
        return Err(io_failure(
            CanonicalAppendPhase::Sync,
            io::ErrorKind::Other.into(),
        ));
    }
    file.sync_all()
        .map_err(|e| io_failure(CanonicalAppendPhase::Sync, e))
}

pub(crate) fn provision_directory(path: &Path) -> Result<(), TranscriptCommitError> {
    let root = path.parent().ok_or(TranscriptCommitError::InvalidAddress)?;
    fs::create_dir_all(root).map_err(|e| io_failure(CanonicalAppendPhase::Write, e))?;
    let (qualification, _) = CanonicalFilesystemQualification::for_existing_managed_root(root)
        .map_err(TranscriptCommitError::Qualification)?;
    let key = CanonicalRecoveryKey::from_opaque_bytes(*Uuid::new_v4().as_bytes());
    barrier(qualification.provision_child_directory(path, key)).map(|_| ())
}

/// Hold floor admission across source and recovery mutations. Lock order is
/// the Session store first, then its transcript directory.
fn with_writable_v1<T>(
    path: &Path,
    session: &str,
    operation: impl FnOnce() -> Result<T, TranscriptCommitError>,
) -> Result<T, TranscriptCommitError> {
    use crate::persistence::session_artifact_manifest::SessionArtifactManifestStore;
    use crate::persistence::session_semantics::{
        GuardedSessionOpenError, SessionSemanticsVersion, guarded_session_open,
    };
    let root = path
        .parent()
        .and_then(Path::parent)
        .ok_or(TranscriptCommitError::InvalidAddress)?;
    let store = SessionArtifactManifestStore::qualified_existing_session(root, session)
        .map_err(|_| TranscriptCommitError::SessionFloorUnavailable)?;
    guarded_session_open(&store, SessionSemanticsVersion::V1, |_| operation()).map_err(|error| {
        match error {
            GuardedSessionOpenError::ContentReader(error) => error,
            _ => TranscriptCommitError::SessionFloorUnavailable,
        }
    })
}

pub(crate) fn ready(path: &Path, session: &str) -> Result<(), TranscriptCommitError> {
    if !path
        .with_added_extension("attempt")
        .try_exists()
        .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
    {
        // A missing event log means a legacy segment log can still be authoritative.
        // Preparing the writer must preserve that distinction.
        CanonicalFilesystemQualification::for_existing_managed_root(
            path.parent().ok_or(TranscriptCommitError::InvalidAddress)?,
        )
        .map_err(TranscriptCommitError::Qualification)?;
        return Ok(());
    }
    with_writable_v1(path, session, || {
        Commit::open(path, session)?
            .run(None, None, process_owner())
            .map(|_| ())
    })
}
pub(crate) fn commit_transcript(
    path: &Path,
    session: &str,
    event: &TranscriptEvent,
) -> Result<TranscriptReceipt, TranscriptCommitError> {
    commit_after(path, session, event, None)
}

pub(crate) fn commit_after(
    path: &Path,
    session: &str,
    event: &TranscriptEvent,
    expected_head: Option<u64>,
) -> Result<TranscriptReceipt, TranscriptCommitError> {
    with_writable_v1(path, session, || {
        Commit::open(path, session)?
            .run(Some(event), expected_head, process_owner())?
            .ok_or(TranscriptCommitError::InvalidHistory)
    })
}

/// Capture starts a fresh session on restart. Reconcile outstanding attempts for
/// older sessions as well. A busy or damaged stream cannot block another one.
pub(crate) fn recover_previous_sessions(
    directory: &Path,
    current_session: &str,
) -> Result<(), TranscriptCommitError> {
    let entries =
        fs::read_dir(directory).map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?;
    for entry in entries {
        let name = entry
            .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
            .file_name();
        let Some(session) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".events.jsonl.attempt"))
        else {
            continue;
        };
        if session == current_session || crate::sessions::validate_session_id(session).is_err() {
            continue;
        }
        let source = directory.join(format!("{session}.events.jsonl"));
        if let Err(error) = ready(&source, session) {
            log::warn!("transcript recovery pending: {error:?}");
        }
    }
    Ok(())
}

/// A reader may overlap a write or run before startup recovery. The persistent
/// intent is installed before the first source write and removed after the
/// file barrier. Fence the read on both sides and only expose its verified base
/// if either observation saw an outstanding attempt.
pub(in crate::persistence) fn read_committed<T: DeserializeOwned>(
    path: &Path,
    session: &str,
) -> Result<CanonicalLogSnapshot<T>, super::super::canonical_reader::CanonicalReaderError> {
    use super::super::canonical_reader::CanonicalReaderError;
    let pending = || {
        checked_bytes_bounded(&path.with_added_extension("attempt"), 4096)
            .and_then(|bytes| {
                bytes
                    .map(|bytes| {
                        serde_json::from_slice::<Intent>(&bytes)
                            .map_err(|_| TranscriptCommitError::InvalidHistory)
                    })
                    .transpose()
            })
            .map_err(|_| CanonicalReaderError::TranscriptAttemptUnreadable)
    };
    validate_stream_context(session, STREAM, 1)?;
    let before = pending()?;
    let directory = path
        .parent()
        .ok_or(CanonicalReaderError::TranscriptAttemptUnreadable)?;
    let acquire =
        || super::super::canonical_durability::CanonicalSharedGuard::open_existing(directory);
    // The first attempt can create a source and later restore absence. A
    // before/after check of the intent alone cannot distinguish that ABA.
    // Hold the existing namespace lock for an ordinary published snapshot.
    let guard = if before.is_none() {
        match acquire() {
            Ok(guard) => Some(guard),
            Err(CanonicalCoordinationError::Missing | CanonicalCoordinationError::Contended) => {
                None
            }
            Err(_) => return Err(CanonicalReaderError::TranscriptAttemptUnreadable),
        }
    } else {
        None
    };
    let bytes = fs::read(path);
    let after = pending()?;
    if guard.is_none()
        && before.is_none()
        && after.is_none()
        && !matches!(acquire(), Err(CanonicalCoordinationError::Missing))
    {
        return Err(CanonicalLogError::LockContended.into());
    }
    let intent = before.or(after);
    if let Some(intent) = &intent
        && intent.source_was_absent
    {
        if intent.base_len != 0 || intent.after.is_some() {
            return Err(CanonicalReaderError::TranscriptAttemptUnreadable);
        }
        return Err(CanonicalLogError::Io {
            operation: CanonicalIoOperation::Read,
            kind: io::ErrorKind::NotFound,
        }
        .into());
    }
    let bytes = match bytes {
        Err(e) if e.kind() == io::ErrorKind::NotFound && intent.is_some() => {
            return Err(CanonicalReaderError::TranscriptAttemptUnreadable);
        }
        result => result.map_err(|e| CanonicalLogError::Io {
            operation: CanonicalIoOperation::Read,
            kind: e.kind(),
        })?,
    };
    let prefix = if let Some(intent) = &intent {
        usize::try_from(intent.base_len)
            .ok()
            .and_then(|n| bytes.get(..n))
            .ok_or(CanonicalReaderError::TranscriptAttemptUnreadable)?
    } else {
        &bytes
    };
    let raw = parse_structural_records(prefix, session, STREAM, 1).map_err(|e| e.error)?;
    let snapshot = raw_to_typed(raw)?;
    if let Some(intent) = intent {
        let actual = snapshot.head.as_ref().map(|head| VerifiedTranscriptHead {
            sequence: head.sequence,
            record_hash: head.record_hash.clone(),
        });
        if actual != intent.after {
            return Err(CanonicalReaderError::TranscriptAttemptUnreadable);
        }
    }
    Ok(snapshot)
}

pub(crate) fn transcript_recovery_artifacts(path: &Path) -> Result<Vec<PathBuf>, io::Error> {
    let prefix = format!(
        "{}.",
        path.file_name()
            .and_then(|n| n.to_str())
            .ok_or(io::ErrorKind::InvalidInput)?
    );
    let entries = match fs::read_dir(path.parent().ok_or(io::ErrorKind::InvalidInput)?) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(suffix) = name.to_str().and_then(|n| n.strip_prefix(&prefix)) else {
            continue;
        };
        if matches!(suffix, "attempt" | "attempt-stage" | "attempt-lock")
            || suffix
                .split_once('.')
                .is_some_and(|(id, kind)| valid_attempt(id) && matches!(kind, "quarantine" | "gap"))
        {
            result.push(entry.path());
        }
    }
    result.sort();
    Ok(result)
}

pub(super) fn decode_gap(bytes: &[u8]) -> Result<TranscriptGap, TranscriptCommitError> {
    let gap: TranscriptGap =
        serde_json::from_slice(bytes).map_err(|_| TranscriptCommitError::InvalidHistory)?;
    if gap
        .after
        .as_ref()
        .is_some_and(|h| h.sequence == 0 || !valid_hash(&h.record_hash))
    {
        return Err(TranscriptCommitError::InvalidHistory);
    }
    Ok(gap)
}

/// Closed, content-free gaps ordered by their preceding canonical sequence.
pub fn transcript_gaps(path: &Path) -> Result<Vec<TranscriptGap>, TranscriptCommitError> {
    let mut gaps: Vec<TranscriptGap> = Vec::new();
    for file in transcript_recovery_artifacts(path)
        .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
    {
        if file.extension().is_some_and(|s| s == "gap") {
            let bytes =
                checked_bytes_bounded(&file, 4096)?.ok_or(TranscriptCommitError::InvalidHistory)?;
            let gap = decode_gap(&bytes)?;
            gaps.push(gap);
        }
    }
    gaps.sort_by_key(|gap| gap.after.as_ref().map(|h| h.sequence));
    Ok(gaps)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod domain_tests;
