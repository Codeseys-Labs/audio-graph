# d3-probe (Phase A scratch crate, audio-graph-60c7)

Standalone parity and latency probe for the Decision 3.0 native runtime. It is outside the `src-tauri`
workspace and is never shipped. Design note: `docs/designs/d3-native-runtime-2026-10-18.md`.

```sh
# CPU
cargo +1.95.0 build --release
target/release/d3-probe parity <hf-snapshot-dir> fixtures/d3-edge.json --device cpu --dtype f32
# CUDA (MSVC developer shell, CUDA 13.x for sm_120, NVCC_APPEND_FLAGS="-Xcompiler /Zc:preprocessor")
cargo +1.95.0 build --release --features cuda
target/release/d3-probe parity <hf-snapshot-dir> fixtures/d3-nano.json --device cuda --dtype f32
target/release/d3-probe bench  <hf-snapshot-dir> fixtures/d3-nano.json --device cuda --dtype bf16 --iters 50
```

`<hf-snapshot-dir>` is a `hf download vllm-sr/d3-edge` (or `d3-nano`) snapshot. The fixtures were made
once by `fixtures/gen_fixtures.py` from the Hugging Face reference (see its docstring); Python is not
used at run time.
