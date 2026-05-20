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

use std::sync::Arc;

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::credentials::CredentialStore;
use crate::playback::{AudioPlayer, PlaybackConfig};
use crate::settings::TtsProvider;
use crate::tts::deepgram_aura::DeepgramAuraProvider;
use crate::tts::{TtsConfig, TtsEvent, TtsProvider as TtsProviderTrait, TtsSession};

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
                    None => return, // stream ended
                    Some(TtsEvent::AudioChunk { samples, .. }) => {
                        let _ = player.push_samples(&samples);
                    }
                    Some(TtsEvent::Status(_)) => {
                        // Status events surface in the chat UI eventually;
                        // not relevant to playback.
                    }
                    Some(TtsEvent::Error { kind, message }) => {
                        log::warn!("TTS error during speak-aloud: {kind:?} {message}");
                        player.cancel();
                        return;
                    }
                }
            }
        }
    }
}

// Re-export Arc so consumers can keep their imports tidy.
pub use std::sync::Arc as _Arc;
