//! Structural telemetry seam for the live-assist / question-card subsystem
//! (audio-graph-81a5).
//!
//! Before this module, the question/live-assist/chat subsystem was entirely
//! unobserved: zero log hits for question/`AgentProposal`/live_assist/chat
//! across an 18k-line field log, and no logging near card creation, approval,
//! or dismissal. That makes it impossible to measure anything about card
//! behavior in the field — including the fragment rate audio-graph-104f needs
//! to gate on.
//!
//! ## Privacy invariant (load-bearing)
//!
//! This module logs **only** session ids, card ids, the closed
//! [`crate::events::AgentProposalKind`] enum, a bucketed [`ConfidenceClass`],
//! timestamps (via the ambient `log` line prefix, not a field here), and
//! counts. It **never** logs question text, utterance text, proposal
//! title/body, answer content, or any string derived from transcript/LLM
//! content.
//!
//! This is enforced structurally, not by convention at each call site: every
//! public function in this module takes only enums, `u64`/`f32` counts, and
//! two narrowly-typed id strings (`session_id`, `card_id`) — there is no
//! parameter of a shape that could carry free-form prose (no `title`, no
//! `body`, no `text`, no `Option<String>` grab-bag). The two id strings are
//! additionally validated by [`sanitize_id`] before they ever reach a log
//! line: anything that isn't a short, id-shaped token (the only thing a
//! session id or a `Uuid::new_v4()` card id ever legitimately is) is replaced
//! with [`ID_REDACTED`] rather than logged verbatim. This is belt-and-
//! suspenders — every caller in this crate passes an already-valid session id
//! (`AppState::current_session_id()`) or card id (`AgentProposalPayload::id`,
//! itself a UUID) — but it means a future caller cannot accidentally leak
//! content by passing it through the `session_id`/`card_id` slot: it would
//! render as `<invalid-id>`, not as the leaked text.
//!
//! ## What is NOT instrumented distinctly here, and why
//!
//! "Ask AI" (a user asking the agent to answer a detected question from a
//! live-assist card) is a purely frontend-composed action: the UI calls the
//! existing `dismiss_agent_proposal` command and then the existing
//! `send_chat_message` / `start_streaming_chat` command with the question
//! text as an ordinary chat message. Neither backend command receives any
//! signal that distinguishes "dismissed because the user clicked Ask AI" from
//! "dismissed because the user clicked Dismiss", nor does the chat command
//! receive any signal that a message originated from a card. Attributing
//! Ask-AI *distinctly* would require either a new IPC parameter (a frontend
//! change, out of scope per this ticket's zero-frontend-changes constraint)
//! or content-sniffing the chat message (which this module's privacy
//! invariant forbids).
//!
//! Two facts worth being explicit about, since they soften — without
//! removing — that gap:
//!
//! - `start_streaming_chat` and `send_chat_message` already log a
//!   content-free `"... called ({N} chars)"` line (pre-existing, not added
//!   by this ticket). Combined with [`log_card_event`]'s `card.dismissed
//!   kind=question` line, a field-log reader can already infer "a question
//!   card was dismissed and a chat call followed shortly after" by
//!   timestamp adjacency — a coarse correlation proxy, not attribution. This
//!   module adds [`log_chat_invoked`], a session-scoped, content-free
//!   invocation counter routed through the same seam as the card events, so
//!   that correlation doesn't require grepping two differently-shaped log
//!   lines. It is still only a proxy: a manually-typed chat message and an
//!   Ask-AI-originated one are indistinguishable at this layer.
//! - `addAgentProposal` (the frontend handler for every newly-arrived
//!   question card, not just Ask-AI) also auto-invokes a third backend
//!   command, `add_question_to_graph`, once per question card. It fires 1:1
//!   with `card.created kind=question` (every question card triggers it,
//!   unconditionally, on arrival — it has nothing to do with Ask-AI), so a
//!   separate counter for it would be redundant with the creation count
//!   already emitted; it is enumerated here rather than separately
//!   instrumented.

use crate::events::{AgentProposalKind, AnswerRefusalReason, SignalGrade};

/// Bucketed confidence/quality class — the only signal this module emits
/// about how good a proposal is. Never the raw float: a fixed 3-word
/// vocabulary keeps every emitted line grep-able forever, independent of any
/// future change to how confidence is computed upstream.
///
/// Boundaries: `>= 0.8` is [`High`](ConfidenceClass::High), `>= 0.5` is
/// [`Medium`](ConfidenceClass::Medium) (this floor matches the frontend's
/// existing `AGENT_QUEUE_CONFIDENCE_FLOOR` fragment-suspect threshold in
/// `src/components/workspace/agentQueue.ts`, so "medium and up" here lines up
/// with "not fragment-suspect" there), otherwise [`Low`](ConfidenceClass::Low).
/// A non-finite confidence (should never happen — `AgentProposalPayload`'s
/// only producer clamps to `0.0..=1.0` — but this is telemetry, so it must
/// degrade rather than panic) is treated as `Low`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceClass {
    High,
    Medium,
    Low,
}

impl ConfidenceClass {
    pub fn from_confidence(confidence: f32) -> Self {
        if !confidence.is_finite() {
            return ConfidenceClass::Low;
        }
        if confidence >= 0.8 {
            ConfidenceClass::High
        } else if confidence >= 0.5 {
            ConfidenceClass::Medium
        } else {
            ConfidenceClass::Low
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ConfidenceClass::High => "high",
            ConfidenceClass::Medium => "medium",
            ConfidenceClass::Low => "low",
        }
    }
}

/// Stable lowercase wire string for [`AgentProposalKind`]. Duplicated rather
/// than reusing the type's `serde(rename_all = "snake_case")` serialization
/// so this module has zero coupling to the IPC wire-format representation —
/// a future change to the serde attribute cannot silently change what a log
/// line says.
fn kind_str(kind: &AgentProposalKind) -> &'static str {
    match kind {
        AgentProposalKind::Note => "note",
        AgentProposalKind::Question => "question",
        AgentProposalKind::GraphSuggestion => "graph_suggestion",
    }
}

/// Placeholder substituted for any `session_id`/`card_id` argument that fails
/// [`sanitize_id`]'s shape check, so a malformed or content-bearing argument
/// never reaches a log line verbatim.
const ID_REDACTED: &str = "<invalid-id>";

/// Longest id this module will log verbatim. A `Uuid::new_v4()` string
/// (36 chars) and every session-id/test-fixture label in this crate are well
/// under this; anything longer is far more likely to be accidentally-passed
/// prose than a real id.
const MAX_ID_LEN: usize = 96;

/// Whether `s` is shaped like an id this module may log verbatim: 1..=96
/// ASCII alphanumerics plus `-`, `_`, `.` — the shape of both a UUID v4
/// string (this crate's only session-id and card-id format in production)
/// and every short test-fixture label used in this crate's own tests (e.g.
/// `"rotated-after-proposal"`). Deliberately excludes whitespace and every
/// other punctuation character, so ordinary prose (which almost always
/// contains a space) cannot pass this check.
///
/// Residual, accepted risk: a single whitespace-free token made only of
/// alphanumerics/`-`/`_`/`.` (e.g. a bare name like `"PatientJohnDoe"`) is
/// structurally indistinguishable from a real id and passes through
/// unchanged. This is the ticket's own documented floor for a validated-id
/// parameter ("if you must accept any String, it must be a validated id") —
/// closing it fully would mean minting `SessionId`/`CardId` newtypes at
/// their origin so no `&str` ever reaches this module's public surface.
/// Declined here as disproportionate to this ticket: every one of this
/// module's five call sites passes `Uuid::new_v4()` card ids or
/// `AppState::current_session_id()` session ids (verified, not merely
/// assumed), so the gap is not reachable today. Revisit if a future caller
/// needs to pass anything else through the `session_id`/`card_id` slots.
fn is_id_shaped(s: &str) -> bool {
    let len = s.chars().count();
    (1..=MAX_ID_LEN).contains(&len)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Validate `raw` as a loggable id: run it through the same secret-scrubber
/// [`crate::analytics`] uses for tag values (belt-and-suspenders — a
/// credential-shaped string like `sk-test-...` is itself id-shaped, so the
/// shape check alone would not catch it), then require the result to be
/// [`is_id_shaped`]. Returns [`ID_REDACTED`] instead of the original value
/// when either check fails — the caller never sees an echo of the rejected
/// input, only the fixed sentinel, so a rejected value can never itself leak
/// through this function's return.
fn sanitize_id(raw: &str) -> String {
    let scrubbed = crate::error::redacted_provider_diagnostic(raw, std::iter::empty::<&str>());
    if is_id_shaped(&scrubbed) {
        scrubbed
    } else {
        ID_REDACTED.to_string()
    }
}

/// Render one structured, single-line telemetry record. Pure and
/// side-effect-free so its output can be pinned directly in tests without a
/// log-capture harness (this crate deliberately has none — see
/// `logging::tests` for the only `log::set_logger` call, which is production
/// init, not a test sink).
fn format_line(
    event_name: &str,
    session_id: &str,
    card_id: Option<&str>,
    kind: Option<&AgentProposalKind>,
    confidence_class: Option<ConfidenceClass>,
    extra: &[(&str, String)],
) -> String {
    let mut line = format!(
        "card_telemetry event={event_name} session_id={}",
        sanitize_id(session_id)
    );
    if let Some(id) = card_id {
        line.push_str(&format!(" card_id={}", sanitize_id(id)));
    }
    if let Some(k) = kind {
        line.push_str(&format!(" kind={}", kind_str(k)));
    }
    if let Some(class) = confidence_class {
        line.push_str(&format!(" confidence_class={}", class.as_str()));
    }
    for (key, value) in extra {
        line.push_str(&format!(" {key}={value}"));
    }
    line
}

/// A per-card lifecycle transition this module observes. `Created` carries
/// the per-session running count (the number of live-assist cards persisted
/// for this session so far, including this one) — the one count deliverable
/// (b) asks for at creation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLifecycleEvent {
    Created { session_running_count: u64 },
    Approved,
    Dismissed,
}

impl CardLifecycleEvent {
    fn name(self) -> &'static str {
        match self {
            CardLifecycleEvent::Created { .. } => "card.created",
            CardLifecycleEvent::Approved => "card.approved",
            CardLifecycleEvent::Dismissed => "card.dismissed",
        }
    }
}

/// Stable lowercase wire string for [`SignalGrade`]. Duplicated rather than
/// reusing the type's own `serde(rename_all = "snake_case")` serialization —
/// same zero-coupling-to-the-wire-format reasoning as [`kind_str`] above.
fn signal_str(signal: SignalGrade) -> &'static str {
    match signal {
        SignalGrade::Strong => "strong",
        SignalGrade::Weak => "weak",
        SignalGrade::Fragment => "fragment",
    }
}

/// Build the line [`log_card_event`] emits, without emitting it. Pure and
/// side-effect-free so tests can assert on the EXACT string production code
/// sends to `log::info!` (not a hand-duplicated stand-in) — see
/// `log_card_event`'s call to this function below. `confidence` is bucketed
/// via [`ConfidenceClass::from_confidence`] before it ever reaches the
/// returned line — the raw float never appears in output.
///
/// `signal` (audio-graph-83cc T3, picking up T2's deferred "grade in the
/// creation line" note) is the card's T2-stamped [`SignalGrade`] — `None`
/// for every record that predates T2 (never for a production mint after it;
/// `run_agent_proposal_task` always stamps one). Omitted from the line
/// entirely when absent, matching this module's existing "absent field ⇒ no
/// key" convention (e.g. `session_running_count` on non-`Created` events).
fn card_event_line(
    session_id: &str,
    card_id: &str,
    kind: &AgentProposalKind,
    confidence: f32,
    signal: Option<SignalGrade>,
    event: CardLifecycleEvent,
) -> String {
    let class = ConfidenceClass::from_confidence(confidence);
    let mut extra: Vec<(&str, String)> = Vec::new();
    if let CardLifecycleEvent::Created {
        session_running_count,
    } = event
    {
        extra.push(("session_running_count", session_running_count.to_string()));
    }
    if let Some(signal) = signal {
        extra.push(("signal", signal_str(signal).to_string()));
    }
    format_line(
        event.name(),
        session_id,
        Some(card_id),
        Some(kind),
        Some(class),
        &extra,
    )
}

/// Log one card lifecycle transition (creation, approval, or dismissal).
pub fn log_card_event(
    session_id: &str,
    card_id: &str,
    kind: &AgentProposalKind,
    confidence: f32,
    signal: Option<SignalGrade>,
    event: CardLifecycleEvent,
) {
    let line = card_event_line(session_id, card_id, kind, confidence, signal, event);
    log::info!("{line}");
}

/// Log a session-scoped, content-free chat-invocation counter. NOT
/// attribution: this fires for every chat invocation in the session
/// (manually-typed messages included), not only ones that originated from an
/// "Ask AI" click on a live-assist card — see the module doc's "What is NOT
/// instrumented distinctly here" section for why true Ask-AI attribution is
/// out of reach without a frontend change. Wired at `start_streaming_chat`
/// and `send_chat_message` so a `card.dismissed kind=question` line
/// immediately followed by a `chat.invoked` line is at least a cheap,
/// same-seam correlation signal instead of two differently-shaped log lines.
pub fn log_chat_invoked(session_id: &str) {
    let line = format_line("chat.invoked", session_id, None, None, None, &[]);
    log::info!("{line}");
}

// ---------------------------------------------------------------------------
// audio-graph-83cc T3: answer-engine telemetry.
//
// Four lifecycle events for a card-answer dispatch attempt: `Requested`
// (the gate passed and a provider call is about to start),
// `Completed`/`Failed` (the terminal frame resolved, successfully or not —
// `CardAnswerStatus::Interrupted` also logs as `Failed`; see that status's
// own doc comment), and `Refused` (the gate declined to dispatch at all,
// carrying the closed, content-free `AnswerRefusalReason` class). Same
// privacy invariant as the rest of this module: `session_id`/`card_id` are
// validated ids, `kind`/`auto`/`reason` are closed enums/bools — there is no
// parameter shape here that could carry question or answer text.
// ---------------------------------------------------------------------------

/// A per-dispatch-attempt telemetry event for the answer engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerLifecycleEvent {
    Requested,
    Completed,
    /// The spend gate declined to dispatch. Carries the typed reason class
    /// (never the compared question content — see
    /// `crate::commands::is_duplicate_answered_question`'s doc comment for
    /// the content-never-escapes proof for the `Duplicate` reason
    /// specifically).
    Refused(AnswerRefusalReason),
    Failed,
}

impl AnswerLifecycleEvent {
    fn name(self) -> &'static str {
        match self {
            AnswerLifecycleEvent::Requested => "answer.requested",
            AnswerLifecycleEvent::Completed => "answer.completed",
            AnswerLifecycleEvent::Refused(_) => "answer.refused",
            AnswerLifecycleEvent::Failed => "answer.failed",
        }
    }
}

/// Stable lowercase wire string for [`AnswerRefusalReason`]. Duplicated
/// rather than reusing the type's own serde rename — same zero-coupling
/// reasoning as [`kind_str`]/[`signal_str`] above.
fn answer_refusal_reason_str(reason: AnswerRefusalReason) -> &'static str {
    match reason {
        AnswerRefusalReason::Disabled => "disabled",
        AnswerRefusalReason::NotQuestion => "not_question",
        AnswerRefusalReason::WeakSignal => "weak_signal",
        AnswerRefusalReason::Converse => "converse",
        AnswerRefusalReason::Duplicate => "duplicate",
        AnswerRefusalReason::Busy => "busy",
        AnswerRefusalReason::Interval => "interval",
        AnswerRefusalReason::Capped => "capped",
    }
}

/// Build the line [`log_answer_event`] emits, without emitting it. Pure and
/// side-effect-free for the same reason [`card_event_line`] is.
fn answer_event_line(
    session_id: &str,
    card_id: &str,
    kind: &AgentProposalKind,
    auto: bool,
    event: AnswerLifecycleEvent,
) -> String {
    let mut extra: Vec<(&str, String)> = vec![("auto", auto.to_string())];
    if let AnswerLifecycleEvent::Refused(reason) = event {
        extra.push(("reason", answer_refusal_reason_str(reason).to_string()));
    }
    format_line(
        event.name(),
        session_id,
        Some(card_id),
        Some(kind),
        None,
        &extra,
    )
}

/// Log one answer-engine lifecycle event (requested / completed / refused /
/// failed) for one card-answer dispatch attempt.
pub fn log_answer_event(
    session_id: &str,
    card_id: &str,
    kind: &AgentProposalKind,
    auto: bool,
    event: AnswerLifecycleEvent,
) {
    let line = answer_event_line(session_id, card_id, kind, auto, event);
    log::info!("{line}");
}

/// Per-session tally of live-assist cards by kind and bucketed confidence
/// class, for the session-stop/finalize summary line (deliverable c). Every
/// field is a plain `u64` counter — there is no field of any shape that could
/// carry a proposal's title/body, so this struct cannot become a channel for
/// content even by future accident.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionCardCounts {
    pub note_high: u64,
    pub note_medium: u64,
    pub note_low: u64,
    pub question_high: u64,
    pub question_medium: u64,
    pub question_low: u64,
    pub graph_suggestion_high: u64,
    pub graph_suggestion_medium: u64,
    pub graph_suggestion_low: u64,
}

impl SessionCardCounts {
    /// Fold one card's `(kind, confidence)` into the tally. Callers extract
    /// these two values themselves from whatever record type they hold
    /// (e.g. `LiveAssistCardRecord`) — this method's signature is
    /// deliberately just an enum and a number, so the record type itself
    /// (which does carry `proposal.body`) never has to be passed into this
    /// module.
    pub fn record(&mut self, kind: &AgentProposalKind, confidence: f32) {
        let field = match (kind, ConfidenceClass::from_confidence(confidence)) {
            (AgentProposalKind::Note, ConfidenceClass::High) => &mut self.note_high,
            (AgentProposalKind::Note, ConfidenceClass::Medium) => &mut self.note_medium,
            (AgentProposalKind::Note, ConfidenceClass::Low) => &mut self.note_low,
            (AgentProposalKind::Question, ConfidenceClass::High) => &mut self.question_high,
            (AgentProposalKind::Question, ConfidenceClass::Medium) => &mut self.question_medium,
            (AgentProposalKind::Question, ConfidenceClass::Low) => &mut self.question_low,
            (AgentProposalKind::GraphSuggestion, ConfidenceClass::High) => {
                &mut self.graph_suggestion_high
            }
            (AgentProposalKind::GraphSuggestion, ConfidenceClass::Medium) => {
                &mut self.graph_suggestion_medium
            }
            (AgentProposalKind::GraphSuggestion, ConfidenceClass::Low) => {
                &mut self.graph_suggestion_low
            }
        };
        *field += 1;
    }

    pub fn total(&self) -> u64 {
        self.note_high
            + self.note_medium
            + self.note_low
            + self.question_high
            + self.question_medium
            + self.question_low
            + self.graph_suggestion_high
            + self.graph_suggestion_medium
            + self.graph_suggestion_low
    }
}

/// Build the line [`log_session_summary`] emits, without emitting it. Pure
/// and side-effect-free for the same reason [`card_event_line`] is — see its
/// doc comment.
fn session_summary_line(session_id: &str, counts: &SessionCardCounts) -> String {
    format_line(
        "session.card_summary",
        session_id,
        None,
        None,
        None,
        &[
            ("note_high", counts.note_high.to_string()),
            ("note_medium", counts.note_medium.to_string()),
            ("note_low", counts.note_low.to_string()),
            ("question_high", counts.question_high.to_string()),
            ("question_medium", counts.question_medium.to_string()),
            ("question_low", counts.question_low.to_string()),
            (
                "graph_suggestion_high",
                counts.graph_suggestion_high.to_string(),
            ),
            (
                "graph_suggestion_medium",
                counts.graph_suggestion_medium.to_string(),
            ),
            (
                "graph_suggestion_low",
                counts.graph_suggestion_low.to_string(),
            ),
            ("total", counts.total().to_string()),
        ],
    )
}

/// Log a per-session summary line (counts by kind and confidence class) at
/// session stop/finalize. See `finalize_session`'s call sites (`lib.rs`'s
/// `RunEvent::Exit` handler and `commands::new_session_cmd`'s rotation path)
/// for the natural hooks this is wired into.
pub fn log_session_summary(session_id: &str, counts: &SessionCardCounts) {
    let line = session_summary_line(session_id, counts);
    log::info!("{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // Output-format pins (deliverable d, part 1): the emitted line must
    // contain the expected structured fields. These call `card_event_line` /
    // `session_summary_line` directly — the SAME pure functions
    // `log_card_event` / `log_session_summary` call before `log::info!` — so
    // the assertion is on production's own field assembly, not a
    // hand-duplicated stand-in array (fix-round finding: the previous
    // version of these tests called `format_line` with a self-supplied
    // `extra` array that merely mirrored what production built, so a
    // mutation to the production array-building code — e.g. dropping
    // `session_running_count`, or swapping the bucketed class for the raw
    // float — passed every test here undetected).
    // ---------------------------------------------------------------------

    #[test]
    fn card_created_line_contains_kind_class_count_and_ids() {
        let line = card_event_line(
            "session-abc-123",
            "card-def-456",
            &AgentProposalKind::Question,
            0.6,
            Some(SignalGrade::Strong),
            CardLifecycleEvent::Created {
                session_running_count: 3,
            },
        );
        assert!(line.contains("event=card.created"), "{line}");
        assert!(line.contains("session_id=session-abc-123"), "{line}");
        assert!(line.contains("card_id=card-def-456"), "{line}");
        assert!(line.contains("kind=question"), "{line}");
        assert!(line.contains("confidence_class=medium"), "{line}");
        assert!(line.contains("session_running_count=3"), "{line}");
        assert!(line.contains("signal=strong"), "{line}");
    }

    #[test]
    fn card_event_line_omits_signal_entirely_when_none() {
        // audio-graph-83cc T3: a pre-T2 record (or any record with no
        // stamped grade) must not emit a `signal=` key at all — distinct
        // from emitting an empty/placeholder value.
        let line = card_event_line(
            "sess-nosig",
            "card-nosig",
            &AgentProposalKind::Note,
            0.9,
            None,
            CardLifecycleEvent::Approved,
        );
        assert!(!line.contains("signal="), "{line}");
    }

    #[test]
    fn card_approved_and_dismissed_lines_carry_the_right_event_name() {
        let approved = card_event_line(
            "sess-1",
            "card-1",
            &AgentProposalKind::Note,
            0.9,
            None,
            CardLifecycleEvent::Approved,
        );
        assert!(approved.contains("event=card.approved"), "{approved}");
        assert!(approved.contains("kind=note"), "{approved}");
        assert!(approved.contains("confidence_class=high"), "{approved}");
        // `Approved` carries no `session_running_count` — only `Created` does.
        assert!(!approved.contains("session_running_count"), "{approved}");

        let dismissed = card_event_line(
            "sess-1",
            "card-2",
            &AgentProposalKind::GraphSuggestion,
            0.2,
            Some(SignalGrade::Fragment),
            CardLifecycleEvent::Dismissed,
        );
        assert!(dismissed.contains("event=card.dismissed"), "{dismissed}");
        assert!(dismissed.contains("kind=graph_suggestion"), "{dismissed}");
        assert!(dismissed.contains("confidence_class=low"), "{dismissed}");
        assert!(dismissed.contains("signal=fragment"), "{dismissed}");
    }

    #[test]
    fn card_event_line_never_emits_the_raw_confidence_float() {
        // Pins the module doc's claim that the raw confidence float never
        // reaches a log line — only the bucketed class does. A mutation that
        // pushed `("confidence", confidence.to_string())` into `extra`
        // instead of relying on the bucketed `confidence_class` field would
        // fail this (0.87 would appear verbatim).
        let line = card_event_line(
            "sess-float",
            "card-float",
            &AgentProposalKind::Note,
            0.8734567,
            None,
            CardLifecycleEvent::Approved,
        );
        assert!(line.contains("confidence_class=high"), "{line}");
        assert!(!line.contains("0.8734567"), "{line}");
        assert!(!line.contains("0.87"), "{line}");
    }

    #[test]
    fn card_event_line_redacts_prose_passed_through_either_id_slot() {
        // Wiring pin (adversarial-review fix-round finding): asserts through
        // the PUBLIC production line-builder, not `sanitize_id` in
        // isolation, that a non-id string in either the session_id or
        // card_id slot renders as the redacted sentinel and never verbatim.
        // A mutation that swapped `sanitize_id(session_id)` for a bare
        // `session_id` (or the same for `card_id`) inside `format_line`
        // fails this test while leaving `sanitize_id`'s own unit tests green
        // (sanitize_id stays reachable via the other slot, so clippy's
        // dead-code lint would not catch it either).
        let session_slot_leak = card_event_line(
            "patient said their social security number aloud",
            "card-def-456",
            &AgentProposalKind::Question,
            0.6,
            None,
            CardLifecycleEvent::Approved,
        );
        assert!(
            session_slot_leak.contains(&format!("session_id={ID_REDACTED}")),
            "{session_slot_leak}"
        );
        assert!(
            !session_slot_leak.contains("social security"),
            "{session_slot_leak}"
        );

        let card_slot_leak = card_event_line(
            "session-abc-123",
            "what time is it and who told you that",
            &AgentProposalKind::Question,
            0.6,
            None,
            CardLifecycleEvent::Approved,
        );
        assert!(
            card_slot_leak.contains(&format!("card_id={ID_REDACTED}")),
            "{card_slot_leak}"
        );
        assert!(!card_slot_leak.contains("what time"), "{card_slot_leak}");
    }

    #[test]
    fn session_summary_line_contains_every_kind_class_field_and_total() {
        let mut counts = SessionCardCounts::default();
        counts.record(&AgentProposalKind::Question, 0.95); // question_high
        counts.record(&AgentProposalKind::Question, 0.6); // question_medium
        counts.record(&AgentProposalKind::Note, 0.1); // note_low
        let line = session_summary_line("sess-summary", &counts);
        assert!(line.contains("event=session.card_summary"), "{line}");
        assert!(line.contains("session_id=sess-summary"), "{line}");
        assert!(line.contains("question_high=1"), "{line}");
        assert!(line.contains("question_medium=1"), "{line}");
        assert!(line.contains("note_low=1"), "{line}");
        assert!(line.contains("note_high=0"), "{line}");
        assert!(line.contains("graph_suggestion_low=0"), "{line}");
        assert!(line.contains("total=3"), "{line}");
        // No card_id/kind/confidence_class fields on the summary line — it is
        // session-scoped, not card-scoped.
        assert!(!line.contains("card_id="), "{line}");
        assert!(!line.contains(" kind="), "{line}");
    }

    #[test]
    fn session_summary_line_redacts_a_malformed_session_id() {
        // Wiring pin for the summary path's own `format_line` call, mirroring
        // `card_event_line_redacts_prose_passed_through_either_id_slot` above.
        let counts = SessionCardCounts::default();
        let line = session_summary_line("not a real session id, just prose", &counts);
        assert!(
            line.contains(&format!("session_id={ID_REDACTED}")),
            "{line}"
        );
        assert!(!line.contains("just prose"), "{line}");
    }

    #[test]
    fn log_chat_invoked_line_contains_only_the_event_and_session_id() {
        let line = format_line("chat.invoked", "sess-chat", None, None, None, &[]);
        assert!(line.contains("event=chat.invoked"), "{line}");
        assert!(line.contains("session_id=sess-chat"), "{line}");
        assert!(!line.contains("card_id="), "{line}");
        assert!(!line.contains(" kind="), "{line}");
    }

    // ---------------------------------------------------------------------
    // audio-graph-83cc T3: answer-engine telemetry pins.
    // ---------------------------------------------------------------------

    #[test]
    fn answer_requested_completed_and_failed_lines_carry_auto_and_event_name() {
        let requested = answer_event_line(
            "sess-a",
            "card-a",
            &AgentProposalKind::Question,
            true,
            AnswerLifecycleEvent::Requested,
        );
        assert!(requested.contains("event=answer.requested"), "{requested}");
        assert!(requested.contains("kind=question"), "{requested}");
        assert!(requested.contains("auto=true"), "{requested}");
        assert!(!requested.contains("reason="), "{requested}");

        let completed = answer_event_line(
            "sess-a",
            "card-a",
            &AgentProposalKind::Question,
            false,
            AnswerLifecycleEvent::Completed,
        );
        assert!(completed.contains("event=answer.completed"), "{completed}");
        assert!(completed.contains("auto=false"), "{completed}");

        let failed = answer_event_line(
            "sess-a",
            "card-a",
            &AgentProposalKind::Question,
            true,
            AnswerLifecycleEvent::Failed,
        );
        assert!(failed.contains("event=answer.failed"), "{failed}");
    }

    #[test]
    fn answer_refused_line_carries_the_reason_class_and_never_a_confidence_class() {
        let line = answer_event_line(
            "sess-a",
            "card-a",
            &AgentProposalKind::Question,
            true,
            AnswerLifecycleEvent::Refused(AnswerRefusalReason::Duplicate),
        );
        assert!(line.contains("event=answer.refused"), "{line}");
        assert!(line.contains("reason=duplicate"), "{line}");
        // The answer telemetry line is not a card-lifecycle line: it never
        // carries a bucketed confidence class (only `log_card_event`'s lines
        // do), which this asserts by construction — `answer_event_line`
        // passes `None` for `confidence_class` to `format_line`.
        assert!(!line.contains("confidence_class="), "{line}");

        for (reason, expected) in [
            (AnswerRefusalReason::Disabled, "disabled"),
            (AnswerRefusalReason::NotQuestion, "not_question"),
            (AnswerRefusalReason::WeakSignal, "weak_signal"),
            (AnswerRefusalReason::Converse, "converse"),
            (AnswerRefusalReason::Duplicate, "duplicate"),
            (AnswerRefusalReason::Busy, "busy"),
            (AnswerRefusalReason::Interval, "interval"),
            (AnswerRefusalReason::Capped, "capped"),
        ] {
            let line = answer_event_line(
                "sess-a",
                "card-a",
                &AgentProposalKind::Question,
                true,
                AnswerLifecycleEvent::Refused(reason),
            );
            assert!(
                line.contains(&format!("reason={expected}")),
                "{reason:?} -> {line}"
            );
        }
    }

    #[test]
    fn answer_event_line_redacts_prose_passed_through_either_id_slot() {
        let line = answer_event_line(
            "please answer this: what is my social security number",
            "card-a",
            &AgentProposalKind::Question,
            true,
            AnswerLifecycleEvent::Requested,
        );
        assert!(
            line.contains(&format!("session_id={ID_REDACTED}")),
            "{line}"
        );
        assert!(!line.contains("social security"), "{line}");
    }

    // ---------------------------------------------------------------------
    // Content non-representability pins (deliverable d, part 2): the
    // `session_id`/`card_id` string arguments are the only strings this
    // module's signatures accept, and both are validated ids, not free-form
    // strings — a caller cannot smuggle content through them.
    // ---------------------------------------------------------------------

    #[test]
    fn sanitize_id_passes_through_well_shaped_ids_unchanged() {
        assert_eq!(sanitize_id("session-abc-123"), "session-abc-123");
        assert_eq!(
            sanitize_id("3fa85f64-5717-4562-b3fc-2c963f66afa6"),
            "3fa85f64-5717-4562-b3fc-2c963f66afa6"
        );
        assert_eq!(
            sanitize_id("rotated-after-proposal"),
            "rotated-after-proposal"
        );
    }

    #[test]
    fn sanitize_id_clamps_prose_to_the_redacted_sentinel() {
        // Free-form prose (contains spaces) must never survive verbatim —
        // this is the exact failure mode the ticket's SECURITY constraint
        // exists to prevent: a caller accidentally passing transcript/
        // question text into the id slot.
        assert_eq!(
            sanitize_id("patient said their social security number aloud"),
            ID_REDACTED
        );
        assert_eq!(
            sanitize_id("Consider answering or linking this question: what time is it?"),
            ID_REDACTED
        );
    }

    #[test]
    fn sanitize_id_clamps_empty_and_overlong_and_ill_shaped_strings() {
        assert_eq!(sanitize_id(""), ID_REDACTED);
        assert_eq!(sanitize_id(&"a".repeat(MAX_ID_LEN + 1)), ID_REDACTED);
        assert_eq!(sanitize_id(&"a".repeat(MAX_ID_LEN)), "a".repeat(MAX_ID_LEN));
        // Punctuation outside the allowed set (id shape excludes `/`, `:`,
        // quotes, etc.) is rejected even without whitespace.
        assert_eq!(sanitize_id("card/../etc/passwd"), ID_REDACTED);
        assert_eq!(sanitize_id("\"quoted\""), ID_REDACTED);
    }

    #[test]
    fn sanitize_id_rejects_a_credential_shaped_value() {
        // A credential-shaped value is ITSELF id-shaped (lowercase
        // alphanumerics + hyphens), so a bare shape check would let it
        // through. The secret-scrub step (shared with `crate::analytics`)
        // must turn it into `<redacted>` first, which then fails the shape
        // check — mirroring the regression this exact pattern already
        // guards against in `analytics::scrub_breadcrumb`.
        const SECRET: &str = "sk-test-supersecret-credential-12345";
        assert!(
            is_id_shaped(SECRET),
            "test premise: the credential-shaped value must itself be id-shaped"
        );
        assert_eq!(sanitize_id(SECRET), ID_REDACTED);
    }

    #[test]
    fn confidence_class_buckets_match_the_documented_thresholds() {
        assert_eq!(ConfidenceClass::from_confidence(1.0), ConfidenceClass::High);
        assert_eq!(ConfidenceClass::from_confidence(0.8), ConfidenceClass::High);
        assert_eq!(
            ConfidenceClass::from_confidence(0.79),
            ConfidenceClass::Medium
        );
        assert_eq!(
            ConfidenceClass::from_confidence(0.5),
            ConfidenceClass::Medium
        );
        assert_eq!(ConfidenceClass::from_confidence(0.49), ConfidenceClass::Low);
        assert_eq!(ConfidenceClass::from_confidence(0.0), ConfidenceClass::Low);
        // Non-finite must degrade to Low, never panic.
        assert_eq!(
            ConfidenceClass::from_confidence(f32::NAN),
            ConfidenceClass::Low
        );
        assert_eq!(
            ConfidenceClass::from_confidence(f32::INFINITY),
            ConfidenceClass::Low
        );
        assert_eq!(
            ConfidenceClass::from_confidence(f32::NEG_INFINITY),
            ConfidenceClass::Low
        );
    }

    #[test]
    fn session_card_counts_record_and_total_are_consistent() {
        let mut counts = SessionCardCounts::default();
        assert_eq!(counts.total(), 0);
        counts.record(&AgentProposalKind::Question, 0.9);
        counts.record(&AgentProposalKind::Note, 0.4);
        counts.record(&AgentProposalKind::GraphSuggestion, 0.55);
        assert_eq!(counts.question_high, 1);
        assert_eq!(counts.note_low, 1);
        assert_eq!(counts.graph_suggestion_medium, 1);
        assert_eq!(counts.total(), 3);
    }

    // ---------------------------------------------------------------------
    // Wiring pin: guard against a mutation that leaves `log_card_event` /
    // `log_session_summary` computing `format_line` but silently dropping
    // the `log::info!` call (this crate has no log-capture harness — see
    // `logging::tests` — so source-text inspection is the cheapest
    // mutation-proof available without adding that infrastructure; same
    // technique `commands.rs`'s
    // `log_abandoned_deferred_retries_after_stop_emits_the_documented_warn_key`
    // uses).
    // ---------------------------------------------------------------------

    #[test]
    fn public_log_functions_still_call_log_info() {
        let source = include_str!("card_telemetry.rs");
        let card_event_start = source
            .find("pub fn log_card_event(")
            .expect("log_card_event must exist");
        // Tight window: ends where the very next item (`log_chat_invoked`)
        // starts, not several items later — a wider window would let a
        // mutation that drops `log_card_event`'s own `log::info!` call hide
        // behind a DIFFERENT function's `log::info!` still being in range.
        let card_event_end = source[card_event_start..]
            .find("pub fn log_chat_invoked(")
            .map(|rel| card_event_start + rel)
            .expect("log_chat_invoked must follow log_card_event");
        assert!(
            source[card_event_start..card_event_end].contains("log::info!"),
            "log_card_event must call log::info!"
        );

        let chat_invoked_start = source
            .find("pub fn log_chat_invoked(")
            .expect("log_chat_invoked must exist");
        let chat_invoked_end = source[chat_invoked_start..]
            .find("pub struct SessionCardCounts")
            .map(|rel| chat_invoked_start + rel)
            .expect("SessionCardCounts must follow log_chat_invoked");
        assert!(
            source[chat_invoked_start..chat_invoked_end].contains("log::info!"),
            "log_chat_invoked must call log::info!"
        );

        let summary_start = source
            .find("pub fn log_session_summary(")
            .expect("log_session_summary must exist");
        let summary_end = source[summary_start..]
            .find("#[cfg(test)]")
            .map(|rel| summary_start + rel)
            .expect("the test module must follow log_session_summary");
        assert!(
            source[summary_start..summary_end].contains("log::info!"),
            "log_session_summary must call log::info!"
        );
    }

    // ---------------------------------------------------------------------
    // Call-site pin (fix-round hardening, minor finding): guards against an
    // accidental deletion of a lifecycle call site regressing this ticket's
    // whole reason for existing — audio-graph-81a5 was filed because NOTHING
    // logged near card creation/approval/dismissal. The format/wiring tests
    // above pin this module's own behavior; this test additionally pins that
    // the production call sites elsewhere in the crate still reach it. Cheap
    // source-text presence/count checks, not a substitute for the tests
    // above.
    // ---------------------------------------------------------------------

    #[test]
    fn production_call_sites_still_invoke_the_telemetry_seam() {
        let commands_src = include_str!("commands.rs");
        let creation_src = include_str!("speech/mod.rs");
        let lib_src = include_str!("lib.rs");

        let count = |src: &str, needle: &str| src.matches(needle).count();

        assert!(
            count(creation_src, "card_telemetry::log_card_event(") >= 1,
            "the card-creation site (speech/mod.rs::run_agent_proposal_task) must still call log_card_event"
        );
        assert!(
            count(commands_src, "card_telemetry::log_card_event(") >= 3,
            "approval, single dismissal, and bulk-clear dismissal must each still call log_card_event (found fewer than 3 call sites)"
        );
        assert!(
            count(commands_src, "card_telemetry::log_chat_invoked(") >= 2,
            "start_streaming_chat and send_chat_message must each still call log_chat_invoked"
        );
        assert!(
            count(commands_src, "card_telemetry::log_answer_event(") >= 3,
            "the answer engine's refused/requested/terminal sites must each still call log_answer_event (audio-graph-83cc T3)"
        );
        assert!(
            lib_src.contains("fn log_session_card_summary_best_effort(")
                && lib_src.contains("crate::card_telemetry::log_session_summary("),
            "the session-summary helper must still call log_session_summary"
        );
        assert!(
            lib_src.contains("log_session_card_summary_best_effort(&current_sid)"),
            "the RunEvent::Exit shutdown hook must still call the summary helper"
        );
        assert!(
            commands_src.contains("log_session_card_summary_best_effort(&previous_id)"),
            "new_session_cmd's rotation hook must still call the summary helper"
        );
    }

    #[test]
    fn creation_site_telemetry_runs_after_the_ui_emit_and_the_guard_drop() {
        // Fix-round finding (scope-honesty review, major): the original
        // creation-site telemetry read ran BEFORE the UI emit, inside the
        // pipeline-latency measurement window, and while still holding the
        // session-generation mutex. This pins the corrected ordering by
        // byte offset in the source text — cheap insurance against a future
        // edit silently reintroducing any of the three orderings.
        let source = include_str!("speech/mod.rs");
        let fn_start = source
            .find("fn run_agent_proposal_task(")
            .expect("run_agent_proposal_task must exist");
        let fn_end = source[fn_start..]
            .find("// Accumulated speech segment")
            .map(|rel| fn_start + rel)
            .expect("the next item must follow run_agent_proposal_task");
        let body = &source[fn_start..fn_end];

        let emit_pos = body
            .find("events::emit_or_log(&app_handle, events::AGENT_PROPOSAL, proposal)")
            .expect("the AGENT_PROPOSAL emit must exist in this function");
        let latency_pos = body
            .find("emit_stage_latency(")
            .expect("the stage-latency emit must exist in this function");
        let guard_drop_pos = body
            .find("drop(_generation_guard)")
            .expect("the generation guard must be explicitly dropped in this function");
        let telemetry_pos = body
            .find("crate::card_telemetry::log_card_event(")
            .expect("the creation-site telemetry call must exist in this function");

        assert!(
            emit_pos < telemetry_pos,
            "the UI emit must precede the telemetry call"
        );
        assert!(
            latency_pos < telemetry_pos,
            "the stage-latency measurement must precede the telemetry call"
        );
        assert!(
            guard_drop_pos < telemetry_pos,
            "the session-generation guard must be dropped before the telemetry call"
        );
    }
}
