//! Shared PCM conversion helpers for provider adapters.
//!
//! The processed-audio bus carries mono `f32` samples in `[-1.0, 1.0]`.
//! Cloud streaming ASR adapters usually need headerless signed 16-bit little
//! endian PCM. Keep that conversion here so provider modules do not drift on
//! scaling, clamping, or NaN handling.

/// Convert one normalized `f32` PCM sample to signed 16-bit PCM.
pub fn f32_sample_to_pcm_s16(sample: f32) -> i16 {
    let clamped = if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    if clamped >= 0.0 {
        (clamped * i16::MAX as f32) as i16
    } else {
        (clamped * -(i16::MIN as f32)) as i16
    }
}

/// Convert normalized mono `f32` PCM samples to headerless signed 16-bit LE PCM.
pub fn f32_mono_to_pcm_s16le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        bytes.extend_from_slice(&f32_sample_to_pcm_s16(sample).to_le_bytes());
    }
    bytes
}

/// Convert one signed 16-bit PCM sample back to normalized `f32` — the exact
/// inverse of [`f32_sample_to_pcm_s16`]'s asymmetric `i16::MAX` /
/// `-(i16::MIN)` scaling, so a decode round trip (e.g. `wav_io::decode` ->
/// this function -> a provider client that expects `f32`) reproduces the same
/// convention the pipeline uses on the encode side.
pub fn pcm_s16_to_f32_sample(sample: i16) -> f32 {
    if sample >= 0 {
        f32::from(sample) / i16::MAX as f32
    } else {
        f32::from(sample) / -(i16::MIN as f32)
    }
}

/// Convert interleaved signed 16-bit PCM samples (e.g. from
/// [`crate::audio::wav_io::decode`]) to normalized `f32` PCM.
pub fn pcm_s16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&s| pcm_s16_to_f32_sample(s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_sample_to_pcm_s16_maps_full_scale_and_zero() {
        assert_eq!(f32_sample_to_pcm_s16(0.0), 0);
        assert_eq!(f32_sample_to_pcm_s16(1.0), i16::MAX);
        assert_eq!(f32_sample_to_pcm_s16(-1.0), i16::MIN);
    }

    #[test]
    fn f32_sample_to_pcm_s16_clamps_and_sanitizes() {
        assert_eq!(f32_sample_to_pcm_s16(2.0), i16::MAX);
        assert_eq!(f32_sample_to_pcm_s16(-2.0), i16::MIN);
        assert_eq!(f32_sample_to_pcm_s16(f32::NAN), 0);
        assert_eq!(f32_sample_to_pcm_s16(f32::INFINITY), 0);
        assert_eq!(f32_sample_to_pcm_s16(f32::NEG_INFINITY), 0);
    }

    #[test]
    fn f32_mono_to_pcm_s16le_bytes_is_little_endian() {
        let bytes = f32_mono_to_pcm_s16le_bytes(&[1.0, -1.0, 0.0]);
        assert_eq!(&bytes[0..2], &i16::MAX.to_le_bytes());
        assert_eq!(&bytes[2..4], &i16::MIN.to_le_bytes());
        assert_eq!(&bytes[4..6], &0i16.to_le_bytes());
    }

    #[test]
    fn pcm_s16_to_f32_sample_maps_full_scale_and_zero() {
        assert_eq!(pcm_s16_to_f32_sample(0), 0.0);
        assert_eq!(pcm_s16_to_f32_sample(i16::MAX), 1.0);
        assert_eq!(pcm_s16_to_f32_sample(i16::MIN), -1.0);
    }

    #[test]
    fn pcm_s16_to_f32_round_trips_through_the_encode_side_convention() {
        // Every full-scale/zero/mid-scale sample must survive an
        // f32 -> i16 -> f32 round trip within one quantization step — this is
        // the load-bearing property for the Deepgram streaming smoke, which
        // decodes a checked-in WAV fixture (i16) and must feed the real
        // client the SAME f32 scaling convention the pipeline's encode side
        // (`f32_sample_to_pcm_s16`) uses, not an ad hoc one.
        for original in [-1.0f32, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0] {
            let encoded = f32_sample_to_pcm_s16(original);
            let decoded = pcm_s16_to_f32_sample(encoded);
            assert!(
                (decoded - original).abs() < 1e-3,
                "round trip drifted too far: original={original} encoded={encoded} decoded={decoded}"
            );
        }
    }

    #[test]
    fn pcm_s16_to_f32_converts_a_whole_interleaved_buffer() {
        let samples = [i16::MIN, 0, i16::MAX];
        let converted = pcm_s16_to_f32(&samples);
        assert_eq!(converted, vec![-1.0, 0.0, 1.0]);
    }
}
