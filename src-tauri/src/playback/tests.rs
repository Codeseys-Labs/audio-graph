//! Tests for the playback subsystem.
//!
//! Most CI environments don't have a real output device available, so these
//! tests focus on the parts of the module that don't require cpal::Stream
//! to actually play audio:
//! - Output device enumeration (returns empty Vec on headless CI; that's OK)
//! - Producer-side push/cancel/resume semantics on the ringbuf
//! - Per-stream HeapRb split-on-open lifecycle
//! - Channel duplication helpers (mono → N-channel interleaved)
//! - Producer-side source-rate → device-rate resampling

use super::*;

#[test]
fn list_output_devices_returns_some_or_empty() {
    // On a real machine this returns ≥1 device; on a headless CI runner it
    // may return 0. Either is acceptable — we just need the call not to
    // panic and the default-flag invariant to hold.
    let devices = list_output_devices();
    let default_count = devices.iter().filter(|d| d.is_default).count();
    assert!(
        default_count <= 1,
        "at most one device should be marked default, got {default_count}"
    );
}

#[test]
fn write_interleaved_i16_duplicates_mono_to_stereo() {
    let mono = vec![100i16, 200, 300];
    let mut out = vec![0i16; 6]; // 3 frames × 2 channels
    write_interleaved_i16(&mut out, &mono, 1, 2);
    assert_eq!(out, vec![100, 100, 200, 200, 300, 300]);
}

#[test]
fn write_interleaved_f32_scales_to_unit_range() {
    let mono = vec![i16::MAX, 0, i16::MIN];
    let mut out = vec![0.0_f32; 6];
    write_interleaved_f32(&mut out, &mono, 1, 2);
    // Scale is 1/i16::MAX, so MAX→1.0, 0→0.0, MIN→approximately -1.0.
    assert!((out[0] - 1.0).abs() < 1e-3);
    assert!((out[1] - 1.0).abs() < 1e-3);
    assert!(out[2].abs() < 1e-6);
    assert!((out[4] + 1.0).abs() < 1e-3);
}

#[test]
fn write_interleaved_u16_centers_at_half() {
    let mono = vec![0i16, i16::MAX, i16::MIN];
    let mut out = vec![0u16; 3];
    write_interleaved_u16(&mut out, &mono, 1, 1);
    assert_eq!(out[0], 32_768); // 0 → midpoint
    assert_eq!(out[1], 65_535); // MAX → max
    assert_eq!(out[2], 0); // MIN → 0
}

#[test]
fn audio_player_new_does_not_open_stream() {
    let player = AudioPlayer::new();
    // No stream open yet, push_samples returns 0 (no producer registered).
    assert_eq!(player.push_samples(&[0, 0, 0]), 0);
    assert_eq!(player.free_samples(), 0);
    drop(player); // graceful shutdown
}

#[test]
fn audio_player_cancel_stops_pushes() {
    let player = AudioPlayer::new();
    player.cancel();
    // Even with cancel set, no producer registered → 0
    assert_eq!(player.push_samples(&[0, 0, 0]), 0);
    player.resume();
    // Still no producer; 0 again.
    assert_eq!(player.push_samples(&[0, 0, 0]), 0);
}

#[test]
fn output_resampler_rejects_zero_rates() {
    assert!(MonoI16OutputResampler::new(0, 48_000).is_err());
    assert!(MonoI16OutputResampler::new(24_000, 0).is_err());
}

#[test]
fn expected_resampled_frame_count_handles_common_playback_rates() {
    assert_eq!(
        expected_resampled_frame_count(24_000, 24_000, 48_000),
        48_000
    );
    assert_eq!(
        expected_resampled_frame_count(16_000, 16_000, 48_000),
        48_000
    );
    assert_eq!(
        expected_resampled_frame_count(44_100, 44_100, 48_000),
        48_000
    );
}

#[test]
fn output_resampler_flushes_24k_to_48k_exact_frame_count() {
    let mut resampler = MonoI16OutputResampler::new(24_000, 48_000).unwrap();
    let mut out = Vec::new();
    let input = vec![0i16; 24_000];

    resampler.process(&input, &mut out).unwrap();
    resampler.finish(&mut out).unwrap();

    assert_eq!(out.len(), 48_000);
}

#[test]
fn output_resampler_flushes_16k_to_48k_exact_frame_count_across_small_chunks() {
    let mut chunked = MonoI16OutputResampler::new(16_000, 48_000).unwrap();
    let mut chunked_out = Vec::new();
    for _ in 0..40 {
        chunked.process(&vec![0i16; 400], &mut chunked_out).unwrap();
    }
    chunked.finish(&mut chunked_out).unwrap();

    let mut one_shot = MonoI16OutputResampler::new(16_000, 48_000).unwrap();
    let mut one_shot_out = Vec::new();
    one_shot
        .process(&vec![0i16; 16_000], &mut one_shot_out)
        .unwrap();
    one_shot.finish(&mut one_shot_out).unwrap();

    assert_eq!(chunked_out.len(), 48_000);
    assert_eq!(one_shot_out.len(), 48_000);
}

#[test]
fn output_resampler_reset_discards_partial_input() {
    for source_rate in [24_000, 16_000] {
        let mut resampler = MonoI16OutputResampler::new(source_rate, 48_000).unwrap();
        let mut out = Vec::new();

        resampler.process(&[1, 2, 3, 4], &mut out).unwrap();
        assert!(
            out.is_empty(),
            "partial {source_rate} Hz input should not emit before reset"
        );
        resampler.reset();
        assert_eq!(
            resampler.finish(&mut out).unwrap(),
            0,
            "reset {source_rate} Hz input should leave nothing to flush"
        );

        assert!(out.is_empty());
    }
}

#[test]
fn audio_player_cancel_resets_16k_resampler_state_before_resume() {
    let player = AudioPlayer::with_capacity(50_000);
    let rb = HeapRb::<i16>::new(50_000);
    let (prod, mut cons) = rb.split();
    *player.producer.lock().unwrap_or_else(|p| p.into_inner()) =
        Some(PlaybackProducer::new(prod, 16_000, 48_000).unwrap());

    assert_eq!(player.push_samples(&[i16::MAX; 400]), 0);
    assert_eq!(cons.occupied_len(), 0);

    player.cancel();
    assert!(player.is_cancelled());
    assert_eq!(cons.occupied_len(), 0);

    player.resume();
    assert_eq!(player.flush_samples(), 0);
    assert_eq!(cons.occupied_len(), 0);

    let input = vec![0i16; 16_000];
    let pushed = player.push_samples(&input);
    let flushed = player.flush_samples();

    assert_eq!(pushed + flushed, 48_000);
    assert_eq!(cons.occupied_len(), 48_000);
    let mut drained = vec![1i16; 48_000];
    assert_eq!(cons.pop_slice(&mut drained), 48_000);
    assert!(drained.iter().all(|sample| *sample == 0));
}

#[test]
fn playback_producer_resamples_before_ring_buffer() {
    let rb = HeapRb::<i16>::new(50_000);
    let (prod, mut cons) = rb.split();
    let mut producer = PlaybackProducer::new(prod, 24_000, 48_000).unwrap();
    let input = vec![0i16; 24_000];

    let pushed = producer.push_source_samples(&input);
    let flushed = producer.flush();

    assert_eq!(pushed + flushed, 48_000);
    assert_eq!(cons.occupied_len(), 48_000);
    let mut drained = vec![1i16; 48_000];
    assert_eq!(cons.pop_slice(&mut drained), 48_000);
    assert!(drained.iter().all(|sample| *sample == 0));
}

/// Wave B intentionally constructs a fresh HeapRb per stream-open. This
/// test checks the contract surface: open_default returns NoDefaultDevice
/// on a headless CI runner without panicking.
///
/// Skipped on Windows because Blacksmith Windows VMs ship without an audio
/// service (Audiosrv absent). cpal's WASAPI default_output_device probe
/// then segfaults inside MMDeviceEnumerator::GetDefaultAudioEndpoint
/// before we can return our NoDefaultDevice error. This is the same
/// limitation rsac ran into on the same runners — see their
/// .github/workflows/ci-audio-tests.yml for the workaround pattern.
#[cfg(not(target_os = "windows"))]
#[test]
fn open_default_handles_missing_device_gracefully() {
    let player = AudioPlayer::new();
    let result = player.open_default(PlaybackConfig::default());
    // On headless CI: NoDefaultDevice (or similar BuildStream error wrapped
    // in PlaybackError). On a real machine: Ok. Either way we should not
    // panic.
    match result {
        Ok(()) => {
            // Real machine — push some samples and verify they fit in the
            // ringbuf.
            assert!(player.free_samples() > 0);
        }
        Err(PlaybackError::NoDefaultDevice) | Err(PlaybackError::BuildStream(_)) => {
            // Acceptable on headless CI.
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}
