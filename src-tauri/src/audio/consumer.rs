//! Registry for processed-audio consumers.
//!
//! The resampling pipeline emits one stream of [`ProcessedAudioChunk`] values.
//! Downstream stages should subscribe through this registry instead of adding
//! another hardcoded channel field and another branch in the dispatcher loop.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crossbeam_channel::{Receiver, Sender, TrySendError};

use super::mixer::MIXED_SOURCE_ID;
use super::pipeline::ProcessedAudioChunk;

pub type ConsumerActiveFn = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessedAudioConsumerStage {
    Speech,
    Notes,
    NativeConverse,
    RealtimeAgent,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessedAudioDropPolicy {
    DropOldest,
    DropNewest,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessedAudioMixingMode {
    PerSource,
    MixedMono,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProcessedAudioSourceFilter {
    All,
    Sources { source_ids: Vec<String> },
}

impl ProcessedAudioSourceFilter {
    fn accepts(&self, source_id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Sources { source_ids } => source_ids.iter().any(|id| id == source_id),
        }
    }

    fn validate(&self, consumer_id: &str) -> Result<(), String> {
        match self {
            Self::All => Ok(()),
            Self::Sources { source_ids } => {
                if source_ids.is_empty() {
                    return Err(format!(
                        "processed audio consumer '{}' source filter must include at least one source",
                        consumer_id
                    ));
                }

                let mut seen = HashSet::new();
                for source_id in source_ids {
                    let trimmed = source_id.trim();
                    if trimmed.is_empty() {
                        return Err(format!(
                            "processed audio consumer '{}' source filter contains an empty source id",
                            consumer_id
                        ));
                    }
                    if trimmed != source_id {
                        return Err(format!(
                            "processed audio consumer '{}' source filter source ids must not include leading or trailing whitespace",
                            consumer_id
                        ));
                    }
                    if !seen.insert(source_id.as_str()) {
                        return Err(format!(
                            "processed audio consumer '{}' source filter contains duplicate source '{}'",
                            consumer_id, source_id
                        ));
                    }
                }

                Ok(())
            }
        }
    }
}

impl ProcessedAudioMixingMode {
    fn accepts_source(&self, source_id: &str) -> bool {
        match self {
            Self::PerSource => source_id != MIXED_SOURCE_ID,
            Self::MixedMono => source_id == MIXED_SOURCE_ID,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProcessedAudioConsumerDescriptor {
    pub id: String,
    pub stage: ProcessedAudioConsumerStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_group: Option<String>,
    pub capacity: usize,
    pub drop_policy: ProcessedAudioDropPolicy,
    pub source_filter: ProcessedAudioSourceFilter,
    pub mixing_mode: ProcessedAudioMixingMode,
}

impl ProcessedAudioConsumerDescriptor {
    fn validate(&self) -> Result<(), String> {
        validate_non_empty_field("processed audio consumer id", &self.id)?;
        validate_optional_field(&self.id, "provider", self.provider.as_deref())?;
        validate_optional_field(&self.id, "conflict_group", self.conflict_group.as_deref())?;

        if self.capacity == 0 {
            return Err(format!(
                "processed audio consumer '{}' must use a positive bounded capacity",
                self.id
            ));
        }

        self.source_filter.validate(&self.id)?;
        self.validate_mixing_filter()
    }

    fn validate_mixing_filter(&self) -> Result<(), String> {
        match (&self.mixing_mode, &self.source_filter) {
            (
                ProcessedAudioMixingMode::PerSource,
                ProcessedAudioSourceFilter::Sources { source_ids },
            ) if source_ids
                .iter()
                .any(|source_id| source_id == MIXED_SOURCE_ID) =>
            {
                Err(format!(
                    "processed audio consumer '{}' uses per_source mixing but filters the synthetic mixed source",
                    self.id
                ))
            }
            (
                ProcessedAudioMixingMode::MixedMono,
                ProcessedAudioSourceFilter::Sources { source_ids },
            ) => {
                if let Some(source_id) = source_ids
                    .iter()
                    .find(|source_id| source_id.as_str() != MIXED_SOURCE_ID)
                {
                    return Err(format!(
                        "processed audio consumer '{}' uses mixed_mono mixing but filters non-mixed source '{}'",
                        self.id, source_id
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn accepts(&self, chunk: &ProcessedAudioChunk) -> bool {
        self.source_filter.accepts(chunk.source_id.as_ref())
            && self.mixing_mode.accepts_source(chunk.source_id.as_ref())
    }
}

pub struct ProcessedAudioConsumerRegistration {
    pub descriptor: ProcessedAudioConsumerDescriptor,
    pub tx: Sender<ProcessedAudioChunk>,
    pub drain_rx: Receiver<ProcessedAudioChunk>,
    pub is_active: ConsumerActiveFn,
}

impl ProcessedAudioConsumerRegistration {
    fn validate(&self) -> Result<(), String> {
        self.descriptor.validate()?;
        validate_channel_capacity(
            &self.descriptor.id,
            "sender",
            self.tx.capacity(),
            self.descriptor.capacity,
        )?;
        validate_channel_capacity(
            &self.descriptor.id,
            "receiver",
            self.drain_rx.capacity(),
            self.descriptor.capacity,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeRealtimeAgentConsumerDescriptor {
    pub id: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_group: Option<String>,
    pub capacity: usize,
    pub drop_policy: ProcessedAudioDropPolicy,
    pub source_filter: ProcessedAudioSourceFilter,
    pub mixing_mode: ProcessedAudioMixingMode,
}

impl RuntimeRealtimeAgentConsumerDescriptor {
    pub fn new(id: impl Into<String>, provider: impl Into<String>, capacity: usize) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            conflict_group: None,
            capacity,
            drop_policy: ProcessedAudioDropPolicy::DropOldest,
            source_filter: ProcessedAudioSourceFilter::All,
            mixing_mode: ProcessedAudioMixingMode::PerSource,
        }
    }

    pub fn with_conflict_group(mut self, conflict_group: impl Into<String>) -> Self {
        self.conflict_group = Some(conflict_group.into());
        self
    }

    pub fn with_drop_policy(mut self, drop_policy: ProcessedAudioDropPolicy) -> Self {
        self.drop_policy = drop_policy;
        self
    }

    pub fn with_source_filter(mut self, source_filter: ProcessedAudioSourceFilter) -> Self {
        self.source_filter = source_filter;
        self
    }

    pub fn with_mixing_mode(mut self, mixing_mode: ProcessedAudioMixingMode) -> Self {
        self.mixing_mode = mixing_mode;
        self
    }

    pub fn into_processed_audio_descriptor(self) -> ProcessedAudioConsumerDescriptor {
        ProcessedAudioConsumerDescriptor {
            id: self.id,
            stage: ProcessedAudioConsumerStage::RealtimeAgent,
            provider: Some(self.provider),
            conflict_group: self.conflict_group,
            capacity: self.capacity,
            drop_policy: self.drop_policy,
            source_filter: self.source_filter,
            mixing_mode: self.mixing_mode,
        }
    }
}

#[derive(Debug)]
pub struct RuntimeProcessedAudioConsumer {
    pub descriptor: ProcessedAudioConsumerDescriptor,
    pub rx: Receiver<ProcessedAudioChunk>,
}

struct ProcessedAudioConsumer {
    descriptor: ProcessedAudioConsumerDescriptor,
    tx: Sender<ProcessedAudioChunk>,
    drain_rx: Receiver<ProcessedAudioChunk>,
    is_active: ConsumerActiveFn,
    sent_chunks: AtomicU64,
    dropped_chunks: AtomicU64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessedAudioDispatchSummary {
    pub active_consumers: usize,
    pub delivered_chunks: usize,
    pub dropped_chunks: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProcessedAudioConsumerHealth {
    pub id: String,
    pub stage: ProcessedAudioConsumerStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_group: Option<String>,
    pub active: bool,
    pub queue_len: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_capacity: Option<usize>,
    pub sent_chunks: u64,
    pub dropped_chunks: u64,
    pub drop_policy: ProcessedAudioDropPolicy,
    pub source_filter: ProcessedAudioSourceFilter,
    pub mixing_mode: ProcessedAudioMixingMode,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProcessedAudioConsumerHealthPayload {
    pub consumers: Vec<ProcessedAudioConsumerHealth>,
}

fn validate_non_empty_field(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if value.trim() != value {
        return Err(format!(
            "{field} must not include leading or trailing whitespace"
        ));
    }
    Ok(())
}

fn validate_optional_field(
    consumer_id: &str,
    field: &str,
    value: Option<&str>,
) -> Result<(), String> {
    if let Some(value) = value {
        validate_non_empty_field(
            &format!("processed audio consumer '{}' {}", consumer_id, field),
            value,
        )?;
    }
    Ok(())
}

fn validate_channel_capacity(
    consumer_id: &str,
    side: &str,
    actual: Option<usize>,
    expected: usize,
) -> Result<(), String> {
    match actual {
        Some(actual) if actual == expected => Ok(()),
        Some(actual) => Err(format!(
            "processed audio consumer '{}' {} capacity {} does not match descriptor capacity {}",
            consumer_id, side, actual, expected
        )),
        None => Err(format!(
            "processed audio consumer '{}' must use a bounded channel; {} is unbounded",
            consumer_id, side
        )),
    }
}

fn try_send_dropping_oldest(
    tx: &Sender<ProcessedAudioChunk>,
    drain_rx: &Receiver<ProcessedAudioChunk>,
    chunk: ProcessedAudioChunk,
) -> (bool, bool) {
    match tx.try_send(chunk) {
        Ok(()) => (true, false),
        Err(TrySendError::Full(returned)) => {
            let dropped = drain_rx.try_recv().is_ok();
            match tx.try_send(returned) {
                Ok(()) => (true, dropped),
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => (false, dropped),
            }
        }
        Err(TrySendError::Disconnected(_)) => (false, false),
    }
}

#[derive(Default)]
pub struct ProcessedAudioConsumerRegistry {
    consumers: RwLock<Vec<Arc<ProcessedAudioConsumer>>>,
}

impl ProcessedAudioConsumerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, registration: ProcessedAudioConsumerRegistration) -> Result<(), String> {
        registration.validate()?;

        let mut consumers = self
            .consumers
            .write()
            .map_err(|_| "processed audio consumer registry lock poisoned".to_string())?;
        if consumers
            .iter()
            .any(|consumer| consumer.descriptor.id == registration.descriptor.id)
        {
            return Err(format!(
                "processed audio consumer '{}' already registered",
                registration.descriptor.id
            ));
        }
        if let Some(conflict_group) = registration.descriptor.conflict_group.as_deref()
            && let Some(conflicting) = consumers.iter().find(|consumer| {
                consumer.descriptor.conflict_group.as_deref() == Some(conflict_group)
            })
        {
            return Err(format!(
                "processed audio consumer '{}' conflicts with registered consumer '{}' in group '{}'",
                registration.descriptor.id, conflicting.descriptor.id, conflict_group
            ));
        }

        consumers.push(Arc::new(ProcessedAudioConsumer {
            descriptor: registration.descriptor,
            tx: registration.tx,
            drain_rx: registration.drain_rx,
            is_active: registration.is_active,
            sent_chunks: AtomicU64::new(0),
            dropped_chunks: AtomicU64::new(0),
        }));
        Ok(())
    }

    pub fn register_runtime_realtime_agent(
        &self,
        descriptor: RuntimeRealtimeAgentConsumerDescriptor,
        is_active: ConsumerActiveFn,
    ) -> Result<RuntimeProcessedAudioConsumer, String> {
        if descriptor.id.trim().is_empty() {
            return Err("runtime realtime audio consumer id must not be empty".to_string());
        }
        if descriptor.provider.trim().is_empty() {
            return Err(format!(
                "runtime realtime audio consumer '{}' must declare a provider",
                descriptor.id
            ));
        }
        if descriptor.capacity == 0 {
            return Err(format!(
                "runtime realtime audio consumer '{}' must use a positive bounded capacity",
                descriptor.id
            ));
        }

        let capacity = descriptor.capacity;
        let descriptor = descriptor.into_processed_audio_descriptor();
        let (tx, rx) = crossbeam_channel::bounded::<ProcessedAudioChunk>(capacity);
        self.register(ProcessedAudioConsumerRegistration {
            descriptor: descriptor.clone(),
            tx,
            drain_rx: rx.clone(),
            is_active,
        })?;

        Ok(RuntimeProcessedAudioConsumer { descriptor, rx })
    }

    pub fn unregister_runtime_realtime_agent(&self, id: &str) -> bool {
        self.unregister(id)
    }

    pub fn unregister(&self, id: &str) -> bool {
        let mut consumers = match self.consumers.write() {
            Ok(consumers) => consumers,
            Err(poisoned) => poisoned.into_inner(),
        };
        let before = consumers.len();
        consumers.retain(|consumer| consumer.descriptor.id != id);
        before != consumers.len()
    }

    pub fn dispatch(&self, chunk: ProcessedAudioChunk) -> ProcessedAudioDispatchSummary {
        let consumers = match self.consumers.read() {
            Ok(consumers) => consumers.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let mut summary = ProcessedAudioDispatchSummary::default();

        for consumer in consumers {
            if !(consumer.is_active)() {
                continue;
            }
            summary.active_consumers += 1;
            if !consumer.descriptor.accepts(&chunk) {
                continue;
            }

            let (sent, dropped) = match consumer.descriptor.drop_policy {
                ProcessedAudioDropPolicy::DropOldest => {
                    try_send_dropping_oldest(&consumer.tx, &consumer.drain_rx, chunk.clone())
                }
                ProcessedAudioDropPolicy::DropNewest => match consumer.tx.try_send(chunk.clone()) {
                    Ok(()) => (true, false),
                    Err(TrySendError::Full(_)) => (false, true),
                    Err(TrySendError::Disconnected(_)) => (false, false),
                },
            };

            if sent {
                consumer.sent_chunks.fetch_add(1, Ordering::Relaxed);
                summary.delivered_chunks += 1;
            }
            if dropped {
                consumer.dropped_chunks.fetch_add(1, Ordering::Relaxed);
                summary.dropped_chunks += 1;
            }
        }

        summary
    }

    pub fn health_payload(&self) -> ProcessedAudioConsumerHealthPayload {
        let consumers = match self.consumers.read() {
            Ok(consumers) => consumers.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        ProcessedAudioConsumerHealthPayload {
            consumers: consumers
                .iter()
                .map(|consumer| ProcessedAudioConsumerHealth {
                    id: consumer.descriptor.id.clone(),
                    stage: consumer.descriptor.stage.clone(),
                    provider: consumer.descriptor.provider.clone(),
                    conflict_group: consumer.descriptor.conflict_group.clone(),
                    active: (consumer.is_active)(),
                    queue_len: consumer.drain_rx.len(),
                    queue_capacity: consumer.drain_rx.capacity(),
                    sent_chunks: consumer.sent_chunks.load(Ordering::Relaxed),
                    dropped_chunks: consumer.dropped_chunks.load(Ordering::Relaxed),
                    drop_policy: consumer.descriptor.drop_policy.clone(),
                    source_filter: consumer.descriptor.source_filter.clone(),
                    mixing_mode: consumer.descriptor.mixing_mode.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use super::*;

    fn chunk(source_id: &str, sample: f32) -> ProcessedAudioChunk {
        ProcessedAudioChunk {
            source_id: Arc::<str>::from(source_id),
            data: vec![sample; 4],
            sample_rate: 16_000,
            num_frames: 4,
            timestamp: Some(Duration::from_millis(10)),
        }
    }

    fn descriptor(id: &str) -> ProcessedAudioConsumerDescriptor {
        ProcessedAudioConsumerDescriptor {
            id: id.to_string(),
            stage: ProcessedAudioConsumerStage::Speech,
            provider: Some("test".to_string()),
            conflict_group: None,
            capacity: 2,
            drop_policy: ProcessedAudioDropPolicy::DropOldest,
            source_filter: ProcessedAudioSourceFilter::All,
            mixing_mode: ProcessedAudioMixingMode::PerSource,
        }
    }

    fn register_speech_consumer(
        registry: &ProcessedAudioConsumerRegistry,
        id: &str,
    ) -> Receiver<ProcessedAudioChunk> {
        let (tx, rx) = crossbeam_channel::bounded(2);
        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: descriptor(id),
                tx,
                drain_rx: rx.clone(),
                is_active: Arc::new(|| true),
            })
            .unwrap();
        rx
    }

    fn try_register_consumer(
        registry: &ProcessedAudioConsumerRegistry,
        desc: ProcessedAudioConsumerDescriptor,
    ) -> Result<Receiver<ProcessedAudioChunk>, String> {
        let (tx, rx) = crossbeam_channel::bounded(desc.capacity);
        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: desc,
                tx,
                drain_rx: rx.clone(),
                is_active: Arc::new(|| true),
            })
            .map(|()| rx)
    }

    fn sample_values(rx: &Receiver<ProcessedAudioChunk>) -> Vec<f32> {
        rx.try_iter().map(|chunk| chunk.data[0]).collect()
    }

    #[test]
    fn dispatches_only_active_consumers() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let active = Arc::new(AtomicBool::new(true));
        let inactive = Arc::new(AtomicBool::new(false));
        let (active_tx, active_rx) = crossbeam_channel::bounded(2);
        let (inactive_tx, inactive_rx) = crossbeam_channel::bounded(2);

        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: descriptor("active"),
                tx: active_tx,
                drain_rx: active_rx.clone(),
                is_active: {
                    let active = active.clone();
                    Arc::new(move || active.load(Ordering::Relaxed))
                },
            })
            .unwrap();
        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: descriptor("inactive"),
                tx: inactive_tx,
                drain_rx: inactive_rx.clone(),
                is_active: {
                    let inactive = inactive.clone();
                    Arc::new(move || inactive.load(Ordering::Relaxed))
                },
            })
            .unwrap();

        let summary = registry.dispatch(chunk("mic", 1.0));

        assert_eq!(summary.active_consumers, 1);
        assert_eq!(summary.delivered_chunks, 1);
        assert_eq!(active_rx.len(), 1);
        assert!(inactive_rx.is_empty());
    }

    #[test]
    fn dispatches_same_chunk_to_multiple_active_consumers() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let (speech_tx, speech_rx) = crossbeam_channel::bounded(2);
        let (notes_tx, notes_rx) = crossbeam_channel::bounded(2);

        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: descriptor("speech"),
                tx: speech_tx,
                drain_rx: speech_rx.clone(),
                is_active: Arc::new(|| true),
            })
            .unwrap();
        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: descriptor("notes"),
                tx: notes_tx,
                drain_rx: notes_rx.clone(),
                is_active: Arc::new(|| true),
            })
            .unwrap();

        let summary = registry.dispatch(chunk("mic", 4.0));

        assert_eq!(summary.active_consumers, 2);
        assert_eq!(summary.delivered_chunks, 2);
        assert_eq!(summary.dropped_chunks, 0);

        let speech = speech_rx.try_recv().unwrap();
        let notes = notes_rx.try_recv().unwrap();
        assert_eq!(speech.source_id.as_ref(), "mic");
        assert_eq!(notes.source_id.as_ref(), "mic");
        assert_eq!(speech.data, notes.data);
        assert_eq!(speech.sample_rate, notes.sample_rate);
        assert_eq!(speech.num_frames, notes.num_frames);
        assert_eq!(speech.timestamp, notes.timestamp);
    }

    #[test]
    fn drop_oldest_policy_keeps_recent_audio_per_consumer() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let (tx, rx) = crossbeam_channel::bounded(2);
        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: descriptor("speech"),
                tx,
                drain_rx: rx.clone(),
                is_active: Arc::new(|| true),
            })
            .unwrap();

        assert_eq!(registry.dispatch(chunk("mic", 1.0)).dropped_chunks, 0);
        assert_eq!(registry.dispatch(chunk("mic", 2.0)).dropped_chunks, 0);
        assert_eq!(registry.dispatch(chunk("mic", 3.0)).dropped_chunks, 1);

        let kept: Vec<f32> = rx.try_iter().map(|chunk| chunk.data[0]).collect();
        assert_eq!(kept, vec![2.0, 3.0]);
        let health = registry.health_payload();
        assert_eq!(health.consumers[0].dropped_chunks, 1);
    }

    #[test]
    fn slow_drop_policy_consumer_does_not_starve_fast_consumer() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let (slow_tx, slow_rx) = crossbeam_channel::bounded(1);
        let (fast_tx, fast_rx) = crossbeam_channel::bounded(4);
        let mut slow = descriptor("slow-notes");
        slow.capacity = 1;
        let mut fast = descriptor("fast-speech");
        fast.capacity = 4;

        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: slow,
                tx: slow_tx,
                drain_rx: slow_rx.clone(),
                is_active: Arc::new(|| true),
            })
            .unwrap();
        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: fast,
                tx: fast_tx,
                drain_rx: fast_rx.clone(),
                is_active: Arc::new(|| true),
            })
            .unwrap();

        for sample in [1.0, 2.0, 3.0] {
            let summary = registry.dispatch(chunk("mic", sample));
            assert_eq!(summary.active_consumers, 2);
            assert_eq!(summary.delivered_chunks, 2);
        }

        let health = registry.health_payload();
        let slow = health
            .consumers
            .iter()
            .find(|consumer| consumer.id == "slow-notes")
            .unwrap();
        let fast = health
            .consumers
            .iter()
            .find(|consumer| consumer.id == "fast-speech")
            .unwrap();
        assert_eq!(slow.sent_chunks, 3);
        assert_eq!(slow.dropped_chunks, 2);
        assert_eq!(slow.queue_len, 1);
        assert_eq!(fast.sent_chunks, 3);
        assert_eq!(fast.dropped_chunks, 0);
        assert_eq!(fast.queue_len, 3);

        assert_eq!(sample_values(&slow_rx), vec![3.0]);
        assert_eq!(sample_values(&fast_rx), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn drop_newest_policy_keeps_queued_audio_and_counts_rejected_chunks() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let (tx, rx) = crossbeam_channel::bounded(2);
        let mut desc = descriptor("drop-newest");
        desc.drop_policy = ProcessedAudioDropPolicy::DropNewest;

        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: desc,
                tx,
                drain_rx: rx.clone(),
                is_active: Arc::new(|| true),
            })
            .unwrap();

        assert_eq!(registry.dispatch(chunk("mic", 1.0)).delivered_chunks, 1);
        assert_eq!(registry.dispatch(chunk("mic", 2.0)).delivered_chunks, 1);
        let dropped = registry.dispatch(chunk("mic", 3.0));

        assert_eq!(dropped.active_consumers, 1);
        assert_eq!(dropped.delivered_chunks, 0);
        assert_eq!(dropped.dropped_chunks, 1);

        let health = registry.health_payload();
        assert_eq!(health.consumers[0].sent_chunks, 2);
        assert_eq!(health.consumers[0].dropped_chunks, 1);
        assert_eq!(health.consumers[0].queue_len, 2);
        assert_eq!(health.consumers[0].queue_capacity, Some(2));
        assert_eq!(
            health.consumers[0].drop_policy,
            ProcessedAudioDropPolicy::DropNewest
        );
        assert_eq!(sample_values(&rx), vec![1.0, 2.0]);
    }

    #[test]
    fn source_filter_skips_unmatched_sources() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let (tx, rx) = crossbeam_channel::bounded(2);
        let mut desc = descriptor("filtered");
        desc.source_filter = ProcessedAudioSourceFilter::Sources {
            source_ids: vec!["system".to_string()],
        };
        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: desc,
                tx,
                drain_rx: rx.clone(),
                is_active: Arc::new(|| true),
            })
            .unwrap();

        let skipped = registry.dispatch(chunk("mic", 1.0));
        let delivered = registry.dispatch(chunk("system", 2.0));

        assert_eq!(skipped.active_consumers, 1);
        assert_eq!(skipped.delivered_chunks, 0);
        assert_eq!(delivered.delivered_chunks, 1);
        assert_eq!(rx.try_recv().unwrap().data[0], 2.0);
    }

    #[test]
    fn mixing_mode_filters_per_source_and_mixed_mono_chunks() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let mut per_source = descriptor("per-source");
        per_source.mixing_mode = ProcessedAudioMixingMode::PerSource;
        let per_source_rx = try_register_consumer(&registry, per_source).unwrap();

        let mut mixed_mono = descriptor("mixed-mono");
        mixed_mono.mixing_mode = ProcessedAudioMixingMode::MixedMono;
        let mixed_mono_rx = try_register_consumer(&registry, mixed_mono).unwrap();

        let per_source_summary = registry.dispatch(chunk("mic", 1.0));
        assert_eq!(per_source_summary.active_consumers, 2);
        assert_eq!(per_source_summary.delivered_chunks, 1);
        assert_eq!(sample_values(&per_source_rx), vec![1.0]);
        assert!(mixed_mono_rx.is_empty());

        let mixed_summary = registry.dispatch(chunk(MIXED_SOURCE_ID, 2.0));
        assert_eq!(mixed_summary.active_consumers, 2);
        assert_eq!(mixed_summary.delivered_chunks, 1);
        assert!(per_source_rx.is_empty());
        assert_eq!(sample_values(&mixed_mono_rx), vec![2.0]);

        let health = registry.health_payload();
        let per_source = health
            .consumers
            .iter()
            .find(|consumer| consumer.id == "per-source")
            .unwrap();
        let mixed_mono = health
            .consumers
            .iter()
            .find(|consumer| consumer.id == "mixed-mono")
            .unwrap();
        assert_eq!(per_source.sent_chunks, 1);
        assert_eq!(mixed_mono.sent_chunks, 1);
        assert_eq!(per_source.dropped_chunks, 0);
        assert_eq!(mixed_mono.dropped_chunks, 0);
    }

    #[test]
    fn register_rejects_invalid_source_filter_and_mixing_mode_contracts() {
        let registry = ProcessedAudioConsumerRegistry::new();

        let mut empty_filter = descriptor("empty-filter");
        empty_filter.source_filter = ProcessedAudioSourceFilter::Sources { source_ids: vec![] };
        let err = try_register_consumer(&registry, empty_filter).unwrap_err();
        assert!(err.contains("source filter must include at least one source"));

        let mut duplicate_filter = descriptor("duplicate-filter");
        duplicate_filter.source_filter = ProcessedAudioSourceFilter::Sources {
            source_ids: vec!["mic".to_string(), "mic".to_string()],
        };
        let err = try_register_consumer(&registry, duplicate_filter).unwrap_err();
        assert!(err.contains("duplicate source 'mic'"));

        let mut per_source_mixed = descriptor("per-source-mixed");
        per_source_mixed.source_filter = ProcessedAudioSourceFilter::Sources {
            source_ids: vec![MIXED_SOURCE_ID.to_string()],
        };
        let err = try_register_consumer(&registry, per_source_mixed).unwrap_err();
        assert!(err.contains("per_source mixing"));
        assert!(err.contains("synthetic mixed source"));

        let mut mixed_mono_source = descriptor("mixed-mono-source");
        mixed_mono_source.mixing_mode = ProcessedAudioMixingMode::MixedMono;
        mixed_mono_source.source_filter = ProcessedAudioSourceFilter::Sources {
            source_ids: vec!["mic".to_string()],
        };
        let err = try_register_consumer(&registry, mixed_mono_source).unwrap_err();
        assert!(err.contains("mixed_mono mixing"));
        assert!(err.contains("non-mixed source 'mic'"));

        assert!(registry.health_payload().consumers.is_empty());
    }

    #[test]
    fn register_rejects_blank_provider_and_conflict_group_labels() {
        let registry = ProcessedAudioConsumerRegistry::new();

        let mut blank_provider = descriptor("blank-provider");
        blank_provider.provider = Some(" ".to_string());
        let err = try_register_consumer(&registry, blank_provider).unwrap_err();
        assert!(err.contains("provider"));
        assert!(err.contains("must not be empty"));

        let mut spaced_provider = descriptor("spaced-provider");
        spaced_provider.provider = Some(" provider ".to_string());
        let err = try_register_consumer(&registry, spaced_provider).unwrap_err();
        assert!(err.contains("provider"));
        assert!(err.contains("must not include leading or trailing whitespace"));

        let mut blank_conflict = descriptor("blank-conflict");
        blank_conflict.conflict_group = Some(" ".to_string());
        let err = try_register_consumer(&registry, blank_conflict).unwrap_err();
        assert!(err.contains("conflict_group"));
        assert!(err.contains("must not be empty"));

        let mut spaced_conflict = descriptor("spaced-conflict");
        spaced_conflict.conflict_group = Some(" provider-slot ".to_string());
        let err = try_register_consumer(&registry, spaced_conflict).unwrap_err();
        assert!(err.contains("conflict_group"));
        assert!(err.contains("must not include leading or trailing whitespace"));

        assert!(registry.health_payload().consumers.is_empty());
    }

    #[test]
    fn register_rejects_unbounded_or_mismatched_channel_capacity() {
        let registry = ProcessedAudioConsumerRegistry::new();

        let (tx, rx) = crossbeam_channel::bounded(1);
        let err = registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: descriptor("capacity-mismatch"),
                tx,
                drain_rx: rx,
                is_active: Arc::new(|| true),
            })
            .unwrap_err();
        assert!(err.contains("sender capacity 1"));
        assert!(err.contains("descriptor capacity 2"));

        let (tx, rx) = crossbeam_channel::unbounded();
        let err = registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: descriptor("unbounded"),
                tx,
                drain_rx: rx,
                is_active: Arc::new(|| true),
            })
            .unwrap_err();
        assert!(err.contains("must use a bounded channel"));
        assert!(err.contains("sender is unbounded"));

        assert!(registry.health_payload().consumers.is_empty());
    }

    #[test]
    fn unregister_removes_consumer_from_dispatch_and_health() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let (tx, rx) = crossbeam_channel::bounded(2);
        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: descriptor("ephemeral"),
                tx,
                drain_rx: rx.clone(),
                is_active: Arc::new(|| true),
            })
            .unwrap();

        assert_eq!(registry.health_payload().consumers.len(), 1);
        assert!(registry.unregister("ephemeral"));
        assert!(!registry.unregister("ephemeral"));

        let summary = registry.dispatch(chunk("mic", 1.0));
        assert_eq!(summary.active_consumers, 0);
        assert!(rx.is_empty());
        assert!(registry.health_payload().consumers.is_empty());
    }

    #[test]
    fn rejects_registered_consumer_in_same_conflict_group() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let (first_tx, first_rx) = crossbeam_channel::bounded(2);
        let (second_tx, second_rx) = crossbeam_channel::bounded(2);
        let mut first = descriptor("gemini-notes");
        first.conflict_group = Some("gemini-live-client".to_string());
        let mut second = descriptor("gemini-converse");
        second.conflict_group = Some("gemini-live-client".to_string());

        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: first,
                tx: first_tx,
                drain_rx: first_rx,
                is_active: Arc::new(|| true),
            })
            .unwrap();

        let err = registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: second,
                tx: second_tx,
                drain_rx: second_rx,
                is_active: Arc::new(|| true),
            })
            .unwrap_err();

        assert!(err.contains("gemini-notes"));
        assert!(err.contains("gemini-live-client"));
    }

    #[test]
    fn unregister_releases_conflict_group() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let (first_tx, first_rx) = crossbeam_channel::bounded(2);
        let (second_tx, second_rx) = crossbeam_channel::bounded(2);
        let mut first = descriptor("gemini-notes");
        first.conflict_group = Some("gemini-live-client".to_string());
        let mut second = descriptor("gemini-converse");
        second.conflict_group = Some("gemini-live-client".to_string());

        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: first,
                tx: first_tx,
                drain_rx: first_rx,
                is_active: Arc::new(|| true),
            })
            .unwrap();
        assert!(registry.unregister("gemini-notes"));

        registry
            .register(ProcessedAudioConsumerRegistration {
                descriptor: second,
                tx: second_tx,
                drain_rx: second_rx,
                is_active: Arc::new(|| true),
            })
            .unwrap();
        let health = registry.health_payload();
        assert_eq!(health.consumers.len(), 1);
        assert_eq!(health.consumers[0].id, "gemini-converse");
    }

    #[test]
    fn runtime_realtime_agent_descriptor_registers_and_unregisters_with_bounded_channel() {
        let registry = ProcessedAudioConsumerRegistry::new();

        let runtime = registry
            .register_runtime_realtime_agent(
                RuntimeRealtimeAgentConsumerDescriptor::new(
                    "openai-realtime-voice",
                    "openai-realtime",
                    3,
                )
                .with_conflict_group("native-s2s-output")
                .with_drop_policy(ProcessedAudioDropPolicy::DropNewest),
                Arc::new(|| true),
            )
            .unwrap();

        assert_eq!(runtime.descriptor.id, "openai-realtime-voice");
        assert_eq!(
            runtime.descriptor.stage,
            ProcessedAudioConsumerStage::RealtimeAgent
        );
        assert_eq!(
            runtime.descriptor.provider.as_deref(),
            Some("openai-realtime")
        );
        assert_eq!(
            runtime.descriptor.conflict_group.as_deref(),
            Some("native-s2s-output")
        );
        assert_eq!(runtime.descriptor.capacity, 3);

        let health = registry.health_payload();
        assert_eq!(health.consumers.len(), 1);
        let consumer = &health.consumers[0];
        assert_eq!(consumer.id, "openai-realtime-voice");
        assert_eq!(consumer.stage, ProcessedAudioConsumerStage::RealtimeAgent);
        assert_eq!(consumer.provider.as_deref(), Some("openai-realtime"));
        assert_eq!(
            consumer.conflict_group.as_deref(),
            Some("native-s2s-output")
        );
        assert_eq!(consumer.queue_capacity, Some(3));
        assert_eq!(consumer.drop_policy, ProcessedAudioDropPolicy::DropNewest);

        let summary = registry.dispatch(chunk("mic", 7.0));
        assert_eq!(summary.active_consumers, 1);
        assert_eq!(summary.delivered_chunks, 1);
        assert_eq!(runtime.rx.try_recv().unwrap().data[0], 7.0);

        assert!(registry.unregister_runtime_realtime_agent("openai-realtime-voice"));
        assert!(!registry.unregister_runtime_realtime_agent("openai-realtime-voice"));
        assert!(registry.health_payload().consumers.is_empty());
    }

    #[test]
    fn runtime_realtime_agent_conflicts_reject_overlap_while_speech_stays_independent() {
        let registry = ProcessedAudioConsumerRegistry::new();
        let speech_rx = register_speech_consumer(&registry, "speech");
        let openai = registry
            .register_runtime_realtime_agent(
                RuntimeRealtimeAgentConsumerDescriptor::new(
                    "openai-realtime-voice",
                    "openai-realtime",
                    2,
                )
                .with_conflict_group("native-s2s-output"),
                Arc::new(|| true),
            )
            .unwrap();

        let err = registry
            .register_runtime_realtime_agent(
                RuntimeRealtimeAgentConsumerDescriptor::new(
                    "local-hybrid-s2s",
                    "local-hybrid-s2s",
                    2,
                )
                .with_conflict_group("native-s2s-output"),
                Arc::new(|| true),
            )
            .unwrap_err();

        assert!(err.contains("local-hybrid-s2s"));
        assert!(err.contains("openai-realtime-voice"));
        assert!(err.contains("native-s2s-output"));

        let health = registry.health_payload();
        assert_eq!(health.consumers.len(), 2);
        assert!(health.consumers.iter().any(|consumer| {
            consumer.id == "speech" && consumer.stage == ProcessedAudioConsumerStage::Speech
        }));
        assert!(health.consumers.iter().any(|consumer| {
            consumer.id == "openai-realtime-voice"
                && consumer.stage == ProcessedAudioConsumerStage::RealtimeAgent
                && consumer.provider.as_deref() == Some("openai-realtime")
                && consumer.conflict_group.as_deref() == Some("native-s2s-output")
                && consumer.queue_capacity == Some(2)
        }));

        let summary = registry.dispatch(chunk("mic", 9.0));
        assert_eq!(summary.active_consumers, 2);
        assert_eq!(summary.delivered_chunks, 2);
        assert_eq!(sample_values(&speech_rx), vec![9.0]);
        assert_eq!(sample_values(&openai.rx), vec![9.0]);
    }

    #[test]
    fn runtime_realtime_agent_registration_validates_required_contract_fields() {
        let registry = ProcessedAudioConsumerRegistry::new();

        let empty_id = registry
            .register_runtime_realtime_agent(
                RuntimeRealtimeAgentConsumerDescriptor::new(" ", "openai-realtime", 2),
                Arc::new(|| true),
            )
            .unwrap_err();
        assert!(empty_id.contains("id must not be empty"));

        let empty_provider = registry
            .register_runtime_realtime_agent(
                RuntimeRealtimeAgentConsumerDescriptor::new("openai-realtime-voice", " ", 2),
                Arc::new(|| true),
            )
            .unwrap_err();
        assert!(empty_provider.contains("must declare a provider"));

        let zero_capacity = registry
            .register_runtime_realtime_agent(
                RuntimeRealtimeAgentConsumerDescriptor::new(
                    "openai-realtime-voice",
                    "openai-realtime",
                    0,
                ),
                Arc::new(|| true),
            )
            .unwrap_err();
        assert!(zero_capacity.contains("positive bounded capacity"));

        assert!(registry.health_payload().consumers.is_empty());
    }
}
