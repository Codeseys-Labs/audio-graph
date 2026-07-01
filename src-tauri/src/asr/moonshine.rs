//! Moonshine streaming ASR adapter seam.
//!
//! This module is intentionally backend-only and native-library-free for the
//! first slice: it defines the fakeable contract and maps Moonshine transcript
//! line updates into AudioGraph span revisions. The later native runtime can
//! implement [`MoonshineStreamingAdapter`] behind `asr-moonshine` without
//! changing the transcript ledger semantics tested here.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::events::{AsrSpanRevisionPayload, AsrSpanStability};

pub const MOONSHINE_PROVIDER_ID: &str = "moonshine";
pub const MOONSHINE_SAMPLE_RATE_HZ: u32 = 16_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoonshineModelValidation {
    pub model_dir: PathBuf,
    pub directory_exists: bool,
    pub missing_required_files: Vec<String>,
    pub invalid_required_files: Vec<String>,
}

impl MoonshineModelValidation {
    pub fn is_ready(&self) -> bool {
        self.directory_exists
            && self.missing_required_files.is_empty()
            && self.invalid_required_files.is_empty()
    }

    pub fn failure_message(&self) -> Option<String> {
        if self.is_ready() {
            return None;
        }

        if !self.directory_exists {
            return Some(format!(
                "Moonshine model directory is missing: {}",
                self.model_dir.display()
            ));
        }

        let mut parts = Vec::new();
        if !self.missing_required_files.is_empty() {
            parts.push(format!(
                "missing required files: {}",
                self.missing_required_files.join(", ")
            ));
        }
        if !self.invalid_required_files.is_empty() {
            parts.push(format!(
                "invalid required files: {}",
                self.invalid_required_files.join(", ")
            ));
        }

        Some(format!(
            "Moonshine model directory is not ready at {} ({})",
            self.model_dir.display(),
            parts.join("; ")
        ))
    }
}

pub fn validate_moonshine_model_dir(model_dir: impl AsRef<Path>) -> MoonshineModelValidation {
    let model_dir = model_dir.as_ref().to_path_buf();
    let directory_exists = model_dir.is_dir();
    let mut missing_required_files = Vec::new();
    let mut invalid_required_files = Vec::new();

    if directory_exists {
        for required in crate::models::MOONSHINE_STREAMING_REQUIRED_FILES {
            let path = model_dir.join(required);
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {}
                Ok(_) => invalid_required_files.push((*required).to_string()),
                Err(_) => missing_required_files.push((*required).to_string()),
            }
        }
    } else {
        missing_required_files.extend(
            crate::models::MOONSHINE_STREAMING_REQUIRED_FILES
                .iter()
                .map(|required| (*required).to_string()),
        );
    }

    MoonshineModelValidation {
        model_dir,
        directory_exists,
        missing_required_files,
        invalid_required_files,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MoonshineRuntimeConfig {
    pub model_dir: PathBuf,
    pub poll_interval: Duration,
}

impl MoonshineRuntimeConfig {
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            poll_interval: Duration::from_millis(500),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MoonshineTranscriptLine {
    pub line_id: String,
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub confidence: Option<f32>,
    pub is_complete: bool,
    pub has_update: bool,
    pub speaker_id: Option<String>,
    pub speaker_label: Option<String>,
    pub channel: Option<String>,
    pub latency_ms: Option<u64>,
}

impl MoonshineTranscriptLine {
    pub fn partial(line_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            line_id: line_id.into(),
            text: text.into(),
            start_time: 0.0,
            end_time: 0.0,
            confidence: None,
            is_complete: false,
            has_update: true,
            speaker_id: None,
            speaker_label: None,
            channel: None,
            latency_ms: None,
        }
    }

    pub fn final_line(line_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            is_complete: true,
            ..Self::partial(line_id, text)
        }
    }
}

pub trait MoonshineStreamingAdapter {
    fn start(&mut self) -> Result<(), MoonshineAdapterError>;
    fn accept_pcm(
        &mut self,
        sample_rate_hz: u32,
        samples: &[f32],
    ) -> Result<(), MoonshineAdapterError>;
    fn poll_updates(&mut self) -> Result<Vec<MoonshineTranscriptLine>, MoonshineAdapterError>;
    fn stop(&mut self) -> Result<(), MoonshineAdapterError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoonshineAdapterError {
    message: String,
}

impl MoonshineAdapterError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for MoonshineAdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for MoonshineAdapterError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoonshineMappingError {
    MissingLineId,
}

impl fmt::Display for MoonshineMappingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLineId => f.write_str("Moonshine transcript line is missing a line id"),
        }
    }
}

impl Error for MoonshineMappingError {}

#[derive(Debug)]
pub enum MoonshineWorkerError {
    Adapter(MoonshineAdapterError),
    Mapping(MoonshineMappingError),
}

impl fmt::Display for MoonshineWorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Adapter(err) => write!(f, "Moonshine adapter error: {err}"),
            Self::Mapping(err) => write!(f, "Moonshine mapping error: {err}"),
        }
    }
}

impl Error for MoonshineWorkerError {}

impl From<MoonshineAdapterError> for MoonshineWorkerError {
    fn from(value: MoonshineAdapterError) -> Self {
        Self::Adapter(value)
    }
}

impl From<MoonshineMappingError> for MoonshineWorkerError {
    fn from(value: MoonshineMappingError) -> Self {
        Self::Mapping(value)
    }
}

#[cfg(feature = "asr-moonshine")]
pub trait MoonshineNativeRuntime: Send {
    fn runtime_version(&self) -> &str;
    fn accept_pcm(
        &mut self,
        sample_rate_hz: u32,
        samples: &[f32],
    ) -> Result<(), MoonshineAdapterError>;
    fn poll_updates(&mut self) -> Result<Vec<MoonshineTranscriptLine>, MoonshineAdapterError>;
    fn stop(&mut self) -> Result<(), MoonshineAdapterError>;
}

#[cfg(feature = "asr-moonshine")]
pub trait MoonshineNativeRuntimeLoader {
    fn load(
        &self,
        config: &MoonshineRuntimeConfig,
    ) -> Result<Box<dyn MoonshineNativeRuntime>, MoonshineAdapterError>;
}

#[cfg(feature = "asr-moonshine")]
#[derive(Debug, Default, Clone, Copy)]
pub struct MoonshineUnavailableNativeLoader;

#[cfg(feature = "asr-moonshine")]
impl MoonshineNativeRuntimeLoader for MoonshineUnavailableNativeLoader {
    fn load(
        &self,
        _config: &MoonshineRuntimeConfig,
    ) -> Result<Box<dyn MoonshineNativeRuntime>, MoonshineAdapterError> {
        Err(MoonshineAdapterError::new(
            "Moonshine native C API adapter is not linked in this build",
        ))
    }
}

#[cfg(feature = "asr-moonshine")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoonshineNativeProbeStatus {
    ModelMissing,
    ModelInvalid,
    LoadFailed,
    Ready,
}

#[cfg(feature = "asr-moonshine")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoonshineNativeProbeResult {
    pub status: MoonshineNativeProbeStatus,
    pub message: String,
    pub model_dir: PathBuf,
    pub runtime_version: Option<String>,
    pub missing_required_files: Vec<String>,
    pub invalid_required_files: Vec<String>,
}

#[cfg(feature = "asr-moonshine")]
pub fn probe_moonshine_native_runtime(
    config: MoonshineRuntimeConfig,
) -> MoonshineNativeProbeResult {
    probe_moonshine_native_runtime_with_loader(config, MoonshineUnavailableNativeLoader)
}

#[cfg(feature = "asr-moonshine")]
pub fn probe_moonshine_native_runtime_with_loader<L>(
    config: MoonshineRuntimeConfig,
    loader: L,
) -> MoonshineNativeProbeResult
where
    L: MoonshineNativeRuntimeLoader,
{
    let validation = validate_moonshine_model_dir(&config.model_dir);
    if !validation.is_ready() {
        return MoonshineNativeProbeResult {
            status: if validation.directory_exists {
                MoonshineNativeProbeStatus::ModelInvalid
            } else {
                MoonshineNativeProbeStatus::ModelMissing
            },
            message: validation.failure_message().unwrap_or_else(|| {
                "Moonshine model directory is not ready for native loading".to_string()
            }),
            model_dir: validation.model_dir,
            runtime_version: None,
            missing_required_files: validation.missing_required_files,
            invalid_required_files: validation.invalid_required_files,
        };
    }

    match loader.load(&config) {
        Ok(runtime) => MoonshineNativeProbeResult {
            status: MoonshineNativeProbeStatus::Ready,
            message: format!(
                "Moonshine native runtime loaded {} successfully from {}.",
                runtime.runtime_version(),
                config.model_dir.display()
            ),
            model_dir: config.model_dir,
            runtime_version: Some(runtime.runtime_version().to_string()),
            missing_required_files: Vec::new(),
            invalid_required_files: Vec::new(),
        },
        Err(err) => MoonshineNativeProbeResult {
            status: MoonshineNativeProbeStatus::LoadFailed,
            message: format!("Moonshine native runtime load failed: {err}"),
            model_dir: config.model_dir,
            runtime_version: None,
            missing_required_files: Vec::new(),
            invalid_required_files: Vec::new(),
        },
    }
}

#[cfg(feature = "asr-moonshine")]
pub struct MoonshineNativeStreamingAdapter<L = MoonshineUnavailableNativeLoader> {
    config: MoonshineRuntimeConfig,
    loader: L,
    runtime: Option<Box<dyn MoonshineNativeRuntime>>,
}

#[cfg(feature = "asr-moonshine")]
impl MoonshineNativeStreamingAdapter<MoonshineUnavailableNativeLoader> {
    pub fn new(config: MoonshineRuntimeConfig) -> Self {
        Self::new_with_loader(config, MoonshineUnavailableNativeLoader)
    }
}

#[cfg(feature = "asr-moonshine")]
impl<L> MoonshineNativeStreamingAdapter<L>
where
    L: MoonshineNativeRuntimeLoader,
{
    pub fn new_with_loader(config: MoonshineRuntimeConfig, loader: L) -> Self {
        Self {
            config,
            loader,
            runtime: None,
        }
    }

    pub fn runtime_version(&self) -> Option<&str> {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.runtime_version())
    }

    fn runtime_mut(&mut self) -> Result<&mut dyn MoonshineNativeRuntime, MoonshineAdapterError> {
        match self.runtime.as_deref_mut() {
            Some(runtime) => Ok(runtime),
            None => Err(MoonshineAdapterError::new(
                "Moonshine native runtime is not started",
            )),
        }
    }
}

#[cfg(feature = "asr-moonshine")]
impl<L> MoonshineStreamingAdapter for MoonshineNativeStreamingAdapter<L>
where
    L: MoonshineNativeRuntimeLoader,
{
    fn start(&mut self) -> Result<(), MoonshineAdapterError> {
        let validation = validate_moonshine_model_dir(&self.config.model_dir);
        if let Some(message) = validation.failure_message() {
            return Err(MoonshineAdapterError::new(message));
        }

        self.runtime = Some(self.loader.load(&self.config)?);
        Ok(())
    }

    fn accept_pcm(
        &mut self,
        sample_rate_hz: u32,
        samples: &[f32],
    ) -> Result<(), MoonshineAdapterError> {
        if sample_rate_hz != MOONSHINE_SAMPLE_RATE_HZ {
            return Err(MoonshineAdapterError::new(format!(
                "Moonshine native runtime requires {MOONSHINE_SAMPLE_RATE_HZ} Hz PCM, got {sample_rate_hz} Hz",
            )));
        }
        self.runtime_mut()?.accept_pcm(sample_rate_hz, samples)
    }

    fn poll_updates(&mut self) -> Result<Vec<MoonshineTranscriptLine>, MoonshineAdapterError> {
        self.runtime_mut()?.poll_updates()
    }

    fn stop(&mut self) -> Result<(), MoonshineAdapterError> {
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.stop()?;
        }
        self.runtime = None;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct MoonshineSpanRevision {
    pub payload: AsrSpanRevisionPayload,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Default)]
pub struct MoonshineSpanMapper {
    revision_numbers_by_span: HashMap<String, u64>,
    finalized_spans: HashSet<String>,
    last_emitted_by_span: HashMap<String, EmittedLineState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EmittedLineState {
    text: String,
    is_complete: bool,
}

impl MoonshineSpanMapper {
    pub fn map_line_update(
        &mut self,
        source_id: &str,
        line: &MoonshineTranscriptLine,
    ) -> Result<Option<MoonshineSpanRevision>, MoonshineMappingError> {
        self.map_line_update_at(source_id, line, current_unix_millis())
    }

    pub fn map_line_update_at(
        &mut self,
        source_id: &str,
        line: &MoonshineTranscriptLine,
        received_at_ms: u64,
    ) -> Result<Option<MoonshineSpanRevision>, MoonshineMappingError> {
        let text = line.text.trim();
        if text.is_empty() {
            return Ok(None);
        }

        let line_id = line.line_id.trim();
        if line_id.is_empty() {
            return Err(MoonshineMappingError::MissingLineId);
        }

        let span_id = moonshine_span_id(source_id, line_id);
        if self.finalized_spans.contains(&span_id) {
            return Ok(None);
        }
        if !line.has_update {
            return Ok(None);
        }

        let candidate_state = EmittedLineState {
            text: text.to_string(),
            is_complete: line.is_complete,
        };
        let changed_since_last_emit =
            self.last_emitted_by_span.get(&span_id) != Some(&candidate_state);
        if !changed_since_last_emit {
            return Ok(None);
        }

        let (revision_number, supersedes) = if line.is_complete {
            self.next_final_revision(&span_id)
        } else {
            self.next_partial_revision(&span_id)
        };
        self.last_emitted_by_span
            .insert(span_id.clone(), candidate_state);
        let start_time = sanitized_seconds(line.start_time);
        let end_time = sanitized_end_seconds(start_time, line.end_time);
        let stability = if line.is_complete {
            AsrSpanStability::Final
        } else {
            AsrSpanStability::Partial
        };
        let raw_event_ref = if line.is_complete {
            "moonshine.line.final"
        } else {
            "moonshine.line.partial"
        };
        let transcript_segment_id = line
            .is_complete
            .then(|| format!("{}@final", span_id.as_str()));

        Ok(Some(MoonshineSpanRevision {
            payload: AsrSpanRevisionPayload {
                span_id: span_id.clone(),
                provider: MOONSHINE_PROVIDER_ID.to_string(),
                source_id: source_id.to_string(),
                provider_item_id: Some(line_id.to_string()),
                transcript_segment_id,
                speaker_id: line.speaker_id.clone(),
                speaker_label: line.speaker_label.clone(),
                channel: line.channel.clone(),
                text: text.to_string(),
                start_time,
                end_time,
                confidence: normalized_confidence(line.confidence),
                is_final: line.is_complete,
                stability,
                revision_number,
                supersedes,
                turn_id: Some(format!("moonshine-line-{line_id}")),
                end_of_turn: line.is_complete,
                raw_event_ref: Some(raw_event_ref.to_string()),
                capture_latency_ms: None,
                asr_latency_ms: line.latency_ms,
                received_at_ms,
            },
            latency_ms: line.latency_ms,
        }))
    }

    fn next_partial_revision(&mut self, span_id: &str) -> (u64, Option<String>) {
        let revision_number = self
            .revision_numbers_by_span
            .entry(span_id.to_string())
            .or_insert(0);
        *revision_number += 1;
        let supersedes =
            (*revision_number > 1).then(|| revision_ref(span_id, *revision_number - 1));
        (*revision_number, supersedes)
    }

    fn next_final_revision(&mut self, span_id: &str) -> (u64, Option<String>) {
        let revision_number = self.revision_numbers_by_span.remove(span_id).unwrap_or(0) + 1;
        self.finalized_spans.insert(span_id.to_string());
        let supersedes = (revision_number > 1).then(|| revision_ref(span_id, revision_number - 1));
        (revision_number, supersedes)
    }
}

#[derive(Debug)]
pub struct MoonshineStreamingWorker<A> {
    adapter: A,
    mapper: MoonshineSpanMapper,
    poll_interval_ms: u64,
    last_poll_at_ms: Option<u64>,
}

impl<A: MoonshineStreamingAdapter> MoonshineStreamingWorker<A> {
    pub fn new(adapter: A) -> Result<Self, MoonshineAdapterError> {
        Self::new_with_config(adapter, MoonshineRuntimeConfig::new(PathBuf::new()))
    }

    pub fn new_with_config(
        mut adapter: A,
        config: MoonshineRuntimeConfig,
    ) -> Result<Self, MoonshineAdapterError> {
        adapter.start()?;
        Ok(Self {
            adapter,
            mapper: MoonshineSpanMapper::default(),
            poll_interval_ms: duration_millis_u64(config.poll_interval),
            last_poll_at_ms: None,
        })
    }

    pub fn process_chunk(
        &mut self,
        source_id: &str,
        samples: &[f32],
    ) -> Result<Vec<MoonshineSpanRevision>, MoonshineWorkerError> {
        let now_ms = current_unix_millis();
        self.process_chunk_at(source_id, samples, now_ms, now_ms)
    }

    pub fn process_chunk_at(
        &mut self,
        source_id: &str,
        samples: &[f32],
        poll_clock_ms: u64,
        received_at_ms: u64,
    ) -> Result<Vec<MoonshineSpanRevision>, MoonshineWorkerError> {
        self.adapter.accept_pcm(MOONSHINE_SAMPLE_RATE_HZ, samples)?;
        self.poll_pending_at(source_id, poll_clock_ms, received_at_ms)
    }

    pub fn poll_pending_at(
        &mut self,
        source_id: &str,
        poll_clock_ms: u64,
        received_at_ms: u64,
    ) -> Result<Vec<MoonshineSpanRevision>, MoonshineWorkerError> {
        if !self.should_poll(poll_clock_ms) {
            return Ok(Vec::new());
        }

        let updates = self.adapter.poll_updates()?;
        let mut revisions = Vec::new();
        for update in updates {
            if let Some(revision) =
                self.mapper
                    .map_line_update_at(source_id, &update, received_at_ms)?
            {
                revisions.push(revision);
            }
        }
        Ok(revisions)
    }

    fn should_poll(&mut self, poll_clock_ms: u64) -> bool {
        let should_poll = match self.last_poll_at_ms {
            None => true,
            Some(last_poll_at_ms) => {
                self.poll_interval_ms == 0
                    || poll_clock_ms.saturating_sub(last_poll_at_ms) >= self.poll_interval_ms
            }
        };

        if should_poll {
            self.last_poll_at_ms = Some(poll_clock_ms);
        }
        should_poll
    }

    pub fn stop(&mut self) -> Result<(), MoonshineAdapterError> {
        self.adapter.stop()
    }
}

fn moonshine_span_id(source_id: &str, line_id: &str) -> String {
    format!("{MOONSHINE_PROVIDER_ID}:{source_id}:{line_id}")
}

fn revision_ref(span_id: &str, revision_number: u64) -> String {
    format!("{span_id}@rev{revision_number}")
}

fn sanitized_seconds(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn sanitized_end_seconds(start_time: f64, value: f64) -> f64 {
    sanitized_seconds(value).max(start_time)
}

fn normalized_confidence(value: Option<f32>) -> f32 {
    value
        .filter(|confidence| confidence.is_finite())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0)
}

fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    #[cfg(feature = "asr-moonshine")]
    use std::sync::{Arc, Mutex};

    use super::*;

    fn unique_test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "audio-graph-moonshine-{label}-{}-{}",
            std::process::id(),
            current_unix_millis()
        ))
    }

    fn write_required_moonshine_model_files(model_dir: &Path) {
        fs::create_dir_all(model_dir).expect("create model dir");
        for required in crate::models::MOONSHINE_STREAMING_REQUIRED_FILES {
            fs::write(model_dir.join(required), b"component").expect("write model component");
        }
    }

    #[test]
    fn model_validation_requires_complete_non_empty_component_directory() {
        let model_dir = unique_test_dir("model-validation");
        let missing_dir = validate_moonshine_model_dir(&model_dir);
        assert!(!missing_dir.is_ready());
        assert!(!missing_dir.directory_exists);
        assert_eq!(
            missing_dir.missing_required_files.len(),
            crate::models::MOONSHINE_STREAMING_REQUIRED_FILES.len()
        );
        assert!(
            missing_dir
                .failure_message()
                .expect("failure message")
                .contains("model directory is missing")
        );

        fs::create_dir_all(&model_dir).expect("create incomplete model dir");
        fs::write(model_dir.join("adapter.ort"), b"").expect("write invalid component");
        let incomplete = validate_moonshine_model_dir(&model_dir);
        assert!(!incomplete.is_ready());
        assert!(incomplete.directory_exists);
        assert_eq!(incomplete.invalid_required_files, vec!["adapter.ort"]);
        assert!(
            incomplete
                .missing_required_files
                .contains(&"tokenizer.bin".to_string())
        );

        write_required_moonshine_model_files(&model_dir);
        let ready = validate_moonshine_model_dir(&model_dir);
        assert!(ready.is_ready());
        assert_eq!(ready.failure_message(), None);

        let _ = fs::remove_dir_all(&model_dir);
    }

    #[test]
    fn line_updates_chain_partial_revisions_into_final() {
        let mut mapper = MoonshineSpanMapper::default();

        let mut first = MoonshineTranscriptLine::partial("line-42", "hel");
        first.start_time = 1.0;
        first.end_time = 1.2;
        first.confidence = Some(0.4);

        let first = mapper
            .map_line_update_at("mic-1", &first, 100)
            .expect("mapping")
            .expect("revision");
        assert_eq!(first.payload.span_id, "moonshine:mic-1:line-42");
        assert_eq!(first.payload.provider_item_id.as_deref(), Some("line-42"));
        assert_eq!(first.payload.revision_number, 1);
        assert_eq!(first.payload.supersedes, None);
        assert!(!first.payload.is_final);
        assert!(!first.payload.end_of_turn);

        let mut second = MoonshineTranscriptLine::partial("line-42", "hello");
        second.start_time = 1.0;
        second.end_time = 1.5;
        let second = mapper
            .map_line_update_at("mic-1", &second, 110)
            .expect("mapping")
            .expect("revision");
        assert_eq!(second.payload.revision_number, 2);
        assert_eq!(
            second.payload.supersedes.as_deref(),
            Some("moonshine:mic-1:line-42@rev1")
        );

        let mut final_line = MoonshineTranscriptLine::final_line("line-42", "hello world");
        final_line.start_time = 1.0;
        final_line.end_time = 2.1;
        final_line.latency_ms = Some(87);
        let final_revision = mapper
            .map_line_update_at("mic-1", &final_line, 120)
            .expect("mapping")
            .expect("revision");
        assert_eq!(final_revision.payload.revision_number, 3);
        assert_eq!(
            final_revision.payload.supersedes.as_deref(),
            Some("moonshine:mic-1:line-42@rev2")
        );
        assert_eq!(final_revision.payload.stability, AsrSpanStability::Final);
        assert!(final_revision.payload.is_final);
        assert!(final_revision.payload.end_of_turn);
        assert_eq!(
            final_revision.payload.transcript_segment_id.as_deref(),
            Some("moonshine:mic-1:line-42@final")
        );
        assert_eq!(
            final_revision.payload.raw_event_ref.as_deref(),
            Some("moonshine.line.final")
        );
        assert_eq!(final_revision.latency_ms, Some(87));

        let duplicate = mapper
            .map_line_update_at("mic-1", &final_line, 130)
            .expect("mapping");
        assert!(
            duplicate.is_none(),
            "finalized lines should not produce duplicate revisions"
        );
    }

    #[test]
    fn finalization_allows_same_text_final_then_blocks_late_line_updates() {
        let mut mapper = MoonshineSpanMapper::default();
        let partial = MoonshineTranscriptLine::partial("line-same", "same words");
        let first = mapper
            .map_line_update_at("mic-1", &partial, 100)
            .expect("mapping")
            .expect("partial revision");
        assert_eq!(first.payload.revision_number, 1);
        assert!(!first.payload.is_final);

        let final_line = MoonshineTranscriptLine::final_line("line-same", "same words");
        let final_revision = mapper
            .map_line_update_at("mic-1", &final_line, 110)
            .expect("mapping")
            .expect("final revision");
        assert_eq!(final_revision.payload.revision_number, 2);
        assert_eq!(
            final_revision.payload.supersedes.as_deref(),
            Some("moonshine:mic-1:line-same@rev1")
        );
        assert!(final_revision.payload.is_final);

        let duplicate_final = mapper
            .map_line_update_at("mic-1", &final_line, 120)
            .expect("mapping");
        assert!(
            duplicate_final.is_none(),
            "same completed line must not emit duplicate finals"
        );

        let late_partial = MoonshineTranscriptLine::partial("line-same", "late correction");
        let late_update = mapper
            .map_line_update_at("mic-1", &late_partial, 130)
            .expect("mapping");
        assert!(
            late_update.is_none(),
            "provider churn after finalization must not reopen the span"
        );
    }

    #[test]
    fn skips_unchanged_and_empty_lines_without_advancing_revision() {
        let mut mapper = MoonshineSpanMapper::default();
        let mut unchanged = MoonshineTranscriptLine::partial("line-7", "ignored");
        unchanged.has_update = false;
        assert!(
            mapper
                .map_line_update_at("system", &unchanged, 1)
                .expect("mapping")
                .is_none()
        );

        let empty = MoonshineTranscriptLine::partial("line-7", "   ");
        assert!(
            mapper
                .map_line_update_at("system", &empty, 2)
                .expect("mapping")
                .is_none()
        );

        let mapped = mapper
            .map_line_update_at(
                "system",
                &MoonshineTranscriptLine::partial("line-7", "now"),
                3,
            )
            .expect("mapping")
            .expect("revision");
        assert_eq!(mapped.payload.revision_number, 1);
        assert_eq!(mapped.payload.supersedes, None);

        let duplicate = mapper
            .map_line_update_at(
                "system",
                &MoonshineTranscriptLine::partial("line-7", "now"),
                4,
            )
            .expect("mapping");
        assert!(
            duplicate.is_none(),
            "unchanged text/finality should not create provider-poll churn"
        );
    }

    #[test]
    fn complete_line_without_prior_partial_emits_final_revision_one() {
        let mut mapper = MoonshineSpanMapper::default();

        let mapped = mapper
            .map_line_update_at(
                "desktop-loopback",
                &MoonshineTranscriptLine::final_line("line-final", "already complete"),
                20,
            )
            .expect("mapping")
            .expect("revision");

        assert_eq!(mapped.payload.revision_number, 1);
        assert_eq!(mapped.payload.supersedes, None);
        assert!(mapped.payload.is_final);
        assert!(mapped.payload.end_of_turn);
        assert_eq!(
            mapped.payload.provider_item_id.as_deref(),
            Some("line-final")
        );
    }

    #[test]
    fn carries_speaker_hints_as_provisional_metadata() {
        let mut mapper = MoonshineSpanMapper::default();
        let mut line = MoonshineTranscriptLine::partial("line-speaker", "speaker hint");
        line.speaker_id = Some("provider-speaker-2".to_string());
        line.speaker_label = Some("Provider speaker 2".to_string());
        line.channel = Some("mixed".to_string());
        line.confidence = Some(1.5);
        line.start_time = f64::NAN;
        line.end_time = -2.0;

        let mapped = mapper
            .map_line_update_at("loopback", &line, 55)
            .expect("mapping")
            .expect("revision");
        assert_eq!(
            mapped.payload.speaker_id.as_deref(),
            Some("provider-speaker-2")
        );
        assert_eq!(
            mapped.payload.speaker_label.as_deref(),
            Some("Provider speaker 2")
        );
        assert_eq!(mapped.payload.channel.as_deref(), Some("mixed"));
        assert_eq!(mapped.payload.confidence, 1.0);
        assert_eq!(mapped.payload.start_time, 0.0);
        assert_eq!(mapped.payload.end_time, 0.0);
    }

    #[test]
    fn missing_line_id_is_a_mapping_error() {
        let mut mapper = MoonshineSpanMapper::default();
        let err = mapper
            .map_line_update_at("mic", &MoonshineTranscriptLine::partial(" ", "text"), 1)
            .expect_err("missing line id should fail");
        assert_eq!(err, MoonshineMappingError::MissingLineId);
    }

    #[test]
    fn worker_processes_fake_adapter_batches_without_native_runtime() {
        let mut adapter = FakeMoonshineAdapter::default();
        adapter.push_batch(vec![
            MoonshineTranscriptLine::partial("line-a", "partial"),
            MoonshineTranscriptLine::final_line("line-a", "final text"),
        ]);
        let mut worker = MoonshineStreamingWorker::new(adapter).expect("worker");

        let revisions = worker
            .process_chunk("mic-1", &[0.1, -0.1, 0.0])
            .expect("process chunk");
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].payload.revision_number, 1);
        assert_eq!(revisions[1].payload.revision_number, 2);
        assert!(revisions[1].payload.is_final);
        worker.stop().expect("stop");
    }

    #[test]
    fn worker_polls_on_configured_interval_without_dropping_audio() {
        let mut adapter = FakeMoonshineAdapter::default();
        adapter.push_batch(vec![MoonshineTranscriptLine::partial("line-a", "first")]);
        adapter.push_batch(vec![MoonshineTranscriptLine::partial("line-a", "second")]);
        let mut config = MoonshineRuntimeConfig::new(PathBuf::from("moonshine-small-streaming-en"));
        config.poll_interval = Duration::from_millis(50);
        let mut worker =
            MoonshineStreamingWorker::new_with_config(adapter, config).expect("worker");

        let first = worker
            .process_chunk_at("mic-1", &[0.1], 1_000, 10_000)
            .expect("first chunk");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].payload.text, "first");

        let skipped = worker
            .process_chunk_at("mic-1", &[0.2], 1_020, 10_020)
            .expect("second chunk before poll interval");
        assert!(skipped.is_empty());

        let second = worker
            .process_chunk_at("mic-1", &[0.3], 1_050, 10_050)
            .expect("third chunk at poll interval");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].payload.text, "second");
        worker.stop().expect("stop");
    }

    #[test]
    fn worker_can_poll_pending_updates_without_new_audio_after_interval() {
        let mut adapter = FakeMoonshineAdapter::default();
        adapter.push_batch(vec![MoonshineTranscriptLine::partial("line-b", "pending")]);
        let mut config = MoonshineRuntimeConfig::new(PathBuf::from("moonshine-small-streaming-en"));
        config.poll_interval = Duration::from_millis(50);
        let mut worker =
            MoonshineStreamingWorker::new_with_config(adapter, config).expect("worker");

        let skipped = worker
            .poll_pending_at("mic-1", 1_000, 10_000)
            .expect("first standalone poll");
        assert_eq!(skipped.len(), 1);

        let no_poll = worker
            .poll_pending_at("mic-1", 1_010, 10_010)
            .expect("poll before interval");
        assert!(no_poll.is_empty());
        worker.stop().expect("stop");
    }

    #[cfg(feature = "asr-moonshine")]
    #[test]
    fn native_probe_fails_closed_without_linked_c_api_adapter() {
        let model_dir = unique_test_dir("native-load-failure");
        write_required_moonshine_model_files(&model_dir);
        let config = MoonshineRuntimeConfig::new(model_dir.clone());

        let probe = probe_moonshine_native_runtime(config.clone());
        assert_eq!(probe.status, MoonshineNativeProbeStatus::LoadFailed);
        assert_eq!(probe.model_dir, model_dir);
        assert!(probe.runtime_version.is_none());
        assert!(probe.message.contains("native runtime load failed"));
        assert!(probe.message.contains("not linked"));

        let mut adapter = MoonshineNativeStreamingAdapter::new(config);
        let err = adapter
            .start()
            .expect_err("default native loader fails closed");
        assert!(err.message().contains("not linked"));

        let _ = fs::remove_dir_all(&model_dir);
    }

    #[cfg(feature = "asr-moonshine")]
    #[test]
    fn native_probe_reports_model_validation_before_attempting_load() {
        let model_dir = unique_test_dir("native-missing-model");
        let config = MoonshineRuntimeConfig::new(model_dir.clone());
        let loader = FakeMoonshineNativeLoader::new(VecDeque::new(), "fake-runtime/0.1");

        let probe = probe_moonshine_native_runtime_with_loader(config, loader);
        assert_eq!(probe.status, MoonshineNativeProbeStatus::ModelMissing);
        assert_eq!(probe.model_dir, model_dir);
        assert!(probe.runtime_version.is_none());
        assert_eq!(
            probe.missing_required_files.len(),
            crate::models::MOONSHINE_STREAMING_REQUIRED_FILES.len()
        );
        assert!(probe.message.contains("model directory is missing"));
    }

    #[cfg(feature = "asr-moonshine")]
    #[test]
    fn native_adapter_can_be_driven_by_fake_loader_after_model_validation() {
        let model_dir = unique_test_dir("native-fake-loader");
        write_required_moonshine_model_files(&model_dir);
        let mut batches = VecDeque::new();
        batches.push_back(vec![
            MoonshineTranscriptLine::partial("line-native", "native partial"),
            MoonshineTranscriptLine::final_line("line-native", "native final"),
        ]);
        let (loader, accepted_sample_rates) =
            FakeMoonshineNativeLoader::with_sample_rate_log(batches, "fake-runtime/0.1");
        let config = MoonshineRuntimeConfig::new(model_dir.clone());

        let probe = probe_moonshine_native_runtime_with_loader(
            config.clone(),
            loader.clone_without_batches(),
        );
        assert_eq!(probe.status, MoonshineNativeProbeStatus::Ready);
        assert_eq!(probe.runtime_version.as_deref(), Some("fake-runtime/0.1"));

        let adapter = MoonshineNativeStreamingAdapter::new_with_loader(config.clone(), loader);
        let mut worker =
            MoonshineStreamingWorker::new_with_config(adapter, config).expect("native worker");
        let revisions = worker
            .process_chunk_at("mic-native", &[0.0, 0.1, -0.1], 1_000, 10_000)
            .expect("process native chunk");

        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].payload.text, "native partial");
        assert!(revisions[1].payload.is_final);
        assert_eq!(
            *accepted_sample_rates
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
            vec![MOONSHINE_SAMPLE_RATE_HZ]
        );
        worker.stop().expect("stop native worker");

        let _ = fs::remove_dir_all(&model_dir);
    }

    #[derive(Default)]
    struct FakeMoonshineAdapter {
        batches: VecDeque<Vec<MoonshineTranscriptLine>>,
        accepted_sample_rates: Vec<u32>,
        started: bool,
        stopped: bool,
    }

    impl FakeMoonshineAdapter {
        fn push_batch(&mut self, batch: Vec<MoonshineTranscriptLine>) {
            self.batches.push_back(batch);
        }
    }

    impl MoonshineStreamingAdapter for FakeMoonshineAdapter {
        fn start(&mut self) -> Result<(), MoonshineAdapterError> {
            self.started = true;
            Ok(())
        }

        fn accept_pcm(
            &mut self,
            sample_rate_hz: u32,
            _samples: &[f32],
        ) -> Result<(), MoonshineAdapterError> {
            assert!(self.started);
            self.accepted_sample_rates.push(sample_rate_hz);
            Ok(())
        }

        fn poll_updates(&mut self) -> Result<Vec<MoonshineTranscriptLine>, MoonshineAdapterError> {
            Ok(self.batches.pop_front().unwrap_or_default())
        }

        fn stop(&mut self) -> Result<(), MoonshineAdapterError> {
            self.stopped = true;
            assert!(
                self.accepted_sample_rates
                    .iter()
                    .all(|sample_rate_hz| *sample_rate_hz == MOONSHINE_SAMPLE_RATE_HZ),
                "worker must feed Moonshine the configured 16 kHz PCM rate"
            );
            Ok(())
        }
    }

    #[cfg(feature = "asr-moonshine")]
    #[derive(Clone)]
    struct FakeMoonshineNativeLoader {
        batches: Arc<Mutex<VecDeque<Vec<MoonshineTranscriptLine>>>>,
        accepted_sample_rates: Arc<Mutex<Vec<u32>>>,
        runtime_version: &'static str,
    }

    #[cfg(feature = "asr-moonshine")]
    impl FakeMoonshineNativeLoader {
        fn new(
            batches: VecDeque<Vec<MoonshineTranscriptLine>>,
            runtime_version: &'static str,
        ) -> Self {
            Self {
                batches: Arc::new(Mutex::new(batches)),
                accepted_sample_rates: Arc::new(Mutex::new(Vec::new())),
                runtime_version,
            }
        }

        fn with_sample_rate_log(
            batches: VecDeque<Vec<MoonshineTranscriptLine>>,
            runtime_version: &'static str,
        ) -> (Self, Arc<Mutex<Vec<u32>>>) {
            let loader = Self::new(batches, runtime_version);
            let accepted_sample_rates = loader.accepted_sample_rates.clone();
            (loader, accepted_sample_rates)
        }

        fn clone_without_batches(&self) -> Self {
            Self {
                batches: Arc::new(Mutex::new(VecDeque::new())),
                accepted_sample_rates: self.accepted_sample_rates.clone(),
                runtime_version: self.runtime_version,
            }
        }
    }

    #[cfg(feature = "asr-moonshine")]
    impl MoonshineNativeRuntimeLoader for FakeMoonshineNativeLoader {
        fn load(
            &self,
            _config: &MoonshineRuntimeConfig,
        ) -> Result<Box<dyn MoonshineNativeRuntime>, MoonshineAdapterError> {
            Ok(Box::new(FakeMoonshineNativeRuntime {
                batches: self.batches.clone(),
                accepted_sample_rates: self.accepted_sample_rates.clone(),
                runtime_version: self.runtime_version,
                stopped: false,
            }))
        }
    }

    #[cfg(feature = "asr-moonshine")]
    struct FakeMoonshineNativeRuntime {
        batches: Arc<Mutex<VecDeque<Vec<MoonshineTranscriptLine>>>>,
        accepted_sample_rates: Arc<Mutex<Vec<u32>>>,
        runtime_version: &'static str,
        stopped: bool,
    }

    #[cfg(feature = "asr-moonshine")]
    impl MoonshineNativeRuntime for FakeMoonshineNativeRuntime {
        fn runtime_version(&self) -> &str {
            self.runtime_version
        }

        fn accept_pcm(
            &mut self,
            sample_rate_hz: u32,
            samples: &[f32],
        ) -> Result<(), MoonshineAdapterError> {
            if self.stopped {
                return Err(MoonshineAdapterError::new("runtime stopped"));
            }
            assert!(!samples.is_empty(), "native adapter should receive PCM");
            self.accepted_sample_rates
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(sample_rate_hz);
            Ok(())
        }

        fn poll_updates(&mut self) -> Result<Vec<MoonshineTranscriptLine>, MoonshineAdapterError> {
            Ok(self
                .batches
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pop_front()
                .unwrap_or_default())
        }

        fn stop(&mut self) -> Result<(), MoonshineAdapterError> {
            self.stopped = true;
            Ok(())
        }
    }
}
