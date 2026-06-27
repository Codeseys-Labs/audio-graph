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
}
