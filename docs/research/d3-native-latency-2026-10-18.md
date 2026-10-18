# Decision 3.0 native backend: latency and parity per size (2026-10-18)

Seed `audio-graph-60c7`. Design and runtime decision: `docs/designs/d3-native-runtime-2026-10-18.md`.
Code: `src-tauri/crates/decision-engine/src/d3/` (feature `decision-d3`).

## Setup

- AMD Ryzen 7 7700X (8 cores, 16 threads), 64-bit Windows 11 23H2 (WDDM), NVIDIA RTX 5090 32 GB,
  driver 610.88, CUDA 13.2, Rust 1.95.0, candle 0.10.2.
- Checkpoints at the revisions pinned in `D3_CHECKPOINTS` (`src-tauri/crates/decision-engine/src/lib.rs`).
- Requests go through `DecisionEngine::system_one`, so the time includes prompt rendering, tokenization,
  `spawn_blocking`, the forward pass, the readout and copying the probabilities to the host. One question
  per request, warm (3 untimed calls first).
  - short: fixture `card-support/route`, 128 tokens, 3 options.
  - long: fixture `long-meeting/topic`, 751 tokens, 4 options.
- No other GPU or build work ran during the timed runs.

Commands (CUDA build inside an MSVC developer shell with `NVCC_APPEND_FLAGS=-Xcompiler /Zc:preprocessor`):

```sh
cd src-tauri
cargo +1.95.0 build --release -p audio-graph-decision-engine --features decision-d3,cuda --example d3_bench
target/release/examples/d3_bench <models_dir> <d3-edge|d3-lite|d3-nano|d3-mini> cuda <f32|bf16> 50
target/release/examples/d3_bench <models_dir> <id> cpu f32 <5|10>
```

## Latency (ms, p50 / p95)

| Model | CUDA FP32 short | CUDA FP32 long | CUDA BF16 short | CUDA BF16 long | CPU FP32 short | CPU FP32 long |
| --- | --- | --- | --- | --- | --- | --- |
| d3-edge | 6.28 / 7.36 | 19.6 / 20.8 | 5.83 / 7.38 | 12.0 / 13.0 | 312 / 400 | 1431 / 1540 |
| d3-lite | 11.3 / 12.9 | 37.0 / 38.4 | 10.2 / 12.0 | 23.0 / 23.6 | 707 / 764 | 2929 / 3121 |
| d3-nano | 18.2 / 19.2 | 66.1 / 68.2 | 9.63 / 11.3 | 32.4 / 33.3 | 1361 / 1711 | 6073 / 6322 |
| d3-mini | 38.8 / 40.1 | 147 / 151 | 14.7 / 15.5 | 59.7 / 61.0 | 3188 / 3851 | 13855 / 14451 |

GPU rows: 50 timed calls. CPU rows: 10 calls (edge, lite) or 5 (nano, mini). Defaults the engine picks:
FP32 for edge and lite, BF16 for nano and mini on CUDA, FP32 on CPU.

First request after construction (model load from local disk plus one forward, CUDA): edge 1.2 to 1.8 s,
lite 1.4 to 2.1 s, nano 2.5 to 4.3 s, mini 4.5 to 6.4 s.

References:

- Model cards (one AMD Instinct MI325X, ROCm 7.2, AutoModel, text request): d3-edge 6.6 ms, d3-nano
  13.4 ms p50.
- Hugging Face reference on this machine (`model.system_one`, transformers 5.17.0, BF16, PyTorch Gated
  DeltaNet since flash-linear-attention is not installed): d3-edge short 32.7 / 42.7 ms, long 52.3 /
  59.4 ms; d3-nano short 71.5 / 80.8 ms, long 123.3 / 147.8 ms; d3-edge on CPU short 173 / 205 ms, long
  819 / 902 ms.

Short GPU requests are bound by kernel launches under WDDM (about 7 us host time per launch); the design
note has the Nsight Systems numbers. CPU runs FP32 through candle's `gemm` and is about 2x slower than
the torch BF16 reference on the same CPU (follow-up `audio-graph-1f5c`).

## Parity (same day, same machine)

`tests/d3_parity.rs` against the committed fixtures (20 questions per size are expressible in the typed
contract; the other 4 use JSON instructions or descriptions, or omit instructions):

```sh
AUDIOGRAPH_MODELS_DIR=<models_dir> cargo +1.95.0 test --release -p audio-graph-decision-engine \
  --features decision-d3 --test d3_parity -- --ignored
AUDIOGRAPH_MODELS_DIR=<models_dir> AUDIOGRAPH_D3_EP=cuda cargo +1.95.0 test --release \
  -p audio-graph-decision-engine --features decision-d3,cuda --test d3_parity -- --ignored --test-threads 1
```

| Model | Path | max abs diff vs FP32 reference | Picks |
| --- | --- | ---: | --- |
| d3-edge | CPU FP32 | 4.29e-6 | all match `system_one()` |
| d3-edge | CUDA FP32 | 2.25e-6 | all match |
| d3-edge | CUDA BF16 | 2.12e-2 | all with a top-two margin above 0.05 match |
| d3-lite | CUDA FP32 | 1.71e-6 | all match |
| d3-nano | CPU FP32 | 1.03e-6 | all match |
| d3-nano | CUDA FP32 | 3.12e-6 | all match |
| d3-nano | CUDA BF16 | 8.01e-3 | all with a top-two margin above 0.05 match |
| d3-mini | CUDA FP32 | 1.08e-6 | all match |

Usage tokens equal the fixture prompt lengths for every request.
