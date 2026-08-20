//! Layered claim-class evidence table (ADR-0037) — the admission rule
//! `projection_llm::validate_operation` applies to every content-creating
//! Session Memory item (`UpsertNote` / `UpsertGraphNode` / `UpsertGraphEdge`)
//! before it may become part of a persisted `ProjectionPatch`.
//!
//! One table, not scattered conditionals: [`evidence_requirement`] is a
//! single `const fn` match from [`ClaimClass`] to its per-class minimums, and
//! [`judge_claim_evidence`] is the ONE function that may produce a FRESH
//! [`AdmittedClaimEvidence`]. Every one of its fields is private, read only
//! through accessors, mirroring `llm::route::AuthorizedRoute` /
//! `AdmittedSkin`'s sealed-constructor pattern: because no field is public,
//! no other module — not `projection_llm`, not `projections`, not
//! `llm::executor` — can construct an `AdmittedClaimEvidence` as a struct
//! literal. An operation cannot carry "admitted" evidence without having gone
//! through the judge (or, for a value re-loaded from a durable materialized
//! artifact, without its shape satisfying [`evidence_requirement`]'s
//! per-class check — see [`AdmittedClaimEvidence`]'s own doc for that
//! narrower, deserialize-only seam and its residual limit).
//!
//! What the model supplies ([`EvidenceAnchor`]) is checked, never believed
//! (ADR-0037 part 2/3): a `span_id` must resolve inside the SAME
//! basis-covered `TranscriptEvent` set the patch was actually derived from
//! (`projection_llm::basis_events`), never the live ledger — resolving
//! against the live ledger would let a `Revised` span (ADR-0031) launder a
//! stale claim into a evidence proof. A `quote` must be a literal substring
//! of the resolved event's text. A `note` must be non-empty; `UnavailableEvidence`
//! additionally requires a `span_id` resolving in the basis (nothing here is a
//! basis-free bypass — see [`judge_claim_evidence`]'s doc). `KnowledgeGap` is
//! specified but never admitted in this slice (see [`judge_claim_evidence`]).

use std::collections::BTreeMap;

use crate::projections::{TranscriptEvent, transcript_event_content_hash};

/// One claim class per ADR-0037's decision text and `CONTEXT.md`'s vocabulary
/// (Evidence Annotation, Grounded Inference, Knowledge Gap, Unavailable
/// Evidence).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ClaimClass {
    /// One identifiable verbatim substring.
    VerifiedQuote,
    /// Aggregate/inferential support: an anchor span, but no single quote.
    GroundedInference,
    /// Absence-shaped ("nothing was said about X"). See [`judge_claim_evidence`]
    /// — specified but not admissible in this slice.
    KnowledgeGap,
    /// Evidence once available is now inaccessible.
    UnavailableEvidence,
}

/// Untrusted, MODEL-SUPPLIED anchor only (ADR-0037 part 2). Deserialized
/// straight off model JSON as part of `ProjectionOperation::UpsertNote` /
/// `UpsertGraphNode` / `UpsertGraphEdge`; nothing here is trusted on its own
/// word until [`judge_claim_evidence`] resolves it against the trusted basis.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct EvidenceAnchor {
    pub claim_class: ClaimClass,
    /// Required for `VerifiedQuote` / `GroundedInference` / `UnavailableEvidence`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    /// Required for `VerifiedQuote` only; verified by containment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// Required for `UnavailableEvidence` only; must be non-empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Default for EvidenceAnchor {
    /// The fallback for a `ProjectionOperation` persisted before this
    /// contract existed (ADR-0027 backward compatibility): a bare
    /// `KnowledgeGap` anchor with no fields set. `KnowledgeGap` is the one
    /// class [`judge_claim_evidence`] refuses UNCONDITIONALLY, so this
    /// default is safe in both directions it can be reached from — a
    /// pre-contract record deserializes without ever being re-validated
    /// (materialization already happened under the old rules), and a FRESH
    /// model draft that omits `evidence` entirely lands here too and is
    /// correctly refused rather than silently admitted.
    fn default() -> Self {
        Self {
            claim_class: ClaimClass::KnowledgeGap,
            span_id: None,
            quote: None,
            note: None,
        }
    }
}

/// THE table: per-class evidence minimums as data, not scattered if/else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimEvidenceRequirement {
    pub requires_span_id: bool,
    pub requires_quote: bool,
    pub requires_note: bool,
    /// `KnowledgeGap` only; unowned/unmet in this slice (see
    /// [`judge_claim_evidence`]).
    pub requires_coverage_marker: bool,
}

/// The single match every per-class decision in this module reads from.
pub const fn evidence_requirement(class: ClaimClass) -> ClaimEvidenceRequirement {
    use ClaimClass::*;
    match class {
        VerifiedQuote => ClaimEvidenceRequirement {
            requires_span_id: true,
            requires_quote: true,
            requires_note: false,
            requires_coverage_marker: false,
        },
        GroundedInference => ClaimEvidenceRequirement {
            requires_span_id: true,
            requires_quote: false,
            requires_note: false,
            requires_coverage_marker: false,
        },
        KnowledgeGap => ClaimEvidenceRequirement {
            requires_span_id: false,
            requires_quote: false,
            requires_note: false,
            requires_coverage_marker: true,
        },
        // `requires_span_id: true` closes a basis-free bypass: without it, any
        // `{"claim_class":"unavailable_evidence","note":"x"}` admitted every
        // content-creating operation unconditionally, with no interaction with
        // `basis` at all — indistinguishable from an unjudged item, and
        // reachable by a model that always declares this class to skip every
        // other check. ADR-0037 Q0.3's own worked example ("retained-audio-
        // range annotations... degrade to Unavailable Evidence") is about a
        // SPECIFIC transcript span whose deeper evidence (the audio) is gone —
        // the transcript span itself is not, so requiring a resolvable
        // `span_id` alongside the `note` keeps that scenario expressible while
        // proving the claim is still anchored to something real in this
        // patch's basis, not merely asserted.
        UnavailableEvidence => ClaimEvidenceRequirement {
            requires_span_id: true,
            requires_quote: false,
            requires_note: true,
            requires_coverage_marker: false,
        },
    }
}

/// BACKEND-DERIVED facts about a resolved span. Never model-supplied; built
/// by resolving `span_id` against the basis-covered `TranscriptEvent` map —
/// the SAME ADR-0031-consistent set the patch was actually derived from, per
/// `projection_llm::basis_events(job, ledger)`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedSpanEvidence {
    pub span_id: String,
    pub revision_number: u64,
    /// Same fnv1a64 family as `projections::transcript_event_content_hash`
    /// (itself derived from `update_hash`, ADR-0037's cited precedent).
    pub content_hash: String,
    /// The resolved event's speaker, backend-derived. Sourced from whichever
    /// `TranscriptEvent` the caller's `basis` map actually hands this
    /// function: `projections::resolve_claim_evidence_basis_events` (the ONE
    /// apply-time builder, live and replay both) pre-corrects each event's
    /// `speaker_id`/`speaker_label` against the canonical `SpeakerTimeline`
    /// latest-wins attribution before it ever reaches here (ADR-0026 §3/§4,
    /// mirroring `timeline::build_session_timeline`'s join), so this reads
    /// the untrusted inline ASR field ONLY as that builder's fallback for a
    /// span the diarization stream never attributed (or when no diarization
    /// history exists at all, e.g. a session that never emitted speaker
    /// revisions). This function itself has no `SpeakerTimeline` to consult —
    /// it trusts whatever `event.speaker_id`/`speaker_label` the caller
    /// already resolved.
    pub speaker_ref: Option<String>,
    /// Char range into the resolved event's text, set only when a quote was
    /// verified by containment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_offset: Option<(usize, usize)>,
}

/// One refusal reason per class-shaped failure mode — not a bag of bools.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ClaimEvidenceDeficiency {
    MissingSpanId {
        class: ClaimClass,
    },
    SpanNotInBasis {
        span_id: String,
    },
    MissingQuote,
    QuoteNotContained {
        span_id: String,
    },
    MissingNote,
    /// `KnowledgeGap`, ALWAYS, until a separately named, versioned
    /// transcript-coverage marker exists elsewhere (ADR-0037 "More
    /// Information" — that marker is explicitly unowned today).
    CoverageMarkerUnavailable,
}

impl std::fmt::Display for ClaimEvidenceDeficiency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSpanId { class } => {
                write!(f, "claim class {class:?} requires a span_id, none supplied")
            }
            Self::SpanNotInBasis { span_id } => {
                write!(f, "span {span_id} is not in this patch's basis-covered set")
            }
            Self::MissingQuote => write!(f, "verified_quote requires a non-empty quote"),
            Self::QuoteNotContained { span_id } => {
                write!(
                    f,
                    "quote is not contained in span {span_id}'s resolved text"
                )
            }
            Self::MissingNote => write!(f, "unavailable_evidence requires a non-empty note"),
            Self::CoverageMarkerUnavailable => write!(
                f,
                "knowledge_gap is specified but not admissible: no transcript-coverage marker exists (ADR-0037)"
            ),
        }
    }
}

/// Proof an item's anchor was judged and satisfied its class.
///
/// Every field is private, read only through the accessors below — mirroring
/// `llm::route::AuthorizedRoute`'s all-private-fields-plus-accessors shape
/// (not clippy's `#[non_exhaustive]`, which only gates CROSS-CRATE
/// construction and would do nothing against a sibling module in this same
/// crate). [`judge_claim_evidence`] is the ONLY constructor anywhere in this
/// crate FOR A FRESH JUDGEMENT — see the module doc comment.
///
/// Deserialization is a SECOND, narrower path this type must still support:
/// `Materialized{Note,GraphNode,GraphEdge}.evidence` persists to
/// `notes.json`/`graph.json` (`persistence::save_materialized_notes`/
/// `load_materialized_notes`), which round-trips through plain JSON with no
/// checksum or manifest. A derived `#[derive(serde::Deserialize)]` would
/// reopen exactly the hole the struct's field privacy exists to close: any
/// hand-edited or corrupted JSON object shaped like this struct deserializes
/// straight into an `Admitted`-looking value, with no span, no basis, and no
/// judge ever involved. The hand-written [`Deserialize`](serde::Deserialize)
/// impl below closes the part of that hole a type can actually close:
/// deserialization re-runs [`evidence_requirement`]'s per-class shape check
/// (the SAME table `judge_claim_evidence` is built from) and refuses a blob
/// whose class/span/quote_verified/note combination `judge_claim_evidence`
/// could never have produced — a `KnowledgeGap` (unconditionally refused,
/// ADR-0037), a `VerifiedQuote` with no resolved span, a `quote_verified:
/// true` with no `span`, and so on. This is real but NOT the whole invariant
/// the module doc's first paragraph claims: it cannot detect a
/// shape-consistent blob whose `span`/`content_hash`/`quote_offset` are
/// individually well-formed but fabricated (re-checking THAT would need the
/// original transcript ledger, which does not exist at deserialize time) —
/// that residual risk is a file-integrity concern the persistence layer
/// would have to close (e.g. a checksum over `notes.json`/`graph.json`), not
/// a type-safety one this struct can fix alone.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AdmittedClaimEvidence {
    claim_class: ClaimClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    span: Option<ResolvedSpanEvidence>,
    quote_verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

/// Wire-shape mirror of [`AdmittedClaimEvidence`], deserialized first so its
/// fields can be shape-checked before the sealed type is ever constructed.
#[derive(serde::Deserialize)]
struct RawAdmittedClaimEvidence {
    claim_class: ClaimClass,
    #[serde(default)]
    span: Option<ResolvedSpanEvidence>,
    quote_verified: bool,
    #[serde(default)]
    note: Option<String>,
}

impl<'de> serde::Deserialize<'de> for AdmittedClaimEvidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawAdmittedClaimEvidence::deserialize(deserializer)?;
        let requirement = evidence_requirement(raw.claim_class);

        // `KnowledgeGap` is `judge_claim_evidence`'s ONE unconditional,
        // always-refused class — an `Admitted` value can never carry it.
        if requirement.requires_coverage_marker {
            return Err(serde::de::Error::custom(format!(
                "AdmittedClaimEvidence cannot carry claim_class {:?}: \
                 judge_claim_evidence refuses it unconditionally (ADR-0037)",
                raw.claim_class
            )));
        }
        if requirement.requires_span_id != raw.span.is_some() {
            return Err(serde::de::Error::custom(format!(
                "AdmittedClaimEvidence for claim_class {:?} must carry a resolved span iff \
                 the class requires one (requires_span_id={}, span_is_some={})",
                raw.claim_class,
                requirement.requires_span_id,
                raw.span.is_some()
            )));
        }
        if requirement.requires_quote != raw.quote_verified {
            return Err(serde::de::Error::custom(format!(
                "AdmittedClaimEvidence for claim_class {:?} must set quote_verified iff \
                 the class requires a quote (requires_quote={}, quote_verified={})",
                raw.claim_class, requirement.requires_quote, raw.quote_verified
            )));
        }
        if raw.quote_verified
            && raw
                .span
                .as_ref()
                .is_none_or(|span| span.quote_offset.is_none())
        {
            return Err(serde::de::Error::custom(
                "AdmittedClaimEvidence has quote_verified=true but no resolved quote_offset",
            ));
        }
        if requirement.requires_note != raw.note.is_some() {
            return Err(serde::de::Error::custom(format!(
                "AdmittedClaimEvidence for claim_class {:?} must carry a note iff the class \
                 requires one (requires_note={}, note_is_some={})",
                raw.claim_class,
                requirement.requires_note,
                raw.note.is_some()
            )));
        }

        Ok(AdmittedClaimEvidence {
            claim_class: raw.claim_class,
            span: raw.span,
            quote_verified: raw.quote_verified,
            note: raw.note,
        })
    }
}

impl AdmittedClaimEvidence {
    pub fn claim_class(&self) -> ClaimClass {
        self.claim_class
    }

    pub fn span(&self) -> Option<&ResolvedSpanEvidence> {
        self.span.as_ref()
    }

    pub fn quote_verified(&self) -> bool {
        self.quote_verified
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

/// The admission decision: exactly two states, no third.
pub enum ClaimAdmission {
    Admitted(AdmittedClaimEvidence),
    Refused(ClaimEvidenceDeficiency),
}

/// THE admission seam: resolves the untrusted anchor against the trusted,
/// basis-covered transcript view and judges class satisfaction.
///
/// `basis` is keyed by `span_id` over exactly the `TranscriptEvent`s
/// `projection_llm::basis_events(job, ledger)` resolved for this job — the
/// caller builds it once per patch and shares it across every content-bearing
/// operation in that patch.
pub fn judge_claim_evidence(
    anchor: &EvidenceAnchor,
    basis: &BTreeMap<&str, &TranscriptEvent>,
) -> ClaimAdmission {
    let requirement = evidence_requirement(anchor.claim_class);

    // KnowledgeGap: ADR-0037's "More Information" section says the absence
    // class is "specified but not admissible" until a separately named,
    // versioned transcript-coverage marker exists — a marker nobody owns
    // today. Refuse unconditionally, before looking at span_id/quote/note at
    // all, so a well-formed anchor can never slip through on class alone.
    if requirement.requires_coverage_marker {
        return ClaimAdmission::Refused(ClaimEvidenceDeficiency::CoverageMarkerUnavailable);
    }

    // `note` is checked before `span_id` so `UnavailableEvidence` — the one
    // class that requires BOTH (see `evidence_requirement`'s comment) — fails
    // on the cheaper, class-defining check first; neither early-returns an
    // `Admitted` on its own anymore, because a class can require either,
    // both, or neither, and the two used to be mutually exclusive `if`
    // branches that could never combine.
    let note = if requirement.requires_note {
        match non_empty_trimmed(anchor.note.as_deref()) {
            Some(note) => Some(note.to_string()),
            None => return ClaimAdmission::Refused(ClaimEvidenceDeficiency::MissingNote),
        }
    } else {
        None
    };

    if requirement.requires_span_id {
        let Some(span_id) = non_empty_trimmed(anchor.span_id.as_deref()) else {
            return ClaimAdmission::Refused(ClaimEvidenceDeficiency::MissingSpanId {
                class: anchor.claim_class,
            });
        };
        let Some(event) = basis.get(span_id) else {
            return ClaimAdmission::Refused(ClaimEvidenceDeficiency::SpanNotInBasis {
                span_id: span_id.to_string(),
            });
        };

        let (quote_verified, quote_offset) = if requirement.requires_quote {
            let Some(quote) = non_empty_trimmed(anchor.quote.as_deref()) else {
                return ClaimAdmission::Refused(ClaimEvidenceDeficiency::MissingQuote);
            };
            match event.text.find(quote) {
                Some(byte_offset) => (true, Some((byte_offset, byte_offset + quote.len()))),
                None => {
                    return ClaimAdmission::Refused(ClaimEvidenceDeficiency::QuoteNotContained {
                        span_id: span_id.to_string(),
                    });
                }
            }
        } else {
            (false, None)
        };

        return ClaimAdmission::Admitted(AdmittedClaimEvidence {
            claim_class: anchor.claim_class,
            span: Some(ResolvedSpanEvidence {
                span_id: span_id.to_string(),
                revision_number: event.revision_number,
                content_hash: transcript_event_content_hash(event),
                speaker_ref: event
                    .speaker_id
                    .clone()
                    .or_else(|| event.speaker_label.clone()),
                quote_offset,
            }),
            quote_verified,
            note,
        });
    }

    if let Some(note) = note {
        return ClaimAdmission::Admitted(AdmittedClaimEvidence {
            claim_class: anchor.claim_class,
            span: None,
            quote_verified: false,
            note: Some(note),
        });
    }

    // Every `evidence_requirement` variant sets at least one of
    // `requires_note` / `requires_span_id` / `requires_coverage_marker`
    // today (see the table). This arm exists so a future class added to
    // `ClaimClass` without a corresponding `evidence_requirement` row fails
    // closed instead of silently admitting an unjudged claim.
    ClaimAdmission::Refused(ClaimEvidenceDeficiency::CoverageMarkerUnavailable)
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projections::TranscriptEventStability;

    fn event(span_id: &str, revision_number: u64, text: &str) -> TranscriptEvent {
        TranscriptEvent {
            span_id: span_id.to_string(),
            provider: "test".to_string(),
            source_id: "source-1".to_string(),
            provider_item_id: None,
            transcript_segment_id: None,
            speaker_id: Some("speaker-1".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: None,
            text: text.to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.9,
            is_final: true,
            stability: TranscriptEventStability::Final,
            revision_number,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000 + revision_number,
        }
    }

    fn basis_of(events: &[TranscriptEvent]) -> BTreeMap<&str, &TranscriptEvent> {
        events
            .iter()
            .map(|event| (event.span_id.as_str(), event))
            .collect()
    }

    #[test]
    fn verified_quote_admits_with_a_contained_quote() {
        let events = vec![event("span-1", 3, "Alice chose Soniox for realtime tests.")];
        let basis = basis_of(&events);
        let anchor = EvidenceAnchor {
            claim_class: ClaimClass::VerifiedQuote,
            span_id: Some("span-1".to_string()),
            quote: Some("chose Soniox".to_string()),
            note: None,
        };

        match judge_claim_evidence(&anchor, &basis) {
            ClaimAdmission::Admitted(evidence) => {
                assert_eq!(evidence.claim_class, ClaimClass::VerifiedQuote);
                assert!(evidence.quote_verified);
                let span = evidence.span.expect("resolved span");
                assert_eq!(span.span_id, "span-1");
                assert_eq!(span.revision_number, 3);
                assert!(span.content_hash.starts_with("fnv1a64:"));
                assert_eq!(span.speaker_ref.as_deref(), Some("speaker-1"));
                assert_eq!(span.quote_offset, Some((6, 18)));
            }
            ClaimAdmission::Refused(deficiency) => {
                panic!("expected admission, got refusal: {deficiency:?}")
            }
        }
    }

    /// NEGATIVE: VerifiedQuote refused for missing quote (span_id present).
    #[test]
    fn verified_quote_refused_without_a_quote() {
        let events = vec![event("span-1", 1, "Alice chose Soniox.")];
        let basis = basis_of(&events);
        let anchor = EvidenceAnchor {
            claim_class: ClaimClass::VerifiedQuote,
            span_id: Some("span-1".to_string()),
            quote: None,
            note: None,
        };

        assert!(matches!(
            judge_claim_evidence(&anchor, &basis),
            ClaimAdmission::Refused(ClaimEvidenceDeficiency::MissingQuote)
        ));
    }

    /// NEGATIVE: VerifiedQuote refused when the quote is a fabricated /
    /// mismatched substring not contained in the resolved span's text.
    #[test]
    fn verified_quote_refused_for_quote_not_contained() {
        let events = vec![event("span-1", 1, "Alice chose Soniox.")];
        let basis = basis_of(&events);
        let anchor = EvidenceAnchor {
            claim_class: ClaimClass::VerifiedQuote,
            span_id: Some("span-1".to_string()),
            quote: Some("Alice chose Deepgram".to_string()),
            note: None,
        };

        assert!(matches!(
            judge_claim_evidence(&anchor, &basis),
            ClaimAdmission::Refused(ClaimEvidenceDeficiency::QuoteNotContained { span_id })
                if span_id == "span-1"
        ));
    }

    #[test]
    fn grounded_inference_admits_without_a_quote() {
        let events = vec![event(
            "span-1",
            2,
            "Across the call, three people converged on the same migration plan.",
        )];
        let basis = basis_of(&events);
        let anchor = EvidenceAnchor {
            claim_class: ClaimClass::GroundedInference,
            span_id: Some("span-1".to_string()),
            quote: None,
            note: None,
        };

        match judge_claim_evidence(&anchor, &basis) {
            ClaimAdmission::Admitted(evidence) => {
                assert_eq!(evidence.claim_class, ClaimClass::GroundedInference);
                assert!(!evidence.quote_verified);
                assert!(evidence.span.is_some());
            }
            ClaimAdmission::Refused(deficiency) => {
                panic!("expected admission, got refusal: {deficiency:?}")
            }
        }
    }

    /// NEGATIVE: GroundedInference refused for a missing span_id.
    #[test]
    fn grounded_inference_refused_without_a_span_id() {
        let basis = BTreeMap::new();
        let anchor = EvidenceAnchor {
            claim_class: ClaimClass::GroundedInference,
            span_id: None,
            quote: None,
            note: None,
        };

        assert!(matches!(
            judge_claim_evidence(&anchor, &basis),
            ClaimAdmission::Refused(ClaimEvidenceDeficiency::MissingSpanId {
                class: ClaimClass::GroundedInference
            })
        ));
    }

    /// NEGATIVE: both span-anchored classes refuse a span_id that resolves to
    /// something OUTSIDE this patch's basis-covered set (a different
    /// session's span, or a span that has since moved to a `Revised`
    /// revision) — proving evidence cannot be laundered from outside the
    /// ADR-0031 basis.
    #[test]
    fn span_anchored_classes_refuse_a_span_outside_the_basis() {
        let events = vec![event("span-in-basis", 1, "in basis")];
        let basis = basis_of(&events);

        for class in [ClaimClass::VerifiedQuote, ClaimClass::GroundedInference] {
            let anchor = EvidenceAnchor {
                claim_class: class,
                span_id: Some("span-from-another-session".to_string()),
                quote: Some("in basis".to_string()),
                note: None,
            };
            assert!(
                matches!(
                    judge_claim_evidence(&anchor, &basis),
                    ClaimAdmission::Refused(ClaimEvidenceDeficiency::SpanNotInBasis { span_id })
                        if span_id == "span-from-another-session"
                ),
                "class {class:?} must refuse a span outside the basis"
            );
        }
    }

    /// NEGATIVE: KnowledgeGap always refused regardless of a well-formed
    /// anchor — pins ADR-0037's "specified but not admissible" consequence as
    /// an executable invariant, not a comment.
    #[test]
    fn knowledge_gap_is_always_refused() {
        let events = vec![event("span-1", 1, "well formed anchor target")];
        let basis = basis_of(&events);
        let anchor = EvidenceAnchor {
            claim_class: ClaimClass::KnowledgeGap,
            span_id: Some("span-1".to_string()),
            quote: Some("well formed anchor target".to_string()),
            note: Some("a note too".to_string()),
        };

        assert!(matches!(
            judge_claim_evidence(&anchor, &basis),
            ClaimAdmission::Refused(ClaimEvidenceDeficiency::CoverageMarkerUnavailable)
        ));
    }

    #[test]
    fn unavailable_evidence_admits_with_a_non_empty_note_and_a_resolved_span() {
        let events = vec![event(
            "span-1",
            1,
            "Original session audio was not retained.",
        )];
        let basis = basis_of(&events);
        let anchor = EvidenceAnchor {
            claim_class: ClaimClass::UnavailableEvidence,
            span_id: Some("span-1".to_string()),
            quote: None,
            note: Some("Original session audio was not retained (ADR-0037 Q0.3).".to_string()),
        };

        match judge_claim_evidence(&anchor, &basis) {
            ClaimAdmission::Admitted(evidence) => {
                assert_eq!(evidence.claim_class, ClaimClass::UnavailableEvidence);
                assert!(!evidence.quote_verified);
                assert_eq!(
                    evidence.span.as_ref().map(|span| span.span_id.as_str()),
                    Some("span-1")
                );
                assert!(evidence.note.is_some());
            }
            ClaimAdmission::Refused(deficiency) => {
                panic!("expected admission, got refusal: {deficiency:?}")
            }
        }
    }

    /// NEGATIVE: UnavailableEvidence refused for a missing/empty note,
    /// regardless of `span_id` — the note check runs first.
    #[test]
    fn unavailable_evidence_refused_without_a_note() {
        let basis = BTreeMap::new();
        for note in [None, Some("   ".to_string())] {
            let anchor = EvidenceAnchor {
                claim_class: ClaimClass::UnavailableEvidence,
                span_id: None,
                quote: None,
                note,
            };
            assert!(matches!(
                judge_claim_evidence(&anchor, &basis),
                ClaimAdmission::Refused(ClaimEvidenceDeficiency::MissingNote)
            ));
        }
    }

    /// NEGATIVE: this is the finding this closes — before `requires_span_id`
    /// on `UnavailableEvidence`, `{"claim_class":"unavailable_evidence",
    /// "note":"x"}` admitted unconditionally with NO interaction with
    /// `basis` at all, so a model that always declared this class bypassed
    /// every other check. A well-formed note with no span (or a span outside
    /// the basis) must now be refused.
    #[test]
    fn unavailable_evidence_refused_without_a_resolvable_span_id() {
        let events = vec![event("span-in-basis", 1, "in basis")];
        let basis = basis_of(&events);
        let well_formed_note = Some("Original session audio was not retained.".to_string());

        assert!(matches!(
            judge_claim_evidence(
                &EvidenceAnchor {
                    claim_class: ClaimClass::UnavailableEvidence,
                    span_id: None,
                    quote: None,
                    note: well_formed_note.clone(),
                },
                &basis,
            ),
            ClaimAdmission::Refused(ClaimEvidenceDeficiency::MissingSpanId {
                class: ClaimClass::UnavailableEvidence
            })
        ));

        assert!(matches!(
            judge_claim_evidence(
                &EvidenceAnchor {
                    claim_class: ClaimClass::UnavailableEvidence,
                    span_id: Some("span-from-another-session".to_string()),
                    quote: None,
                    note: well_formed_note,
                },
                &basis,
            ),
            ClaimAdmission::Refused(ClaimEvidenceDeficiency::SpanNotInBasis { span_id })
                if span_id == "span-from-another-session"
        ));
    }

    /// Sealed-constructor invariant: the only way to observe an
    /// `AdmittedClaimEvidence` anywhere in this crate is through
    /// `ClaimAdmission::Admitted`, which only `judge_claim_evidence` produces.
    /// Every field of `AdmittedClaimEvidence` is private (checked
    /// structurally by the compiler, not by this test), which is what makes
    /// constructing one directly from another module a compile error —
    /// mirroring `llm::route::AuthorizedRoute`'s invariant, which is likewise
    /// enforced by field privacy rather than a runtime assertion. This test
    /// pins the *observable* half of that contract: judging the same anchor
    /// twice against the same basis is deterministic, so nothing upstream can
    /// inject a differently-shaped "admitted" value through any path other
    /// than this function.
    #[test]
    fn judge_claim_evidence_is_the_sole_admission_path_and_is_deterministic() {
        let events = vec![event("span-1", 1, "Alice chose Soniox.")];
        let basis = basis_of(&events);
        let anchor = EvidenceAnchor {
            claim_class: ClaimClass::VerifiedQuote,
            span_id: Some("span-1".to_string()),
            quote: Some("chose Soniox".to_string()),
            note: None,
        };

        let first = judge_claim_evidence(&anchor, &basis);
        let second = judge_claim_evidence(&anchor, &basis);
        match (first, second) {
            (ClaimAdmission::Admitted(a), ClaimAdmission::Admitted(b)) => assert_eq!(a, b),
            _ => panic!("expected both judgements to admit identically"),
        }
    }

    /// The half the test above does NOT cover: `AdmittedClaimEvidence` also
    /// derives `serde::Deserialize` indirectly through a hand-written impl
    /// (persistence needs to round-trip it), which reopens a construction
    /// path outside `judge_claim_evidence` for anything shaped like the
    /// struct. This pins that the deserialize seam still refuses a shape
    /// `judge_claim_evidence` could never have produced — a `KnowledgeGap`
    /// (unconditionally refused) is the sharpest case, since ANY well-formed
    /// anchor of that class is always refused, so an `Admitted` value
    /// carrying it is proof-by-construction of tampering or corruption, not
    /// a legitimate judgement.
    #[test]
    fn admitted_claim_evidence_deserialize_refuses_a_knowledge_gap_shaped_blob() {
        let corrupted = serde_json::json!({
            "claim_class": "knowledge_gap",
            "quote_verified": false
        });
        let error = serde_json::from_value::<AdmittedClaimEvidence>(corrupted)
            .expect_err("a KnowledgeGap-classed AdmittedClaimEvidence must never deserialize");
        assert!(
            error.to_string().contains("knowledge_gap") || error.to_string().contains("ADR-0037"),
            "got: {error}"
        );
    }

    /// Same seam, a subtler shape mismatch: `verified_quote` claims
    /// `quote_verified: true` but carries no resolved `span` at all, which
    /// `judge_claim_evidence` can never produce (a `VerifiedQuote` either
    /// resolves a span with an offset or is refused before constructing
    /// anything).
    #[test]
    fn admitted_claim_evidence_deserialize_refuses_quote_verified_without_a_span() {
        let corrupted = serde_json::json!({
            "claim_class": "verified_quote",
            "quote_verified": true
        });
        assert!(
            serde_json::from_value::<AdmittedClaimEvidence>(corrupted).is_err(),
            "quote_verified=true with no resolved span must be refused"
        );
    }

    /// The legitimate shape for every class round-trips through the SAME
    /// Deserialize seam that refuses the malformed shapes above.
    #[test]
    fn admitted_claim_evidence_deserialize_round_trips_every_legitimate_class_shape() {
        let events = vec![event("span-1", 1, "Alice chose Soniox.")];
        let basis = basis_of(&events);

        let verified_quote = judge_claim_evidence(
            &EvidenceAnchor {
                claim_class: ClaimClass::VerifiedQuote,
                span_id: Some("span-1".to_string()),
                quote: Some("chose Soniox".to_string()),
                note: None,
            },
            &basis,
        );
        let unavailable_evidence = judge_claim_evidence(
            &EvidenceAnchor {
                claim_class: ClaimClass::UnavailableEvidence,
                span_id: Some("span-1".to_string()),
                quote: None,
                note: Some("audio not retained".to_string()),
            },
            &basis,
        );

        for admission in [verified_quote, unavailable_evidence] {
            let ClaimAdmission::Admitted(evidence) = admission else {
                panic!("expected admission");
            };
            let json = serde_json::to_value(&evidence).expect("serializes");
            let round_tripped: AdmittedClaimEvidence =
                serde_json::from_value(json).expect("a legitimate shape must deserialize");
            assert_eq!(round_tripped, evidence);
        }
    }

    #[test]
    fn evidence_requirement_table_matches_the_adr() {
        assert_eq!(
            evidence_requirement(ClaimClass::VerifiedQuote),
            ClaimEvidenceRequirement {
                requires_span_id: true,
                requires_quote: true,
                requires_note: false,
                requires_coverage_marker: false,
            }
        );
        assert_eq!(
            evidence_requirement(ClaimClass::GroundedInference),
            ClaimEvidenceRequirement {
                requires_span_id: true,
                requires_quote: false,
                requires_note: false,
                requires_coverage_marker: false,
            }
        );
        assert_eq!(
            evidence_requirement(ClaimClass::KnowledgeGap),
            ClaimEvidenceRequirement {
                requires_span_id: false,
                requires_quote: false,
                requires_note: false,
                requires_coverage_marker: true,
            }
        );
        assert_eq!(
            evidence_requirement(ClaimClass::UnavailableEvidence),
            ClaimEvidenceRequirement {
                requires_span_id: true,
                requires_quote: false,
                requires_note: true,
                requires_coverage_marker: false,
            }
        );
    }
}
