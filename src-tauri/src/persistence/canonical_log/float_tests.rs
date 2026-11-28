use super::*;
use std::time::Duration;
const SAMPLES: usize = 16_384;
const EARLY_TICKS: usize = 4_096;
const SESSION_SECONDS: u64 = 8 * 60 * 60;

// SplitMix64 has a fixed mapping on every target. Use independent seeds for
// each clock so that their samples span different locations in a session.
fn draws(mut state: u64) -> impl Iterator<Item = u64> {
    std::iter::from_fn(move || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut word = state;
        word = (word ^ (word >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        word = (word ^ (word >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Some(word ^ (word >> 31))
    })
}

fn clock_ticks(rate: u64, seed: u64) -> impl Iterator<Item = u64> {
    (0..EARLY_TICKS as u64).chain(
        draws(seed)
            .take(SAMPLES - EARLY_TICKS)
            .map(move |word| word % (SESSION_SECONDS * rate)),
    )
}

fn verify(group: &str, values: impl IntoIterator<Item = f64>) -> usize {
    let values: Vec<_> = values.into_iter().collect();
    for chunk in values.chunks(512) {
        let payload = serde_json::json!({"nested": [chunk]});
        let hash = payload_digest(&payload).unwrap();
        let record_hash = framed_record_hash(
            "session",
            "numbers",
            1,
            1,
            "event",
            &[],
            &Default::default(),
            ZERO_HASH,
            &hash,
        )
        .unwrap();
        let wire = WireRecordV1 {
            format_version: CANONICAL_LOG_FORMAT_VERSION,
            session_id: "session".into(),
            stream_id: "numbers".into(),
            domain_schema_version: 1,
            sequence: 1,
            event_id: "event".into(),
            causal_event_ids: vec![],
            basis_heads: Default::default(),
            previous_hash: ZERO_HASH.into(),
            payload_hash: hash,
            record_hash,
            payload,
        };
        let json = serde_json::to_string(&wire).unwrap();
        let frame = format!("AGCL1 {:016x} {json}", json.len());
        let read =
            parse_framed_v1(frame.as_bytes(), "session", "numbers", 1, 1, ZERO_HASH).unwrap();
        let numbers = read.payload["nested"][0].as_array().unwrap();
        assert_eq!(numbers.len(), chunk.len());
        for (got, want) in numbers.iter().zip(chunk) {
            assert_eq!(got.as_f64().unwrap().to_bits(), want.to_bits(), "{group}");
        }
    }
    values.len()
}

#[test]
fn framed_reader_round_trips_realistic_and_edge_float_corpora() {
    let realistic = [
        verify(
            "milliseconds",
            clock_ticks(1_000, 11).map(|n| n as f64 / 1_000.0),
        ),
        verify(
            "samples_16khz",
            clock_ticks(16_000, 23).map(|n| n as f64 / 16_000.0),
        ),
        verify(
            "samples_48khz",
            clock_ticks(48_000, 37).map(|n| n as f64 / 48_000.0),
        ),
        verify(
            "nanosecond_durations",
            clock_ticks(1_000_000_000, 53).map(|n| Duration::from_nanos(n).as_secs_f64()),
        ),
        verify(
            "arbitrary_session_times",
            draws(67).take(SAMPLES).map(|word| {
                // Normal finite doubles from just under a microsecond to several
                // hours, spanning mantissas that integer clock conversion misses.
                let exponent = 1023 - 20 + ((word >> 52) % 35);
                f64::from_bits((exponent << 52) | (word & ((1_u64 << 52) - 1)))
            }),
        ),
        verify(
            "rounded_f32_confidences",
            (0..=10_000).map(|n| f64::from(n as f32 / 10_000.0_f32)),
        ),
        verify(
            "uniform_f32_probabilities",
            draws(83)
                .take(SAMPLES)
                .map(|word| f64::from((word >> 40) as f32 / 16_777_216.0_f32)),
        ),
        verify(
            "f32_probability_mantissas",
            draws(97).take(SAMPLES).map(|word| {
                let exponent = 110 + ((word >> 32) % 17) as u32;
                f64::from(f32::from_bits((exponent << 23) | (word as u32 & 0x7f_ffff)))
            }),
        ),
    ];

    let edges = [
        verify(
            "signed_extrema",
            [
                0.0,
                -0.0,
                f64::MAX,
                -f64::MAX,
                f64::MIN_POSITIVE,
                -f64::MIN_POSITIVE,
            ],
        ),
        verify(
            "subnormals",
            draws(101)
                .take(2_048)
                .flat_map(|word| {
                    let subnormal = f64::from_bits((word & ((1_u64 << 52) - 1)).max(1));
                    [subnormal, -subnormal]
                })
                .chain([f64::from_bits(1), -f64::from_bits(1)]),
        ),
        verify(
            "neighbors_of_powers_of_two",
            (1_u64..=2_046).flat_map(|exponent| {
                let power = exponent << 52;
                [power - 1, power, power + 1].into_iter().flat_map(|bits| {
                    let number = f64::from_bits(bits);
                    [number, -number]
                })
            }),
        ),
        verify(
            "long_decimals",
            draws(113).zip(draws(127)).take(2_048).map(|(a, b)| {
                let exponent = (b % 601) as i32 - 300;
                let sign = if a & 1 == 0 { "-" } else { "" };
                format!("{sign}{}.{a:020}{b:020}e{exponent}", 1 + b % 9)
                    .parse::<f64>()
                    .expect("finite decimal corpus value")
            }),
        ),
    ];

    assert_eq!(realistic.iter().sum::<usize>(), 124_689);
    assert_eq!(edges.iter().sum::<usize>(), 18_428);
}

#[test]
fn predecessor_misreads_pinned_timestamps_and_confidences() {
    for (original, previous) in [
        (0x3eff1e7cb2717395, 0x3eff1e7cb2717396),
        (0x3fbd916880000000, 0x3fbd91687fffffff),
        (0x3fcf08f380000000, 0x3fcf08f37fffffff),
        (0x3f1e8040c0000000, 0x3f1e8040bfffffff),
    ] {
        let number = f64::from_bits(original);
        let token = serde_json::to_string(&number).unwrap();
        let UniqueJsonValue(old) = serde_json::from_str(&token).unwrap();
        assert_eq!(old.as_f64().unwrap().to_bits(), previous);
        assert_ne!(
            payload_digest(&old).unwrap(),
            payload_digest(&Value::from(number)).unwrap()
        );
        verify("predecessor regression", [number]);
    }
}

#[test]
fn exponent_formatter_payload_digest_goldens() {
    for (value, token, expected) in [
        (
            1e16_f64,
            "1e+16",
            "33492c63fe0a487bf7e0fe6c44ae14d7cb3c66a89494536c90ed907b75ebae1a",
        ),
        (
            1e-20_f64,
            "1e-20",
            "fcaa4abc94a41de2b30f13eda0a3c57c207a0bc7d8322b0eaac49af17a47f888",
        ),
    ] {
        let payload = Value::from(value);
        assert_eq!(serde_json::to_string(&payload).unwrap(), token);
        assert_eq!(payload_digest(&payload).unwrap(), expected);
    }
}
