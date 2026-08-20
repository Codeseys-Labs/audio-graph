//! Minimal WAV PCM16 reader/writer for CI audio fixtures.
//!
//! No `hound` (or other WAV crate) dependency is added for this: the repo's
//! Cargo.toml intentionally holds the dependency line for CI-only fixture
//! tooling, and every WAV this repo checks in already uses the same simple
//! layout — RIFF/WAVE preamble, one 16-byte `fmt ` chunk, one `data` chunk,
//! no extensible-fmt, no extra chunks (`source_separation_fixtures.rs` and
//! `aec_vad_fixtures.rs` hand-parse this exact layout for validation; this
//! module is the write side plus a shared, error-typed read side).
//!
//! ## Header layout (44 bytes total)
//!
//! | Offset | Size | Field                                    |
//! |--------|------|-------------------------------------------|
//! | 0      | 4    | `"RIFF"`                                  |
//! | 4      | 4    | RIFF chunk size (`36 + data_len`, LE u32) |
//! | 8      | 4    | `"WAVE"`                                  |
//! | 12     | 4    | `"fmt "`                                  |
//! | 16     | 4    | fmt chunk size (`16`, LE u32)              |
//! | 20     | 2    | audio format (`1` = PCM, LE u16)          |
//! | 22     | 2    | channels (LE u16)                         |
//! | 24     | 4    | sample rate (LE u32)                      |
//! | 28     | 4    | byte rate = `rate * channels * 2` (LE u32)|
//! | 32     | 2    | block align = `channels * 2` (LE u16)     |
//! | 34     | 2    | bits per sample (`16`, LE u16)             |
//! | 36     | 4    | `"data"`                                  |
//! | 40     | 4    | data chunk size (LE u32)                  |
//! | 44     | ...  | interleaved signed 16-bit LE PCM samples  |

use std::io::{self, Write};

/// Length of the canonical header this module reads/writes (see module docs).
pub const CANONICAL_HEADER_LEN: usize = 44;

/// Decoded interleaved signed 16-bit PCM audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavPcm16 {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved samples: for `channels > 1`, frame `f`'s channel `c` is
    /// `samples[f * channels + c]`.
    pub samples: Vec<i16>,
}

impl WavPcm16 {
    /// Number of complete frames (`samples.len() / channels`), or `0` if
    /// `channels` is `0`.
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            return 0;
        }
        self.samples.len() / usize::from(self.channels)
    }

    /// Duration in whole milliseconds, or `0` if `sample_rate` is `0`.
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        (self.frame_count() as u64 * 1000) / u64::from(self.sample_rate)
    }
}

/// Typed WAV parse failures. Every variant names exactly what was expected so
/// a bad fixture fails loudly instead of silently misreading garbage as
/// audio.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WavError {
    #[error("WAV data too short for a header: {0} byte(s), need >= {CANONICAL_HEADER_LEN}")]
    TooShort(usize),
    #[error("missing 'RIFF' tag at offset 0")]
    MissingRiff,
    #[error("missing 'WAVE' tag at offset 8")]
    MissingWave,
    #[error("missing 'fmt ' chunk")]
    MissingFmtChunk,
    #[error("'fmt ' chunk is only {0} byte(s), need >= 16")]
    FmtChunkTooShort(usize),
    #[error("unsupported WAV format tag {0} (only PCM = 1 is supported)")]
    UnsupportedFormatTag(u16),
    #[error("unsupported bit depth {0} (only 16-bit PCM is supported)")]
    UnsupportedBitDepth(u16),
    #[error("missing 'data' chunk")]
    MissingDataChunk,
    #[error("chunk starting at offset {0} runs past the end of the buffer")]
    TruncatedChunk(usize),
    #[error("'data' chunk is {0} byte(s), not a whole number of {1}-channel frames")]
    UnalignedDataChunk(u32, u16),
}

/// Encode interleaved signed 16-bit PCM as a canonical 44-byte-header WAV
/// file. Pure and deterministic: identical `(samples, sample_rate,
/// channels)` always produces identical bytes.
pub fn encode(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    const BITS_PER_SAMPLE: u16 = 16;
    let byte_rate = sample_rate * u32::from(channels) * u32::from(BITS_PER_SAMPLE / 8);
    let block_align = channels * (BITS_PER_SAMPLE / 8);
    let data_len = (samples.len() * 2) as u32;
    let riff_len = 36 + data_len;

    let mut out = io::Cursor::new(Vec::with_capacity(CANONICAL_HEADER_LEN + samples.len() * 2));
    write_all_infallible(&mut out, b"RIFF");
    write_all_infallible(&mut out, &riff_len.to_le_bytes());
    write_all_infallible(&mut out, b"WAVE");
    write_all_infallible(&mut out, b"fmt ");
    write_all_infallible(&mut out, &16u32.to_le_bytes());
    write_all_infallible(&mut out, &1u16.to_le_bytes()); // PCM
    write_all_infallible(&mut out, &channels.to_le_bytes());
    write_all_infallible(&mut out, &sample_rate.to_le_bytes());
    write_all_infallible(&mut out, &byte_rate.to_le_bytes());
    write_all_infallible(&mut out, &block_align.to_le_bytes());
    write_all_infallible(&mut out, &BITS_PER_SAMPLE.to_le_bytes());
    write_all_infallible(&mut out, b"data");
    write_all_infallible(&mut out, &data_len.to_le_bytes());
    for &sample in samples {
        write_all_infallible(&mut out, &sample.to_le_bytes());
    }
    out.into_inner()
}

/// `Cursor<Vec<u8>>` writes never fail (no OS handle, no capacity limit
/// worth handling); centralising the `expect` keeps `encode` free of `?`
/// noise while still routing every write through `std::io::Write`.
fn write_all_infallible(out: &mut io::Cursor<Vec<u8>>, buf: &[u8]) {
    out.write_all(buf)
        .expect("writes into an in-memory Vec<u8> cursor cannot fail");
}

/// Decode a canonical WAV byte buffer into interleaved 16-bit PCM. Chunks
/// other than `fmt ` / `data` are skipped (not rejected), so a well-formed
/// WAV with e.g. a `LIST` metadata chunk still parses.
pub fn decode(bytes: &[u8]) -> Result<WavPcm16, WavError> {
    if bytes.len() < CANONICAL_HEADER_LEN {
        return Err(WavError::TooShort(bytes.len()));
    }
    if &bytes[0..4] != b"RIFF" {
        return Err(WavError::MissingRiff);
    }
    if &bytes[8..12] != b"WAVE" {
        return Err(WavError::MissingWave);
    }

    let mut offset = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (format_tag, channels, sample_rate, bits_per_sample)
    let mut data: Option<(usize, usize)> = None; // (start, len)

    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = u32::from_le_bytes(
            bytes[offset + 4..offset + 8]
                .try_into()
                .expect("4-byte slice"),
        ) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start
            .checked_add(chunk_size)
            .ok_or(WavError::TruncatedChunk(offset))?;
        if chunk_end > bytes.len() {
            return Err(WavError::TruncatedChunk(offset));
        }

        if chunk_id == b"fmt " {
            if chunk_size < 16 {
                return Err(WavError::FmtChunkTooShort(chunk_size));
            }
            let format_tag = u16::from_le_bytes(
                bytes[chunk_start..chunk_start + 2]
                    .try_into()
                    .expect("2-byte slice"),
            );
            let channels = u16::from_le_bytes(
                bytes[chunk_start + 2..chunk_start + 4]
                    .try_into()
                    .expect("2-byte slice"),
            );
            let sample_rate = u32::from_le_bytes(
                bytes[chunk_start + 4..chunk_start + 8]
                    .try_into()
                    .expect("4-byte slice"),
            );
            let bits_per_sample = u16::from_le_bytes(
                bytes[chunk_start + 14..chunk_start + 16]
                    .try_into()
                    .expect("2-byte slice"),
            );
            fmt = Some((format_tag, channels, sample_rate, bits_per_sample));
        } else if chunk_id == b"data" {
            data = Some((chunk_start, chunk_size));
        }

        // WAV chunks are word-aligned: an odd-sized chunk is followed by one
        // pad byte that is not part of the chunk.
        offset = chunk_end + (chunk_size % 2);
    }

    let (format_tag, channels, sample_rate, bits_per_sample) =
        fmt.ok_or(WavError::MissingFmtChunk)?;
    if format_tag != 1 {
        return Err(WavError::UnsupportedFormatTag(format_tag));
    }
    if bits_per_sample != 16 {
        return Err(WavError::UnsupportedBitDepth(bits_per_sample));
    }
    let (data_start, data_len) = data.ok_or(WavError::MissingDataChunk)?;
    let frame_bytes = usize::from(channels) * 2;
    if frame_bytes == 0 || data_len % frame_bytes != 0 {
        return Err(WavError::UnalignedDataChunk(data_len as u32, channels));
    }

    let samples = bytes[data_start..data_start + data_len]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();

    Ok(WavPcm16 {
        sample_rate,
        channels,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trips_mono() {
        let samples: Vec<i16> = vec![0, 100, -100, i16::MAX, i16::MIN, 42];
        let bytes = encode(&samples, 16_000, 1);
        assert_eq!(bytes.len(), CANONICAL_HEADER_LEN + samples.len() * 2);
        let decoded = decode(&bytes).expect("decode must succeed");
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples, samples);
        assert_eq!(decoded.frame_count(), samples.len());
    }

    #[test]
    fn encode_decode_round_trips_stereo_interleaved() {
        let samples: Vec<i16> = vec![1, -1, 2, -2, 3, -3];
        let bytes = encode(&samples, 48_000, 2);
        let decoded = decode(&bytes).expect("decode must succeed");
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.frame_count(), 3);
        assert_eq!(decoded.samples, samples);
    }

    #[test]
    fn encode_is_byte_identical_across_calls() {
        let samples: Vec<i16> = (0..1000).map(|i| (i % 200) as i16).collect();
        let first = encode(&samples, 16_000, 1);
        let second = encode(&samples, 16_000, 1);
        assert_eq!(
            first, second,
            "encoding the same input twice must be byte-identical"
        );
    }

    #[test]
    fn empty_samples_encode_to_header_only() {
        let bytes = encode(&[], 16_000, 1);
        assert_eq!(bytes.len(), CANONICAL_HEADER_LEN);
        let decoded = decode(&bytes).expect("decode must succeed");
        assert!(decoded.samples.is_empty());
        assert_eq!(decoded.duration_ms(), 0);
    }

    #[test]
    fn duration_ms_matches_frame_count_and_rate() {
        let samples = vec![0i16; 16_000 * 2]; // 2 seconds @ 16 kHz mono
        let bytes = encode(&samples, 16_000, 1);
        let decoded = decode(&bytes).expect("decode must succeed");
        assert_eq!(decoded.duration_ms(), 2_000);
    }

    #[test]
    fn decode_rejects_too_short_buffer() {
        assert_eq!(decode(&[0u8; 10]), Err(WavError::TooShort(10)));
    }

    #[test]
    fn decode_rejects_bad_riff_tag() {
        let mut bytes = encode(&[0i16, 1, 2], 16_000, 1);
        bytes[0] = b'X';
        assert_eq!(decode(&bytes), Err(WavError::MissingRiff));
    }

    #[test]
    fn decode_rejects_bad_wave_tag() {
        let mut bytes = encode(&[0i16, 1, 2], 16_000, 1);
        bytes[8] = b'X';
        assert_eq!(decode(&bytes), Err(WavError::MissingWave));
    }

    #[test]
    fn decode_rejects_non_pcm_format_tag() {
        let mut bytes = encode(&[0i16, 1, 2], 16_000, 1);
        // Offset 20..22 is the format tag; 3 = IEEE float, not PCM.
        bytes[20] = 3;
        bytes[21] = 0;
        assert_eq!(decode(&bytes), Err(WavError::UnsupportedFormatTag(3)));
    }

    #[test]
    fn decode_rejects_non_16_bit_depth() {
        let mut bytes = encode(&[0i16, 1, 2], 16_000, 1);
        // Offset 34..36 is bits-per-sample; claim 8-bit.
        bytes[34] = 8;
        bytes[35] = 0;
        assert_eq!(decode(&bytes), Err(WavError::UnsupportedBitDepth(8)));
    }

    #[test]
    fn decode_rejects_truncated_data_chunk() {
        let mut bytes = encode(&[0i16, 1, 2, 3], 16_000, 1);
        // Claim the data chunk is far larger than the buffer actually is.
        let data_len_offset = CANONICAL_HEADER_LEN - 4;
        bytes[data_len_offset..data_len_offset + 4].copy_from_slice(&9_999u32.to_le_bytes());
        assert_eq!(decode(&bytes), Err(WavError::TruncatedChunk(36)));
    }

    #[test]
    fn decode_rejects_unaligned_data_chunk_for_channel_count() {
        // 3 bytes of PCM16 stereo data cannot be a whole number of 4-byte frames.
        let mut bytes = encode(&[0i16, 1], 16_000, 2);
        let data_len_offset = CANONICAL_HEADER_LEN - 4;
        bytes[data_len_offset..data_len_offset + 4].copy_from_slice(&3u32.to_le_bytes());
        bytes.truncate(CANONICAL_HEADER_LEN + 3);
        assert_eq!(decode(&bytes), Err(WavError::UnalignedDataChunk(3, 2)));
    }
}
