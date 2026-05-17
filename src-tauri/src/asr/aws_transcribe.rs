//! AWS Transcribe Streaming ASR integration.
//!
//! Uses the aws-sdk-transcribestreaming crate to stream audio to AWS
//! and receive real-time transcription results with optional speaker diarization.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use aws_sdk_transcribestreaming as transcribe;
use aws_sdk_transcribestreaming::primitives::Blob;
use aws_sdk_transcribestreaming::types::{Alternative, AudioEvent, AudioStream, MediaEncoding};
use crossbeam_channel::Receiver;
use uuid::Uuid;

use crate::audio::pipeline::ProcessedAudioChunk;
use crate::aws_util::build_aws_sdk_config;
use crate::settings::AwsCredentialSource;
use crate::state::TranscriptSegment;

pub struct AwsTranscribeConfig {
    pub region: String,
    pub language_code: String,
    pub credential_source: AwsCredentialSource,
    pub enable_diarization: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AwsTranscribePartial {
    pub source_id: String,
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub confidence: f32,
}

const AWS_TRANSCRIBE_SOURCE_FALLBACK: &str = "aws-transcribe-stream";

fn f32_to_pcm_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let i16_val = (clamped * 32767.0) as i16;
        bytes.extend_from_slice(&i16_val.to_le_bytes());
    }
    bytes
}

fn source_hint_or_fallback(source_id_hint: &Arc<RwLock<Option<String>>>) -> String {
    source_id_hint
        .read()
        .ok()
        .and_then(|hint| hint.clone())
        .unwrap_or_else(|| AWS_TRANSCRIBE_SOURCE_FALLBACK.to_string())
}

fn transcript_text(alt: &Alternative) -> Option<String> {
    let text = alt.transcript()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn speaker_label(alt: &Alternative) -> Option<String> {
    alt.items()
        .iter()
        .find_map(|item| item.speaker().map(str::to_string))
}

fn alternative_confidence(alt: &Alternative) -> Option<f32> {
    let mut sum = 0.0f32;
    let mut count = 0usize;

    for confidence in alt.items().iter().filter_map(|item| item.confidence()) {
        sum += confidence as f32;
        count += 1;
    }

    (count > 0).then_some(sum / count as f32)
}

fn partial_from_result(
    result: &transcribe::types::Result,
    source_id: String,
) -> Option<AwsTranscribePartial> {
    if !result.is_partial() {
        return None;
    }

    result.alternatives().iter().find_map(|alt| {
        transcript_text(alt).map(|text| AwsTranscribePartial {
            source_id: source_id.clone(),
            text,
            start_time: result.start_time(),
            end_time: result.end_time(),
            confidence: alternative_confidence(alt).unwrap_or(0.0),
        })
    })
}

fn final_segments_from_result(
    result: &transcribe::types::Result,
    source_id: &str,
) -> Vec<TranscriptSegment> {
    if result.is_partial() {
        return Vec::new();
    }

    let result_start = result.start_time();
    let result_end = result.end_time();

    result
        .alternatives()
        .iter()
        .filter_map(|alt| {
            let text = transcript_text(alt)?;
            let speaker_label = speaker_label(alt);
            let confidence = alternative_confidence(alt).unwrap_or(0.9);

            Some(TranscriptSegment {
                id: Uuid::new_v4().to_string(),
                source_id: source_id.to_string(),
                speaker_id: speaker_label.clone(),
                speaker_label,
                text,
                start_time: result_start,
                end_time: result_end,
                confidence,
            })
        })
        .collect()
}

/// Run an AWS Transcribe streaming session. Blocking — meant for a dedicated thread.
///
/// Reads ProcessedAudioChunks from the receiver, streams them to AWS Transcribe,
/// and returns TranscriptSegments via the provided callback.
pub fn run_aws_transcribe_session(
    audio_rx: Receiver<ProcessedAudioChunk>,
    is_transcribing: Arc<AtomicBool>,
    config: AwsTranscribeConfig,
    on_transcript: impl FnMut(TranscriptSegment) + Send + 'static,
    on_partial: impl FnMut(AwsTranscribePartial) + Send + 'static,
) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Failed to create tokio runtime: {}", e))?;

    rt.block_on(async {
        run_streaming_session(audio_rx, is_transcribing, config, on_transcript, on_partial).await
    })
}

async fn run_streaming_session(
    audio_rx: Receiver<ProcessedAudioChunk>,
    is_transcribing: Arc<AtomicBool>,
    config: AwsTranscribeConfig,
    mut on_transcript: impl FnMut(TranscriptSegment) + Send + 'static,
    mut on_partial: impl FnMut(AwsTranscribePartial) + Send + 'static,
) -> Result<(), String> {
    let sdk_config = build_aws_sdk_config(&config.region, config.credential_source.clone()).await?;
    let client = transcribe::Client::new(&sdk_config);

    let (audio_tx, audio_stream_rx) = tokio::sync::mpsc::channel::<
        Result<AudioStream, transcribe::types::error::AudioStreamError>,
    >(16);

    let audio_stream: aws_smithy_http::event_stream::EventStreamSender<
        AudioStream,
        transcribe::types::error::AudioStreamError,
    > = aws_smithy_http::event_stream::EventStreamSender::from(
        tokio_stream::wrappers::ReceiverStream::new(audio_stream_rx),
    );

    let language_code = config
        .language_code
        .parse::<transcribe::types::LanguageCode>()
        .unwrap_or(transcribe::types::LanguageCode::EnUs);

    let mut builder = client
        .start_stream_transcription()
        .language_code(language_code)
        .media_sample_rate_hertz(16000)
        .media_encoding(MediaEncoding::Pcm)
        .audio_stream(audio_stream);

    if config.enable_diarization {
        builder = builder.show_speaker_label(true);
    }

    let mut output = builder
        .send()
        .await
        .map_err(|e| format!("Failed to start AWS Transcribe stream: {}", e))?;

    log::info!("AWS Transcribe: streaming session started");

    let source_id_hint = Arc::new(RwLock::new(None::<String>));
    let is_transcribing_sender = is_transcribing.clone();
    let source_id_hint_sender = Arc::clone(&source_id_hint);
    tokio::spawn(async move {
        loop {
            if !is_transcribing_sender.load(Ordering::Relaxed) {
                break;
            }

            match audio_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(chunk) => {
                    if let Ok(mut hint) = source_id_hint_sender.write() {
                        *hint = Some(chunk.source_id.clone());
                    }

                    let pcm_bytes = f32_to_pcm_bytes(&chunk.data);
                    let audio_event = AudioEvent::builder()
                        .audio_chunk(Blob::new(pcm_bytes))
                        .build();
                    if audio_tx
                        .send(Ok(AudioStream::AudioEvent(audio_event)))
                        .await
                        .is_err()
                    {
                        log::info!("AWS Transcribe: audio channel closed");
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    log::info!("AWS Transcribe: audio source disconnected");
                    break;
                }
            }
        }
        drop(audio_tx);
    });

    while let Some(event) = output
        .transcript_result_stream
        .recv()
        .await
        .map_err(|e| format!("AWS Transcribe stream error: {}", e))?
    {
        if !is_transcribing.load(Ordering::Relaxed) {
            break;
        }

        if let transcribe::types::TranscriptResultStream::TranscriptEvent(ev) = event {
            if let Some(transcript) = ev.transcript {
                for result in transcript.results.unwrap_or_default() {
                    let source_id = source_hint_or_fallback(&source_id_hint);

                    if result.is_partial() {
                        if let Some(partial) = partial_from_result(&result, source_id) {
                            on_partial(partial);
                        }
                        continue;
                    }

                    for segment in final_segments_from_result(&result, &source_id) {
                        on_transcript(segment);
                    }
                }
            }
        }
    }

    log::info!("AWS Transcribe: streaming session ended");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_transcribestreaming::types::Item;

    fn alt(text: &str, confidence: f64, speaker: Option<&str>) -> Alternative {
        let mut item = Item::builder().confidence(confidence);
        if let Some(speaker) = speaker {
            item = item.speaker(speaker);
        }

        Alternative::builder()
            .transcript(text)
            .items(item.build())
            .build()
    }

    #[test]
    fn source_hint_uses_fallback_until_audio_source_arrives() {
        let hint = Arc::new(RwLock::new(None::<String>));
        assert_eq!(
            source_hint_or_fallback(&hint),
            AWS_TRANSCRIBE_SOURCE_FALLBACK
        );

        *hint.write().unwrap() = Some("process:123".to_string());
        assert_eq!(source_hint_or_fallback(&hint), "process:123");
    }

    #[test]
    fn partial_result_is_normalized_with_source_and_timing() {
        let result = transcribe::types::Result::builder()
            .is_partial(true)
            .start_time(1.25)
            .end_time(2.5)
            .alternatives(alt(" hello aws ", 0.75, None))
            .build();

        let partial = partial_from_result(&result, "mic".to_string()).unwrap();

        assert_eq!(partial.source_id, "mic");
        assert_eq!(partial.text, "hello aws");
        assert_eq!(partial.start_time, 1.25);
        assert_eq!(partial.end_time, 2.5);
        assert!((partial.confidence - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn final_result_preserves_source_and_speaker_label() {
        let result = transcribe::types::Result::builder()
            .is_partial(false)
            .start_time(3.0)
            .end_time(4.0)
            .alternatives(alt(" final text ", 0.9, Some("spk_0")))
            .build();

        let segments = final_segments_from_result(&result, "system");

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].source_id, "system");
        assert_eq!(segments[0].speaker_id.as_deref(), Some("spk_0"));
        assert_eq!(segments[0].speaker_label.as_deref(), Some("spk_0"));
        assert_eq!(segments[0].text, "final text");
    }
}
