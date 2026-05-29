//! Pure mixing math for the audio mixer, isolated with **zero non-std deps** so
//! it can be unit-tested standalone (`rustc --test src/audio/mix_math.rs`) even
//! though the full crate's test harness can't launch on this Windows box (the
//! ML libs link a mismatched MSVC runtime — see seeds 9f6e). `mixer.rs` uses
//! these directly, so the tests exercise the real code.

use std::collections::VecDeque;

/// Frame size pulled per mix step (~32 ms at 16 kHz; matches the pipeline).
pub const FRAME: usize = 512;

/// Sum one `FRAME` from each source (each slice is silence-padded to `FRAME`),
/// scale by `1/sqrt(active)` to preserve loudness without letting a dominant
/// source vanish, then hard-clamp to `[-1, 1]`.
pub fn mix_frame(frames: &[Vec<f32>]) -> Vec<f32> {
    let mut out = vec![0.0f32; FRAME];
    if frames.is_empty() {
        return out;
    }
    for frame in frames {
        for (o, &s) in out.iter_mut().zip(frame.iter()) {
            *o += s;
        }
    }
    let scale = 1.0 / (frames.len() as f32).sqrt();
    for o in out.iter_mut() {
        *o = (*o * scale).clamp(-1.0, 1.0);
    }
    out
}

/// Pull up to `FRAME` samples from a buffer, silence-padding the tail when the
/// source is short (jitter / just-stopped). Returns `None` when fully empty.
pub fn take_frame(buf: &mut VecDeque<f32>) -> Option<Vec<f32>> {
    if buf.is_empty() {
        return None;
    }
    let mut frame = Vec::with_capacity(FRAME);
    for _ in 0..FRAME {
        frame.push(buf.pop_front().unwrap_or(0.0));
    }
    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_source_passes_through_scaled() {
        let out = mix_frame(&[vec![0.5f32; FRAME]]);
        assert_eq!(out.len(), FRAME);
        assert!((out[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn two_sources_sum_and_scale() {
        let out = mix_frame(&[vec![0.5f32; FRAME], vec![0.5f32; FRAME]]);
        assert!((out[0] - (1.0 / 2.0_f32.sqrt())).abs() < 1e-4);
    }

    #[test]
    fn loud_sum_is_clamped() {
        let out = mix_frame(&[vec![1.0; FRAME], vec![1.0; FRAME], vec![1.0; FRAME]]);
        assert!(out.iter().all(|&s| (-1.0..=1.0).contains(&s)));
    }

    #[test]
    fn empty_frames_is_silence() {
        let out = mix_frame(&[]);
        assert_eq!(out.len(), FRAME);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn take_frame_silence_pads_short_source() {
        let mut buf: VecDeque<f32> = VecDeque::from(vec![1.0f32; 10]);
        let f = take_frame(&mut buf).unwrap();
        assert_eq!(f.len(), FRAME);
        assert_eq!(f[0], 1.0);
        assert_eq!(f[FRAME - 1], 0.0);
        assert!(buf.is_empty());
    }

    #[test]
    fn take_frame_consumes_exactly_one_frame() {
        let mut buf: VecDeque<f32> = VecDeque::from(vec![0.2f32; FRAME + 100]);
        let f = take_frame(&mut buf).unwrap();
        assert_eq!(f.len(), FRAME);
        assert_eq!(buf.len(), 100);
    }

    #[test]
    fn empty_buffer_yields_no_frame() {
        let mut buf: VecDeque<f32> = VecDeque::new();
        assert!(take_frame(&mut buf).is_none());
    }
}
