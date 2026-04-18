//! Unit tests for `AudioAccumulator` — the speech-pipeline helper that
//! batches fixed-size `ProcessedAudioChunk`s (~32 ms each at 16 kHz) into
//! ~2 s `AccumulatedSegment`s suitable for Whisper transcription, while
//! retaining a 0.5 s overlap between consecutive segments so words that
//! straddle a segment boundary are captured twice.
//!
//! loop-15 A3 (closing loop-12 HIGH #2's open test gap — per loop-12 A3's
//! retrospective, this was the highest-ROI 2-hour test addition).
//!
//! API surface under test (all `struct`/`fn` items are module-private — these
//! tests are a child `#[cfg(test)] mod` so they can see them via `super::*`):
//!
//! - `AudioAccumulator::new() -> Self`
//! - `AudioAccumulator::feed(&mut self, &ProcessedAudioChunk) -> Option<AccumulatedSegment>`
//! - `AudioAccumulator::flush(self) -> Option<AccumulatedSegment>` — consumes `self`
//!
//! API observations (noted rather than changed — A3 brief is tests, not
//! refactors):
//!
//! - There is no public `reset` method. State is cleared by dropping the
//!   accumulator after `flush()`, or implicitly by `take()` (private) which
//!   `feed()` calls internally when `TARGET_FRAMES` is reached.
//! - There is no backpressure signal. `feed()` is always non-blocking and
//!   simply keeps extending `audio` — if downstream stalls, memory grows
//!   unbounded. Backpressure lives in the channel upstream of the
//!   accumulator, not in the accumulator itself.
//! - There is no silence-gap detector. Segment boundaries are driven purely
//!   by accumulated frame count (`>= TARGET_FRAMES`), not by gaps in the
//!   timestamp stream. These tests therefore do not exercise silence
//!   handling — there is nothing to exercise.
//! - `source_id` is captured on the first chunk and then pinned for the
//!   lifetime of the accumulator: subsequent chunks with a different
//!   `source_id` do **not** overwrite it. This is intentional (one
//!   accumulator per source in the real pipeline) and is pinned here as a
//!   test so a future refactor noticing the "unused" later `source_id`
//!   cannot silently change the behavior.

use std::time::Duration;

use super::*;
use crate::audio::pipeline::ProcessedAudioChunk;

/// Build a `ProcessedAudioChunk` filled with `value` samples.
fn chunk(
    source_id: &str,
    frames: usize,
    value: f32,
    timestamp_ms: Option<u64>,
) -> ProcessedAudioChunk {
    ProcessedAudioChunk {
        source_id: source_id.to_string(),
        data: vec![value; frames],
        sample_rate: 16_000,
        num_frames: frames,
        timestamp: timestamp_ms.map(Duration::from_millis),
    }
}

// ---------------------------------------------------------------------------
// Basic feed / no-emit below threshold
// ---------------------------------------------------------------------------

#[test]
fn feed_below_target_returns_none_and_preserves_order() {
    let mut acc = AudioAccumulator::new();

    // Three chunks, each 512 frames → 1536 frames total, well under TARGET_FRAMES (32_000).
    // Distinct sample values so we can check concatenation order after a later flush.
    assert!(acc.feed(&chunk("src", 512, 0.1, Some(0))).is_none());
    assert!(acc.feed(&chunk("src", 512, 0.2, Some(32))).is_none());
    assert!(acc.feed(&chunk("src", 512, 0.3, Some(64))).is_none());

    let seg = acc.flush().expect("partial audio must flush as a segment");
    assert_eq!(seg.num_frames, 1536);
    assert_eq!(seg.audio.len(), 1536);
    // Order preserved: first 512 are 0.1, next 512 are 0.2, last 512 are 0.3.
    assert!((seg.audio[0] - 0.1).abs() < 1e-6);
    assert!((seg.audio[512] - 0.2).abs() < 1e-6);
    assert!((seg.audio[1024] - 0.3).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// Target reached → emit; overlap retained for next segment
// ---------------------------------------------------------------------------

#[test]
fn feed_reaching_target_emits_segment_with_full_audio_and_timing() {
    let mut acc = AudioAccumulator::new();

    // Push exactly TARGET_FRAMES in one chunk. This must trigger emission.
    let seg = acc
        .feed(&chunk("src-a", TARGET_FRAMES, 0.25, Some(0)))
        .expect("reaching TARGET_FRAMES must emit a segment");

    assert_eq!(seg.source_id, "src-a");
    assert_eq!(seg.num_frames, TARGET_FRAMES);
    assert_eq!(seg.audio.len(), TARGET_FRAMES);
    assert_eq!(seg.start_time, Duration::from_millis(0));
    // With only one chunk, start == end (the end timestamp is the *chunk's*
    // timestamp, not the "end of the chunk's audio" timestamp — the
    // accumulator doesn't know the chunk duration).
    assert_eq!(seg.end_time, Duration::from_millis(0));
}

#[test]
fn emit_retains_overlap_for_next_segment() {
    let mut acc = AudioAccumulator::new();

    // First segment: TARGET_FRAMES of value 0.5, timestamp at end = 2000 ms.
    let seg1 = acc
        .feed(&chunk("src", TARGET_FRAMES, 0.5, Some(2_000)))
        .expect("first segment emitted");
    assert_eq!(seg1.num_frames, TARGET_FRAMES);

    // After emission, the accumulator should hold exactly OVERLAP_FRAMES
    // samples from the tail of seg1 (all 0.5). Feeding just enough new
    // frames to reach TARGET_FRAMES again should trigger a second emission.
    let remaining = TARGET_FRAMES - OVERLAP_FRAMES;
    let seg2 = acc
        .feed(&chunk("src", remaining, 0.9, Some(4_000)))
        .expect("second segment emitted once target reached again");

    assert_eq!(seg2.num_frames, TARGET_FRAMES);
    // Overlap region (first OVERLAP_FRAMES samples) comes from seg1's tail = 0.5.
    assert!((seg2.audio[0] - 0.5).abs() < 1e-6);
    assert!((seg2.audio[OVERLAP_FRAMES - 1] - 0.5).abs() < 1e-6);
    // New samples start right after the overlap = 0.9.
    assert!((seg2.audio[OVERLAP_FRAMES] - 0.9).abs() < 1e-6);
    assert!((seg2.audio[TARGET_FRAMES - 1] - 0.9).abs() < 1e-6);
}

#[test]
fn second_segment_start_time_backdates_by_overlap_duration() {
    let mut acc = AudioAccumulator::new();

    // seg1 ends at 2_000 ms. OVERLAP_FRAMES is 8_000 samples at 16 kHz = 500 ms.
    // So seg2.start_time must be 2_000 - 500 = 1_500 ms.
    let _ = acc
        .feed(&chunk("src", TARGET_FRAMES, 0.5, Some(2_000)))
        .expect("seg1 emitted");

    let remaining = TARGET_FRAMES - OVERLAP_FRAMES;
    let seg2 = acc
        .feed(&chunk("src", remaining, 0.9, Some(4_000)))
        .expect("seg2 emitted");

    assert_eq!(seg2.start_time, Duration::from_millis(1_500));
    assert_eq!(seg2.end_time, Duration::from_millis(4_000));
}

// ---------------------------------------------------------------------------
// source_id pinning on first chunk
// ---------------------------------------------------------------------------

#[test]
fn source_id_is_captured_from_first_chunk_and_pinned() {
    let mut acc = AudioAccumulator::new();

    // First chunk sets source_id = "mic".
    assert!(acc.feed(&chunk("mic", 1_000, 0.1, Some(0))).is_none());
    // Second chunk with a DIFFERENT source_id must NOT overwrite.
    assert!(acc.feed(&chunk("system", 1_000, 0.2, Some(32))).is_none());

    let seg = acc.flush().expect("flush emits segment");
    assert_eq!(
        seg.source_id, "mic",
        "source_id must be pinned to the first chunk's value"
    );
}

// ---------------------------------------------------------------------------
// flush semantics
// ---------------------------------------------------------------------------

#[test]
fn flush_on_empty_accumulator_returns_none() {
    let acc = AudioAccumulator::new();
    assert!(
        acc.flush().is_none(),
        "flushing a fresh accumulator must return None"
    );
}

#[test]
fn flush_after_partial_feed_returns_segment_with_remaining_audio() {
    let mut acc = AudioAccumulator::new();
    assert!(acc.feed(&chunk("src", 4_096, 0.7, Some(100))).is_none());

    let seg = acc.flush().expect("partial audio must flush");
    assert_eq!(seg.num_frames, 4_096);
    assert_eq!(seg.source_id, "src");
    assert_eq!(seg.start_time, Duration::from_millis(100));
    assert_eq!(seg.end_time, Duration::from_millis(100));
}

#[test]
fn flush_after_emit_still_returns_overlap_tail() {
    // After an emission, the accumulator retains OVERLAP_FRAMES of tail audio.
    // If the caller then flushes without feeding more, those retained frames
    // should come back as a trailing segment — otherwise the last 0.5 s of
    // the stream would be silently dropped.
    let mut acc = AudioAccumulator::new();
    let _ = acc
        .feed(&chunk("src", TARGET_FRAMES, 0.4, Some(2_000)))
        .expect("first emission");

    let tail = acc
        .flush()
        .expect("overlap tail must flush, not be dropped");
    assert_eq!(tail.num_frames, OVERLAP_FRAMES);
    assert!((tail.audio[0] - 0.4).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// Timestamp handling: None timestamps are coerced to ZERO without panicking
// ---------------------------------------------------------------------------

#[test]
fn none_timestamps_are_treated_as_zero_without_panic() {
    let mut acc = AudioAccumulator::new();

    // First chunk has no timestamp → segment_start stays None until the
    // *current* code-path pulls it. Feed a second with a timestamp to cover
    // both branches.
    assert!(acc.feed(&chunk("src", 1_000, 0.1, None)).is_none());
    assert!(acc.feed(&chunk("src", 1_000, 0.2, None)).is_none());

    let seg = acc.flush().expect("flush works with None timestamps");
    // Both start and end default to ZERO when no timestamp is ever seen.
    assert_eq!(seg.start_time, Duration::ZERO);
    assert_eq!(seg.end_time, Duration::ZERO);
    assert_eq!(seg.num_frames, 2_000);
}

// ---------------------------------------------------------------------------
// Overflow: one oversized chunk exceeds TARGET_FRAMES
// ---------------------------------------------------------------------------

#[test]
fn single_oversize_chunk_emits_whole_chunk_as_one_segment() {
    // The accumulator does NOT split oversize chunks — it just lets them
    // through and emits when `len >= TARGET_FRAMES`. A chunk of 3x
    // TARGET_FRAMES therefore produces ONE segment of 3x TARGET_FRAMES, not
    // three separate segments. Pinning this behavior so future "split large
    // inputs" optimizations are forced through code review.
    let mut acc = AudioAccumulator::new();
    let big = TARGET_FRAMES * 3;
    let seg = acc
        .feed(&chunk("src", big, 0.5, Some(0)))
        .expect("oversize chunk must still emit");
    assert_eq!(seg.num_frames, big);

    // Overlap retained is still only OVERLAP_FRAMES — not proportional to
    // the oversize chunk.
    let tail = acc.flush().expect("overlap tail");
    assert_eq!(tail.num_frames, OVERLAP_FRAMES);
}

// ---------------------------------------------------------------------------
// Multi-segment invariant: total frames emitted == total fed + overlap per emission
// ---------------------------------------------------------------------------

#[test]
fn multi_segment_invariant_total_frames_match_fed_plus_overlap() {
    // Property: after N emissions, sum(segment.num_frames) ==
    // total_fed + (N-1) * OVERLAP_FRAMES  — each subsequent segment re-emits
    // OVERLAP_FRAMES of the previous tail, so those frames are counted once
    // extra per boundary.
    //
    // We drive three full emissions by feeding TARGET_FRAMES + 2*(TARGET_FRAMES - OVERLAP_FRAMES)
    // fresh samples across a series of 1_000-frame chunks.
    let mut acc = AudioAccumulator::new();
    let fresh_per_emit_after_first = TARGET_FRAMES - OVERLAP_FRAMES;
    let total_fresh = TARGET_FRAMES + 2 * fresh_per_emit_after_first;

    let chunk_size = 1_000;
    let num_chunks = total_fresh / chunk_size;
    assert_eq!(
        total_fresh % chunk_size,
        0,
        "test arithmetic: total_fresh must be a clean multiple of chunk_size"
    );

    let mut segments = Vec::new();
    for i in 0..num_chunks {
        if let Some(seg) = acc.feed(&chunk("src", chunk_size, 0.5, Some((i * 60) as u64))) {
            segments.push(seg);
        }
    }
    if let Some(seg) = acc.flush() {
        segments.push(seg);
    }

    assert!(
        segments.len() >= 3,
        "expected at least 3 segments, got {}",
        segments.len()
    );

    let total_emitted: usize = segments.iter().map(|s| s.num_frames).sum();
    let n = segments.len();
    let expected = total_fresh + (n - 1) * OVERLAP_FRAMES;
    assert_eq!(
        total_emitted, expected,
        "total emitted frames must equal fed frames plus (N-1)*OVERLAP for N segments"
    );
}

// ---------------------------------------------------------------------------
// Feed exactly TARGET_FRAMES in many small chunks → triggers on the last
// ---------------------------------------------------------------------------

#[test]
fn emission_triggers_only_on_the_chunk_that_crosses_the_threshold() {
    let mut acc = AudioAccumulator::new();
    let chunk_size = 512;
    let needed = TARGET_FRAMES / chunk_size; // 62.5 → 62 full chunks < target, 63rd crosses

    // Feed exactly `needed - 1` = 62 chunks (31_744 frames) — below target.
    for i in 0..(needed) {
        let out = acc.feed(&chunk("src", chunk_size, 0.1, Some((i * 32) as u64)));
        assert!(
            out.is_none(),
            "chunk {} of {} must not emit (still below target)",
            i + 1,
            needed
        );
    }

    // One more chunk crosses the threshold and emits.
    let seg = acc
        .feed(&chunk("src", chunk_size, 0.2, Some(9_999)))
        .expect("crossing-threshold chunk must emit");
    // Emitted segment contains every frame fed so far: needed*chunk_size + chunk_size.
    assert_eq!(seg.num_frames, (needed + 1) * chunk_size);
    assert!(seg.num_frames >= TARGET_FRAMES);
    assert_eq!(seg.end_time, Duration::from_millis(9_999));
}
