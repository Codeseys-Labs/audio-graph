//! Data-movement ledgering for the projection LLM path (ADR-0025 §2g / seed
//! audio-graph-72d5).
//!
//! The context-efficiency layer creates *new* transcript-derived artifacts that
//! leave the device — the rolling summary (§2c) and the vendor-cached prefix
//! (§2d, `cache_control` persists a copy on the provider for the TTL). Each
//! projection LLM call is therefore recorded in the session data-movement
//! ledger via [`DataMovementLedgerBuilder`], the codebase's first-class privacy
//! substrate (seed 70a3).
//!
//! Two invariants this module enforces:
//!
//! 1. **Content-free.** Events carry data-class tags + sizes (token/char
//!    counts) + destinations, never transcript text, summary text, or prompt
//!    bodies. The ledger schema has no text-bearing field by construction; this
//!    module simply never tries to smuggle content into the count fields.
//! 2. **Policy-gated.** The whole remote path is gated behind the same
//!    cloud-transfer policy that governs every other cloud flow. A session
//!    pinned to local-only providers records the call as a *local* movement and
//!    writes NO summary/prefix to a remote destination — so a local-only
//!    session never ledgers a remote cache write.
//!
//! This is the first *production* emitter on the data-movement write path (the
//! builder + repository append + schema shipped with seed 70a3 but had only
//! test callers). Timeline-P1 flagged that no production read path emitted
//! these yet — this module is that emitter for the projection-submission path.

use crate::persistence::{
    ArtifactStorageKind, DataClass, DataMovementActor, DataMovementDestination, DataMovementEvent,
    DataMovementEventType, DataMovementLedgerBuilder, DataMovementResult, MovementBasis,
    MovementCounts, MovementModel, MovementPolicy, PrivacyMode, RetentionClass,
};
use std::path::Path;

/// Content-free inputs describing one projection LLM submission.
///
/// Every field is a count, id, or class — never text. `Debug` is safe to log:
/// there is no content-bearing field.
#[derive(Debug, Clone)]
pub struct ProjectionMovementFacts {
    pub session_id: String,
    /// The registry provider id (`llm.*`) of the route this call actually
    /// reached or attempted, e.g. `"llm.cerebras"`. Callers stamp this from the
    /// `Configured`/`Actual`/`FailedRoute` identity resolved in
    /// `speech::mod::projection_movement_facts` — not a fixed "configured
    /// provider" tag — so a Cerebras attempt is inventoried under `llm.cerebras`
    /// whether it succeeded or failed (seeds audio-graph-862c / audio-graph-7da4).
    pub provider_id: String,
    /// Model id when known (empty string when the local sentinel).
    pub model_id: String,
    /// Whether the resolved path actually leaves the device. When `false` the
    /// call is a local movement and no remote summary/prefix is recorded.
    pub requires_cloud_transfer: bool,
    /// Session policy: does the user consent to cloud content transfer
    /// (`PrivacyMode::ByokCloud`)? Gates the remote path.
    pub cloud_transfer_allowed: bool,
    /// Projection sequence for the basis link.
    pub projection_sequence: u64,
    /// Whether the prompt carried a rolling summary of older turns (a new
    /// transcript-derived off-device artifact, `DataClass::Notes`).
    pub has_rolling_summary: bool,
    /// Whether a `cache_control` breakpoint was set (the vendor persists the
    /// cached prefix for the TTL — a durable off-device copy).
    pub has_cached_prefix: bool,
    /// Char count of the pinned typed-fact block (graph-derived context). 0
    /// when absent.
    pub pinned_fact_chars: u64,
    /// Char count of the Notes-kind live document-order outline block (seed
    /// audio-graph-253c part 2, outline replacing the old recency-sorted
    /// snapshot as of audio-graph-a6b5 W2) — content-free like every other
    /// field here: a count only, never the note text itself. 0 for a
    /// Graph-kind call, when no snapshot was available for this tick, OR when
    /// a snapshot was available but held zero notes — a fresh session's first
    /// Notes tick still renders a static "(no sections yet)" block into the
    /// prompt (`projection_llm`'s prompt builder), but that block is
    /// boilerplate with no note content, so this field (and
    /// `notes_snapshot_entries`) reports 0 rather than counting it (see
    /// `projection_llm::ProjectionPromptShape::notes_snapshot_chars`, which
    /// this is copied from).
    pub notes_snapshot_chars: u64,
    /// Number of existing notes rendered into the snapshot block. 0 under the
    /// same conditions as `notes_snapshot_chars`.
    ///
    /// NOTE (audio-graph-253c part 2 review): unlike `notes_snapshot_chars`
    /// (which folds into the durable ledger's `text_chars` via
    /// [`movement_counts`]), this field has no sink in
    /// [`crate::persistence::DataMovementEvent`] — `MovementCounts` (the
    /// `audio-graph-ipc-contract` crate) has no entry-count field, and adding
    /// one is an ipc-contract schema change this seed deliberately stayed out
    /// of. It is populated and unit-tested at the `ProjectionMovementFacts`
    /// level only (in-memory observability of the ADR-0025 §2g compliance
    /// contract this module enforces), not persisted. If the durable ledger
    /// ever needs an entry count alongside `text_chars`, that is a follow-up
    /// ipc-contract change, not a bug in this field.
    pub notes_snapshot_entries: u32,
    /// Count of `upsert_note` operations the ingest no-op filter dropped from
    /// this patch at admission (audio-graph-a6b5 W2 / design-b §1.5b) — see
    /// `projection_llm::TrustedProjectionPatch::no_op_filtered_count`. 0 for
    /// a Graph-kind call, or when the caller had no live notes state to
    /// compare against. Same posture as `notes_snapshot_entries` above: a
    /// count only, populated and unit-tested at the `ProjectionMovementFacts`
    /// level, with no dedicated `MovementCounts` sink (an ipc-contract change
    /// this ticket deliberately stays out of) — surfaced instead via the
    /// `speech::mod::run_projection_job` apply-success log line, which is
    /// what makes the no-op-filter fix verifiable from logs.
    pub no_op_filtered_count: u32,
    /// Total input token count as reported by the provider (0 when unknown).
    pub tokens_in: u64,
    /// Total output token count (0 when unknown).
    pub tokens_out: u64,
}

/// Where the projection call's data actually went, and under what policy.
///
/// Returns `None` when the movement should not be recorded at all (a
/// non-cloud provider on a local-only session moves nothing off-device that is
/// interesting to the *remote* ledger — but see [`build_events`], which still
/// records the local movement for completeness).
fn resolved_destination(
    facts: &ProjectionMovementFacts,
) -> (DataMovementDestination, MovementPolicy) {
    // A remote flow happens only when the provider requires cloud transfer AND
    // the session policy allows it. Otherwise the call is a local movement.
    let remote = facts.requires_cloud_transfer && facts.cloud_transfer_allowed;
    if remote {
        (
            DataMovementDestination::provider(facts.provider_id.clone(), "chat_completions"),
            MovementPolicy {
                privacy_mode: PrivacyMode::ByokCloud,
                user_visible: true,
                // Prompt bodies are transient; the vendor-cached prefix persists
                // only for the provider TTL, which the ledger notes via the
                // cached-prefix data class rather than a retention promotion.
                retention_class: RetentionClass::Transient,
            },
        )
    } else {
        (
            DataMovementDestination::local(),
            MovementPolicy {
                privacy_mode: if facts.cloud_transfer_allowed {
                    PrivacyMode::ByokCloud
                } else {
                    PrivacyMode::LocalOnly
                },
                user_visible: true,
                retention_class: RetentionClass::Transient,
            },
        )
    }
}

/// Data classes moved by this call. Remote calls tag the transcript-derived and
/// graph-derived artifacts; local calls stay minimal.
fn data_classes(facts: &ProjectionMovementFacts, remote: bool) -> Vec<DataClass> {
    let mut classes = vec![DataClass::Prompts, DataClass::TranscriptText];
    if remote {
        if facts.has_rolling_summary || facts.notes_snapshot_chars > 0 {
            // Rolling summary is transcript-derived note text; the live
            // Notes-kind snapshot block (seed audio-graph-253c part 2) is
            // literally the materialized notes state — both are `Notes`
            // class content, so a call carrying either (or both) is tagged
            // once, not doubled.
            classes.push(DataClass::Notes);
        }
        if facts.pinned_fact_chars > 0 {
            // Pinned typed facts are graph-derived context.
            classes.push(DataClass::GraphContext);
        }
    }
    classes
}

fn movement_counts(facts: &ProjectionMovementFacts) -> MovementCounts {
    // `text_chars` sums every text-bearing block's char count into the one
    // existing ledger field rather than adding a new one (seed
    // audio-graph-253c part 2 deliberately makes no ipc-contract schema
    // change — `notes_snapshot_chars`/`notes_snapshot_entries` stay internal
    // to `ProjectionMovementFacts`, which callers/tests can still inspect
    // directly).
    let text_chars = facts
        .pinned_fact_chars
        .saturating_add(facts.notes_snapshot_chars);
    MovementCounts {
        audio_ms: None,
        text_chars: (text_chars > 0).then_some(text_chars),
        tokens_in: (facts.tokens_in > 0).then_some(facts.tokens_in),
        tokens_out: (facts.tokens_out > 0).then_some(facts.tokens_out),
        bytes: None,
    }
}

/// Build the content-free data-movement events for one projection submission.
///
/// - `ProviderCallStarted` (before the call) and a terminal
///   `ProviderCallSucceeded`/`ProviderCallFailed` are the two lifecycle events
///   the seed asks for. This helper builds the *terminal* event (the one that
///   knows the token counts and outcome); [`build_started_event`] builds the
///   pre-call one.
/// - A `ProjectionPatchAccepted`/`ProjectionPatchRejected` event captures the
///   apply outcome.
///
/// On a local-only session with a cloud provider, the destination is `Local`
/// and neither the rolling-summary (`Notes`) nor cached-prefix marker is
/// recorded against a remote boundary — satisfying "a local-only-pinned session
/// writes NO summary/prefix to a remote cache".
pub fn build_terminal_event(
    facts: &ProjectionMovementFacts,
    succeeded: bool,
    error_code: Option<&str>,
) -> DataMovementEvent {
    let remote = facts.requires_cloud_transfer && facts.cloud_transfer_allowed;
    let (destination, policy) = resolved_destination(facts);
    let event_type = if succeeded {
        DataMovementEventType::ProviderCallSucceeded
    } else {
        DataMovementEventType::ProviderCallFailed
    };

    let mut builder = DataMovementLedgerBuilder::new(
        facts.session_id.clone(),
        DataMovementActor::System,
        event_type,
        policy,
        destination,
    )
    .data_classes(data_classes(facts, remote))
    .model(MovementModel {
        provider_id: Some(facts.provider_id.clone()),
        model_id: (!facts.model_id.is_empty()).then(|| facts.model_id.clone()),
    })
    .counts(movement_counts(facts))
    .basis(MovementBasis {
        transcript_sequence: None,
        projection_sequence: Some(facts.projection_sequence),
    });

    // The vendor-cached prefix is a durable off-device copy (persists for the
    // provider TTL). Only a *remote* call writes it; a local call never does.
    // Recorded as a synthetic artifact ref so the privacy report can show the
    // vendor-side persistence without carrying any prompt bytes.
    if remote && facts.has_cached_prefix {
        builder = builder.artifact(
            "vendor_cached_prompt_prefix",
            ArtifactStorageKind::RepositoryRecord,
            Path::new(&format!(
                "provider-cache://{}/{}",
                facts.provider_id, facts.session_id
            )),
        );
    }

    if !succeeded {
        builder = builder.result(DataMovementResult::failed(
            error_code.unwrap_or("projection_call_failed"),
            "projection LLM call failed",
        ));
    }

    builder.build()
}

/// Build the pre-call `ProviderCallStarted` lifecycle event (no token counts
/// yet). Same destination/policy gating as the terminal event.
pub fn build_started_event(facts: &ProjectionMovementFacts) -> DataMovementEvent {
    let remote = facts.requires_cloud_transfer && facts.cloud_transfer_allowed;
    let (destination, policy) = resolved_destination(facts);
    DataMovementLedgerBuilder::new(
        facts.session_id.clone(),
        DataMovementActor::System,
        DataMovementEventType::ProviderCallStarted,
        policy,
        destination,
    )
    .data_classes(data_classes(facts, remote))
    .model(MovementModel {
        provider_id: Some(facts.provider_id.clone()),
        model_id: (!facts.model_id.is_empty()).then(|| facts.model_id.clone()),
    })
    .basis(MovementBasis {
        transcript_sequence: None,
        projection_sequence: Some(facts.projection_sequence),
    })
    .result(DataMovementResult::started())
    .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{DestinationBoundary, MovementStatus};

    fn cloud_facts() -> ProjectionMovementFacts {
        ProjectionMovementFacts {
            session_id: "session-1".to_string(),
            provider_id: "llm.openrouter".to_string(),
            model_id: "anthropic/claude-sonnet-4.5".to_string(),
            requires_cloud_transfer: true,
            cloud_transfer_allowed: true,
            projection_sequence: 4,
            has_rolling_summary: true,
            has_cached_prefix: true,
            pinned_fact_chars: 120,
            notes_snapshot_chars: 0,
            notes_snapshot_entries: 0,
            no_op_filtered_count: 0,
            tokens_in: 300,
            tokens_out: 80,
        }
    }

    #[test]
    fn cloud_call_records_remote_destination_summary_and_cached_prefix() {
        let event = build_terminal_event(&cloud_facts(), true, None);

        assert_eq!(event.destination.boundary, DestinationBoundary::Provider);
        assert_eq!(
            event.destination.provider_id.as_deref(),
            Some("llm.openrouter")
        );
        assert_eq!(
            event.event_type,
            DataMovementEventType::ProviderCallSucceeded
        );
        // Transcript-derived summary (Notes) + graph-derived pinned facts
        // (GraphContext) are recorded as moved data classes.
        assert!(event.data_classes.contains(&DataClass::Notes));
        assert!(event.data_classes.contains(&DataClass::GraphContext));
        assert!(event.data_classes.contains(&DataClass::Prompts));
        // Vendor-side cached prefix persistence is recorded as an artifact ref.
        assert!(
            event
                .artifact_refs
                .iter()
                .any(|a| a.kind == "vendor_cached_prompt_prefix")
        );
        // Content-free: counts carry sizes, no text field exists on the schema.
        let counts = event.counts.expect("counts present");
        assert_eq!(counts.tokens_in, Some(300));
        assert_eq!(counts.tokens_out, Some(80));
        // Serialized form never contains the transcript/summary text.
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(!json.contains("Speaker"));
        assert!(!json.to_lowercase().contains("transcript text"));
    }

    /// A local-only-pinned session with a cloud provider writes NO summary/prefix
    /// to a remote cache — the acceptance criterion for seed 72d5.
    #[test]
    fn local_only_session_writes_no_remote_summary_or_prefix() {
        let mut facts = cloud_facts();
        facts.cloud_transfer_allowed = false; // local-only policy

        let event = build_terminal_event(&facts, true, None);

        // Destination stayed on-device.
        assert_eq!(event.destination.boundary, DestinationBoundary::Local);
        assert!(event.destination.provider_id.is_none());
        // No rolling-summary (Notes) or pinned-fact (GraphContext) class is
        // recorded against a remote boundary.
        assert!(!event.data_classes.contains(&DataClass::Notes));
        assert!(!event.data_classes.contains(&DataClass::GraphContext));
        // No vendor-cache artifact — the prefix never left the device.
        assert!(event.artifact_refs.is_empty());
    }

    /// A genuinely local provider (loopback API / local llama) never records a
    /// remote flow even when the policy would allow cloud transfer.
    #[test]
    fn local_provider_records_local_movement() {
        let mut facts = cloud_facts();
        facts.requires_cloud_transfer = false;
        facts.provider_id = "llm.local_llama".to_string();

        let event = build_terminal_event(&facts, true, None);
        assert_eq!(event.destination.boundary, DestinationBoundary::Local);
        assert!(event.artifact_refs.is_empty());
    }

    #[test]
    fn failed_call_carries_redacted_result_code() {
        let event = build_terminal_event(&cloud_facts(), false, Some("provider_timeout"));
        assert_eq!(event.event_type, DataMovementEventType::ProviderCallFailed);
        assert_eq!(event.result.status, MovementStatus::Failed);
        assert_eq!(event.result.error_code.as_deref(), Some("provider_timeout"));
    }

    #[test]
    fn started_event_has_no_token_counts() {
        let event = build_started_event(&cloud_facts());
        assert_eq!(event.event_type, DataMovementEventType::ProviderCallStarted);
        assert_eq!(event.result.status, MovementStatus::Started);
    }

    // ----- audio-graph-253c part 2: live notes-state snapshot ledgering ----

    /// A Notes-kind call carrying a live notes snapshot tags `DataClass::Notes`
    /// and folds the snapshot's char count into the existing `text_chars`
    /// ledger field, even when NO rolling summary is present — the two
    /// conditions are independent triggers for the same class (seed
    /// audio-graph-253c part 2: without this, the notes-snapshot block leaves
    /// the device unledgered once `speech/mod.rs` is wired to it).
    #[test]
    fn notes_snapshot_alone_records_notes_class_and_folds_into_text_chars() {
        let mut facts = cloud_facts();
        facts.has_rolling_summary = false;
        facts.pinned_fact_chars = 0;
        facts.notes_snapshot_chars = 240;
        facts.notes_snapshot_entries = 5;

        let event = build_terminal_event(&facts, true, None);
        assert!(
            event.data_classes.contains(&DataClass::Notes),
            "a live notes snapshot must tag DataClass::Notes even with no rolling summary"
        );
        let counts = event.counts.expect("counts present");
        assert_eq!(
            counts.text_chars,
            Some(240),
            "the snapshot's char count must reach the ledger's existing text_chars field"
        );
        // Content-free: only the count, never note titles/bodies, in the
        // serialized event.
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(!json.contains("note:"));
    }

    /// A Graph-kind call (or a Notes-kind call with no snapshot available)
    /// carries zero notes-snapshot facts and must not spuriously tag
    /// `DataClass::Notes` or inflate `text_chars` on their account.
    #[test]
    fn zero_notes_snapshot_facts_do_not_tag_notes_class() {
        let mut facts = cloud_facts();
        facts.has_rolling_summary = false;
        facts.pinned_fact_chars = 0;
        facts.notes_snapshot_chars = 0;
        facts.notes_snapshot_entries = 0;

        let event = build_terminal_event(&facts, true, None);
        assert!(!event.data_classes.contains(&DataClass::Notes));
        assert!(event.counts.expect("counts present").text_chars.is_none());
    }

    /// Both a rolling summary AND a live notes snapshot present must still
    /// tag `DataClass::Notes` exactly once (not doubled) and sum both char
    /// counts into `text_chars` alongside the pinned facts.
    #[test]
    fn rolling_summary_and_notes_snapshot_together_sum_into_one_text_chars_count() {
        let mut facts = cloud_facts(); // has_rolling_summary: true, pinned_fact_chars: 120
        facts.notes_snapshot_chars = 60;
        facts.notes_snapshot_entries = 2;

        let event = build_terminal_event(&facts, true, None);
        assert_eq!(
            event
                .data_classes
                .iter()
                .filter(|c| **c == DataClass::Notes)
                .count(),
            1,
            "DataClass::Notes must appear exactly once regardless of how many \
             notes-bearing conditions are true"
        );
        assert_eq!(event.counts.expect("counts present").text_chars, Some(180));
    }
}
