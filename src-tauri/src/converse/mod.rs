//! Provider-agnostic converse turn-state machine (ADR-0018 §4 / research §4).
//!
//! This module is the orchestration core for native speech-to-speech
//! (Gemini Live, OpenAI Realtime voice) and the pipelined STT→LLM→TTS path. It
//! is intentionally **pure and engine-agnostic**: it performs no I/O, owns no
//! sockets, no audio devices, and no clock. It consumes normalized
//! [`TurnSignal`]s, advances a [`TurnState`] machine
//! (`Idle → Listening → Thinking → Speaking → Interrupted`), and emits
//! [`TurnAction`]s the caller executes against whichever engine/playback is
//! live. Each engine maps its raw events onto [`TurnSignal`] (see
//! [`gemini_event_to_signal`]) so the FSM never knows which provider drives it.
//!
//! # Why pure
//!
//! Purity is what makes the highest-value, hardest-to-otherwise-test logic —
//! the transition table and the barge-in **gating** (AEC warmup window +
//! minimum interruption duration) — fully unit-testable with **no hardware,
//! no network, and no models**. The caller threads in the wall-clock (as
//! milliseconds since the turn started speaking) and the measured speech
//! duration; the FSM decides, deterministically, whether a barge-in is real.
//!
//! # Echo / barge-in gating (research §3.2, ADR-0018)
//!
//! A naive VAD fires on the assistant's own re-captured TTS (the echo loop).
//! Two data-only gates suppress that without any AEC implementation here:
//!
//! * **AEC warmup window** — for the first [`InterruptionGate::aec_warmup_ms`]
//!   of `Speaking`, audio-activity interruptions are ignored so a
//!   reference-cancelling AEC has time to converge (LiveKit's trick).
//! * **Minimum interruption duration** — a barge-in is only honored once the
//!   user has spoken for at least
//!   [`InterruptionGate::min_interruption_duration_ms`] (don't cut on a cough).
//!
//! The *actual* AEC, the live socket, and audio playback live elsewhere and
//! are out of scope for this module (they are runtime-gated; see ADR-0018
//! "OUT OF SCOPE"). This module supplies the decisions they enforce.
//!
//! # OpenAI seam
//!
//! Only the Gemini event map is implemented (B18). The OpenAI Realtime voice
//! map is B-future; [`gemini_event_to_signal`] documents the seam and the
//! `TurnSignal` surface is already provider-neutral, so the OpenAI adapter is
//! a sibling function that returns the same enum.

use crate::gemini::GeminiEvent;

// ---------------------------------------------------------------------------
// States
// ---------------------------------------------------------------------------

/// The converse turn lifecycle (ADR-0018 §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TurnState {
    /// Session open, not in a turn; no audio flowing to the model.
    #[default]
    Idle,
    /// Capturing user audio, streaming to the provider; awaiting end-of-speech.
    Listening,
    /// End-of-user-turn detected; model generating, no audio out yet.
    Thinking,
    /// Assistant audio chunks arriving / being played. The first
    /// `aec_warmup_ms` is a sub-phase during which barge-in is suppressed.
    Speaking,
    /// Barge-in confirmed during `Speaking`; the cancel/flush action is being
    /// run. Transient — collapses to `Listening` once the engine confirms.
    Interrupted,
}

// ---------------------------------------------------------------------------
// Normalized signals (engine-agnostic input)
// ---------------------------------------------------------------------------

/// Coarse classification for a converse-layer error, mirrored from the
/// engine. Kept minimal and provider-neutral.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnErrorCategory {
    /// Transport / network failure (recoverable by reconnect).
    Network,
    /// Authentication / authorization failure.
    Auth,
    /// Server-side failure.
    Server,
    /// Anything not positively classified.
    Unknown,
}

/// The normalized event surface every engine maps its raw events onto
/// (ADR-0018 §4.3 / research §4.3). The FSM consumes only these.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnSignal {
    /// User began speaking. While `Speaking`, this is a *candidate* barge-in
    /// (subject to the [`InterruptionGate`]); otherwise it keeps us listening.
    UserSpeechStarted,
    /// User finished speaking (end-of-utterance / server VAD endpoint).
    UserSpeechEnded,
    /// A chunk of assistant audio (PCM16 LE @ 24 kHz). Drives `Thinking →
    /// Speaking` on the first chunk; feeds playback thereafter.
    AssistantAudio { pcm24: Vec<u8> },
    /// Streaming transcript of the assistant's spoken reply, routed to graph
    /// proposals. `final_` marks the closing fragment of the turn's transcript.
    AssistantTranscript { text: String, final_: bool },
    /// Model generation for the turn is complete (precedes `TurnComplete`).
    GenerationComplete,
    /// The turn is fully complete; once playback drains, return to `Listening`.
    TurnComplete,
    /// Server/engine signaled an interruption was applied (Gemini auto-fires;
    /// OpenAI client-driven). Forces `Speaking → Interrupted` regardless of
    /// the gate (the engine already decided).
    Interrupted,
    /// A non-fatal error from the engine.
    Error {
        category: TurnErrorCategory,
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Actions (engine-agnostic output)
// ---------------------------------------------------------------------------

/// Side-effect requests the FSM emits for the caller to execute. The FSM
/// itself performs none of these — it is pure.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnAction {
    /// Begin streaming captured user audio to the engine (entering
    /// `Listening`).
    StartCapture,
    /// Stop streaming captured user audio (leaving `Listening`).
    StopCapture,
    /// Signal end-of-user-turn to the engine (e.g. Gemini `audioStreamEnd` /
    /// OpenAI commit) so it starts generating.
    EndUserTurn,
    /// Enqueue an assistant audio chunk for playback.
    PlayAudio { pcm24: Vec<u8> },
    /// Route an assistant transcript fragment to the graph-proposal queue.
    EmitTranscript { text: String, final_: bool },
    /// Stop and flush local playback immediately (barge-in / interruption).
    StopPlayback,
    /// Run the per-engine cancel sequence (Gemini: local flush only, the
    /// server already canceled; OpenAI: `response.cancel` +
    /// `conversation.item.truncate{audio_end_ms}`; pipelined: clear TTS queue).
    /// The FSM is engine-agnostic; the caller dispatches the right sequence.
    CancelGeneration,
    /// Trip the per-turn cancellation token (ADR-0003): in-flight async work
    /// for this turn must abort at its next await boundary.
    CancelToken,
    /// A barge-in candidate was **suppressed** by the gate (warmup window or
    /// below minimum duration). Informational — lets the caller log / meter
    /// false-interrupt suppression without changing state.
    SuppressedBargeIn { reason: SuppressedReason },
    /// Surface an error to the caller (logging / UI), state unchanged unless a
    /// transition also occurred.
    ReportError {
        category: TurnErrorCategory,
        message: String,
    },
}

/// Why a barge-in candidate was suppressed (for metering / logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressedReason {
    /// Still inside the AEC warmup window after entering `Speaking`.
    AecWarmup,
    /// User speech shorter than the minimum interruption duration.
    BelowMinDuration,
}

// ---------------------------------------------------------------------------
// Interruption gate (data-only echo mitigation)
// ---------------------------------------------------------------------------

/// Data-only gating for barge-in, per ADR-0018 / research §3.2. Holds no
/// state and does no timing itself; the caller supplies elapsed times so the
/// decision is deterministic and unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptionGate {
    /// Whether user-audio-activity barge-in is enabled at all. When `false`,
    /// only an explicit engine [`TurnSignal::Interrupted`] can break a reply
    /// (the half-duplex fallback for the no-AEC case, ADR-0018).
    pub enabled: bool,
    /// AEC warmup window: for this many ms after entering `Speaking`, an
    /// audio-activity barge-in candidate is suppressed so the AEC adaptive
    /// filter can converge (LiveKit's trick).
    pub aec_warmup_ms: u64,
    /// Minimum sustained user-speech duration (ms) for a barge-in to count.
    /// Below this, the candidate is treated as a cough/backchannel.
    pub min_interruption_duration_ms: u64,
}

impl Default for InterruptionGate {
    fn default() -> Self {
        // Defaults follow the production patterns in research §3.2 (LiveKit
        // aec_warmup a few hundred ms; min_interruption_duration ~500 ms).
        Self {
            enabled: true,
            aec_warmup_ms: 300,
            min_interruption_duration_ms: 500,
        }
    }
}

impl InterruptionGate {
    /// Decide whether a user-audio-activity barge-in candidate during
    /// `Speaking` should be honored.
    ///
    /// * `ms_since_speaking_started` — wall-clock ms since the FSM entered
    ///   `Speaking` (the warmup reference).
    /// * `user_speech_ms` — measured duration of the candidate user speech so
    ///   far (from the caller's VAD / the engine's reported activity).
    ///
    /// Returns `Ok(())` if the barge-in is real, or `Err(reason)` naming why
    /// it was suppressed. When the gate is disabled, audio-activity barge-in
    /// never fires (`Err(AecWarmup)` is *not* returned — see [`Self::enabled`];
    /// the FSM checks `enabled` before consulting timing).
    pub fn evaluate(
        &self,
        ms_since_speaking_started: u64,
        user_speech_ms: u64,
    ) -> Result<(), SuppressedReason> {
        if ms_since_speaking_started < self.aec_warmup_ms {
            return Err(SuppressedReason::AecWarmup);
        }
        if user_speech_ms < self.min_interruption_duration_ms {
            return Err(SuppressedReason::BelowMinDuration);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Timing context for a signal
// ---------------------------------------------------------------------------

/// The timing the caller threads in alongside a [`TurnSignal`] so the FSM can
/// evaluate the [`InterruptionGate`] without owning a clock.
///
/// Both fields are only consulted for a [`TurnSignal::UserSpeechStarted`]
/// received while `Speaking`. For every other signal they are ignored, so
/// callers may pass [`SignalContext::default`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SignalContext {
    /// Wall-clock ms since the FSM entered `Speaking` (warmup reference).
    pub ms_since_speaking_started: u64,
    /// Measured ms of the candidate user speech so far (min-duration gate).
    pub user_speech_ms: u64,
}

// ---------------------------------------------------------------------------
// The state machine
// ---------------------------------------------------------------------------

/// The pure converse turn-state machine (ADR-0018 §4). Construct with
/// [`TurnMachine::new`], feed [`TurnSignal`]s via [`TurnMachine::on_signal`],
/// and execute the returned [`TurnAction`]s. Holds only the current state and
/// the gate config — no I/O, no clock.
#[derive(Debug, Clone)]
pub struct TurnMachine {
    state: TurnState,
    gate: InterruptionGate,
}

impl TurnMachine {
    /// Create a machine in [`TurnState::Idle`] with the given gate config.
    pub fn new(gate: InterruptionGate) -> Self {
        Self {
            state: TurnState::Idle,
            gate,
        }
    }

    /// Create a machine with [`InterruptionGate::default`].
    pub fn with_default_gate() -> Self {
        Self::new(InterruptionGate::default())
    }

    /// The current state.
    pub fn state(&self) -> TurnState {
        self.state
    }

    /// The active gate config.
    pub fn gate(&self) -> InterruptionGate {
        self.gate
    }

    /// Drive the FSM with a signal whose only timing-sensitive case is a
    /// barge-in candidate; passes a default [`SignalContext`]. Convenience for
    /// signals that are never gated.
    pub fn on_signal(&mut self, signal: TurnSignal) -> Vec<TurnAction> {
        self.on_signal_ctx(signal, SignalContext::default())
    }

    /// Drive the FSM with a signal and its timing context. Returns the ordered
    /// list of [`TurnAction`]s the caller must execute. The transition table
    /// is exactly ADR-0018 §4.2.
    pub fn on_signal_ctx(&mut self, signal: TurnSignal, ctx: SignalContext) -> Vec<TurnAction> {
        use TurnSignal as S;
        use TurnState as St;

        match (self.state, signal) {
            // ── Idle / Listening: user speech begins ────────────────────
            // Idle → Listening on first user speech (session ready & mic on).
            (St::Idle, S::UserSpeechStarted) => {
                self.state = St::Listening;
                vec![TurnAction::StartCapture]
            }
            // Already listening: more user speech is a no-op (keep capturing).
            (St::Listening, S::UserSpeechStarted) => vec![],

            // ── Listening → Thinking: end-of-user-turn ──────────────────
            (St::Listening, S::UserSpeechEnded) => {
                self.state = St::Thinking;
                vec![TurnAction::StopCapture, TurnAction::EndUserTurn]
            }

            // ── Thinking → Speaking: first assistant audio ──────────────
            (St::Thinking, S::AssistantAudio { pcm24 }) => {
                self.state = St::Speaking;
                vec![TurnAction::PlayAudio { pcm24 }]
            }
            // Subsequent audio while speaking: just play it.
            (St::Speaking, S::AssistantAudio { pcm24 }) => {
                vec![TurnAction::PlayAudio { pcm24 }]
            }

            // ── Assistant transcript (Thinking/Speaking) → graph queue ──
            (St::Thinking | St::Speaking, S::AssistantTranscript { text, final_ }) => {
                vec![TurnAction::EmitTranscript { text, final_ }]
            }

            // ── Generation bookkeeping (Speaking) ───────────────────────
            (St::Speaking, S::GenerationComplete) => vec![],

            // ── Speaking → Listening: turn done (playback assumed drained)
            (St::Speaking | St::Thinking, S::TurnComplete) => {
                self.state = St::Listening;
                vec![TurnAction::StartCapture]
            }

            // ── Speaking: barge-in candidate (gated) ────────────────────
            (St::Speaking, S::UserSpeechStarted) => {
                if !self.gate.enabled {
                    // Audio-activity barge-in disabled (no-AEC half-duplex
                    // fallback): only an explicit engine `Interrupted` breaks
                    // the reply. Suppress, stay Speaking.
                    return vec![TurnAction::SuppressedBargeIn {
                        reason: SuppressedReason::AecWarmup,
                    }];
                }
                match self
                    .gate
                    .evaluate(ctx.ms_since_speaking_started, ctx.user_speech_ms)
                {
                    Ok(()) => {
                        self.state = St::Interrupted;
                        vec![
                            TurnAction::StopPlayback,
                            TurnAction::CancelToken,
                            TurnAction::CancelGeneration,
                        ]
                    }
                    Err(reason) => vec![TurnAction::SuppressedBargeIn { reason }],
                }
            }

            // ── Speaking → Interrupted: engine-confirmed interruption ───
            // The engine already decided (Gemini auto-fires `interrupted`);
            // honor it unconditionally (bypasses the gate).
            (St::Speaking, S::Interrupted) => {
                self.state = St::Interrupted;
                vec![
                    TurnAction::StopPlayback,
                    TurnAction::CancelToken,
                    TurnAction::CancelGeneration,
                ]
            }

            // ── Interrupted → Listening: cancel/truncate complete ───────
            // The transient Interrupted state collapses to Listening once the
            // engine confirms the turn ended (TurnComplete) or generation
            // stopped (GenerationComplete), and we resume capture.
            (St::Interrupted, S::TurnComplete | S::GenerationComplete) => {
                self.state = St::Listening;
                vec![TurnAction::StartCapture]
            }
            // Late audio after a confirmed interruption is discarded, not
            // played (the user has moved on).
            (St::Interrupted, S::AssistantAudio { .. }) => vec![],
            // A duplicate `Interrupted` (server may repeat) is idempotent.
            (St::Interrupted, S::Interrupted) => vec![],

            // ── Errors (any state) ──────────────────────────────────────
            (_, S::Error { category, message }) => {
                vec![TurnAction::ReportError { category, message }]
            }

            // ── Catch-all: ignore signals that don't apply in this state ─
            // (e.g. AssistantAudio while Idle/Listening, UserSpeechEnded while
            // Thinking/Speaking, GenerationComplete while Idle/Listening). The
            // FSM is total: every (state, signal) pair has a defined outcome.
            _ => vec![],
        }
    }

    /// Force the machine back to [`TurnState::Idle`] (user stop / session
    /// close / fatal error). Emits the teardown actions. Per ADR-0018 §4.2
    /// `any → Idle`.
    ///
    /// Teardown is state-sensitive: stopping local capture/playback is not
    /// enough when a turn is in flight. From `Thinking`/`Speaking`/
    /// `Interrupted` the engine is actively generating, so reset must ALSO
    /// cancel the active turn ([`TurnAction::CancelToken`] +
    /// [`TurnAction::CancelGeneration`]) — otherwise the engine keeps running
    /// and late assistant output can continue after a user stop / session
    /// teardown. From `Listening` only user audio is flowing (no generation),
    /// so stopping capture/playback suffices.
    pub fn reset(&mut self) -> Vec<TurnAction> {
        let prior = self.state;
        self.state = TurnState::Idle;
        match prior {
            // Nothing in flight — already torn down.
            TurnState::Idle => vec![],
            // Only user audio is flowing; no generation to cancel.
            TurnState::Listening => vec![TurnAction::StopCapture, TurnAction::StopPlayback],
            // A turn is generating/streaming on the engine: stop local I/O AND
            // cancel the active turn so the engine side does not keep running.
            TurnState::Thinking | TurnState::Speaking | TurnState::Interrupted => vec![
                TurnAction::StopCapture,
                TurnAction::StopPlayback,
                TurnAction::CancelToken,
                TurnAction::CancelGeneration,
            ],
        }
    }
}

impl Default for TurnMachine {
    fn default() -> Self {
        Self::with_default_gate()
    }
}

// ---------------------------------------------------------------------------
// Engine → TurnSignal adapters
// ---------------------------------------------------------------------------

/// Map a raw [`GeminiEvent`] onto the engine-agnostic [`TurnSignal`] the FSM
/// consumes (research §4, ADR-0018 §4.3). Returns `None` for transport /
/// lifecycle events (`Connected`, `Disconnected`, `Reconnecting`,
/// `Reconnected`) that the FSM does not model — the caller handles those at
/// the connection layer.
///
/// Mapping (Gemini → normalized):
/// * `Transcription`            → not a turn signal here. User-speech *text*
///   feeds the graph directly; turn boundaries come from the server VAD via
///   the caller, not from transcription frames. Returns `None`.
/// * `AudioChunk`               → `AssistantAudio { pcm24 }`
/// * `OutputTranscription`      → `AssistantTranscript { text, final_: false }`
/// * `ModelResponse` (text)     → `AssistantTranscript { text, final_: false }`
///   (TEXT-mode reply text; in AUDIO mode the transcript arrives via
///   `OutputTranscription` instead).
/// * `GenerationComplete`       → `GenerationComplete`
/// * `TurnComplete`             → `TurnComplete`
/// * `Interrupted`              → `Interrupted`
/// * `Error { category, .. }`   → `Error { category, message }`
///
/// # OpenAI seam (B-future)
///
/// The OpenAI Realtime voice client will get a sibling
/// `openai_event_to_signal` returning the same [`TurnSignal`] type:
/// `response.output_audio.delta → AssistantAudio`,
/// `response.output_audio_transcript.delta → AssistantTranscript`,
/// `input_audio_buffer.speech_started → UserSpeechStarted` (the OpenAI
/// barge-in trigger — gated client-side), `input_audio_buffer.speech_stopped
/// → UserSpeechEnded`, `response.done → TurnComplete`. The FSM is unchanged.
pub fn gemini_event_to_signal(event: GeminiEvent) -> Option<TurnSignal> {
    use base64::Engine as _;
    match event {
        // Decode the base64 audio at the point of use (the event carries it as a
        // compact string to avoid JSON int-array bloat over IPC — see
        // GeminiEvent::AudioChunk). Drop a chunk that won't decode rather than
        // feeding garbage to playback.
        GeminiEvent::AudioChunk { data_base64, .. } => base64::engine::general_purpose::STANDARD
            .decode(&data_base64)
            .ok()
            .filter(|b| !b.is_empty())
            .map(|pcm24| TurnSignal::AssistantAudio { pcm24 }),
        GeminiEvent::OutputTranscription { text } => Some(TurnSignal::AssistantTranscript {
            text,
            final_: false,
        }),
        GeminiEvent::ModelResponse { text } => Some(TurnSignal::AssistantTranscript {
            text,
            final_: false,
        }),
        GeminiEvent::GenerationComplete => Some(TurnSignal::GenerationComplete),
        GeminiEvent::TurnComplete { .. } => Some(TurnSignal::TurnComplete),
        GeminiEvent::Interrupted => Some(TurnSignal::Interrupted),
        GeminiEvent::Error { category, message } => Some(TurnSignal::Error {
            category: gemini_error_category(category),
            message,
        }),
        // Input transcription is routed to the graph by the caller, not the
        // FSM; turn boundaries come from server VAD signals supplied directly.
        GeminiEvent::Transcription { .. } => None,
        // Connection lifecycle — handled at the transport layer, not the FSM.
        GeminiEvent::Connected
        | GeminiEvent::Disconnected
        | GeminiEvent::Reconnecting { .. }
        | GeminiEvent::Reconnected { .. } => None,
    }
}

/// Normalize a [`crate::gemini::GeminiErrorCategory`] into the FSM's coarser
/// [`TurnErrorCategory`].
fn gemini_error_category(cat: crate::gemini::GeminiErrorCategory) -> TurnErrorCategory {
    use crate::gemini::GeminiErrorCategory as G;
    match cat {
        G::Auth | G::AuthExpired => TurnErrorCategory::Auth,
        G::RateLimit { .. } | G::Server => TurnErrorCategory::Server,
        G::Network => TurnErrorCategory::Network,
        G::Unknown => TurnErrorCategory::Unknown,
    }
}

/// Map a raw [`crate::asr::openai_realtime::OpenAiRealtimeEvent`] onto the
/// engine-agnostic [`TurnSignal`] (the OpenAI sibling of
/// [`gemini_event_to_signal`]). Returns `None` for transport/lifecycle frames
/// the FSM does not model (the caller handles those at the connection layer).
///
/// # Scope (B15 STT client vs. B-future voice client)
///
/// The OpenAI Realtime client wired today is the **transcription** session
/// (`OpenAiRealtimeEvent::Transcript`) — it produces *user-speech text*, not
/// assistant audio. That maps to nothing in the turn FSM (turn boundaries come
/// from server VAD / the caller, and user-speech text feeds the graph directly,
/// exactly as `GeminiEvent::Transcription` does), so this returns `None` for
/// `Transcript` and only forwards `Error`.
///
/// When the full OpenAI Realtime **voice** (S2S) client lands, its richer event
/// surface maps like this (the FSM is unchanged): `response.output_audio.delta
/// → AssistantAudio`, `response.output_audio_transcript.delta →
/// AssistantTranscript`, `input_audio_buffer.speech_started → UserSpeechStarted`
/// (the client-gated barge-in trigger), `input_audio_buffer.speech_stopped →
/// UserSpeechEnded`, `response.done → TurnComplete`. Those variants do not exist
/// on the STT enum yet; this adapter is the seam that will grow them.
pub fn openai_event_to_signal(
    event: crate::asr::openai_realtime::OpenAiRealtimeEvent,
) -> Option<TurnSignal> {
    use crate::asr::openai_realtime::OpenAiRealtimeEvent as E;
    match event {
        // User-speech transcript → graph, not a turn signal (mirrors Gemini's
        // Transcription handling). Turn boundaries come from VAD/the caller.
        E::Transcript { .. } => None,
        E::Error { message } => Some(TurnSignal::Error {
            // The STT client does not classify errors; default to Server (a
            // recoverable engine-side failure) so the FSM surfaces it without
            // claiming an auth/network root cause it can't know here.
            category: TurnErrorCategory::Server,
            message,
        }),
        // Connection lifecycle — handled at the transport layer, not the FSM.
        E::Connected | E::Disconnected | E::Reconnecting { .. } | E::Reconnected => None,
    }
}

// ---------------------------------------------------------------------------
// Playback decode (TurnAction::PlayAudio binding)
// ---------------------------------------------------------------------------

/// Decode the PCM bytes carried by [`TurnAction::PlayAudio`] (and
/// [`TurnSignal::AssistantAudio`]) into the `i16` samples
/// [`crate::playback::AudioPlayer::push_samples`] expects.
///
/// Gemini Live (and OpenAI Realtime voice) deliver assistant audio as 24 kHz
/// mono **PCM16 little-endian** bytes; the player wants `&[i16]`. A trailing
/// odd byte (a truncated sample at a chunk boundary) is dropped rather than
/// misaligning the whole stream — `chunks_exact(2)` ignores the remainder. Pure
/// and allocation-bounded; the runtime driver calls this then `push_samples`.
pub fn pcm16_le_bytes_to_i16(pcm: &[u8]) -> Vec<i16> {
    pcm.chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect()
}

// ---------------------------------------------------------------------------
// Runtime driver (production wiring for the pure FSM — B18 / ADR-0018)
// ---------------------------------------------------------------------------

/// The side-effect surface a [`ConverseDriver`] dispatches [`TurnAction`]s onto.
///
/// Splitting the effects behind a trait keeps the driver itself
/// engine/hardware-free and **unit-testable** (a recording mock sink proves the
/// dispatch order with no socket and no audio device), exactly mirroring why the
/// FSM is pure. The production implementation (in `commands.rs`) wraps the live
/// `GeminiLiveClient` + `AudioPlayer` + capture gate + cancellation token.
pub trait ConverseSink {
    /// Begin streaming captured user audio to the engine.
    fn start_capture(&mut self);
    /// Stop streaming captured user audio.
    fn stop_capture(&mut self);
    /// Signal end-of-user-turn to the engine so it starts generating
    /// (Gemini `audioStreamEnd` without closing the socket).
    fn end_user_turn(&mut self);
    /// Enqueue an assistant audio chunk (PCM16 LE @ 24 kHz bytes) for playback.
    fn play_audio(&mut self, pcm24: &[u8]);
    /// Stop and flush local playback immediately (barge-in / interruption).
    fn stop_playback(&mut self);
    /// Run the per-engine cancel sequence for the active turn.
    fn cancel_generation(&mut self);
    /// Trip the per-turn cancellation token (ADR-0003).
    fn cancel_token(&mut self);
    /// Route an assistant transcript fragment to the graph-proposal queue.
    fn emit_transcript(&mut self, text: &str, final_: bool);
    /// A barge-in candidate was suppressed by the gate (informational).
    fn suppressed_barge_in(&mut self, reason: SuppressedReason);
    /// Surface a (non-fatal) engine error to the caller.
    fn report_error(&mut self, category: TurnErrorCategory, message: &str);
}

/// Drives the pure [`TurnMachine`] from live engine signals, supplying the
/// clock the FSM deliberately lacks and dispatching each [`TurnAction`] to a
/// [`ConverseSink`]. This is the production "remainder" of ADR-0018 (B18):
/// the FSM decides, the driver executes.
///
/// # Clock
///
/// The driver records `now_ms` when the FSM enters [`TurnState::Speaking`] and
/// derives `ms_since_speaking_started` from it (the AEC-warmup reference). The
/// other gate input, `user_speech_ms`, is the caller's VAD measurement, threaded
/// in per signal — the Gemini server-VAD path has no client VAD and passes 0,
/// relying on the engine's explicit [`TurnSignal::Interrupted`] for barge-in
/// (which bypasses the gate). A client-VAD full-duplex path would pass a real
/// duration here to enable audio-activity barge-in.
#[derive(Debug)]
pub struct ConverseDriver {
    machine: TurnMachine,
    /// `now_ms` at which the FSM most recently entered `Speaking`; `None` when
    /// not speaking. Source of `SignalContext::ms_since_speaking_started`.
    speaking_started_ms: Option<u64>,
}

impl ConverseDriver {
    /// Create a driver wrapping a fresh [`TurnMachine`] with the given gate.
    pub fn new(gate: InterruptionGate) -> Self {
        Self {
            machine: TurnMachine::new(gate),
            speaking_started_ms: None,
        }
    }

    /// The current FSM state (for diagnostics / status).
    pub fn state(&self) -> TurnState {
        self.machine.state()
    }

    /// Feed one normalized signal with the current monotonic clock (`now_ms`,
    /// e.g. ms since session start) and the caller's measured `user_speech_ms`
    /// (0 when there is no client VAD). Builds the [`SignalContext`], advances
    /// the FSM, maintains the speaking clock, and dispatches the resulting
    /// actions to `sink` in order.
    pub fn on_signal(
        &mut self,
        signal: TurnSignal,
        now_ms: u64,
        user_speech_ms: u64,
        sink: &mut impl ConverseSink,
    ) {
        let ctx = SignalContext {
            ms_since_speaking_started: self
                .speaking_started_ms
                .map(|s| now_ms.saturating_sub(s))
                .unwrap_or(0),
            user_speech_ms,
        };
        let actions = self.machine.on_signal_ctx(signal, ctx);

        // Maintain the speaking clock from the resulting state. Entering
        // Speaking starts the warmup reference; returning to Idle/Listening/
        // Thinking clears it (Interrupted keeps it — warmup already elapsed).
        match self.machine.state() {
            TurnState::Speaking if self.speaking_started_ms.is_none() => {
                self.speaking_started_ms = Some(now_ms);
            }
            TurnState::Idle | TurnState::Listening | TurnState::Thinking => {
                self.speaking_started_ms = None;
            }
            _ => {}
        }

        for action in actions {
            dispatch_action(action, sink);
        }
    }

    /// Feed a raw [`GeminiEvent`]: maps it via [`gemini_event_to_signal`] (a
    /// transport/lifecycle event maps to `None` and is a no-op here — the caller
    /// handles those at the connection layer) and drives the FSM. Convenience
    /// for the Gemini path.
    pub fn on_gemini_event(
        &mut self,
        event: GeminiEvent,
        now_ms: u64,
        user_speech_ms: u64,
        sink: &mut impl ConverseSink,
    ) {
        if let Some(signal) = gemini_event_to_signal(event) {
            self.on_signal(signal, now_ms, user_speech_ms, sink);
        }
    }

    /// Prime the FSM into [`TurnState::Listening`] when a converse session goes
    /// live, bridging Gemini's **server-side VAD** to the FSM's explicit-turn
    /// model. Gemini does not emit `UserSpeechStarted`/`UserSpeechEnded` (the
    /// server decides turn boundaries), so without priming the FSM would sit in
    /// `Idle` and the first `AssistantAudio` would be dropped by the catch-all.
    /// From `Idle` this synthesizes `UserSpeechStarted` to enter `Listening`
    /// (dispatching `StartCapture`); from any other state it is a no-op.
    ///
    /// On the Gemini path the engine drives turn transitions thereafter:
    /// assistant audio → `Speaking`, server `interrupted` → `Interrupted`,
    /// `turnComplete` → back to `Listening`. A client-VAD path would instead
    /// feed real `UserSpeech*` signals and not call this.
    pub fn begin_listening(&mut self, now_ms: u64, sink: &mut impl ConverseSink) {
        if self.machine.state() == TurnState::Idle {
            self.on_signal(TurnSignal::UserSpeechStarted, now_ms, 0, sink);
        }
    }

    /// Force the FSM back to [`TurnState::Idle`] (user stop / session close),
    /// dispatching the teardown actions. Clears the speaking clock.
    pub fn reset(&mut self, sink: &mut impl ConverseSink) {
        let actions = self.machine.reset();
        self.speaking_started_ms = None;
        for action in actions {
            dispatch_action(action, sink);
        }
    }
}

/// Dispatch a single [`TurnAction`] onto a [`ConverseSink`]. Kept separate so
/// both [`ConverseDriver::on_signal`] and [`ConverseDriver::reset`] route
/// through one exhaustive match (a new `TurnAction` variant fails to compile
/// until it is handled here).
fn dispatch_action(action: TurnAction, sink: &mut impl ConverseSink) {
    match action {
        TurnAction::StartCapture => sink.start_capture(),
        TurnAction::StopCapture => sink.stop_capture(),
        TurnAction::EndUserTurn => sink.end_user_turn(),
        TurnAction::PlayAudio { pcm24 } => sink.play_audio(&pcm24),
        TurnAction::EmitTranscript { text, final_ } => sink.emit_transcript(&text, final_),
        TurnAction::StopPlayback => sink.stop_playback(),
        TurnAction::CancelGeneration => sink.cancel_generation(),
        TurnAction::CancelToken => sink.cancel_token(),
        TurnAction::SuppressedBargeIn { reason } => sink.suppressed_barge_in(reason),
        TurnAction::ReportError { category, message } => sink.report_error(category, &message),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- helpers ------------------------------------------------------------

    /// Drive a happy-path turn up to and including `Speaking` and return the
    /// machine. Gate is the default.
    fn machine_speaking() -> TurnMachine {
        let mut m = TurnMachine::with_default_gate();
        assert_eq!(
            m.on_signal(TurnSignal::UserSpeechStarted),
            vec![TurnAction::StartCapture]
        );
        assert_eq!(m.state(), TurnState::Listening);
        m.on_signal(TurnSignal::UserSpeechEnded);
        assert_eq!(m.state(), TurnState::Thinking);
        m.on_signal(TurnSignal::AssistantAudio { pcm24: vec![1, 2] });
        assert_eq!(m.state(), TurnState::Speaking);
        m
    }

    // -- core lifecycle -----------------------------------------------------

    #[test]
    fn starts_idle() {
        let m = TurnMachine::with_default_gate();
        assert_eq!(m.state(), TurnState::Idle);
        assert_eq!(TurnState::default(), TurnState::Idle);
        assert_eq!(TurnMachine::default().state(), TurnState::Idle);
    }

    #[test]
    fn idle_to_listening_on_user_speech() {
        let mut m = TurnMachine::with_default_gate();
        let actions = m.on_signal(TurnSignal::UserSpeechStarted);
        assert_eq!(m.state(), TurnState::Listening);
        assert_eq!(actions, vec![TurnAction::StartCapture]);
    }

    #[test]
    fn listening_to_thinking_on_end_of_speech() {
        let mut m = TurnMachine::with_default_gate();
        m.on_signal(TurnSignal::UserSpeechStarted);
        let actions = m.on_signal(TurnSignal::UserSpeechEnded);
        assert_eq!(m.state(), TurnState::Thinking);
        assert_eq!(
            actions,
            vec![TurnAction::StopCapture, TurnAction::EndUserTurn]
        );
    }

    #[test]
    fn thinking_to_speaking_on_first_audio() {
        let mut m = TurnMachine::with_default_gate();
        m.on_signal(TurnSignal::UserSpeechStarted);
        m.on_signal(TurnSignal::UserSpeechEnded);
        let actions = m.on_signal(TurnSignal::AssistantAudio { pcm24: vec![9, 9] });
        assert_eq!(m.state(), TurnState::Speaking);
        assert_eq!(actions, vec![TurnAction::PlayAudio { pcm24: vec![9, 9] }]);
    }

    #[test]
    fn speaking_plays_subsequent_audio_without_state_change() {
        let mut m = machine_speaking();
        let actions = m.on_signal(TurnSignal::AssistantAudio { pcm24: vec![3, 4] });
        assert_eq!(m.state(), TurnState::Speaking);
        assert_eq!(actions, vec![TurnAction::PlayAudio { pcm24: vec![3, 4] }]);
    }

    #[test]
    fn speaking_to_listening_on_turn_complete() {
        let mut m = machine_speaking();
        let actions = m.on_signal(TurnSignal::TurnComplete);
        assert_eq!(m.state(), TurnState::Listening);
        assert_eq!(actions, vec![TurnAction::StartCapture]);
    }

    #[test]
    fn generation_complete_is_bookkeeping_only_while_speaking() {
        let mut m = machine_speaking();
        let actions = m.on_signal(TurnSignal::GenerationComplete);
        assert_eq!(
            m.state(),
            TurnState::Speaking,
            "generationComplete must not leave Speaking"
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn transcript_routes_to_graph_in_thinking_and_speaking() {
        let mut m = TurnMachine::with_default_gate();
        m.on_signal(TurnSignal::UserSpeechStarted);
        m.on_signal(TurnSignal::UserSpeechEnded); // Thinking
        let a = m.on_signal(TurnSignal::AssistantTranscript {
            text: "hello".into(),
            final_: false,
        });
        assert_eq!(
            a,
            vec![TurnAction::EmitTranscript {
                text: "hello".into(),
                final_: false
            }]
        );
        assert_eq!(m.state(), TurnState::Thinking);

        m.on_signal(TurnSignal::AssistantAudio { pcm24: vec![1] }); // Speaking
        let a = m.on_signal(TurnSignal::AssistantTranscript {
            text: " world".into(),
            final_: true,
        });
        assert_eq!(
            a,
            vec![TurnAction::EmitTranscript {
                text: " world".into(),
                final_: true
            }]
        );
        assert_eq!(m.state(), TurnState::Speaking);
    }

    // -- barge-in gating (the high-value coverage) --------------------------

    #[test]
    fn barge_in_honored_past_warmup_and_min_duration() {
        let mut m = machine_speaking(); // default gate: warmup 300, min 500
        let ctx = SignalContext {
            ms_since_speaking_started: 1_000,
            user_speech_ms: 600,
        };
        let actions = m.on_signal_ctx(TurnSignal::UserSpeechStarted, ctx);
        assert_eq!(m.state(), TurnState::Interrupted);
        assert_eq!(
            actions,
            vec![
                TurnAction::StopPlayback,
                TurnAction::CancelToken,
                TurnAction::CancelGeneration,
            ]
        );
    }

    #[test]
    fn barge_in_suppressed_during_aec_warmup() {
        let mut m = machine_speaking();
        // 100 ms < 300 ms warmup → suppressed even with long speech.
        let ctx = SignalContext {
            ms_since_speaking_started: 100,
            user_speech_ms: 9_999,
        };
        let actions = m.on_signal_ctx(TurnSignal::UserSpeechStarted, ctx);
        assert_eq!(
            m.state(),
            TurnState::Speaking,
            "warmup must not allow barge-in"
        );
        assert_eq!(
            actions,
            vec![TurnAction::SuppressedBargeIn {
                reason: SuppressedReason::AecWarmup
            }]
        );
    }

    #[test]
    fn barge_in_suppressed_below_min_duration() {
        let mut m = machine_speaking();
        // Past warmup, but only a 100 ms cough (< 500 ms) → suppressed.
        let ctx = SignalContext {
            ms_since_speaking_started: 2_000,
            user_speech_ms: 100,
        };
        let actions = m.on_signal_ctx(TurnSignal::UserSpeechStarted, ctx);
        assert_eq!(m.state(), TurnState::Speaking);
        assert_eq!(
            actions,
            vec![TurnAction::SuppressedBargeIn {
                reason: SuppressedReason::BelowMinDuration
            }]
        );
    }

    #[test]
    fn barge_in_boundary_exact_warmup_and_min_duration() {
        // Exactly at the thresholds: warmup uses `<` (so == passes), min uses
        // `<` (so == passes). Both boundaries should HONOR the barge-in.
        let mut m = machine_speaking();
        let ctx = SignalContext {
            ms_since_speaking_started: 300, // == aec_warmup_ms
            user_speech_ms: 500,            // == min_interruption_duration_ms
        };
        let actions = m.on_signal_ctx(TurnSignal::UserSpeechStarted, ctx);
        assert_eq!(m.state(), TurnState::Interrupted);
        assert_eq!(actions.first(), Some(&TurnAction::StopPlayback));
    }

    #[test]
    fn disabled_gate_suppresses_audio_activity_barge_in() {
        let gate = InterruptionGate {
            enabled: false,
            ..InterruptionGate::default()
        };
        let mut m = TurnMachine::new(gate);
        m.on_signal(TurnSignal::UserSpeechStarted);
        m.on_signal(TurnSignal::UserSpeechEnded);
        m.on_signal(TurnSignal::AssistantAudio { pcm24: vec![1] });
        assert_eq!(m.state(), TurnState::Speaking);
        // Even a long, late speech is suppressed because the gate is disabled.
        let ctx = SignalContext {
            ms_since_speaking_started: 10_000,
            user_speech_ms: 10_000,
        };
        let actions = m.on_signal_ctx(TurnSignal::UserSpeechStarted, ctx);
        assert_eq!(m.state(), TurnState::Speaking);
        assert!(matches!(
            actions.as_slice(),
            [TurnAction::SuppressedBargeIn { .. }]
        ));
    }

    #[test]
    fn engine_interrupted_bypasses_gate() {
        // An explicit engine Interrupted (Gemini server auto-fire) must break
        // the reply even inside the warmup window — the engine already decided.
        let mut m = machine_speaking();
        let actions = m.on_signal(TurnSignal::Interrupted);
        assert_eq!(m.state(), TurnState::Interrupted);
        assert_eq!(
            actions,
            vec![
                TurnAction::StopPlayback,
                TurnAction::CancelToken,
                TurnAction::CancelGeneration,
            ]
        );
    }

    #[test]
    fn interrupted_collapses_to_listening_on_turn_complete() {
        let mut m = machine_speaking();
        m.on_signal(TurnSignal::Interrupted);
        assert_eq!(m.state(), TurnState::Interrupted);
        let actions = m.on_signal(TurnSignal::TurnComplete);
        assert_eq!(m.state(), TurnState::Listening);
        assert_eq!(actions, vec![TurnAction::StartCapture]);
    }

    #[test]
    fn interrupted_collapses_to_listening_on_generation_complete() {
        let mut m = machine_speaking();
        m.on_signal(TurnSignal::Interrupted);
        let actions = m.on_signal(TurnSignal::GenerationComplete);
        assert_eq!(m.state(), TurnState::Listening);
        assert_eq!(actions, vec![TurnAction::StartCapture]);
    }

    #[test]
    fn interrupted_discards_late_audio() {
        let mut m = machine_speaking();
        m.on_signal(TurnSignal::Interrupted);
        let actions = m.on_signal(TurnSignal::AssistantAudio { pcm24: vec![7, 7] });
        assert_eq!(m.state(), TurnState::Interrupted);
        assert!(
            actions.is_empty(),
            "late audio after interruption is dropped"
        );
    }

    #[test]
    fn duplicate_interrupted_is_idempotent() {
        let mut m = machine_speaking();
        m.on_signal(TurnSignal::Interrupted);
        let actions = m.on_signal(TurnSignal::Interrupted);
        assert_eq!(m.state(), TurnState::Interrupted);
        assert!(actions.is_empty());
    }

    // -- gate unit (pure decision) ------------------------------------------

    #[test]
    fn gate_evaluate_matrix() {
        let gate = InterruptionGate {
            enabled: true,
            aec_warmup_ms: 300,
            min_interruption_duration_ms: 500,
        };
        assert_eq!(gate.evaluate(299, 600), Err(SuppressedReason::AecWarmup));
        assert_eq!(gate.evaluate(300, 600), Ok(()));
        assert_eq!(
            gate.evaluate(1000, 499),
            Err(SuppressedReason::BelowMinDuration)
        );
        assert_eq!(gate.evaluate(1000, 500), Ok(()));
        // Warmup is checked first: short-speech-in-warmup reports AecWarmup.
        assert_eq!(gate.evaluate(0, 0), Err(SuppressedReason::AecWarmup));
    }

    #[test]
    fn gate_default_values() {
        let g = InterruptionGate::default();
        assert!(g.enabled);
        assert_eq!(g.aec_warmup_ms, 300);
        assert_eq!(g.min_interruption_duration_ms, 500);
    }

    // -- error + reset + catch-all ------------------------------------------

    #[test]
    fn error_reports_without_changing_state() {
        let mut m = machine_speaking();
        let actions = m.on_signal(TurnSignal::Error {
            category: TurnErrorCategory::Network,
            message: "socket reset".into(),
        });
        assert_eq!(
            m.state(),
            TurnState::Speaking,
            "non-fatal error keeps state"
        );
        assert_eq!(
            actions,
            vec![TurnAction::ReportError {
                category: TurnErrorCategory::Network,
                message: "socket reset".into()
            }]
        );
    }

    #[test]
    fn reset_from_speaking_cancels_active_turn() {
        // From Speaking the engine is actively generating/streaming, so reset
        // must stop local I/O AND cancel the active turn (CancelToken +
        // CancelGeneration) — stopping capture/playback alone leaves the engine
        // running and late assistant output can continue after a user stop.
        let mut m = machine_speaking();
        let actions = m.reset();
        assert_eq!(m.state(), TurnState::Idle);
        assert_eq!(
            actions,
            vec![
                TurnAction::StopCapture,
                TurnAction::StopPlayback,
                TurnAction::CancelToken,
                TurnAction::CancelGeneration,
            ]
        );
    }

    #[test]
    fn reset_from_thinking_and_interrupted_cancels_active_turn() {
        // Thinking: the engine is generating but no audio is out yet.
        let mut m = TurnMachine::with_default_gate();
        m.on_signal(TurnSignal::UserSpeechStarted);
        m.on_signal(TurnSignal::UserSpeechEnded);
        assert_eq!(m.state(), TurnState::Thinking);
        assert_eq!(
            m.reset(),
            vec![
                TurnAction::StopCapture,
                TurnAction::StopPlayback,
                TurnAction::CancelToken,
                TurnAction::CancelGeneration,
            ]
        );
        assert_eq!(m.state(), TurnState::Idle);

        // Interrupted: a cancel was already in flight; resetting must still
        // emit the cancel actions (idempotent on the engine side) so teardown
        // is unconditional.
        let mut m = machine_speaking();
        m.on_signal(TurnSignal::Interrupted);
        assert_eq!(m.state(), TurnState::Interrupted);
        assert_eq!(
            m.reset(),
            vec![
                TurnAction::StopCapture,
                TurnAction::StopPlayback,
                TurnAction::CancelToken,
                TurnAction::CancelGeneration,
            ]
        );
        assert_eq!(m.state(), TurnState::Idle);
    }

    #[test]
    fn reset_from_listening_stops_local_io_only() {
        // While Listening only user audio is flowing — there is no generation
        // to cancel, so reset stops capture/playback but emits no cancel.
        let mut m = TurnMachine::with_default_gate();
        m.on_signal(TurnSignal::UserSpeechStarted);
        assert_eq!(m.state(), TurnState::Listening);
        let actions = m.reset();
        assert_eq!(m.state(), TurnState::Idle);
        assert_eq!(
            actions,
            vec![TurnAction::StopCapture, TurnAction::StopPlayback]
        );
        assert!(
            !actions.contains(&TurnAction::CancelGeneration),
            "no generation in flight while Listening"
        );
    }

    #[test]
    fn reset_from_idle_is_noop() {
        let mut m = TurnMachine::with_default_gate();
        let actions = m.reset();
        assert_eq!(m.state(), TurnState::Idle);
        assert!(actions.is_empty());
    }

    #[test]
    fn out_of_band_signals_are_ignored() {
        // The FSM is total: nonsensical (state, signal) pairs are no-ops and
        // never panic or change state.
        let mut m = TurnMachine::with_default_gate(); // Idle
        assert!(
            m.on_signal(TurnSignal::AssistantAudio { pcm24: vec![1] })
                .is_empty()
        );
        assert!(m.on_signal(TurnSignal::TurnComplete).is_empty());
        assert!(m.on_signal(TurnSignal::GenerationComplete).is_empty());
        assert!(m.on_signal(TurnSignal::Interrupted).is_empty());
        assert_eq!(m.state(), TurnState::Idle);

        m.on_signal(TurnSignal::UserSpeechStarted); // Listening
        // UserSpeechStarted while already Listening is a no-op.
        assert!(m.on_signal(TurnSignal::UserSpeechStarted).is_empty());
        assert_eq!(m.state(), TurnState::Listening);
    }

    #[test]
    fn full_turn_with_barge_in_round_trip() {
        // Idle → Listening → Thinking → Speaking → (barge-in) → Interrupted →
        // Listening, then a clean second turn through to TurnComplete.
        let mut m = TurnMachine::with_default_gate();
        m.on_signal(TurnSignal::UserSpeechStarted);
        m.on_signal(TurnSignal::UserSpeechEnded);
        m.on_signal(TurnSignal::AssistantAudio { pcm24: vec![1] });
        assert_eq!(m.state(), TurnState::Speaking);
        m.on_signal_ctx(
            TurnSignal::UserSpeechStarted,
            SignalContext {
                ms_since_speaking_started: 800,
                user_speech_ms: 700,
            },
        );
        assert_eq!(m.state(), TurnState::Interrupted);
        m.on_signal(TurnSignal::TurnComplete);
        assert_eq!(m.state(), TurnState::Listening);
        // Second turn.
        m.on_signal(TurnSignal::UserSpeechEnded);
        assert_eq!(m.state(), TurnState::Thinking);
        m.on_signal(TurnSignal::AssistantAudio { pcm24: vec![2] });
        m.on_signal(TurnSignal::TurnComplete);
        assert_eq!(m.state(), TurnState::Listening);
    }

    // -- Gemini → TurnSignal adapter ----------------------------------------

    #[test]
    fn gemini_audio_chunk_maps_to_assistant_audio() {
        let sig = gemini_event_to_signal(GeminiEvent::AudioChunk {
            data_base64: "AQID".into(), // base64 of [1,2,3]
            sample_rate: 24_000,
        });
        assert_eq!(
            sig,
            Some(TurnSignal::AssistantAudio {
                pcm24: vec![1, 2, 3]
            })
        );
    }

    #[test]
    fn gemini_invalid_base64_audio_maps_to_none() {
        // Decode happens here (point of use); a chunk whose base64 won't decode
        // is dropped (-> None) rather than feeding garbage to playback. This is
        // the downstream half of the "dropped, not panicked" guarantee that used
        // to live in handle_server_message before decode was deferred.
        let sig = gemini_event_to_signal(GeminiEvent::AudioChunk {
            data_base64: "!!!notb64!!!".into(),
            sample_rate: 24_000,
        });
        assert_eq!(sig, None);
        // An empty (but valid) payload also yields nothing to play.
        let empty = gemini_event_to_signal(GeminiEvent::AudioChunk {
            data_base64: String::new(),
            sample_rate: 24_000,
        });
        assert_eq!(empty, None);
    }

    #[test]
    fn gemini_output_transcription_maps_to_assistant_transcript() {
        let sig = gemini_event_to_signal(GeminiEvent::OutputTranscription {
            text: "spoken".into(),
        });
        assert_eq!(
            sig,
            Some(TurnSignal::AssistantTranscript {
                text: "spoken".into(),
                final_: false
            })
        );
    }

    #[test]
    fn gemini_interrupted_maps() {
        assert_eq!(
            gemini_event_to_signal(GeminiEvent::Interrupted),
            Some(TurnSignal::Interrupted)
        );
    }

    #[test]
    fn gemini_generation_and_turn_complete_map() {
        assert_eq!(
            gemini_event_to_signal(GeminiEvent::GenerationComplete),
            Some(TurnSignal::GenerationComplete)
        );
        assert_eq!(
            gemini_event_to_signal(GeminiEvent::TurnComplete { usage: None }),
            Some(TurnSignal::TurnComplete)
        );
    }

    #[test]
    fn gemini_error_category_normalized() {
        let sig = gemini_event_to_signal(GeminiEvent::Error {
            category: crate::gemini::GeminiErrorCategory::AuthExpired,
            message: "token expired".into(),
        });
        assert_eq!(
            sig,
            Some(TurnSignal::Error {
                category: TurnErrorCategory::Auth,
                message: "token expired".into()
            })
        );
        // RateLimit folds into Server (recoverable backpressure category).
        let sig = gemini_event_to_signal(GeminiEvent::Error {
            category: crate::gemini::GeminiErrorCategory::RateLimit {
                retry_after_secs: Some(5),
            },
            message: "429".into(),
        });
        assert!(matches!(
            sig,
            Some(TurnSignal::Error {
                category: TurnErrorCategory::Server,
                ..
            })
        ));
    }

    #[test]
    fn gemini_transcription_and_lifecycle_events_are_not_turn_signals() {
        assert_eq!(
            gemini_event_to_signal(GeminiEvent::Transcription {
                text: "hi".into(),
                is_final: true
            }),
            None
        );
        assert_eq!(gemini_event_to_signal(GeminiEvent::Connected), None);
        assert_eq!(gemini_event_to_signal(GeminiEvent::Disconnected), None);
        assert_eq!(
            gemini_event_to_signal(GeminiEvent::Reconnecting {
                attempt: 1,
                backoff_secs: 1
            }),
            None
        );
        assert_eq!(
            gemini_event_to_signal(GeminiEvent::Reconnected { resumed: true }),
            None
        );
    }

    #[test]
    fn gemini_model_response_text_maps_to_transcript() {
        // TEXT-mode reply text still routes to the graph-proposal queue.
        let sig = gemini_event_to_signal(GeminiEvent::ModelResponse {
            text: "reply".into(),
        });
        assert_eq!(
            sig,
            Some(TurnSignal::AssistantTranscript {
                text: "reply".into(),
                final_: false
            })
        );
    }

    /// End-to-end: feed a sequence of raw Gemini events through the adapter
    /// into the FSM and assert the resulting state trajectory. Proves the
    /// adapter + FSM compose as the engine-agnostic driver ADR-0018 specifies.
    #[test]
    fn gemini_event_stream_drives_fsm() {
        let mut m = TurnMachine::with_default_gate();
        // User-speech boundaries come from the caller (server VAD), not from
        // Gemini frames, so we inject them directly.
        m.on_signal(TurnSignal::UserSpeechStarted);
        m.on_signal(TurnSignal::UserSpeechEnded);
        assert_eq!(m.state(), TurnState::Thinking);

        // First audio chunk from Gemini → Speaking.
        if let Some(sig) = gemini_event_to_signal(GeminiEvent::AudioChunk {
            data_base64: "AAE=".into(), // base64 of [0,1]
            sample_rate: 24_000,
        }) {
            m.on_signal(sig);
        }
        assert_eq!(m.state(), TurnState::Speaking);

        // Output transcript → graph queue (state unchanged).
        if let Some(sig) = gemini_event_to_signal(GeminiEvent::OutputTranscription {
            text: "the answer".into(),
        }) {
            let actions = m.on_signal(sig);
            assert_eq!(
                actions,
                vec![TurnAction::EmitTranscript {
                    text: "the answer".into(),
                    final_: false
                }]
            );
        }
        assert_eq!(m.state(), TurnState::Speaking);

        // generationComplete then turnComplete → back to Listening.
        if let Some(sig) = gemini_event_to_signal(GeminiEvent::GenerationComplete) {
            m.on_signal(sig);
        }
        assert_eq!(m.state(), TurnState::Speaking);
        if let Some(sig) = gemini_event_to_signal(GeminiEvent::TurnComplete { usage: None }) {
            m.on_signal(sig);
        }
        assert_eq!(m.state(), TurnState::Listening);
    }

    // -- ConverseDriver (runtime wiring) ------------------------------------

    /// Records dispatched effects as a flat string log so tests assert the
    /// exact dispatch order with no socket / audio device.
    #[derive(Default)]
    struct RecordingSink {
        log: Vec<String>,
    }
    impl ConverseSink for RecordingSink {
        fn start_capture(&mut self) {
            self.log.push("start_capture".into());
        }
        fn stop_capture(&mut self) {
            self.log.push("stop_capture".into());
        }
        fn end_user_turn(&mut self) {
            self.log.push("end_user_turn".into());
        }
        fn play_audio(&mut self, pcm24: &[u8]) {
            self.log.push(format!("play_audio({})", pcm24.len()));
        }
        fn stop_playback(&mut self) {
            self.log.push("stop_playback".into());
        }
        fn cancel_generation(&mut self) {
            self.log.push("cancel_generation".into());
        }
        fn cancel_token(&mut self) {
            self.log.push("cancel_token".into());
        }
        fn emit_transcript(&mut self, text: &str, final_: bool) {
            self.log.push(format!("emit_transcript({text},{final_})"));
        }
        fn suppressed_barge_in(&mut self, reason: SuppressedReason) {
            self.log.push(format!("suppressed({reason:?})"));
        }
        fn report_error(&mut self, category: TurnErrorCategory, message: &str) {
            self.log.push(format!("error({category:?},{message})"));
        }
    }

    #[test]
    fn driver_runs_a_full_gemini_turn_in_order() {
        let mut d = ConverseDriver::new(InterruptionGate::default());
        let mut s = RecordingSink::default();
        // A Gemini turn: user speaks (server VAD) → ends → audio reply → done.
        // Gemini server-VAD doesn't emit UserSpeechStarted, so we drive the
        // FSM's listening entry with the normalized signal directly, then the
        // real Gemini events for the assistant side.
        d.on_signal(TurnSignal::UserSpeechStarted, 0, 0, &mut s);
        d.on_signal(TurnSignal::UserSpeechEnded, 100, 0, &mut s);
        // First assistant audio chunk → Thinking→Speaking, starts the clock.
        d.on_gemini_event(
            GeminiEvent::AudioChunk {
                data_base64: base64_pcm(&[1, 2]),
                sample_rate: 24_000,
            },
            200,
            0,
            &mut s,
        );
        assert_eq!(d.state(), TurnState::Speaking);
        d.on_gemini_event(GeminiEvent::TurnComplete { usage: None }, 900, 0, &mut s);
        assert_eq!(d.state(), TurnState::Listening);
        assert_eq!(
            s.log,
            vec![
                "start_capture",
                "stop_capture",
                "end_user_turn",
                "play_audio(4)", // two i16 samples = 4 bytes
                "start_capture",
            ]
        );
    }

    #[test]
    fn driver_engine_interrupt_cancels_and_resumes() {
        let mut d = ConverseDriver::new(InterruptionGate::default());
        let mut s = RecordingSink::default();
        d.on_signal(TurnSignal::UserSpeechStarted, 0, 0, &mut s);
        d.on_signal(TurnSignal::UserSpeechEnded, 50, 0, &mut s);
        d.on_gemini_event(
            GeminiEvent::AudioChunk {
                data_base64: base64_pcm(&[7]),
                sample_rate: 24_000,
            },
            100,
            0,
            &mut s,
        );
        s.log.clear();
        // Engine-confirmed barge-in bypasses the gate.
        d.on_gemini_event(GeminiEvent::Interrupted, 150, 0, &mut s);
        assert_eq!(d.state(), TurnState::Interrupted);
        assert_eq!(
            s.log,
            vec!["stop_playback", "cancel_token", "cancel_generation"]
        );
        // Generation-complete after the cancel → resume listening.
        s.log.clear();
        d.on_gemini_event(GeminiEvent::GenerationComplete, 160, 0, &mut s);
        assert_eq!(d.state(), TurnState::Listening);
        assert_eq!(s.log, vec!["start_capture"]);
    }

    #[test]
    fn driver_populates_speaking_clock_for_gate() {
        // The driver must supply ms_since_speaking_started so the warmup gate
        // works. Default gate: 300ms warmup, 500ms min duration.
        let mut d = ConverseDriver::new(InterruptionGate::default());
        let mut s = RecordingSink::default();
        d.on_signal(TurnSignal::UserSpeechStarted, 0, 0, &mut s);
        d.on_signal(TurnSignal::UserSpeechEnded, 10, 0, &mut s);
        d.on_gemini_event(
            GeminiEvent::AudioChunk {
                data_base64: base64_pcm(&[1]),
                sample_rate: 24_000,
            },
            1000, // Speaking starts at t=1000
            0,
            &mut s,
        );
        s.log.clear();
        // Barge-in at t=1100 (only 100ms into Speaking) → suppressed (warmup).
        d.on_signal(TurnSignal::UserSpeechStarted, 1100, 600, &mut s);
        assert_eq!(d.state(), TurnState::Speaking);
        assert_eq!(s.log, vec!["suppressed(AecWarmup)"]);
        // Barge-in at t=1400 (400ms in, past warmup) with 600ms speech → honored.
        s.log.clear();
        d.on_signal(TurnSignal::UserSpeechStarted, 1400, 600, &mut s);
        assert_eq!(d.state(), TurnState::Interrupted);
        assert_eq!(
            s.log,
            vec!["stop_playback", "cancel_token", "cancel_generation"]
        );
    }

    #[test]
    fn driver_begin_listening_primes_from_idle_then_is_noop() {
        let mut d = ConverseDriver::new(InterruptionGate::default());
        let mut s = RecordingSink::default();
        // From Idle: synthesize the listening entry (Gemini server-VAD bridge).
        d.begin_listening(0, &mut s);
        assert_eq!(d.state(), TurnState::Listening);
        assert_eq!(s.log, vec!["start_capture"]);
        // Calling again while not Idle is a no-op (no duplicate StartCapture).
        s.log.clear();
        d.begin_listening(10, &mut s);
        assert_eq!(d.state(), TurnState::Listening);
        assert!(s.log.is_empty());
    }

    #[test]
    fn driver_reset_from_speaking_tears_down_and_cancels() {
        let mut d = ConverseDriver::new(InterruptionGate::default());
        let mut s = RecordingSink::default();
        d.on_signal(TurnSignal::UserSpeechStarted, 0, 0, &mut s);
        d.on_signal(TurnSignal::UserSpeechEnded, 10, 0, &mut s);
        d.on_gemini_event(
            GeminiEvent::AudioChunk {
                data_base64: base64_pcm(&[1]),
                sample_rate: 24_000,
            },
            20,
            0,
            &mut s,
        );
        s.log.clear();
        d.reset(&mut s);
        assert_eq!(d.state(), TurnState::Idle);
        assert_eq!(
            s.log,
            vec![
                "stop_capture",
                "stop_playback",
                "cancel_token",
                "cancel_generation"
            ]
        );
    }

    // -- OpenAI adapter seam ------------------------------------------------

    #[test]
    fn openai_transcript_is_not_a_turn_signal() {
        use crate::asr::openai_realtime::OpenAiRealtimeEvent as E;
        // User-speech transcript feeds the graph, not the FSM (mirrors Gemini).
        assert_eq!(
            openai_event_to_signal(E::Transcript {
                text: "hello".into(),
                item_id: "i1".into(),
                is_final: true,
            }),
            None
        );
    }

    #[test]
    fn openai_error_maps_to_turn_error_and_lifecycle_is_none() {
        use crate::asr::openai_realtime::OpenAiRealtimeEvent as E;
        assert_eq!(
            openai_event_to_signal(E::Error {
                message: "boom".into()
            }),
            Some(TurnSignal::Error {
                category: TurnErrorCategory::Server,
                message: "boom".into()
            })
        );
        // Transport/lifecycle frames are handled at the connection layer.
        assert_eq!(openai_event_to_signal(E::Connected), None);
        assert_eq!(openai_event_to_signal(E::Disconnected), None);
        assert_eq!(
            openai_event_to_signal(E::Reconnecting {
                attempt: 1,
                backoff_secs: 2
            }),
            None
        );
        assert_eq!(openai_event_to_signal(E::Reconnected), None);
    }

    /// Helper: base64-encode raw PCM16 bytes the way GeminiEvent::AudioChunk
    /// carries them.
    fn base64_pcm(samples: &[i16]) -> String {
        use base64::Engine as _;
        let mut bytes = Vec::new();
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    }

    // -- PlayAudio decode ---------------------------------------------------

    #[test]
    fn pcm16_le_decode_roundtrips_samples() {
        // 0x0100 LE = 1, 0xFFFF LE = -1, 0x0080 ... build known little-endian bytes.
        let samples: [i16; 4] = [0, 1, -1, i16::MAX];
        let mut bytes = Vec::new();
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        assert_eq!(pcm16_le_bytes_to_i16(&bytes), samples);
    }

    #[test]
    fn pcm16_le_decode_drops_trailing_odd_byte() {
        // A truncated sample at a chunk boundary must not misalign the stream:
        // chunks_exact(2) ignores the dangling byte.
        let bytes = [0x34, 0x12, 0x77]; // one full sample (0x1234) + 1 stray byte
        assert_eq!(pcm16_le_bytes_to_i16(&bytes), vec![0x1234_i16]);
    }

    #[test]
    fn pcm16_le_decode_empty_is_empty() {
        assert!(pcm16_le_bytes_to_i16(&[]).is_empty());
    }
}
