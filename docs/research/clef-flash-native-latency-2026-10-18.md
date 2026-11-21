# Clef-flash native backend: latency, memory and parity (2026-10-18)

Seed `audio-graph-4bc6`. Design: `docs/designs/clef-flash-native-2026-10-18.md`.
Code: `src-tauri/crates/decision-engine/src/clef/` (feature `decision-clef`).

## Setup

- AMD Ryzen 7 7700X, 64 GB RAM, Windows 11 (WDDM), RTX 5090 32 GB, driver 610.88, CUDA 13.2, Rust 1.95.0,
  candle 0.10.2. `Cloudflare/clef-flash@8b2e5fd17c09fd49bc2880b805d5323ec7ff4ff3`.
- Requests go through `DecisionEngine::system_one` (`ClefEngine` registered with `clef::register`), so the
  time includes prompt encoding, tokenization, `spawn_blocking`, the backbone forward pass, the joint head
  and building the answers. Warm: 3 untimed calls, then 30 timed.
  - short: fixture `support-ticket`, 3 questions, 335 tokens.
  - long: fixture `long-meeting`, 4 questions, 1,930 tokens.
- One forward pass answers every question of a request.

```sh
cd src-tauri   # MSVC developer shell, CUDA 13.2, NVCC_APPEND_FLAGS=-Xcompiler /Zc:preprocessor
cargo +1.95.0 build --locked --release -p audio-graph-decision-engine --features decision-clef,cuda --example clef_bench
target/release/examples/clef_bench <models_dir> cuda bf16 30
target/release/examples/clef_bench <models_dir> cuda f32 30
```

## Results (ms, p50 / p95)

| Path | short (3 q, 335 tok) | long (4 q, 1,930 tok) | Device memory | Load + first request |
| --- | --- | --- | ---: | ---: |
| Rust CUDA BF16 (default) | 57.6 / 60.0 | 279 / 284 | 14.2 GB | 20 to 24 s |
| Rust CUDA FP32 compute, BF16 weights | 176 / 179 | 769 / 785 | 15.1 GB | 23 s |
| Reference `systemone()` CUDA BF16 | 160 / 183 | 385 / 423 | 18.5 GB peak | |

Device memory is the in-process device-wide delta across load and the timed runs (`cuMemGetInfo`).
The reference figure is torch's peak allocation for `load_release_model` + `systemone`. Load time is
dominated by reading 19 GB of shards from local disk.

CPU was not measured: FP32 compute on the BF16 weights needs about 18 GB of RAM (14 GB of layers plus the
2 GB embedding and 2 GB `lm_head` tables on the host), and this machine had 14 to 20 GB available during
the session.

## Parity (same day, same machine)

```sh
AUDIOGRAPH_CLEF_DIR=<snapshot> AUDIOGRAPH_CLEF_EP=cuda-f32 cargo +1.95.0 test --locked --release \
  -p audio-graph-decision-engine --features decision-clef,cuda --test clef_parity -- --ignored
AUDIOGRAPH_CLEF_DIR=<snapshot> AUDIOGRAPH_CLEF_EP=cuda cargo +1.95.0 test --locked --release \
  -p audio-graph-decision-engine --features decision-clef,cuda --test clef_parity -- --ignored
```

| Mode | Reference | max abs diff | Picks |
| --- | --- | ---: | --- |
| CUDA FP32 compute | FP32 reference | 5.96e-7 | 23/23 |
| CUDA BF16 | published BF16 path | 6.44e-3 | 23/23 |

Token ids, question spans, option spans and option order equal `encode_record` on all 9 requests.
