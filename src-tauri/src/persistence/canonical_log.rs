//! Crash-aware canonical append-log primitives.
//!
//! This module deliberately does not replace any runtime writer yet. It defines
//! the framing, integrity, idempotency, durability, and tail-recovery contract
//! that a later migration can adopt one stream at a time.
//!
//! A v1 record is one newline-terminated frame:
//! `AGCL1 <16 hex JSON byte length> <JSON envelope>\n`. The commit newline is
//! part of the durability contract. Existing plain JSONL is read as a
//! deterministic legacy prefix so a v1 writer can extend a legacy stream
//! without rewriting it. Blank legacy lines remain compatible with the current
//! JSONL reader and do not consume sequence numbers.
//!
//! `CanonicalAppender` owns one cooperative OS-level exclusive lock for its
//! lifetime. Rust's standard file locks have been stable since Rust 1.89, so
//! they are available under AudioGraph's Rust 1.95+ toolchain. The lock
//! serializes this API across processes, but it cannot stop a legacy or external
//! writer that ignores the lock. Runtime migration must therefore quiesce and
//! replace old writers atomically before this appender is used.
//!
//! File `sync_all` is intentionally not presented as parent-directory
//! durability. New-file directory-entry persistence and quarantine lifecycle
//! registration remain explicit integration blockers.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const CANONICAL_LOG_FORMAT_VERSION: u8 = 1;

const FRAME_PREFIX_V1: &[u8] = b"AGCL1 ";
const FRAME_MAGIC: &[u8] = b"AGCL";
const FRAME_LENGTH_HEX_BYTES: usize = 16;
const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_FRAME_JSON_BYTES: usize = 64 * 1024 * 1024;

static QUARANTINE_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalRecordEncoding {
    LegacyJsonl,
    FramedV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalBasisHead {
    pub sequence: u64,
    pub event_id: String,
    pub record_hash: String,
}

pub type CanonicalBasisHeadVector = BTreeMap<String, CanonicalBasisHead>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalEventMetadata {
    pub event_id: String,
    pub causal_event_ids: Vec<String>,
    pub basis_heads: CanonicalBasisHeadVector,
}

impl CanonicalEventMetadata {
    pub fn new(event_id: impl Into<String>) -> Self {
        Self {
            event_id: event_id.into(),
            causal_event_ids: Vec::new(),
            basis_heads: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalRecord<T> {
    pub encoding: CanonicalRecordEncoding,
    pub session_id: String,
    pub stream_id: String,
    pub domain_schema_version: u32,
    pub sequence: u64,
    pub event_id: String,
    pub causal_event_ids: Vec<String>,
    pub basis_heads: CanonicalBasisHeadVector,
    pub previous_hash: String,
    pub payload_hash: String,
    pub record_hash: String,
    pub payload: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalStreamHead {
    pub sequence: u64,
    pub event_id: String,
    pub record_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalTailQuarantineReceipt {
    pub quarantine_path: PathBuf,
    pub retained_bytes: u64,
    pub quarantined_bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalLogSnapshot<T> {
    pub records: Vec<CanonicalRecord<T>>,
    pub head: Option<CanonicalStreamHead>,
    pub tail_quarantine: Option<CanonicalTailQuarantineReceipt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalTailRecovery {
    Strict,
    QuarantineUnterminatedTail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalIoOperation {
    Read,
    OpenAppender,
    LockAppender,
    InitialSync,
    QuarantineWrite,
    QuarantineFlush,
    QuarantineSync,
    Truncate,
    TruncateSync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalCorruptionReason {
    EmptyRecord,
    InvalidJson,
    InvalidFrame,
    UnsupportedFrameVersion,
    FrameLengthMismatch,
    FrameTooLarge,
    EnvelopeVersionMismatch,
    SessionMismatch,
    StreamMismatch,
    DomainSchemaVersionMismatch,
    SequenceMismatch,
    PreviousHashMismatch,
    PayloadHashMismatch,
    RecordHashMismatch,
    InvalidEventId,
    InvalidCausalEventId,
    InvalidBasisHead,
    DuplicateEventId,
    LegacyRecordAfterFramedRecord,
    MissingFrameTerminator,
}

/// A content-redacted load/open failure. Payload bytes and identifiers are
/// intentionally excluded from both `Debug` and `Display`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalLogError {
    InvalidSessionId,
    InvalidStreamId,
    InvalidDomainSchemaVersion,
    LockContended,
    Io {
        operation: CanonicalIoOperation,
        kind: io::ErrorKind,
    },
    CorruptRecord {
        record_index: usize,
        reason: CanonicalCorruptionReason,
        newline_terminated: bool,
    },
    PayloadDecode {
        record_index: usize,
    },
}

impl fmt::Display for CanonicalLogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSessionId => formatter.write_str("invalid canonical session identifier"),
            Self::InvalidStreamId => formatter.write_str("invalid canonical stream identifier"),
            Self::InvalidDomainSchemaVersion => {
                formatter.write_str("invalid canonical domain schema version")
            }
            Self::LockContended => {
                formatter.write_str("canonical log already has an active appender")
            }
            Self::Io { operation, kind } => {
                write!(formatter, "canonical log {operation:?} failed ({kind:?})")
            }
            Self::CorruptRecord {
                record_index,
                reason,
                newline_terminated,
            } => write!(
                formatter,
                "canonical log record {record_index} is corrupt ({reason:?}, newline_terminated={newline_terminated})"
            ),
            Self::PayloadDecode { record_index } => write!(
                formatter,
                "canonical log record {record_index} does not match the requested payload schema"
            ),
        }
    }
}

impl std::error::Error for CanonicalLogError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalDurability {
    /// `File::sync_all` succeeded after the complete newline-terminated frame
    /// was written and flushed. This names file data + file metadata durability;
    /// it does not claim that a newly-created parent directory was synced.
    FileDataAndMetadataSynced,
    /// The same appender had previously synced this validated event.
    ValidatedExistingRecord,
    /// Recovery found the complete event and a fresh flush + `sync_all`
    /// durability barrier succeeded before returning `AlreadyAccepted`.
    RecoveryBarrierSynced,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalAppendDurabilityReceipt {
    pub sequence: u64,
    pub record_hash: String,
    pub payload_hash: String,
    pub appended_bytes: u64,
    pub durability: CanonicalDurability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalAppendRejection {
    InvalidEventId,
    InvalidCausalEventId,
    InvalidBasisHead,
    PayloadSerialization,
    EventIdConflict,
    SequenceExhausted,
    ConcurrentModification,
    /// A prior append is uncertain. Only an identical retry of that event may
    /// be attempted until recovery succeeds or the appender is dropped.
    AppenderPoisoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalAppendPhase {
    Write,
    Flush,
    Sync,
    RecoveryRead,
    RecoveryFlush,
    RecoverySync,
    RecoveryQuarantine,
    RecoveryTruncate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalAppendUncertaintyReason {
    ShortWrite,
    Io(io::ErrorKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalAppendUncertainty {
    pub sequence: u64,
    pub phase: CanonicalAppendPhase,
    pub reason: CanonicalAppendUncertaintyReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalAppendRecoveryReason {
    Stream(CanonicalLogError),
    ConcurrentModification,
    EventConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalAppendRecoveryRequired {
    pub sequence: u64,
    pub reason: CanonicalAppendRecoveryReason,
}

#[must_use = "canonical append outcomes must be reconciled before live state advances"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalAppendOutcome {
    Accepted(CanonicalAppendDurabilityReceipt),
    AlreadyAccepted(CanonicalAppendDurabilityReceipt),
    Rejected(CanonicalAppendRejection),
    /// At least one append operation was attempted. The appender is poisoned;
    /// retry only the identical event.
    OutcomeUncertain(CanonicalAppendUncertainty),
    /// Recovery could not safely decide or repair the stream. The appender
    /// remains poisoned and must not accept another event.
    RecoveryRequired(CanonicalAppendRecoveryRequired),
}

#[derive(Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRecordV1 {
    format_version: u8,
    session_id: String,
    stream_id: String,
    domain_schema_version: u32,
    sequence: u64,
    event_id: String,
    causal_event_ids: Vec<String>,
    basis_heads: CanonicalBasisHeadVector,
    previous_hash: String,
    payload_hash: String,
    record_hash: String,
    payload: Value,
}

/// JSON value parsed with duplicate object members rejected at every depth.
/// Canonical v1 commits semantic JSON values, so accepting duplicate names
/// would leave the source text with an ambiguous first/last-member meaning.
struct UniqueJsonValue(Value);

impl<'de> serde::Deserialize<'de> for UniqueJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueJsonValueVisitor)
    }
}

struct UniqueJsonValueVisitor;

impl<'de> serde::de::Visitor<'de> for UniqueJsonValueVisitor {
    type Value = UniqueJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object members")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueJsonValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        serde::Deserialize::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(UniqueJsonValue(value)) = sequence.next_element()? {
            values.push(value);
        }
        Ok(UniqueJsonValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        let mut values = serde_json::Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(serde::de::Error::custom("duplicate JSON object member"));
            }
            let UniqueJsonValue(value) = object.next_value()?;
            values.insert(key, value);
        }
        Ok(UniqueJsonValue(Value::Object(values)))
    }
}

#[derive(Clone)]
struct RawCanonicalRecord {
    encoding: CanonicalRecordEncoding,
    session_id: String,
    stream_id: String,
    domain_schema_version: u32,
    sequence: u64,
    event_id: String,
    causal_event_ids: Vec<String>,
    basis_heads: CanonicalBasisHeadVector,
    previous_hash: String,
    payload_hash: String,
    record_hash: String,
    commitment_hash: String,
    payload: Value,
}

struct RawSnapshot {
    records: Vec<RawCanonicalRecord>,
    tail_quarantine: Option<CanonicalTailQuarantineReceipt>,
    byte_len: u64,
    ends_with_newline: bool,
}

struct StructuralFailure {
    error: CanonicalLogError,
    valid_up_to: usize,
    repairable_unterminated_tail: bool,
}

trait LogFileOps {
    fn read_log(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn quarantine_and_truncate(
        &self,
        path: &Path,
        valid_up_to: u64,
        tail: &[u8],
    ) -> Result<PathBuf, (CanonicalIoOperation, io::ErrorKind)>;
}

struct StdFileOps;

impl LogFileOps for StdFileOps {
    fn read_log(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn quarantine_and_truncate(
        &self,
        path: &Path,
        valid_up_to: u64,
        tail: &[u8],
    ) -> Result<PathBuf, (CanonicalIoOperation, io::ErrorKind)> {
        let quarantine_path = create_quarantine_file(path, tail)?;
        let source = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|error| (CanonicalIoOperation::Truncate, error.kind()))?;
        source
            .set_len(valid_up_to)
            .map_err(|error| (CanonicalIoOperation::Truncate, error.kind()))?;
        source
            .sync_all()
            .map_err(|error| (CanonicalIoOperation::TruncateSync, error.kind()))?;
        Ok(quarantine_path)
    }
}

fn create_quarantine_file(
    source_path: &Path,
    tail: &[u8],
) -> Result<PathBuf, (CanonicalIoOperation, io::ErrorKind)> {
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("canonical-log");
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);

    for _ in 0..32 {
        let nonce = QUARANTINE_NONCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            "{file_name}.corrupt-tail-{millis}-{}-{nonce}",
            std::process::id()
        ));
        let mut quarantine = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err((CanonicalIoOperation::QuarantineWrite, error.kind()));
            }
        };
        crate::fs_util::set_owner_only(&candidate);
        quarantine
            .write_all(tail)
            .map_err(|error| (CanonicalIoOperation::QuarantineWrite, error.kind()))?;
        quarantine
            .flush()
            .map_err(|error| (CanonicalIoOperation::QuarantineFlush, error.kind()))?;
        quarantine
            .sync_all()
            .map_err(|error| (CanonicalIoOperation::QuarantineSync, error.kind()))?;
        return Ok(candidate);
    }

    Err((
        CanonicalIoOperation::QuarantineWrite,
        io::ErrorKind::AlreadyExists,
    ))
}

/// Read and validate a canonical stream. Recovery mutates only after the valid
/// prefix has also decoded as `T`. Callers must quiesce any active writer before
/// selecting `QuarantineUnterminatedTail`; runtime reader/writer coordination is
/// intentionally not provided by this isolated kernel.
pub fn load_canonical_stream<T: DeserializeOwned>(
    path: &Path,
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
    tail_recovery: CanonicalTailRecovery,
) -> Result<CanonicalLogSnapshot<T>, CanonicalLogError> {
    load_canonical_stream_with_ops(
        path,
        session_id,
        stream_id,
        domain_schema_version,
        tail_recovery,
        &StdFileOps,
    )
}

fn load_canonical_stream_with_ops<T: DeserializeOwned, O: LogFileOps>(
    path: &Path,
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
    tail_recovery: CanonicalTailRecovery,
    ops: &O,
) -> Result<CanonicalLogSnapshot<T>, CanonicalLogError> {
    validate_stream_context(session_id, stream_id, domain_schema_version)?;
    let bytes = ops.read_log(path).map_err(|error| CanonicalLogError::Io {
        operation: CanonicalIoOperation::Read,
        kind: error.kind(),
    })?;
    match parse_structural_records(&bytes, session_id, stream_id, domain_schema_version) {
        Ok(snapshot) => raw_to_typed(snapshot),
        Err(failure)
            if failure.repairable_unterminated_tail
                && tail_recovery == CanonicalTailRecovery::QuarantineUnterminatedTail =>
        {
            let tail = &bytes[failure.valid_up_to..];
            let mut prefix = parse_structural_records(
                &bytes[..failure.valid_up_to],
                session_id,
                stream_id,
                domain_schema_version,
            )
            .map_err(|failure| failure.error)?;

            // Prove the retained prefix is semantically usable before mutation.
            validate_payload_schema::<T>(&prefix.records)?;
            let quarantine_path = ops
                .quarantine_and_truncate(path, failure.valid_up_to as u64, tail)
                .map_err(|(operation, kind)| CanonicalLogError::Io { operation, kind })?;
            prefix.byte_len = failure.valid_up_to as u64;
            prefix.tail_quarantine = Some(CanonicalTailQuarantineReceipt {
                quarantine_path,
                retained_bytes: failure.valid_up_to as u64,
                quarantined_bytes: tail.len() as u64,
            });
            raw_to_typed(prefix)
        }
        Err(failure) => Err(failure.error),
    }
}

trait LockedAppenderFile: Send {
    fn read_all(&mut self) -> io::Result<Vec<u8>>;
    fn len(&self) -> io::Result<u64>;
    fn write_once(&mut self, bytes: &[u8]) -> io::Result<usize>;
    fn flush(&mut self) -> io::Result<()>;
    fn sync_all(&mut self) -> io::Result<()>;
    fn truncate(&mut self, len: u64) -> io::Result<()>;
}

struct StdLockedAppenderFile(File);

impl LockedAppenderFile for StdLockedAppenderFile {
    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        self.0.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        self.0.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn len(&self) -> io::Result<u64> {
        self.0.metadata().map(|metadata| metadata.len())
    }

    fn write_once(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.seek(SeekFrom::End(0))?;
        self.0.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }

    fn sync_all(&mut self) -> io::Result<()> {
        self.0.sync_all()
    }

    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.0.set_len(len)
    }
}

impl Drop for StdLockedAppenderFile {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn open_locked_appender(path: &Path) -> Result<StdLockedAppenderFile, CanonicalLogError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| CanonicalLogError::Io {
            operation: CanonicalIoOperation::OpenAppender,
            kind: error.kind(),
        })?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| CanonicalLogError::Io {
            operation: CanonicalIoOperation::OpenAppender,
            kind: error.kind(),
        })?;
    crate::fs_util::set_owner_only(path);
    match file.try_lock() {
        Ok(()) => Ok(StdLockedAppenderFile(file)),
        Err(TryLockError::WouldBlock) => Err(CanonicalLogError::LockContended),
        Err(TryLockError::Error(error)) => Err(CanonicalLogError::Io {
            operation: CanonicalIoOperation::LockAppender,
            kind: error.kind(),
        }),
    }
}

#[derive(Clone)]
struct CachedEvent {
    sequence: u64,
    record_hash: String,
    payload_hash: String,
    commitment_hash: String,
}

struct CachedStreamState {
    head: Option<CanonicalStreamHead>,
    events: HashMap<String, CachedEvent>,
    byte_len: u64,
    ends_with_newline: bool,
}

#[derive(Clone)]
struct PendingAppend {
    event_id: String,
    sequence: u64,
    record_hash: String,
    payload_hash: String,
    commitment_hash: String,
    frame: Vec<u8>,
    base_byte_len: u64,
    base_head: Option<CanonicalStreamHead>,
    base_ends_with_newline: bool,
}

/// A typed, long-lived, exclusively locked canonical stream appender.
///
/// Opening performs one complete structural and `T` schema validation, builds
/// an in-memory event-ID index/head, and synchronizes the existing file. Normal
/// appends use only the cached state plus an O(1) file-length guard. A complete
/// rescan occurs only during explicit recovery from an uncertain append.
pub struct CanonicalAppender<T> {
    path: PathBuf,
    session_id: String,
    stream_id: String,
    domain_schema_version: u32,
    tail_recovery: CanonicalTailRecovery,
    file: Box<dyn LockedAppenderFile>,
    head: Option<CanonicalStreamHead>,
    events: HashMap<String, CachedEvent>,
    byte_len: u64,
    ends_with_newline: bool,
    poisoned: Option<PendingAppend>,
    quarantine_receipts: Vec<CanonicalTailQuarantineReceipt>,
    full_scan_count: u64,
    marker: PhantomData<T>,
}

impl<T> CanonicalAppender<T>
where
    T: Serialize + DeserializeOwned,
{
    pub fn open(
        path: &Path,
        session_id: &str,
        stream_id: &str,
        domain_schema_version: u32,
        tail_recovery: CanonicalTailRecovery,
    ) -> Result<Self, CanonicalLogError> {
        validate_stream_context(session_id, stream_id, domain_schema_version)?;
        let file = open_locked_appender(path)?;
        Self::from_locked_file(
            path.to_path_buf(),
            session_id.to_string(),
            stream_id.to_string(),
            domain_schema_version,
            tail_recovery,
            Box::new(file),
        )
    }

    fn from_locked_file(
        path: PathBuf,
        session_id: String,
        stream_id: String,
        domain_schema_version: u32,
        tail_recovery: CanonicalTailRecovery,
        mut file: Box<dyn LockedAppenderFile>,
    ) -> Result<Self, CanonicalLogError> {
        validate_stream_context(&session_id, &stream_id, domain_schema_version)?;
        let bytes = file.read_all().map_err(|error| CanonicalLogError::Io {
            operation: CanonicalIoOperation::Read,
            kind: error.kind(),
        })?;
        let raw = match parse_structural_records(
            &bytes,
            &session_id,
            &stream_id,
            domain_schema_version,
        ) {
            Ok(snapshot) => snapshot,
            Err(failure)
                if failure.repairable_unterminated_tail
                    && tail_recovery == CanonicalTailRecovery::QuarantineUnterminatedTail =>
            {
                let tail = &bytes[failure.valid_up_to..];
                let mut prefix = parse_structural_records(
                    &bytes[..failure.valid_up_to],
                    &session_id,
                    &stream_id,
                    domain_schema_version,
                )
                .map_err(|failure| failure.error)?;
                validate_payload_schema::<T>(&prefix.records)?;
                let quarantine_path = create_quarantine_file(&path, tail)
                    .map_err(|(operation, kind)| CanonicalLogError::Io { operation, kind })?;
                file.truncate(failure.valid_up_to as u64).map_err(|error| {
                    CanonicalLogError::Io {
                        operation: CanonicalIoOperation::Truncate,
                        kind: error.kind(),
                    }
                })?;
                file.sync_all().map_err(|error| CanonicalLogError::Io {
                    operation: CanonicalIoOperation::TruncateSync,
                    kind: error.kind(),
                })?;
                prefix.byte_len = failure.valid_up_to as u64;
                prefix.tail_quarantine = Some(CanonicalTailQuarantineReceipt {
                    quarantine_path,
                    retained_bytes: failure.valid_up_to as u64,
                    quarantined_bytes: tail.len() as u64,
                });
                prefix
            }
            Err(failure) => return Err(failure.error),
        };
        let initial_quarantine = raw.tail_quarantine.clone();
        let cache = cache_from_raw::<T>(&raw)?;

        // A fresh barrier on open prevents a post-crash, previously uncertain
        // complete frame from being treated as accepted merely because it is
        // readable from the page cache.
        file.flush().map_err(|error| CanonicalLogError::Io {
            operation: CanonicalIoOperation::InitialSync,
            kind: error.kind(),
        })?;
        file.sync_all().map_err(|error| CanonicalLogError::Io {
            operation: CanonicalIoOperation::InitialSync,
            kind: error.kind(),
        })?;

        Ok(Self {
            path,
            session_id,
            stream_id,
            domain_schema_version,
            tail_recovery,
            file,
            head: cache.head,
            events: cache.events,
            byte_len: cache.byte_len,
            ends_with_newline: cache.ends_with_newline,
            poisoned: None,
            quarantine_receipts: initial_quarantine.into_iter().collect(),
            full_scan_count: 1,
            marker: PhantomData,
        })
    }

    pub fn head(&self) -> Option<&CanonicalStreamHead> {
        self.head.as_ref()
    }

    pub fn cached_event_count(&self) -> usize {
        self.events.len()
    }

    pub fn recovery_required(&self) -> bool {
        self.poisoned.is_some()
    }

    /// Drain quarantine receipts so a future runtime integration can register
    /// them with the typed artifact manifest. The current manifest does not yet
    /// consume these receipts, so deletion parity remains an open blocker.
    pub fn take_quarantine_receipts(&mut self) -> Vec<CanonicalTailQuarantineReceipt> {
        std::mem::take(&mut self.quarantine_receipts)
    }

    pub fn append(
        &mut self,
        metadata: &CanonicalEventMetadata,
        payload: &T,
    ) -> CanonicalAppendOutcome {
        let normalized = match normalize_event_metadata(metadata) {
            Ok(metadata) => metadata,
            Err(rejection) => return CanonicalAppendOutcome::Rejected(rejection),
        };
        let payload = match serde_json::to_value(payload) {
            Ok(payload) => canonicalize_json_value(payload),
            Err(_) => {
                return CanonicalAppendOutcome::Rejected(
                    CanonicalAppendRejection::PayloadSerialization,
                );
            }
        };
        let payload_hash = match payload_digest(&payload) {
            Ok(hash) => hash,
            Err(_) => {
                return CanonicalAppendOutcome::Rejected(
                    CanonicalAppendRejection::PayloadSerialization,
                );
            }
        };
        let commitment_hash = match event_commitment_hash(
            &self.session_id,
            &self.stream_id,
            self.domain_schema_version,
            &normalized.event_id,
            &normalized.causal_event_ids,
            &normalized.basis_heads,
            &payload_hash,
        ) {
            Ok(hash) => hash,
            Err(_) => {
                return CanonicalAppendOutcome::Rejected(
                    CanonicalAppendRejection::PayloadSerialization,
                );
            }
        };

        if let Some(poisoned) = &self.poisoned {
            if poisoned.event_id != normalized.event_id {
                return CanonicalAppendOutcome::Rejected(
                    CanonicalAppendRejection::AppenderPoisoned,
                );
            }
            if poisoned.commitment_hash != commitment_hash {
                return CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::EventIdConflict);
            }
            return self.recover_poisoned();
        }

        if let Some(existing) = self.events.get(&normalized.event_id) {
            if existing.commitment_hash != commitment_hash {
                return CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::EventIdConflict);
            }
            return CanonicalAppendOutcome::AlreadyAccepted(CanonicalAppendDurabilityReceipt {
                sequence: existing.sequence,
                record_hash: existing.record_hash.clone(),
                payload_hash: existing.payload_hash.clone(),
                appended_bytes: 0,
                durability: CanonicalDurability::ValidatedExistingRecord,
            });
        }

        let sequence = match self
            .head
            .as_ref()
            .map_or(Some(1), |head| head.sequence.checked_add(1))
        {
            Some(sequence) => sequence,
            None => {
                return CanonicalAppendOutcome::Rejected(
                    CanonicalAppendRejection::SequenceExhausted,
                );
            }
        };
        let previous_hash = self
            .head
            .as_ref()
            .map_or_else(|| ZERO_HASH.to_string(), |head| head.record_hash.clone());
        let record_hash = match framed_record_hash(
            &self.session_id,
            &self.stream_id,
            self.domain_schema_version,
            sequence,
            &normalized.event_id,
            &normalized.causal_event_ids,
            &normalized.basis_heads,
            &previous_hash,
            &payload_hash,
        ) {
            Ok(hash) => hash,
            Err(_) => {
                return CanonicalAppendOutcome::Rejected(
                    CanonicalAppendRejection::PayloadSerialization,
                );
            }
        };
        let wire = WireRecordV1 {
            format_version: CANONICAL_LOG_FORMAT_VERSION,
            session_id: self.session_id.clone(),
            stream_id: self.stream_id.clone(),
            domain_schema_version: self.domain_schema_version,
            sequence,
            event_id: normalized.event_id.clone(),
            causal_event_ids: normalized.causal_event_ids,
            basis_heads: normalized.basis_heads,
            previous_hash,
            payload_hash: payload_hash.clone(),
            record_hash: record_hash.clone(),
            payload,
        };
        let json = match serde_json::to_vec(&wire) {
            Ok(json) if json.len() <= MAX_FRAME_JSON_BYTES => json,
            _ => {
                return CanonicalAppendOutcome::Rejected(
                    CanonicalAppendRejection::PayloadSerialization,
                );
            }
        };
        let mut frame = Vec::with_capacity(
            usize::from(!self.ends_with_newline && self.byte_len > 0)
                + FRAME_PREFIX_V1.len()
                + FRAME_LENGTH_HEX_BYTES
                + 1
                + json.len()
                + 1,
        );
        if !self.ends_with_newline && self.byte_len > 0 {
            frame.push(b'\n');
        }
        frame.extend_from_slice(FRAME_PREFIX_V1);
        frame.extend_from_slice(format!("{:016x}", json.len()).as_bytes());
        frame.push(b' ');
        frame.extend_from_slice(&json);
        frame.push(b'\n');

        self.attempt_pending(PendingAppend {
            event_id: wire.event_id,
            sequence,
            record_hash,
            payload_hash,
            commitment_hash,
            frame,
            base_byte_len: self.byte_len,
            base_head: self.head.clone(),
            base_ends_with_newline: self.ends_with_newline,
        })
    }

    fn attempt_pending(&mut self, pending: PendingAppend) -> CanonicalAppendOutcome {
        let observed_len = match self.file.len() {
            Ok(len) => len,
            Err(_) => {
                return CanonicalAppendOutcome::Rejected(
                    CanonicalAppendRejection::ConcurrentModification,
                );
            }
        };
        if observed_len != pending.base_byte_len {
            return CanonicalAppendOutcome::Rejected(
                CanonicalAppendRejection::ConcurrentModification,
            );
        }

        match self.file.write_once(&pending.frame) {
            Ok(written) if written == pending.frame.len() => {}
            Ok(_) => {
                return self.poison_with(
                    pending,
                    CanonicalAppendPhase::Write,
                    CanonicalAppendUncertaintyReason::ShortWrite,
                );
            }
            Err(error) => {
                return self.poison_with(
                    pending,
                    CanonicalAppendPhase::Write,
                    CanonicalAppendUncertaintyReason::Io(error.kind()),
                );
            }
        }
        if let Err(error) = self.file.flush() {
            return self.poison_with(
                pending,
                CanonicalAppendPhase::Flush,
                CanonicalAppendUncertaintyReason::Io(error.kind()),
            );
        }
        if let Err(error) = self.file.sync_all() {
            return self.poison_with(
                pending,
                CanonicalAppendPhase::Sync,
                CanonicalAppendUncertaintyReason::Io(error.kind()),
            );
        }

        self.commit_pending(
            pending,
            CanonicalDurability::FileDataAndMetadataSynced,
            false,
        )
    }

    fn poison_with(
        &mut self,
        pending: PendingAppend,
        phase: CanonicalAppendPhase,
        reason: CanonicalAppendUncertaintyReason,
    ) -> CanonicalAppendOutcome {
        let sequence = pending.sequence;
        self.poisoned = Some(pending);
        CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
            sequence,
            phase,
            reason,
        })
    }

    fn recover_poisoned(&mut self) -> CanonicalAppendOutcome {
        let Some(pending) = self.poisoned.clone() else {
            return CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned);
        };
        let bytes = match self.file.read_all() {
            Ok(bytes) => bytes,
            Err(error) => {
                return uncertain(
                    pending.sequence,
                    CanonicalAppendPhase::RecoveryRead,
                    CanonicalAppendUncertaintyReason::Io(error.kind()),
                );
            }
        };
        self.full_scan_count = self.full_scan_count.saturating_add(1);

        let raw = match parse_structural_records(
            &bytes,
            &self.session_id,
            &self.stream_id,
            self.domain_schema_version,
        ) {
            Ok(snapshot) => snapshot,
            Err(failure)
                if failure.repairable_unterminated_tail
                    && self.tail_recovery == CanonicalTailRecovery::QuarantineUnterminatedTail =>
            {
                return self.recover_exact_pending_suffix(&bytes, pending);
            }
            Err(failure) => {
                return recovery_required(
                    pending.sequence,
                    CanonicalAppendRecoveryReason::Stream(failure.error),
                );
            }
        };

        let cache = match cache_from_raw::<T>(&raw) {
            Ok(cache) => cache,
            Err(error) => {
                return recovery_required(
                    pending.sequence,
                    CanonicalAppendRecoveryReason::Stream(error),
                );
            }
        };
        match cache.events.get(&pending.event_id) {
            Some(existing)
                if existing.sequence == pending.sequence
                    && existing.record_hash == pending.record_hash
                    && existing.commitment_hash == pending.commitment_hash =>
            {
                if cache.head.as_ref().map(|head| head.sequence) != Some(pending.sequence) {
                    return recovery_required(
                        pending.sequence,
                        CanonicalAppendRecoveryReason::ConcurrentModification,
                    );
                }
                if let Err(error) = self.file.flush() {
                    return uncertain(
                        pending.sequence,
                        CanonicalAppendPhase::RecoveryFlush,
                        CanonicalAppendUncertaintyReason::Io(error.kind()),
                    );
                }
                if let Err(error) = self.file.sync_all() {
                    return uncertain(
                        pending.sequence,
                        CanonicalAppendPhase::RecoverySync,
                        CanonicalAppendUncertaintyReason::Io(error.kind()),
                    );
                }
                self.head = cache.head;
                self.events = cache.events;
                self.byte_len = cache.byte_len;
                self.ends_with_newline = cache.ends_with_newline;
                self.poisoned = None;
                CanonicalAppendOutcome::AlreadyAccepted(CanonicalAppendDurabilityReceipt {
                    sequence: pending.sequence,
                    record_hash: pending.record_hash,
                    payload_hash: pending.payload_hash,
                    appended_bytes: 0,
                    durability: CanonicalDurability::RecoveryBarrierSynced,
                })
            }
            Some(_) => recovery_required(
                pending.sequence,
                CanonicalAppendRecoveryReason::EventConflict,
            ),
            None if cache.byte_len == pending.base_byte_len
                && cache.head == pending.base_head
                && cache.ends_with_newline == pending.base_ends_with_newline =>
            {
                self.attempt_pending(pending)
            }
            None if self.tail_recovery == CanonicalTailRecovery::QuarantineUnterminatedTail => {
                self.recover_exact_pending_suffix(&bytes, pending)
            }
            None => recovery_required(
                pending.sequence,
                CanonicalAppendRecoveryReason::ConcurrentModification,
            ),
        }
    }

    /// Reconcile only bytes proven to be an exact prefix of this appender's
    /// immutable attempted frame, following the semantic stream state from
    /// which that frame was built. Length alone is not append identity.
    fn recover_exact_pending_suffix(
        &mut self,
        bytes: &[u8],
        pending: PendingAppend,
    ) -> CanonicalAppendOutcome {
        let Ok(base_len) = usize::try_from(pending.base_byte_len) else {
            return recovery_required(
                pending.sequence,
                CanonicalAppendRecoveryReason::ConcurrentModification,
            );
        };
        if bytes.len() < base_len {
            return recovery_required(
                pending.sequence,
                CanonicalAppendRecoveryReason::ConcurrentModification,
            );
        }

        let base = match parse_structural_records(
            &bytes[..base_len],
            &self.session_id,
            &self.stream_id,
            self.domain_schema_version,
        ) {
            Ok(base) => base,
            Err(failure) => {
                return recovery_required(
                    pending.sequence,
                    CanonicalAppendRecoveryReason::Stream(failure.error),
                );
            }
        };
        let base_cache = match cache_from_raw::<T>(&base) {
            Ok(cache) => cache,
            Err(error) => {
                return recovery_required(
                    pending.sequence,
                    CanonicalAppendRecoveryReason::Stream(error),
                );
            }
        };
        if base_cache.byte_len != pending.base_byte_len
            || base_cache.head != pending.base_head
            || base_cache.ends_with_newline != pending.base_ends_with_newline
        {
            return recovery_required(
                pending.sequence,
                CanonicalAppendRecoveryReason::ConcurrentModification,
            );
        }

        let suffix = &bytes[base_len..];
        if suffix.is_empty() {
            return self.attempt_pending(pending);
        }
        if suffix.len() >= pending.frame.len() || !pending.frame.starts_with(suffix) {
            return recovery_required(
                pending.sequence,
                CanonicalAppendRecoveryReason::ConcurrentModification,
            );
        }

        let quarantine_path = match create_quarantine_file(&self.path, suffix) {
            Ok(path) => path,
            Err((_, kind)) => {
                return uncertain(
                    pending.sequence,
                    CanonicalAppendPhase::RecoveryQuarantine,
                    CanonicalAppendUncertaintyReason::Io(kind),
                );
            }
        };
        self.quarantine_receipts
            .push(CanonicalTailQuarantineReceipt {
                quarantine_path,
                retained_bytes: pending.base_byte_len,
                quarantined_bytes: suffix.len() as u64,
            });
        if let Err(error) = self.file.truncate(pending.base_byte_len) {
            return uncertain(
                pending.sequence,
                CanonicalAppendPhase::RecoveryTruncate,
                CanonicalAppendUncertaintyReason::Io(error.kind()),
            );
        }
        if let Err(error) = self.file.sync_all() {
            return uncertain(
                pending.sequence,
                CanonicalAppendPhase::RecoverySync,
                CanonicalAppendUncertaintyReason::Io(error.kind()),
            );
        }
        self.attempt_pending(pending)
    }

    fn commit_pending(
        &mut self,
        pending: PendingAppend,
        durability: CanonicalDurability,
        already_accepted: bool,
    ) -> CanonicalAppendOutcome {
        self.byte_len = self.byte_len.saturating_add(pending.frame.len() as u64);
        self.ends_with_newline = true;
        self.head = Some(CanonicalStreamHead {
            sequence: pending.sequence,
            event_id: pending.event_id.clone(),
            record_hash: pending.record_hash.clone(),
        });
        self.events.insert(
            pending.event_id,
            CachedEvent {
                sequence: pending.sequence,
                record_hash: pending.record_hash.clone(),
                payload_hash: pending.payload_hash.clone(),
                commitment_hash: pending.commitment_hash,
            },
        );
        self.poisoned = None;
        let receipt = CanonicalAppendDurabilityReceipt {
            sequence: pending.sequence,
            record_hash: pending.record_hash,
            payload_hash: pending.payload_hash,
            appended_bytes: pending.frame.len() as u64,
            durability,
        };
        if already_accepted {
            CanonicalAppendOutcome::AlreadyAccepted(receipt)
        } else {
            CanonicalAppendOutcome::Accepted(receipt)
        }
    }
}

fn uncertain(
    sequence: u64,
    phase: CanonicalAppendPhase,
    reason: CanonicalAppendUncertaintyReason,
) -> CanonicalAppendOutcome {
    CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
        sequence,
        phase,
        reason,
    })
}

fn recovery_required(
    sequence: u64,
    reason: CanonicalAppendRecoveryReason,
) -> CanonicalAppendOutcome {
    CanonicalAppendOutcome::RecoveryRequired(CanonicalAppendRecoveryRequired { sequence, reason })
}

fn raw_to_typed<T: DeserializeOwned>(
    raw: RawSnapshot,
) -> Result<CanonicalLogSnapshot<T>, CanonicalLogError> {
    let mut records = Vec::with_capacity(raw.records.len());
    for (index, record) in raw.records.into_iter().enumerate() {
        let payload = serde_json::from_value(record.payload).map_err(|_| {
            CanonicalLogError::PayloadDecode {
                record_index: index + 1,
            }
        })?;
        records.push(CanonicalRecord {
            encoding: record.encoding,
            session_id: record.session_id,
            stream_id: record.stream_id,
            domain_schema_version: record.domain_schema_version,
            sequence: record.sequence,
            event_id: record.event_id,
            causal_event_ids: record.causal_event_ids,
            basis_heads: record.basis_heads,
            previous_hash: record.previous_hash,
            payload_hash: record.payload_hash,
            record_hash: record.record_hash,
            payload,
        });
    }
    let head = records.last().map(|record| CanonicalStreamHead {
        sequence: record.sequence,
        event_id: record.event_id.clone(),
        record_hash: record.record_hash.clone(),
    });
    Ok(CanonicalLogSnapshot {
        records,
        head,
        tail_quarantine: raw.tail_quarantine,
    })
}

fn validate_payload_schema<T: DeserializeOwned>(
    records: &[RawCanonicalRecord],
) -> Result<(), CanonicalLogError> {
    for (index, record) in records.iter().enumerate() {
        serde_json::from_value::<T>(record.payload.clone()).map_err(|_| {
            CanonicalLogError::PayloadDecode {
                record_index: index + 1,
            }
        })?;
    }
    Ok(())
}

fn cache_from_raw<T: DeserializeOwned>(
    raw: &RawSnapshot,
) -> Result<CachedStreamState, CanonicalLogError> {
    validate_payload_schema::<T>(&raw.records)?;
    let mut events = HashMap::with_capacity(raw.records.len());
    for record in &raw.records {
        events.insert(
            record.event_id.clone(),
            CachedEvent {
                sequence: record.sequence,
                record_hash: record.record_hash.clone(),
                payload_hash: record.payload_hash.clone(),
                commitment_hash: record.commitment_hash.clone(),
            },
        );
    }
    let head = raw.records.last().map(|record| CanonicalStreamHead {
        sequence: record.sequence,
        event_id: record.event_id.clone(),
        record_hash: record.record_hash.clone(),
    });
    Ok(CachedStreamState {
        head,
        events,
        byte_len: raw.byte_len,
        ends_with_newline: raw.ends_with_newline,
    })
}

fn parse_structural_records(
    bytes: &[u8],
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
) -> Result<RawSnapshot, StructuralFailure> {
    let mut records = Vec::new();
    let mut event_ids = HashSet::new();
    let mut offset = 0;
    let mut framed_seen = false;

    while offset < bytes.len() {
        let newline = bytes[offset..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|relative| offset + relative);
        let newline_terminated = newline.is_some();
        let line_end = newline.unwrap_or(bytes.len());
        let mut line = &bytes[offset..line_end];
        if newline_terminated && line.ends_with(b"\r") {
            line = &line[..line.len() - 1];
        }

        // Match the existing load_jsonl behavior: whitespace-only legacy rows
        // are ignored and do not consume a sequence number.
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            offset = line_end + usize::from(newline_terminated);
            continue;
        }

        let record_index = records.len() + 1;
        let expected_sequence = record_index as u64;
        let previous_hash = records
            .last()
            .map_or(ZERO_HASH, |record: &RawCanonicalRecord| {
                record.record_hash.as_str()
            });

        match parse_structural_record(
            line,
            session_id,
            stream_id,
            domain_schema_version,
            expected_sequence,
            previous_hash,
            framed_seen,
        ) {
            Ok(record) => {
                if record.encoding == CanonicalRecordEncoding::FramedV1 && !newline_terminated {
                    return Err(StructuralFailure {
                        error: CanonicalLogError::CorruptRecord {
                            record_index,
                            reason: CanonicalCorruptionReason::MissingFrameTerminator,
                            newline_terminated: false,
                        },
                        valid_up_to: offset,
                        repairable_unterminated_tail: true,
                    });
                }
                if !event_ids.insert(record.event_id.clone()) {
                    return Err(StructuralFailure {
                        error: CanonicalLogError::CorruptRecord {
                            record_index,
                            reason: CanonicalCorruptionReason::DuplicateEventId,
                            newline_terminated,
                        },
                        valid_up_to: offset,
                        repairable_unterminated_tail: !newline_terminated,
                    });
                }
                framed_seen |= record.encoding == CanonicalRecordEncoding::FramedV1;
                records.push(record);
            }
            Err(reason) => {
                return Err(StructuralFailure {
                    error: CanonicalLogError::CorruptRecord {
                        record_index,
                        reason,
                        newline_terminated,
                    },
                    valid_up_to: offset,
                    repairable_unterminated_tail: !newline_terminated,
                });
            }
        }
        offset = line_end + usize::from(newline_terminated);
    }

    Ok(RawSnapshot {
        records,
        tail_quarantine: None,
        byte_len: bytes.len() as u64,
        ends_with_newline: bytes.is_empty() || bytes.ends_with(b"\n"),
    })
}

#[allow(clippy::too_many_arguments)]
fn parse_structural_record(
    line: &[u8],
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
    expected_sequence: u64,
    expected_previous_hash: &str,
    framed_seen: bool,
) -> Result<RawCanonicalRecord, CanonicalCorruptionReason> {
    if line.is_empty() {
        return Err(CanonicalCorruptionReason::EmptyRecord);
    }
    if line.starts_with(FRAME_PREFIX_V1) {
        return parse_framed_v1(
            line,
            session_id,
            stream_id,
            domain_schema_version,
            expected_sequence,
            expected_previous_hash,
        );
    }
    if line.starts_with(FRAME_MAGIC) {
        return Err(CanonicalCorruptionReason::UnsupportedFrameVersion);
    }
    if framed_seen {
        return Err(CanonicalCorruptionReason::LegacyRecordAfterFramedRecord);
    }

    let payload: Value =
        serde_json::from_slice(line).map_err(|_| CanonicalCorruptionReason::InvalidJson)?;
    let payload = canonicalize_json_value(payload);
    let payload_hash =
        payload_digest(&payload).map_err(|_| CanonicalCorruptionReason::InvalidJson)?;
    let event_id = legacy_event_id(
        session_id,
        stream_id,
        domain_schema_version,
        expected_sequence,
        &payload_hash,
    );
    let causal_event_ids = Vec::new();
    let basis_heads = BTreeMap::new();
    let record_hash = legacy_record_hash(
        session_id,
        stream_id,
        domain_schema_version,
        expected_sequence,
        &event_id,
        &causal_event_ids,
        &basis_heads,
        expected_previous_hash,
        &payload_hash,
    )
    .map_err(|_| CanonicalCorruptionReason::InvalidJson)?;
    let commitment_hash = event_commitment_hash(
        session_id,
        stream_id,
        domain_schema_version,
        &event_id,
        &causal_event_ids,
        &basis_heads,
        &payload_hash,
    )
    .map_err(|_| CanonicalCorruptionReason::InvalidJson)?;
    Ok(RawCanonicalRecord {
        encoding: CanonicalRecordEncoding::LegacyJsonl,
        session_id: session_id.to_string(),
        stream_id: stream_id.to_string(),
        domain_schema_version,
        sequence: expected_sequence,
        event_id,
        causal_event_ids,
        basis_heads,
        previous_hash: expected_previous_hash.to_string(),
        payload_hash,
        record_hash,
        commitment_hash,
        payload,
    })
}

fn parse_framed_v1(
    line: &[u8],
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
    expected_sequence: u64,
    expected_previous_hash: &str,
) -> Result<RawCanonicalRecord, CanonicalCorruptionReason> {
    let length_start = FRAME_PREFIX_V1.len();
    let length_end = length_start + FRAME_LENGTH_HEX_BYTES;
    if line.len() <= length_end || line.get(length_end) != Some(&b' ') {
        return Err(CanonicalCorruptionReason::InvalidFrame);
    }
    let length_text = std::str::from_utf8(&line[length_start..length_end])
        .map_err(|_| CanonicalCorruptionReason::InvalidFrame)?;
    if !length_text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CanonicalCorruptionReason::InvalidFrame);
    }
    let declared_length = usize::from_str_radix(length_text, 16)
        .map_err(|_| CanonicalCorruptionReason::InvalidFrame)?;
    if declared_length > MAX_FRAME_JSON_BYTES {
        return Err(CanonicalCorruptionReason::FrameTooLarge);
    }
    let json = &line[length_end + 1..];
    if json.len() != declared_length {
        return Err(CanonicalCorruptionReason::FrameLengthMismatch);
    }
    let UniqueJsonValue(unique_json) =
        serde_json::from_slice(json).map_err(|_| CanonicalCorruptionReason::InvalidJson)?;
    let mut wire: WireRecordV1 =
        serde_json::from_value(unique_json).map_err(|_| CanonicalCorruptionReason::InvalidJson)?;
    if wire.format_version != CANONICAL_LOG_FORMAT_VERSION {
        return Err(CanonicalCorruptionReason::EnvelopeVersionMismatch);
    }
    if wire.session_id != session_id {
        return Err(CanonicalCorruptionReason::SessionMismatch);
    }
    if wire.stream_id != stream_id {
        return Err(CanonicalCorruptionReason::StreamMismatch);
    }
    if wire.domain_schema_version != domain_schema_version {
        return Err(CanonicalCorruptionReason::DomainSchemaVersionMismatch);
    }
    if wire.sequence != expected_sequence {
        return Err(CanonicalCorruptionReason::SequenceMismatch);
    }
    if wire.previous_hash != expected_previous_hash {
        return Err(CanonicalCorruptionReason::PreviousHashMismatch);
    }
    wire.payload = canonicalize_json_value(wire.payload);
    validate_stored_metadata(&wire.event_id, &wire.causal_event_ids, &wire.basis_heads)?;
    let payload_hash =
        payload_digest(&wire.payload).map_err(|_| CanonicalCorruptionReason::InvalidJson)?;
    if payload_hash != wire.payload_hash {
        return Err(CanonicalCorruptionReason::PayloadHashMismatch);
    }
    let record_hash = framed_record_hash(
        session_id,
        stream_id,
        domain_schema_version,
        wire.sequence,
        &wire.event_id,
        &wire.causal_event_ids,
        &wire.basis_heads,
        &wire.previous_hash,
        &wire.payload_hash,
    )
    .map_err(|_| CanonicalCorruptionReason::InvalidJson)?;
    if record_hash != wire.record_hash {
        return Err(CanonicalCorruptionReason::RecordHashMismatch);
    }
    let commitment_hash = event_commitment_hash(
        session_id,
        stream_id,
        domain_schema_version,
        &wire.event_id,
        &wire.causal_event_ids,
        &wire.basis_heads,
        &wire.payload_hash,
    )
    .map_err(|_| CanonicalCorruptionReason::InvalidJson)?;
    Ok(RawCanonicalRecord {
        encoding: CanonicalRecordEncoding::FramedV1,
        session_id: wire.session_id,
        stream_id: wire.stream_id,
        domain_schema_version: wire.domain_schema_version,
        sequence: wire.sequence,
        event_id: wire.event_id,
        causal_event_ids: wire.causal_event_ids,
        basis_heads: wire.basis_heads,
        previous_hash: wire.previous_hash,
        payload_hash: wire.payload_hash,
        record_hash: wire.record_hash,
        commitment_hash,
        payload: wire.payload,
    })
}

fn validate_stream_context(
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
) -> Result<(), CanonicalLogError> {
    if !valid_identifier(session_id) {
        return Err(CanonicalLogError::InvalidSessionId);
    }
    if !valid_identifier(stream_id) {
        return Err(CanonicalLogError::InvalidStreamId);
    }
    if domain_schema_version == 0 {
        return Err(CanonicalLogError::InvalidDomainSchemaVersion);
    }
    Ok(())
}

fn normalize_event_metadata(
    metadata: &CanonicalEventMetadata,
) -> Result<CanonicalEventMetadata, CanonicalAppendRejection> {
    if !valid_identifier(&metadata.event_id) {
        return Err(CanonicalAppendRejection::InvalidEventId);
    }
    let mut causal_event_ids = metadata.causal_event_ids.clone();
    if causal_event_ids.iter().any(|id| !valid_identifier(id)) {
        return Err(CanonicalAppendRejection::InvalidCausalEventId);
    }
    causal_event_ids.sort();
    if causal_event_ids.windows(2).any(|ids| ids[0] == ids[1]) {
        return Err(CanonicalAppendRejection::InvalidCausalEventId);
    }
    if !valid_basis_heads(&metadata.basis_heads) {
        return Err(CanonicalAppendRejection::InvalidBasisHead);
    }
    Ok(CanonicalEventMetadata {
        event_id: metadata.event_id.clone(),
        causal_event_ids,
        basis_heads: metadata.basis_heads.clone(),
    })
}

fn validate_stored_metadata(
    event_id: &str,
    causal_event_ids: &[String],
    basis_heads: &CanonicalBasisHeadVector,
) -> Result<(), CanonicalCorruptionReason> {
    if !valid_identifier(event_id) {
        return Err(CanonicalCorruptionReason::InvalidEventId);
    }
    if causal_event_ids.iter().any(|id| !valid_identifier(id))
        || causal_event_ids.windows(2).any(|ids| ids[0] >= ids[1])
    {
        return Err(CanonicalCorruptionReason::InvalidCausalEventId);
    }
    if !valid_basis_heads(basis_heads) {
        return Err(CanonicalCorruptionReason::InvalidBasisHead);
    }
    Ok(())
}

fn valid_basis_heads(basis_heads: &CanonicalBasisHeadVector) -> bool {
    basis_heads.iter().all(|(stream_id, head)| {
        valid_identifier(stream_id)
            && head.sequence > 0
            && valid_identifier(&head.event_id)
            && valid_hash(&head.record_hash)
    })
}

fn valid_identifier(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier.len() <= MAX_IDENTIFIER_BYTES
        && !identifier.chars().any(char::is_control)
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Canonical-log v1 treats JSON object member order as non-semantic. Sorting
/// every object before both storage and hashing keeps commitments independent
/// of `serde_json::Map`'s Cargo-feature-selected backing map. Array order and
/// scalar representation remain part of the v1 payload contract.
fn canonicalize_json_value(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object
                .into_iter()
                .map(|(key, value)| (key, canonicalize_json_value(value)))
                .collect::<Vec<_>>();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            let mut canonical = serde_json::Map::new();
            for (key, value) in entries {
                canonical.insert(key, value);
            }
            Value::Object(canonical)
        }
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonicalize_json_value).collect())
        }
        scalar => scalar,
    }
}

fn payload_digest(payload: &Value) -> Result<String, serde_json::Error> {
    let canonical = canonicalize_json_value(payload.clone());
    serde_json::to_vec(&canonical).map(|bytes| hash_fields(&[b"payload-v1", &bytes]))
}

fn legacy_event_id(
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
    sequence: u64,
    payload_hash: &str,
) -> String {
    let domain_schema_version = domain_schema_version.to_be_bytes();
    let sequence = sequence.to_be_bytes();
    format!(
        "legacy-v1-{}",
        hash_fields(&[
            b"legacy-event-id-v1",
            session_id.as_bytes(),
            stream_id.as_bytes(),
            &domain_schema_version,
            &sequence,
            payload_hash.as_bytes(),
        ])
    )
}

#[allow(clippy::too_many_arguments)]
fn legacy_record_hash(
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
    sequence: u64,
    event_id: &str,
    causal_event_ids: &[String],
    basis_heads: &CanonicalBasisHeadVector,
    previous_hash: &str,
    payload_hash: &str,
) -> Result<String, serde_json::Error> {
    record_hash(
        b"legacy-record-v1",
        session_id,
        stream_id,
        domain_schema_version,
        sequence,
        event_id,
        causal_event_ids,
        basis_heads,
        previous_hash,
        payload_hash,
    )
}

#[allow(clippy::too_many_arguments)]
fn framed_record_hash(
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
    sequence: u64,
    event_id: &str,
    causal_event_ids: &[String],
    basis_heads: &CanonicalBasisHeadVector,
    previous_hash: &str,
    payload_hash: &str,
) -> Result<String, serde_json::Error> {
    record_hash(
        b"framed-record-v1",
        session_id,
        stream_id,
        domain_schema_version,
        sequence,
        event_id,
        causal_event_ids,
        basis_heads,
        previous_hash,
        payload_hash,
    )
}

#[allow(clippy::too_many_arguments)]
fn record_hash(
    domain: &[u8],
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
    sequence: u64,
    event_id: &str,
    causal_event_ids: &[String],
    basis_heads: &CanonicalBasisHeadVector,
    previous_hash: &str,
    payload_hash: &str,
) -> Result<String, serde_json::Error> {
    let domain_schema_version = domain_schema_version.to_be_bytes();
    let sequence = sequence.to_be_bytes();
    let causal = serde_json::to_vec(causal_event_ids)?;
    let basis = serde_json::to_vec(basis_heads)?;
    Ok(hash_fields(&[
        domain,
        &[CANONICAL_LOG_FORMAT_VERSION],
        session_id.as_bytes(),
        stream_id.as_bytes(),
        &domain_schema_version,
        &sequence,
        event_id.as_bytes(),
        &causal,
        &basis,
        previous_hash.as_bytes(),
        payload_hash.as_bytes(),
    ]))
}

fn event_commitment_hash(
    session_id: &str,
    stream_id: &str,
    domain_schema_version: u32,
    event_id: &str,
    causal_event_ids: &[String],
    basis_heads: &CanonicalBasisHeadVector,
    payload_hash: &str,
) -> Result<String, serde_json::Error> {
    let domain_schema_version = domain_schema_version.to_be_bytes();
    let causal = serde_json::to_vec(causal_event_ids)?;
    let basis = serde_json::to_vec(basis_heads)?;
    Ok(hash_fields(&[
        b"event-commitment-v1",
        &[CANONICAL_LOG_FORMAT_VERSION],
        session_id.as_bytes(),
        stream_id.as_bytes(),
        &domain_schema_version,
        event_id.as_bytes(),
        &causal,
        &basis,
        payload_hash.as_bytes(),
    ]))
}

fn hash_fields(fields: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const SESSION: &str = "session-1";
    const STREAM: &str = "transcript";
    const SCHEMA: u32 = 1;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TestPayload {
        value: u64,
    }

    fn temp_log(label: &str) -> PathBuf {
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "audio-graph-canonical-log-{label}-{}-{nonce}",
                std::process::id()
            ))
            .join("events.jsonl")
    }

    fn cleanup(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    fn event(event_id: &str) -> CanonicalEventMetadata {
        CanonicalEventMetadata::new(event_id)
    }

    fn open_appender(path: &Path) -> CanonicalAppender<TestPayload> {
        CanonicalAppender::open(
            path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::QuarantineUnterminatedTail,
        )
        .expect("open canonical appender")
    }

    #[test]
    fn strict_reader_missing_file_is_redacted_not_found_and_non_mutating() {
        let path = temp_log("missing-read");
        cleanup(&path);
        let parent = path.parent().expect("fixture parent");

        let error = load_canonical_stream::<TestPayload>(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect_err("a missing stream must remain distinguishable from present-empty");

        assert_eq!(
            error,
            CanonicalLogError::Io {
                operation: CanonicalIoOperation::Read,
                kind: io::ErrorKind::NotFound,
            }
        );
        assert!(!error.to_string().contains(SESSION));
        assert!(!format!("{error:?}").contains(&path.display().to_string()));
        assert!(!path.exists());
        assert!(!parent.exists());
    }

    #[test]
    fn framed_round_trip_binds_context_metadata_and_hash_chain() {
        let path = temp_log("round-trip");
        let mut appender = open_appender(&path);
        let mut second_metadata = event("event-2");
        second_metadata.causal_event_ids = vec!["event-1".to_string()];
        second_metadata.basis_heads.insert(
            "speaker".to_string(),
            CanonicalBasisHead {
                sequence: 3,
                event_id: "speaker-3".to_string(),
                record_hash: "a".repeat(64),
            },
        );
        assert!(matches!(
            appender.append(&event("event-1"), &TestPayload { value: 1 }),
            CanonicalAppendOutcome::Accepted(_)
        ));
        assert!(matches!(
            appender.append(&second_metadata, &TestPayload { value: 2 }),
            CanonicalAppendOutcome::Accepted(_)
        ));
        drop(appender);

        let loaded: CanonicalLogSnapshot<TestPayload> = load_canonical_stream(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("load framed stream");
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.records[1].session_id, SESSION);
        assert_eq!(loaded.records[1].stream_id, STREAM);
        assert_eq!(loaded.records[1].domain_schema_version, SCHEMA);
        assert_eq!(loaded.records[1].causal_event_ids, vec!["event-1"]);
        assert_eq!(loaded.records[1].basis_heads.len(), 1);
        assert_eq!(
            loaded.records[1].previous_hash,
            loaded.records[0].record_hash
        );
        cleanup(&path);
    }

    #[test]
    fn v1_payload_commitment_is_key_canonical_and_fixture_stable() {
        let path = temp_log("v1-golden-fixture");
        let mut nested = serde_json::Map::new();
        nested.insert("b".to_string(), Value::from(2));
        nested.insert("a".to_string(), Value::from(1));
        let mut array_item = serde_json::Map::new();
        array_item.insert("y".to_string(), Value::Bool(true));
        array_item.insert("x".to_string(), Value::Null);
        let mut insertion_order_payload = serde_json::Map::new();
        insertion_order_payload.insert("z".to_string(), Value::Object(nested));
        insertion_order_payload.insert(
            "a".to_string(),
            Value::Array(vec![Value::Object(array_item)]),
        );

        let mut canonical_nested = serde_json::Map::new();
        canonical_nested.insert("a".to_string(), Value::from(1));
        canonical_nested.insert("b".to_string(), Value::from(2));
        let mut canonical_array_item = serde_json::Map::new();
        canonical_array_item.insert("x".to_string(), Value::Null);
        canonical_array_item.insert("y".to_string(), Value::Bool(true));
        let mut equivalent_payload = serde_json::Map::new();
        equivalent_payload.insert(
            "a".to_string(),
            Value::Array(vec![Value::Object(canonical_array_item)]),
        );
        equivalent_payload.insert("z".to_string(), Value::Object(canonical_nested));

        let mut appender = CanonicalAppender::<Value>::open(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("open value appender");
        assert!(matches!(
            appender.append(
                &event("fixture-event"),
                &Value::Object(insertion_order_payload)
            ),
            CanonicalAppendOutcome::Accepted(_)
        ));
        assert!(matches!(
            appender.append(&event("fixture-event"), &Value::Object(equivalent_payload)),
            CanonicalAppendOutcome::AlreadyAccepted(_)
        ));
        drop(appender);

        let frame = fs::read_to_string(&path).expect("read fixture frame");
        let loaded: CanonicalLogSnapshot<Value> = load_canonical_stream(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("load fixture frame");
        const EXPECTED_FRAME: &str = concat!(
            "AGCL1 00000000000001dd ",
            "{\"format_version\":1,\"session_id\":\"session-1\",",
            "\"stream_id\":\"transcript\",\"domain_schema_version\":1,",
            "\"sequence\":1,\"event_id\":\"fixture-event\",",
            "\"causal_event_ids\":[],\"basis_heads\":{},",
            "\"previous_hash\":\"0000000000000000000000000000000000000000000000000000000000000000\",",
            "\"payload_hash\":\"86263c62c3e78ee4187c5e8af70b580b3f125af7c298dc37ee36dff66ae17f3a\",",
            "\"record_hash\":\"736878be50bdbd000b421b9122c5f2e2f1e5799af21b01c0d2e9c8d8039b3c6a\",",
            "\"payload\":{\"a\":[{\"x\":null,\"y\":true}],\"z\":{\"a\":1,\"b\":2}}}\n"
        );
        assert_eq!(frame, EXPECTED_FRAME);
        assert_eq!(
            loaded.records[0].payload_hash,
            "86263c62c3e78ee4187c5e8af70b580b3f125af7c298dc37ee36dff66ae17f3a"
        );
        assert_eq!(
            loaded.records[0].record_hash,
            "736878be50bdbd000b421b9122c5f2e2f1e5799af21b01c0d2e9c8d8039b3c6a"
        );
        assert_eq!(
            loaded.head,
            Some(CanonicalStreamHead {
                sequence: 1,
                event_id: "fixture-event".to_string(),
                record_hash: "736878be50bdbd000b421b9122c5f2e2f1e5799af21b01c0d2e9c8d8039b3c6a"
                    .to_string(),
            })
        );
        assert_eq!(
            loaded.records[0].payload,
            serde_json::json!({"a": [{"x": null, "y": true}], "z": {"a": 1, "b": 2}})
        );
        cleanup(&path);
    }

    #[test]
    fn v1_scalar_encoding_fixture_is_stable() {
        let path = temp_log("v1-scalar-fixture");
        let payload = serde_json::json!({
            "unsigned": u64::MAX,
            "unicode": "Grüße 東京 🦀",
            "signed": i64::MIN,
            "fraction": 0.125,
            "escaped": "line\nquote\"slash\\backspace\u{0008}",
        });
        let mut appender = CanonicalAppender::<Value>::open(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("open scalar fixture appender");
        assert!(matches!(
            appender.append(&event("scalar-fixture"), &payload),
            CanonicalAppendOutcome::Accepted(_)
        ));
        drop(appender);
        let frame = fs::read_to_string(&path).expect("read scalar fixture frame");
        let loaded: CanonicalLogSnapshot<Value> = load_canonical_stream(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("load scalar fixture");
        const EXPECTED_FRAME: &str = concat!(
            "AGCL1 000000000000024d ",
            "{\"format_version\":1,\"session_id\":\"session-1\",",
            "\"stream_id\":\"transcript\",\"domain_schema_version\":1,",
            "\"sequence\":1,\"event_id\":\"scalar-fixture\",",
            "\"causal_event_ids\":[],\"basis_heads\":{},",
            "\"previous_hash\":\"0000000000000000000000000000000000000000000000000000000000000000\",",
            "\"payload_hash\":\"7ddb86a82c1e1e03a0f12566f7fd48856b111f5b3241e5ed8c8be2feb7986572\",",
            "\"record_hash\":\"be8718055bb3bf7f62d5c808c2fdc834a617504939f99e8511968d182c6adbdf\",",
            "\"payload\":{\"escaped\":\"line\\nquote\\\"slash\\\\backspace\\b\",",
            "\"fraction\":0.125,\"signed\":-9223372036854775808,",
            "\"unicode\":\"Grüße 東京 🦀\",\"unsigned\":18446744073709551615}}\n"
        );
        assert_eq!(frame, EXPECTED_FRAME);
        assert_eq!(
            loaded.records[0].payload_hash,
            "7ddb86a82c1e1e03a0f12566f7fd48856b111f5b3241e5ed8c8be2feb7986572"
        );
        assert_eq!(
            loaded.records[0].record_hash,
            "be8718055bb3bf7f62d5c808c2fdc834a617504939f99e8511968d182c6adbdf"
        );
        assert_eq!(loaded.records[0].payload, payload);
        assert_eq!(
            loaded.head,
            Some(CanonicalStreamHead {
                sequence: 1,
                event_id: "scalar-fixture".to_string(),
                record_hash: "be8718055bb3bf7f62d5c808c2fdc834a617504939f99e8511968d182c6adbdf"
                    .to_string(),
            })
        );
        cleanup(&path);
    }

    #[test]
    fn stable_event_id_is_idempotent_but_any_commitment_change_conflicts() {
        let path = temp_log("idempotency");
        let mut appender = open_appender(&path);
        let accepted = appender.append(&event("stable-event"), &TestPayload { value: 7 });
        let replay = appender.append(&event("stable-event"), &TestPayload { value: 7 });
        let payload_conflict = appender.append(&event("stable-event"), &TestPayload { value: 8 });
        let mut metadata_conflict = event("stable-event");
        metadata_conflict.causal_event_ids = vec!["other-event".to_string()];
        let metadata_conflict = appender.append(&metadata_conflict, &TestPayload { value: 7 });
        assert!(matches!(accepted, CanonicalAppendOutcome::Accepted(_)));
        assert!(matches!(
            replay,
            CanonicalAppendOutcome::AlreadyAccepted(CanonicalAppendDurabilityReceipt {
                appended_bytes: 0,
                durability: CanonicalDurability::ValidatedExistingRecord,
                ..
            })
        ));
        assert_eq!(
            payload_conflict,
            CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::EventIdConflict)
        );
        assert_eq!(
            metadata_conflict,
            CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::EventIdConflict)
        );
        drop(appender);
        cleanup(&path);
    }

    #[test]
    fn legacy_blank_lines_are_ignored_and_prefix_can_be_extended() {
        let path = temp_log("legacy-blank-lines");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, b"\n{\"value\":1}\n  \r\n{\"value\":2}   ").expect("write legacy log");

        let before: CanonicalLogSnapshot<TestPayload> = load_canonical_stream(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("load legacy stream");
        assert_eq!(before.records.len(), 2);

        let mut appender = open_appender(&path);
        assert!(matches!(
            appender.append(&event("event-3"), &TestPayload { value: 3 }),
            CanonicalAppendOutcome::Accepted(_)
        ));
        drop(appender);
        let after: CanonicalLogSnapshot<TestPayload> = load_canonical_stream(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("load extended stream");
        assert_eq!(after.records.len(), 3);
        assert_eq!(after.records[2].sequence, 3);
        cleanup(&path);
    }

    #[test]
    fn unterminated_corrupt_tail_is_quarantined_after_typed_prefix_validation() {
        let path = temp_log("tail-repair");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, b"{\"value\":1}\nprivate incomplete tail").expect("write damaged stream");

        let loaded: CanonicalLogSnapshot<TestPayload> = load_canonical_stream(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::QuarantineUnterminatedTail,
        )
        .expect("repair tail");
        assert_eq!(loaded.records.len(), 1);
        let receipt = loaded.tail_quarantine.expect("quarantine receipt");
        assert_eq!(
            fs::read(receipt.quarantine_path).expect("read quarantine"),
            b"private incomplete tail"
        );
        cleanup(&path);
    }

    #[test]
    fn appender_repairs_tail_through_its_exclusively_locked_handle() {
        let path = temp_log("locked-tail-repair");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, b"{\"value\":1}\nunterminated tail").expect("write damaged stream");

        let mut appender = open_appender(&path);
        assert_eq!(appender.take_quarantine_receipts().len(), 1);
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::Accepted(_)
        ));
        drop(appender);

        let loaded: CanonicalLogSnapshot<TestPayload> = load_canonical_stream(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("load repaired and extended stream");
        assert_eq!(loaded.records.len(), 2);
        cleanup(&path);
    }

    #[test]
    fn typed_invalid_prefix_prevents_recovery_mutation_and_appender_open() {
        let path = temp_log("typed-invalid");
        let original = b"{\"wrong\":1}\nunterminated tail";
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, original).expect("write schema-invalid stream");

        let error = load_canonical_stream::<TestPayload>(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::QuarantineUnterminatedTail,
        )
        .expect_err("typed-invalid prefix must fail before mutation");
        assert!(matches!(
            error,
            CanonicalLogError::PayloadDecode { record_index: 1 }
        ));
        assert_eq!(fs::read(&path).expect("read unchanged"), original);
        assert!(matches!(
            CanonicalAppender::<TestPayload>::open(
                &path,
                SESSION,
                STREAM,
                SCHEMA,
                CanonicalTailRecovery::QuarantineUnterminatedTail,
            ),
            Err(CanonicalLogError::PayloadDecode { record_index: 1 })
        ));
        cleanup(&path);
    }

    #[derive(Default)]
    struct FaultPlan {
        fail_reads: usize,
        fail_syncs: usize,
        fail_flushes: usize,
        fail_truncates: usize,
        short_writes: usize,
        exact_short_write: Option<usize>,
        write_error_after: Option<usize>,
        reads: usize,
        writes: usize,
        syncs: usize,
    }

    struct MemoryLockedFile {
        bytes: Arc<Mutex<Vec<u8>>>,
        plan: Arc<Mutex<FaultPlan>>,
    }

    impl LockedAppenderFile for MemoryLockedFile {
        fn read_all(&mut self) -> io::Result<Vec<u8>> {
            let mut plan = self.plan.lock().expect("plan");
            plan.reads += 1;
            if plan.fail_reads > 0 {
                plan.fail_reads -= 1;
                return Err(io::Error::from(io::ErrorKind::Other));
            }
            drop(plan);
            Ok(self.bytes.lock().expect("bytes").clone())
        }

        fn len(&self) -> io::Result<u64> {
            Ok(self.bytes.lock().expect("bytes").len() as u64)
        }

        fn write_once(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let mut plan = self.plan.lock().expect("plan");
            plan.writes += 1;
            if let Some(requested) = plan.write_error_after.take() {
                let written = requested.min(bytes.len());
                self.bytes
                    .lock()
                    .expect("bytes")
                    .extend_from_slice(&bytes[..written]);
                return Err(io::Error::from(io::ErrorKind::Other));
            }
            if let Some(requested) = plan.exact_short_write.take() {
                let written = requested.min(bytes.len());
                self.bytes
                    .lock()
                    .expect("bytes")
                    .extend_from_slice(&bytes[..written]);
                return Ok(written);
            }
            if plan.short_writes > 0 {
                plan.short_writes -= 1;
                let written = (bytes.len() / 2).max(1);
                self.bytes
                    .lock()
                    .expect("bytes")
                    .extend_from_slice(&bytes[..written]);
                return Ok(written);
            }
            self.bytes.lock().expect("bytes").extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            let mut plan = self.plan.lock().expect("plan");
            if plan.fail_flushes > 0 {
                plan.fail_flushes -= 1;
                return Err(io::Error::from(io::ErrorKind::Other));
            }
            Ok(())
        }

        fn sync_all(&mut self) -> io::Result<()> {
            let mut plan = self.plan.lock().expect("plan");
            plan.syncs += 1;
            if plan.fail_syncs > 0 {
                plan.fail_syncs -= 1;
                return Err(io::Error::from(io::ErrorKind::Other));
            }
            Ok(())
        }

        fn truncate(&mut self, len: u64) -> io::Result<()> {
            let mut plan = self.plan.lock().expect("plan");
            if plan.fail_truncates > 0 {
                plan.fail_truncates -= 1;
                return Err(io::Error::from(io::ErrorKind::Other));
            }
            drop(plan);
            self.bytes.lock().expect("bytes").truncate(len as usize);
            Ok(())
        }
    }

    type MemoryAppenderHarness = (
        CanonicalAppender<TestPayload>,
        Arc<Mutex<Vec<u8>>>,
        Arc<Mutex<FaultPlan>>,
    );

    fn memory_appender_with_bytes(path: PathBuf, initial_bytes: Vec<u8>) -> MemoryAppenderHarness {
        memory_appender_with_bytes_and_mode(
            path,
            initial_bytes,
            CanonicalTailRecovery::QuarantineUnterminatedTail,
        )
    }

    fn memory_appender_with_bytes_and_mode(
        path: PathBuf,
        initial_bytes: Vec<u8>,
        tail_recovery: CanonicalTailRecovery,
    ) -> MemoryAppenderHarness {
        let bytes = Arc::new(Mutex::new(initial_bytes));
        let plan = Arc::new(Mutex::new(FaultPlan::default()));
        let appender = CanonicalAppender::from_locked_file(
            path,
            SESSION.to_string(),
            STREAM.to_string(),
            SCHEMA,
            tail_recovery,
            Box::new(MemoryLockedFile {
                bytes: Arc::clone(&bytes),
                plan: Arc::clone(&plan),
            }),
        )
        .expect("open memory appender");
        (appender, bytes, plan)
    }

    fn memory_appender() -> MemoryAppenderHarness {
        memory_appender_with_bytes(PathBuf::from("redacted-test-log"), Vec::new())
    }

    fn attempted_frame_for_base(initial_bytes: &[u8]) -> Vec<u8> {
        let (mut appender, bytes, _) = memory_appender_with_bytes(
            PathBuf::from("redacted-frame-fixture"),
            initial_bytes.to_vec(),
        );
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::Accepted(_)
        ));
        let frame = bytes.lock().expect("bytes")[initial_bytes.len()..].to_vec();
        drop(appender);
        frame
    }

    fn assert_strict_reopen_bytes(bytes: &[u8], expected_records: usize) {
        let path = temp_log("strict-reopen-bytes");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, bytes).expect("persist memory stream for strict reopen");
        let reopened: CanonicalLogSnapshot<TestPayload> = load_canonical_stream(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("strict reopen after successful reconciliation");
        assert_eq!(reopened.records.len(), expected_records);
        assert_eq!(
            reopened.head.as_ref().map(|head| head.sequence),
            Some(expected_records as u64)
        );
        cleanup(&path);
    }

    #[test]
    fn uncertainty_retry_requires_fresh_sync_before_already_accepted() {
        let (mut appender, _, plan) = memory_appender();
        plan.lock().expect("plan").fail_syncs = 1;
        let first = appender.append(&event("event-1"), &TestPayload { value: 1 });
        assert!(matches!(
            first,
            CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                phase: CanonicalAppendPhase::Sync,
                ..
            })
        ));
        assert!(appender.recovery_required());
        let syncs_before_retry = plan.lock().expect("plan").syncs;

        let retry = appender.append(&event("event-1"), &TestPayload { value: 1 });
        assert!(matches!(
            retry,
            CanonicalAppendOutcome::AlreadyAccepted(CanonicalAppendDurabilityReceipt {
                durability: CanonicalDurability::RecoveryBarrierSynced,
                ..
            })
        ));
        assert!(plan.lock().expect("plan").syncs > syncs_before_retry);
        assert!(!appender.recovery_required());
        assert_eq!(appender.cached_event_count(), 1);
    }

    #[test]
    fn poisoned_appender_rejects_next_event_until_same_event_recovers() {
        let (mut appender, _, plan) = memory_appender();
        plan.lock().expect("plan").fail_syncs = 1;
        assert!(matches!(
            appender.append(&event("event-1"), &TestPayload { value: 1 }),
            CanonicalAppendOutcome::OutcomeUncertain(_)
        ));
        assert_eq!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned)
        );
        assert!(matches!(
            appender.append(&event("event-1"), &TestPayload { value: 1 }),
            CanonicalAppendOutcome::AlreadyAccepted(_)
        ));
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::Accepted(_)
        ));
    }

    #[test]
    fn initial_io_uncertainty_matrix_retains_poison_until_reconciled() {
        #[derive(Clone, Copy)]
        enum InitialFault {
            ZeroByteWriteError,
            FullFrameWriteError,
            Flush,
            Sync,
        }

        for fault in [
            InitialFault::ZeroByteWriteError,
            InitialFault::FullFrameWriteError,
            InitialFault::Flush,
            InitialFault::Sync,
        ] {
            let (mut appender, bytes, plan) = memory_appender();
            let expected_phase = match fault {
                InitialFault::ZeroByteWriteError | InitialFault::FullFrameWriteError => {
                    plan.lock().expect("plan").write_error_after = Some(match fault {
                        InitialFault::ZeroByteWriteError => 0,
                        InitialFault::FullFrameWriteError => usize::MAX,
                        InitialFault::Flush | InitialFault::Sync => unreachable!(),
                    });
                    CanonicalAppendPhase::Write
                }
                InitialFault::Flush => {
                    plan.lock().expect("plan").fail_flushes = 1;
                    CanonicalAppendPhase::Flush
                }
                InitialFault::Sync => {
                    plan.lock().expect("plan").fail_syncs = 1;
                    CanonicalAppendPhase::Sync
                }
            };

            assert_eq!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                    sequence: 1,
                    phase: expected_phase,
                    reason: CanonicalAppendUncertaintyReason::Io(io::ErrorKind::Other),
                })
            );
            assert!(appender.recovery_required());
            assert_eq!(
                appender.append(&event("event-2"), &TestPayload { value: 2 }),
                CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned)
            );

            let retry = appender.append(&event("event-1"), &TestPayload { value: 1 });
            match fault {
                InitialFault::ZeroByteWriteError => {
                    assert!(matches!(retry, CanonicalAppendOutcome::Accepted(_)));
                }
                InitialFault::FullFrameWriteError | InitialFault::Flush | InitialFault::Sync => {
                    assert!(matches!(
                        retry,
                        CanonicalAppendOutcome::AlreadyAccepted(CanonicalAppendDurabilityReceipt {
                            durability: CanonicalDurability::RecoveryBarrierSynced,
                            ..
                        })
                    ));
                }
            }
            assert!(!appender.recovery_required());
            assert!(appender.take_quarantine_receipts().is_empty());
            let final_bytes = bytes.lock().expect("bytes").clone();
            drop(appender);
            assert_strict_reopen_bytes(&final_bytes, 1);
        }
    }

    #[test]
    fn exact_write_boundaries_recover_or_fail_closed_by_mode() {
        let bases: [&[u8]; 3] = [b"", b"{\"value\":1}\n", b"{\"value\":1}"];

        for (base_index, base) in bases.into_iter().enumerate() {
            let frame = attempted_frame_for_base(base);
            let separator_len = usize::from(!base.is_empty() && !base.ends_with(b"\n"));
            let mut cuts = vec![
                0,
                1,
                separator_len + FRAME_PREFIX_V1.len() + FRAME_LENGTH_HEX_BYTES + 1,
                frame.len() - 1,
            ];
            cuts.sort_unstable();
            cuts.dedup();

            for mode in [
                CanonicalTailRecovery::QuarantineUnterminatedTail,
                CanonicalTailRecovery::Strict,
            ] {
                for &cut in &cuts {
                    assert!(cut < frame.len());
                    let path = temp_log(&format!("exact-cut-{base_index}-{mode:?}-{cut}"));
                    fs::create_dir_all(path.parent().expect("parent"))
                        .expect("create recovery parent");
                    let (mut appender, bytes, plan) =
                        memory_appender_with_bytes_and_mode(path.clone(), base.to_vec(), mode);
                    plan.lock().expect("plan").exact_short_write = Some(cut);

                    assert_eq!(
                        appender.append(&event("event-2"), &TestPayload { value: 2 }),
                        CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                            sequence: if base.is_empty() { 1 } else { 2 },
                            phase: CanonicalAppendPhase::Write,
                            reason: CanonicalAppendUncertaintyReason::ShortWrite,
                        })
                    );
                    assert!(appender.recovery_required());
                    assert_eq!(
                        appender.append(&event("other-event"), &TestPayload { value: 9 }),
                        CanonicalAppendOutcome::Rejected(
                            CanonicalAppendRejection::AppenderPoisoned
                        )
                    );
                    let before_retry = bytes.lock().expect("bytes").clone();
                    let retry = appender.append(&event("event-2"), &TestPayload { value: 2 });

                    if mode == CanonicalTailRecovery::Strict && cut > 0 {
                        assert!(matches!(retry, CanonicalAppendOutcome::RecoveryRequired(_)));
                        assert!(appender.recovery_required());
                        assert_eq!(*bytes.lock().expect("bytes"), before_retry);
                        assert!(appender.take_quarantine_receipts().is_empty());
                        assert!(matches!(
                            appender.append(&event("event-2"), &TestPayload { value: 2 }),
                            CanonicalAppendOutcome::RecoveryRequired(_)
                        ));
                        assert_eq!(*bytes.lock().expect("bytes"), before_retry);
                    } else {
                        assert!(matches!(retry, CanonicalAppendOutcome::Accepted(_)));
                        assert!(!appender.recovery_required());
                        let receipts = appender.take_quarantine_receipts();
                        assert_eq!(receipts.len(), usize::from(cut > 0));
                        let final_bytes = bytes.lock().expect("bytes").clone();
                        drop(appender);
                        assert_strict_reopen_bytes(
                            &final_bytes,
                            if base.is_empty() { 1 } else { 2 },
                        );
                        cleanup(&path);
                        continue;
                    }
                    drop(appender);
                    cleanup(&path);
                }

                let path = temp_log(&format!("complete-sync-{base_index}-{mode:?}"));
                let (mut appender, bytes, plan) =
                    memory_appender_with_bytes_and_mode(path, base.to_vec(), mode);
                plan.lock().expect("plan").fail_syncs = 1;
                assert!(matches!(
                    appender.append(&event("event-2"), &TestPayload { value: 2 }),
                    CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                        phase: CanonicalAppendPhase::Sync,
                        ..
                    })
                ));
                assert!(matches!(
                    appender.append(&event("event-2"), &TestPayload { value: 2 }),
                    CanonicalAppendOutcome::AlreadyAccepted(CanonicalAppendDurabilityReceipt {
                        durability: CanonicalDurability::RecoveryBarrierSynced,
                        ..
                    })
                ));
                assert!(!appender.recovery_required());
                let final_bytes = bytes.lock().expect("bytes").clone();
                drop(appender);
                assert_strict_reopen_bytes(&final_bytes, if base.is_empty() { 1 } else { 2 });
            }
        }
    }

    #[test]
    fn recovery_failure_matrix_retains_poison_and_mutates_only_proven_bytes() {
        // Recovery read failure: no bytes or receipts change, and the next
        // identical retry can still prove and repair the pending prefix.
        {
            let path = temp_log("recovery-read-failure");
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            let (mut appender, bytes, plan) = memory_appender_with_bytes(path.clone(), Vec::new());
            plan.lock().expect("plan").exact_short_write = Some(1);
            assert!(matches!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(_)
            ));
            let before_retry = bytes.lock().expect("bytes").clone();
            plan.lock().expect("plan").fail_reads = 1;
            assert_eq!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                    sequence: 1,
                    phase: CanonicalAppendPhase::RecoveryRead,
                    reason: CanonicalAppendUncertaintyReason::Io(io::ErrorKind::Other),
                })
            );
            assert!(appender.recovery_required());
            assert_eq!(*bytes.lock().expect("bytes"), before_retry);
            assert!(appender.take_quarantine_receipts().is_empty());
            assert_eq!(
                appender.append(&event("event-2"), &TestPayload { value: 2 }),
                CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned)
            );
            assert!(matches!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::Accepted(_)
            ));
            assert!(!appender.recovery_required());
            let final_bytes = bytes.lock().expect("bytes").clone();
            drop(appender);
            assert_strict_reopen_bytes(&final_bytes, 1);
            cleanup(&path);
        }

        // A complete pending event must cross both recovery barriers. Either
        // barrier may fail without clearing poison or changing bytes.
        for recovery_phase in [
            CanonicalAppendPhase::RecoveryFlush,
            CanonicalAppendPhase::RecoverySync,
        ] {
            let (mut appender, bytes, plan) = memory_appender();
            plan.lock().expect("plan").fail_syncs = 1;
            assert!(matches!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                    phase: CanonicalAppendPhase::Sync,
                    ..
                })
            ));
            let before_retry = bytes.lock().expect("bytes").clone();
            match recovery_phase {
                CanonicalAppendPhase::RecoveryFlush => {
                    plan.lock().expect("plan").fail_flushes = 1;
                }
                CanonicalAppendPhase::RecoverySync => {
                    plan.lock().expect("plan").fail_syncs = 1;
                }
                _ => unreachable!(),
            }
            assert_eq!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                    sequence: 1,
                    phase: recovery_phase,
                    reason: CanonicalAppendUncertaintyReason::Io(io::ErrorKind::Other),
                })
            );
            assert!(appender.recovery_required());
            assert_eq!(*bytes.lock().expect("bytes"), before_retry);
            assert_eq!(
                appender.append(&event("event-2"), &TestPayload { value: 2 }),
                CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned)
            );
            assert!(matches!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::AlreadyAccepted(CanonicalAppendDurabilityReceipt {
                    durability: CanonicalDurability::RecoveryBarrierSynced,
                    ..
                })
            ));
            assert!(!appender.recovery_required());
            let final_bytes = bytes.lock().expect("bytes").clone();
            drop(appender);
            assert_strict_reopen_bytes(&final_bytes, 1);
        }

        // Quarantine creation failure is non-destructive. Once the parent is
        // made available, the same immutable pending frame may recover.
        {
            let path = temp_log("recovery-quarantine-failure");
            cleanup(&path);
            let (mut appender, bytes, plan) = memory_appender_with_bytes(path.clone(), Vec::new());
            plan.lock().expect("plan").exact_short_write = Some(1);
            assert!(matches!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(_)
            ));
            let before_retry = bytes.lock().expect("bytes").clone();
            assert_eq!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                    sequence: 1,
                    phase: CanonicalAppendPhase::RecoveryQuarantine,
                    reason: CanonicalAppendUncertaintyReason::Io(io::ErrorKind::NotFound),
                })
            );
            assert!(appender.recovery_required());
            assert_eq!(*bytes.lock().expect("bytes"), before_retry);
            assert!(appender.take_quarantine_receipts().is_empty());
            assert_eq!(
                appender.append(&event("event-2"), &TestPayload { value: 2 }),
                CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned)
            );
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            assert!(matches!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::Accepted(_)
            ));
            assert_eq!(appender.take_quarantine_receipts().len(), 1);
            let final_bytes = bytes.lock().expect("bytes").clone();
            drop(appender);
            assert_strict_reopen_bytes(&final_bytes, 1);
            cleanup(&path);
        }

        // A truncate failure occurs only after a durable quarantine copy. The
        // source is unchanged, poison remains, and a later retry may repeat the
        // quarantine before completing the proven truncation.
        {
            let path = temp_log("recovery-truncate-failure");
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            let (mut appender, bytes, plan) = memory_appender_with_bytes(path.clone(), Vec::new());
            plan.lock().expect("plan").exact_short_write = Some(1);
            assert!(matches!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(_)
            ));
            let before_retry = bytes.lock().expect("bytes").clone();
            plan.lock().expect("plan").fail_truncates = 1;
            assert_eq!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                    sequence: 1,
                    phase: CanonicalAppendPhase::RecoveryTruncate,
                    reason: CanonicalAppendUncertaintyReason::Io(io::ErrorKind::Other),
                })
            );
            assert!(appender.recovery_required());
            assert_eq!(*bytes.lock().expect("bytes"), before_retry);
            assert_eq!(appender.take_quarantine_receipts().len(), 1);
            assert_eq!(
                appender.append(&event("event-2"), &TestPayload { value: 2 }),
                CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned)
            );
            assert!(matches!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::Accepted(_)
            ));
            assert_eq!(appender.take_quarantine_receipts().len(), 1);
            let final_bytes = bytes.lock().expect("bytes").clone();
            drop(appender);
            assert_strict_reopen_bytes(&final_bytes, 1);
            cleanup(&path);
        }

        // Failure after proven truncate leaves the source at the captured base
        // and poisoned; the next retry writes the immutable frame from there.
        {
            let path = temp_log("recovery-post-truncate-sync-failure");
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            let (mut appender, bytes, plan) = memory_appender_with_bytes(path.clone(), Vec::new());
            plan.lock().expect("plan").exact_short_write = Some(1);
            assert!(matches!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(_)
            ));
            plan.lock().expect("plan").fail_syncs = 1;
            assert_eq!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                    sequence: 1,
                    phase: CanonicalAppendPhase::RecoverySync,
                    reason: CanonicalAppendUncertaintyReason::Io(io::ErrorKind::Other),
                })
            );
            assert!(appender.recovery_required());
            assert!(bytes.lock().expect("bytes").is_empty());
            assert_eq!(appender.take_quarantine_receipts().len(), 1);
            assert_eq!(
                appender.append(&event("event-2"), &TestPayload { value: 2 }),
                CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned)
            );
            assert!(matches!(
                appender.append(&event("event-1"), &TestPayload { value: 1 }),
                CanonicalAppendOutcome::Accepted(_)
            ));
            assert!(!appender.recovery_required());
            let final_bytes = bytes.lock().expect("bytes").clone();
            drop(appender);
            assert_strict_reopen_bytes(&final_bytes, 1);
            cleanup(&path);
        }
    }

    #[test]
    fn short_write_recovery_quarantines_tail_and_retries_same_event() {
        let path = temp_log("short-write-recovery");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let (mut appender, bytes, plan) = memory_appender_with_bytes(path.clone(), Vec::new());
        plan.lock().expect("plan").short_writes = 1;
        assert!(matches!(
            appender.append(&event("event-1"), &TestPayload { value: 1 }),
            CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                reason: CanonicalAppendUncertaintyReason::ShortWrite,
                ..
            })
        ));
        assert!(matches!(
            appender.append(&event("event-1"), &TestPayload { value: 1 }),
            CanonicalAppendOutcome::Accepted(_)
        ));
        assert_eq!(appender.cached_event_count(), 1);
        assert_eq!(appender.take_quarantine_receipts().len(), 1);
        let final_bytes = bytes.lock().expect("bytes").clone();
        drop(appender);
        fs::write(&path, final_bytes).expect("persist memory file for strict reopen");
        let reopened: CanonicalLogSnapshot<TestPayload> = load_canonical_stream(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("strict reopen after recovery");
        assert_eq!(reopened.records.len(), 1);
        assert_eq!(reopened.head.as_ref().map(|head| head.sequence), Some(1));
        cleanup(&path);
    }

    #[test]
    fn legacy_without_newline_short_write_recovers_exactly_once() {
        let path = temp_log("legacy-short-write-recovery");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let initial = br#"{"value":1}"#.to_vec();
        let (mut appender, bytes, plan) = memory_appender_with_bytes(path.clone(), initial);
        plan.lock().expect("plan").short_writes = 1;

        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::OutcomeUncertain(CanonicalAppendUncertainty {
                reason: CanonicalAppendUncertaintyReason::ShortWrite,
                ..
            })
        ));
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::Accepted(_)
        ));
        assert_eq!(appender.cached_event_count(), 2);
        assert_eq!(appender.take_quarantine_receipts().len(), 1);

        let final_bytes = bytes.lock().expect("bytes").clone();
        drop(appender);
        fs::write(&path, final_bytes).expect("persist memory file for strict reopen");
        let reopened: CanonicalLogSnapshot<TestPayload> = load_canonical_stream(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("strict reopen after legacy recovery");
        assert_eq!(reopened.records.len(), 2);
        assert_eq!(reopened.head.as_ref().map(|head| head.sequence), Some(2));
        cleanup(&path);
    }

    #[test]
    fn same_length_base_substitution_does_not_mutate_or_retry() {
        let path = temp_log("same-length-base-substitution");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let original = b"{\"value\":1}\n";
        let replacement = b"{\"value\":9}\n";
        assert_eq!(original.len(), replacement.len());
        let (mut appender, bytes, plan) =
            memory_appender_with_bytes(path.clone(), original.to_vec());
        plan.lock().expect("plan").short_writes = 1;
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::OutcomeUncertain(_)
        ));

        {
            let mut current = bytes.lock().expect("bytes");
            let pending_suffix = current[original.len()..].to_vec();
            current.clear();
            current.extend_from_slice(replacement);
            current.extend_from_slice(&pending_suffix);
        }
        let before_retry = bytes.lock().expect("bytes").clone();
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::RecoveryRequired(CanonicalAppendRecoveryRequired {
                reason: CanonicalAppendRecoveryReason::ConcurrentModification,
                ..
            })
        ));
        assert!(appender.recovery_required());
        assert_eq!(*bytes.lock().expect("bytes"), before_retry);
        assert!(appender.take_quarantine_receipts().is_empty());
        assert_eq!(
            appender.append(&event("event-3"), &TestPayload { value: 3 }),
            CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned)
        );
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::RecoveryRequired(CanonicalAppendRecoveryRequired {
                reason: CanonicalAppendRecoveryReason::ConcurrentModification,
                ..
            })
        ));
        assert_eq!(*bytes.lock().expect("bytes"), before_retry);
        drop(appender);
        cleanup(&path);
    }

    #[test]
    fn foreign_unterminated_suffix_does_not_mutate_or_retry() {
        let path = temp_log("foreign-recovery-suffix");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let original = b"{\"value\":1}\n";
        let (mut appender, bytes, plan) =
            memory_appender_with_bytes(path.clone(), original.to_vec());
        plan.lock().expect("plan").short_writes = 1;
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::OutcomeUncertain(_)
        ));

        {
            let mut current = bytes.lock().expect("bytes");
            current.truncate(original.len());
            current.extend_from_slice(b"foreign unterminated suffix");
        }
        let before_retry = bytes.lock().expect("bytes").clone();
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::RecoveryRequired(CanonicalAppendRecoveryRequired {
                reason: CanonicalAppendRecoveryReason::ConcurrentModification,
                ..
            })
        ));
        assert!(appender.recovery_required());
        assert_eq!(*bytes.lock().expect("bytes"), before_retry);
        assert!(appender.take_quarantine_receipts().is_empty());
        assert_eq!(
            appender.append(&event("event-3"), &TestPayload { value: 3 }),
            CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned)
        );
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::RecoveryRequired(CanonicalAppendRecoveryRequired {
                reason: CanonicalAppendRecoveryReason::ConcurrentModification,
                ..
            })
        ));
        assert_eq!(*bytes.lock().expect("bytes"), before_retry);
        drop(appender);
        cleanup(&path);
    }

    #[test]
    fn strict_mode_foreign_suffix_is_repeatably_non_mutating() {
        let original = b"{\"value\":1}\n";
        let (mut appender, bytes, plan) = memory_appender_with_bytes_and_mode(
            PathBuf::from("redacted-strict-foreign"),
            original.to_vec(),
            CanonicalTailRecovery::Strict,
        );
        plan.lock().expect("plan").exact_short_write = Some(1);
        assert!(matches!(
            appender.append(&event("event-2"), &TestPayload { value: 2 }),
            CanonicalAppendOutcome::OutcomeUncertain(_)
        ));
        {
            let mut current = bytes.lock().expect("bytes");
            current.truncate(original.len());
            current.extend_from_slice(b"foreign unterminated suffix");
        }
        let before_retry = bytes.lock().expect("bytes").clone();
        for _ in 0..2 {
            assert!(matches!(
                appender.append(&event("event-2"), &TestPayload { value: 2 }),
                CanonicalAppendOutcome::RecoveryRequired(_)
            ));
            assert!(appender.recovery_required());
            assert_eq!(*bytes.lock().expect("bytes"), before_retry);
            assert!(appender.take_quarantine_receipts().is_empty());
        }
        assert_eq!(
            appender.append(&event("event-3"), &TestPayload { value: 3 }),
            CanonicalAppendOutcome::Rejected(CanonicalAppendRejection::AppenderPoisoned)
        );
    }

    #[test]
    fn competing_appenders_are_excluded_by_os_file_lock() {
        let path = temp_log("exclusive-lock");
        let first = open_appender(&path);
        assert!(matches!(
            CanonicalAppender::<TestPayload>::open(
                &path,
                SESSION,
                STREAM,
                SCHEMA,
                CanonicalTailRecovery::Strict,
            ),
            Err(CanonicalLogError::LockContended)
        ));
        drop(first);
        let reopened = open_appender(&path);
        drop(reopened);
        cleanup(&path);
    }

    #[test]
    fn normal_appends_use_cached_head_without_full_rescan() {
        let (mut appender, _, plan) = memory_appender();
        let initial_reads = plan.lock().expect("plan").reads;
        for value in 1..=128 {
            assert!(matches!(
                appender.append(&event(&format!("event-{value}")), &TestPayload { value }),
                CanonicalAppendOutcome::Accepted(_)
            ));
        }
        assert_eq!(appender.cached_event_count(), 128);
        assert_eq!(appender.head().map(|head| head.sequence), Some(128));
        assert_eq!(appender.full_scan_count, 1);
        assert_eq!(plan.lock().expect("plan").reads, initial_reads);
    }

    #[test]
    fn unknown_envelope_fields_are_rejected() {
        let path = temp_log("unknown-envelope-field");
        let mut appender = open_appender(&path);
        assert!(matches!(
            appender.append(&event("event-1"), &TestPayload { value: 1 }),
            CanonicalAppendOutcome::Accepted(_)
        ));
        drop(appender);
        let bytes = fs::read(&path).expect("read frame");
        let mut line = bytes.strip_suffix(b"\n").expect("newline").to_vec();
        let length_start = FRAME_PREFIX_V1.len();
        let length_end = length_start + FRAME_LENGTH_HEX_BYTES;
        let json_start = length_end + 1;
        let mut wire: Value = serde_json::from_slice(&line[json_start..]).expect("wire json");
        wire.as_object_mut()
            .expect("wire object")
            .insert("unknown".to_string(), Value::Bool(true));
        let json = serde_json::to_vec(&wire).expect("serialize modified wire");
        line.truncate(length_start);
        line.extend_from_slice(format!("{:016x}", json.len()).as_bytes());
        line.push(b' ');
        line.extend_from_slice(&json);
        line.push(b'\n');
        fs::write(&path, line).expect("write modified frame");

        let error = load_canonical_stream::<TestPayload>(
            &path,
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect_err("unknown envelope field must fail");
        assert!(matches!(
            error,
            CanonicalLogError::CorruptRecord {
                reason: CanonicalCorruptionReason::InvalidJson,
                ..
            }
        ));
        cleanup(&path);
    }

    #[test]
    fn duplicate_object_members_are_rejected_at_every_payload_depth() {
        for (label, original, duplicate) in [
            (
                "top-level",
                "\"payload\":{\"value\":1}",
                "\"payload\":{\"value\":0,\"value\":1}",
            ),
            (
                "nested",
                "\"payload\":{\"nested\":{\"value\":1}}",
                "\"payload\":{\"nested\":{\"value\":0,\"value\":1}}",
            ),
        ] {
            let path = temp_log(&format!("duplicate-member-{label}"));
            let payload = if label == "top-level" {
                serde_json::json!({"value": 1})
            } else {
                serde_json::json!({"nested": {"value": 1}})
            };
            let mut appender = CanonicalAppender::<Value>::open(
                &path,
                SESSION,
                STREAM,
                SCHEMA,
                CanonicalTailRecovery::Strict,
            )
            .expect("open duplicate fixture appender");
            assert!(matches!(
                appender.append(&event("event-1"), &payload),
                CanonicalAppendOutcome::Accepted(_)
            ));
            drop(appender);

            let bytes = fs::read(&path).expect("read frame");
            let line = bytes.strip_suffix(b"\n").expect("newline");
            let json_start = FRAME_PREFIX_V1.len() + FRAME_LENGTH_HEX_BYTES + 1;
            let json = std::str::from_utf8(&line[json_start..]).expect("wire utf8");
            let modified = json.replacen(original, duplicate, 1);
            assert_ne!(modified, json);
            let rewritten = format!(
                "{}{:016x} {}\n",
                std::str::from_utf8(FRAME_PREFIX_V1).expect("frame prefix"),
                modified.len(),
                modified
            );
            fs::write(&path, rewritten).expect("write duplicate-member frame");

            let error = load_canonical_stream::<Value>(
                &path,
                SESSION,
                STREAM,
                SCHEMA,
                CanonicalTailRecovery::Strict,
            )
            .expect_err("duplicate member must fail before semantic hashing");
            assert!(matches!(
                error,
                CanonicalLogError::CorruptRecord {
                    reason: CanonicalCorruptionReason::InvalidJson,
                    ..
                }
            ));
            cleanup(&path);
        }
    }

    #[test]
    fn diagnostics_do_not_include_payload_or_identifier_content() {
        let path = temp_log("redacted-diagnostics");
        let private_content = "private transcript sentence and credential-shaped text";
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, private_content.as_bytes()).expect("write invalid tail");
        let error = load_canonical_stream::<TestPayload>(
            &path,
            "private-session-name",
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect_err("invalid tail");
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains(private_content));
        assert!(!diagnostic.contains("private-session-name"));
        cleanup(&path);
    }
}
