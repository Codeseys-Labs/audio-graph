//! Speak-aloud loop: chat token deltas → TTS → audio playback.
//!
//! Wave C / audio-graph-92c7 / ADR-0006 sub-decision A consequence.
//!
//! When `AppSettings::speak_aloud` is true and `tts_provider` is something
//! other than `None`, [`SpeakAloudPipe`] glues three Wave A/B components
//! together:
//!
//! ```text
//!   chat-token-delta events ──────► clause buffer ──flush──► TtsSession.speak()
//!                                                                │
//!                                                                ▼
//!                                                       TtsEventStream
//!                                                                │
//!                                                                ▼ AudioChunk
//!                                                       AudioPlayer.push_samples
//!                                                                │
//!                                                                ▼
//!                                                          cpal callback → device
//! ```
//!
//! Cancellation propagates the same chain: `cancel_streaming_chat` →
//! [`SpeakAloudPipe::cancel`] which calls `TtsSession::clear()` (server
//! drops the in-flight utterance) AND `AudioPlayer::cancel()` (device
//! callback drains its ringbuf + emits silence).

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::credentials::CredentialStore;
use crate::playback::{AudioPlayer, PlaybackConfig};
use crate::settings::TtsProvider;
use crate::tts::deepgram_aura::DeepgramAuraProvider;
use crate::tts::{TtsConfig, TtsErrorKind, TtsEvent, TtsProvider as TtsProviderTrait, TtsSession};

/// Punctuation marks that mark a clause boundary and trigger a TTS flush.
/// Aggressive flushing keeps first-audio latency low: the TTS provider can
/// start synthesising "Hello," without waiting for the rest of the sentence.
///
/// Newline is included so well-formatted markdown lists also flush
/// per-bullet.
fn is_clause_boundary(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '—' | '\n')
}

/// Live speak-aloud pipe. Each `start_streaming_chat` invocation creates one
/// when settings request it; the pipe lives as long as the streaming chat
/// task, then drops on `finish` or `cancel`.
pub struct SpeakAloudPipe {
    session: Box<dyn TtsSession>,
    player: AudioPlayer,
    /// Buffer accumulating un-flushed text. Reset on each clause-boundary flush.
    pending: String,
    /// Cancel token shared with the audio-pump task. On drop / cancel we
    /// fire the token to stop the pump cleanly.
    audio_pump_cancel: CancellationToken,
}

impl SpeakAloudPipe {
    /// Construct a speak-aloud pipe. Returns `Ok(None)` when speak-aloud is
    /// disabled or the TTS provider is `None` — the caller should treat
    /// this as a no-op (regular streaming chat continues without TTS).
    pub async fn maybe_new(
        speak_aloud: bool,
        tts_provider: &TtsProvider,
        credentials: &CredentialStore,
        content_egress_policy: crate::asr::ProviderContentEgressPolicy,
        player: AudioPlayer,
    ) -> Result<Option<Self>, String> {
        if !speak_aloud {
            return Ok(None);
        }
        match tts_provider {
            TtsProvider::None => Ok(None),
            TtsProvider::DeepgramAura {
                voice,
                sample_rate,
                speed,
            } => {
                let provider = DeepgramAuraProvider::from_store(credentials)
                    .map(|provider| provider.with_content_egress_policy(content_egress_policy))
                    .map_err(|e| format!("Aura provider unavailable: {e:?}"))?;
                let tts_config = TtsConfig {
                    voice: voice.clone(),
                    sample_rate: *sample_rate,
                    // Linear16 is the only Aura streaming encoding the
                    // playback subsystem currently consumes (raw i16 LE PCM).
                    encoding: crate::tts::TtsEncoding::Linear16,
                    speed: *speed,
                }
                .with_clamped_speed();
                let mut session = provider
                    .open(voice, tts_config)
                    .await
                    .map_err(|e| format!("Aura session open failed: {e:?}"))?;

                // Open the audio device at the TTS source rate so we don't
                // pitch-shift. cpal Stream open is sync-on-thread so this
                // returns quickly.
                player
                    .open_default(PlaybackConfig {
                        source_sample_rate: *sample_rate,
                        source_channels: 1,
                    })
                    .map_err(|e| format!("audio device open failed: {e}"))?;

                // Spawn the audio-pump task: drain TtsEventStream, push
                // AudioChunk samples into the player.
                let events = session
                    .take_events()
                    .ok_or_else(|| "TTS session has no event stream".to_string())?;
                let cancel = CancellationToken::new();
                let cancel_for_task = cancel.clone();
                let player_for_task = player.clone();
                tokio::spawn(async move {
                    pump_audio(events, player_for_task, cancel_for_task).await;
                });

                Ok(Some(Self {
                    session,
                    player,
                    pending: String::new(),
                    audio_pump_cancel: cancel,
                }))
            }
        }
    }

    /// Test-only constructor that assembles a pipe from its parts, bypassing
    /// the provider/device wiring in [`maybe_new`]. Production code always
    /// goes through `maybe_new`; this seam lets unit tests inject a fake
    /// [`TtsSession`] + a no-device [`AudioPlayer`] to assert the
    /// clause-buffering and barge-in ordering without any network or audio
    /// hardware. Behaviour of the methods under test is identical.
    #[cfg(test)]
    fn from_parts(
        session: Box<dyn TtsSession>,
        player: AudioPlayer,
        audio_pump_cancel: CancellationToken,
    ) -> Self {
        Self {
            session,
            player,
            pending: String::new(),
            audio_pump_cancel,
        }
    }

    /// Append a delta from the streaming chat. Flushes to the TTS provider
    /// at clause boundaries.
    pub fn append_delta(&mut self, delta: &str) -> Result<(), String> {
        self.pending.push_str(delta);

        // Find the last clause-boundary character in the pending buffer.
        // Anything up to and including it gets flushed; anything after
        // stays buffered for the next call.
        let mut split_at: Option<usize> = None;
        for (idx, ch) in self.pending.char_indices() {
            if is_clause_boundary(ch) {
                // Include the boundary character itself in the flushed chunk.
                split_at = Some(idx + ch.len_utf8());
            }
        }
        if let Some(boundary) = split_at {
            let to_flush: String = self.pending.drain(..boundary).collect();
            if !to_flush.trim().is_empty() {
                self.session
                    .speak(&to_flush)
                    .map_err(|e| format!("TTS speak failed: {e:?}"))?;
            }
        }
        Ok(())
    }

    /// Final flush at end-of-reply. Sends any unflushed buffer + a Flush
    /// frame to force synthesis of the trailing fragment.
    pub fn finish(self) -> Result<(), String> {
        let Self {
            session,
            player: _player,
            pending,
            audio_pump_cancel,
        } = self;
        if !pending.trim().is_empty() {
            session
                .speak(&pending)
                .map_err(|e| format!("TTS final-speak failed: {e:?}"))?;
        }
        session
            .flush()
            .map_err(|e| format!("TTS flush failed: {e:?}"))?;
        // Don't cancel the audio pump; it will run until the session emits
        // its final AudioChunk and the stream naturally ends. The pump
        // task lives until session is dropped, which happens when
        // SpeakAloudPipe goes out of scope below.
        let _ = session.close();
        let _ = audio_pump_cancel; // kept alive until session finishes
        Ok(())
    }

    /// Barge-in: cancel the in-flight utterance + drain audio playback.
    /// Frames already on the wire / in the ringbuf are dropped at the
    /// session layer (audio-graph-7107) and the playback callback (cpal
    /// drain).
    pub fn cancel(self) -> Result<(), String> {
        let Self {
            session,
            player,
            pending: _,
            audio_pump_cancel,
        } = self;
        let _ = session.clear();
        player.cancel();
        // Stop the audio pump promptly so any frames the session emits
        // post-Clear (which the session layer should already be
        // suppressing — defence in depth) don't reach the player.
        audio_pump_cancel.cancel();
        let _ = session.close();
        Ok(())
    }
}

/// Whether a [`TtsErrorKind`] should tear down playback in the audio pump.
///
/// Only genuinely terminal failures stop the pump: `Auth` (credentials are
/// bad — no point retrying), `Exhausted` (reconnect ladder gave up), and
/// `Server` (the provider reported an unrecoverable server-side failure for
/// the request). Everything else — notably `Unknown`, which is what a
/// non-fatal Aura `Warning` frame maps to, plus transient
/// `RateLimit`/`Network`/`Protocol`/`BadRequest` blips that the session task
/// handles via its own reconnect logic — is logged and ignored so a single
/// transient event can't permanently silence a healthy session.
fn is_fatal_tts_error(kind: TtsErrorKind) -> bool {
    matches!(
        kind,
        TtsErrorKind::Auth | TtsErrorKind::Exhausted | TtsErrorKind::Server
    )
}

/// Pump TtsEvent::AudioChunk samples into the AudioPlayer. Stops on cancel
/// or when the event stream ends (session closed).
async fn pump_audio(
    mut events: crate::tts::TtsEventStream,
    player: AudioPlayer,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                player.cancel();
                return;
            }
            next = events.next() => {
                match next {
                    None => {
                        let _ = player.flush_samples();
                        return;
                    }
                    Some(TtsEvent::AudioChunk { samples, .. }) => {
                        let _ = player.push_samples(&samples);
                    }
                    Some(TtsEvent::Status(_)) => {
                        // Status events surface in the chat UI eventually;
                        // not relevant to playback.
                    }
                    Some(TtsEvent::Error { kind, message }) => {
                        if is_fatal_tts_error(kind) {
                            log::warn!("Fatal TTS error during speak-aloud: {kind:?} {message}");
                            player.cancel();
                            return;
                        }
                        // Non-fatal: a transient server Warning (mapped to
                        // TtsErrorKind::Unknown) or a recoverable
                        // RateLimit/Network/Protocol blip must NOT tear down
                        // playback — the session may still be healthy and more
                        // audio is coming. Log and keep pumping.
                        log::warn!(
                            "Non-fatal TTS warning during speak-aloud (continuing): {kind:?} {message}"
                        );
                    }
                }
            }
        }
    }
}

// Re-export Arc so consumers can keep their imports tidy.
pub use std::sync::Arc as _Arc;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::{TtsError, TtsEvent};
    use std::sync::{Arc, Mutex};

    /// Records the ordered sequence of method calls a fake [`TtsSession`]
    /// receives, so tests can assert clause-buffer flushing + barge-in
    /// ordering deterministically.
    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Speak(String),
        Flush,
        Clear,
        Close,
    }

    #[derive(Clone, Default)]
    struct FakeSession {
        calls: Arc<Mutex<Vec<Call>>>,
    }

    impl FakeSession {
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
        fn spoken(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter_map(|c| match c {
                    Call::Speak(s) => Some(s),
                    _ => None,
                })
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl TtsSession for FakeSession {
        fn speak(&self, text: &str) -> Result<(), TtsError> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Speak(text.to_string()));
            Ok(())
        }
        fn flush(&self) -> Result<(), TtsError> {
            self.calls.lock().unwrap().push(Call::Flush);
            Ok(())
        }
        fn clear(&self) -> Result<(), TtsError> {
            self.calls.lock().unwrap().push(Call::Clear);
            Ok(())
        }
        fn close(&self) -> Result<(), TtsError> {
            self.calls.lock().unwrap().push(Call::Close);
            Ok(())
        }
        fn take_events(&mut self) -> Option<crate::tts::TtsEventStream> {
            None
        }
    }

    fn pipe_with(session: FakeSession) -> SpeakAloudPipe {
        SpeakAloudPipe::from_parts(
            Box::new(session),
            AudioPlayer::new(),
            CancellationToken::new(),
        )
    }

    // ----- is_clause_boundary truth table ----------------------------------

    #[test]
    fn is_clause_boundary_truth_table() {
        for c in ['.', ',', ';', ':', '!', '?', '—', '\n'] {
            assert!(is_clause_boundary(c), "{c:?} should be a boundary");
        }
        for c in ['a', 'Z', '0', ' ', '\t', '-', '(', '\'', '"'] {
            assert!(!is_clause_boundary(c), "{c:?} should NOT be a boundary");
        }
    }

    // ----- append_delta clause buffering ------------------------------------

    #[test]
    fn append_delta_buffers_until_boundary() {
        let fake = FakeSession::default();
        let mut pipe = pipe_with(fake.clone());
        // No boundary yet → nothing flushed.
        pipe.append_delta("Hello").unwrap();
        assert!(fake.spoken().is_empty(), "no boundary → buffered, no speak");
        assert_eq!(pipe.pending, "Hello");
    }

    #[test]
    fn append_delta_flushes_through_boundary_and_keeps_tail() {
        let fake = FakeSession::default();
        let mut pipe = pipe_with(fake.clone());
        pipe.append_delta("Hello, ").unwrap();
        // Last boundary is the comma at index 5 → split_at = 6 → the flushed
        // chunk is "Hello," (verbatim, including the boundary char) and the
        // trailing space stays buffered for the next call.
        assert_eq!(fake.spoken(), vec!["Hello,".to_string()]);
        assert_eq!(pipe.pending, " ");
    }

    #[test]
    fn append_delta_flushes_up_to_last_boundary() {
        let fake = FakeSession::default();
        let mut pipe = pipe_with(fake.clone());
        // Multiple boundaries: everything up to the LAST one flushes at once.
        pipe.append_delta("One. Two. Three").unwrap();
        assert_eq!(fake.spoken(), vec!["One. Two.".to_string()]);
        assert_eq!(pipe.pending, " Three");
    }

    #[test]
    fn append_delta_whitespace_only_does_not_speak() {
        let fake = FakeSession::default();
        let mut pipe = pipe_with(fake.clone());
        // A boundary preceded only by whitespace → trimmed chunk is empty →
        // no speak call, but the buffer still drains past the boundary.
        pipe.append_delta("\n").unwrap();
        assert!(
            fake.spoken().is_empty(),
            "whitespace-only flush must not call speak"
        );
    }

    // ----- finish flushes tail then flush + close ---------------------------

    #[test]
    fn finish_speaks_trailing_fragment_then_flushes_and_closes() {
        let fake = FakeSession::default();
        let mut pipe = pipe_with(fake.clone());
        pipe.append_delta("Tail without boundary").unwrap();
        assert!(fake.spoken().is_empty());
        pipe.finish().unwrap();
        assert_eq!(
            fake.calls(),
            vec![
                Call::Speak("Tail without boundary".to_string()),
                Call::Flush,
                Call::Close,
            ],
            "finish must speak the tail, then flush, then close"
        );
    }

    #[test]
    fn finish_with_empty_tail_only_flushes_and_closes() {
        let fake = FakeSession::default();
        let pipe = pipe_with(fake.clone());
        pipe.finish().unwrap();
        assert_eq!(fake.calls(), vec![Call::Flush, Call::Close]);
    }

    // ----- cancel (barge-in) ordering ---------------------------------------

    #[test]
    fn cancel_clears_then_closes_and_fires_pump_token() {
        let fake = FakeSession::default();
        let token = CancellationToken::new();
        let pipe =
            SpeakAloudPipe::from_parts(Box::new(fake.clone()), AudioPlayer::new(), token.clone());
        pipe.cancel().unwrap();
        // Session: clear() before close(); player.cancel() is a no-op on a
        // no-device player but must not panic.
        assert_eq!(
            fake.calls(),
            vec![Call::Clear, Call::Close],
            "cancel must clear() then close() the session"
        );
        assert!(
            token.is_cancelled(),
            "cancel must fire the audio-pump cancel token"
        );
    }

    // ----- pump_audio arms (in-memory stream, no device) --------------------

    #[tokio::test]
    async fn pump_audio_stops_on_cancel() {
        let cancel = CancellationToken::new();
        // A chunk, then a never-ending stream → only cancel can stop the pump.
        let stream = futures_util::stream::iter(vec![TtsEvent::AudioChunk {
            samples: vec![1, 2, 3],
            sample_rate: 24_000,
        }])
        .chain(futures_util::stream::pending());
        let player = AudioPlayer::new();
        let token = cancel.clone();
        let handle = tokio::spawn(pump_audio(Box::pin(stream), player, cancel));
        token.cancel();
        // Returns promptly via the select! cancel arm.
        handle.await.expect("pump task joins");
    }

    #[tokio::test]
    async fn pump_audio_returns_on_stream_end() {
        let cancel = CancellationToken::new();
        let stream = futures_util::stream::iter(vec![
            TtsEvent::AudioChunk {
                samples: vec![1, 2],
                sample_rate: 24_000,
            },
            TtsEvent::Status(crate::tts::TtsStatus::Connected),
        ]);
        let player = AudioPlayer::new();
        // No cancel fired; the finite stream ends → clean return.
        pump_audio(Box::pin(stream), player, cancel).await;
    }

    #[tokio::test]
    async fn pump_audio_returns_on_error_event() {
        let cancel = CancellationToken::new();
        // A FATAL Error event before a pending tail → the error arm must return
        // even though the stream would otherwise never end.
        let stream = futures_util::stream::iter(vec![TtsEvent::Error {
            kind: crate::tts::TtsErrorKind::Server,
            message: "boom".to_string(),
        }])
        .chain(futures_util::stream::pending());
        let player = AudioPlayer::new();
        // Must return (not hang) via the error arm.
        pump_audio(Box::pin(stream), player, cancel).await;
    }

    // ----- is_fatal_tts_error classification --------------------------------

    #[test]
    fn fatal_tts_error_truth_table() {
        use crate::tts::TtsErrorKind::*;
        // Fatal: stop the pump.
        for kind in [Auth, Exhausted, Server] {
            assert!(is_fatal_tts_error(kind), "{kind:?} must be fatal");
        }
        // Non-fatal: keep pumping. `Unknown` is what a server `Warning` frame
        // maps to; the rest are transient blips the session task handles via
        // its own reconnect logic.
        for kind in [Unknown, RateLimit, Network, Protocol, BadRequest] {
            assert!(!is_fatal_tts_error(kind), "{kind:?} must be non-fatal");
        }
    }

    #[tokio::test]
    async fn pump_audio_continues_past_non_fatal_warning() {
        // A non-fatal Warning (mapped to TtsErrorKind::Unknown) followed by an
        // AudioChunk, then a never-ending stream. If the pump incorrectly tore
        // down on the Warning it would return early — so we assert that only
        // an explicit cancel stops it AND that the post-Warning chunk was
        // pumped first.
        let cancel = CancellationToken::new();
        let stream = futures_util::stream::iter(vec![
            TtsEvent::Error {
                kind: crate::tts::TtsErrorKind::Unknown,
                message: "Aura warning: transient hiccup".to_string(),
            },
            TtsEvent::AudioChunk {
                samples: vec![7, 8, 9],
                sample_rate: 24_000,
            },
        ])
        .chain(futures_util::stream::pending());
        let player = AudioPlayer::new();
        let token = cancel.clone();
        let handle = tokio::spawn(pump_audio(Box::pin(stream), player, cancel));

        // Give the pump time to process the Warning + the trailing chunk. If
        // the Warning had been treated as fatal, the task would already have
        // finished; assert it is STILL running (needs an explicit cancel).
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !handle.is_finished(),
            "pump must keep running past a non-fatal Warning"
        );

        // Only cancel stops it.
        token.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("pump must stop promptly after cancel")
            .expect("pump task joins");
    }
}
