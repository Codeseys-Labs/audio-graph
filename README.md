# AudioGraph

> Live audio capture to speech recognition to temporal knowledge graph.

[![Rust](https://img.shields.io/badge/Rust-1.95%2B-orange)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/Tauri-v2-blue)](https://v2.tauri.app/)
[![React](https://img.shields.io/badge/React-18-61dafb)](https://react.dev/)
[![License](https://img.shields.io/badge/license-see%20root-green)](/LICENSE)

AudioGraph is a cross-platform desktop app (Tauri v2 + React) that taps system audio, runs it through a real-time pipeline of VAD, speech recognition, speaker diarization, entity extraction, and chat, and streams the results into a live temporal knowledge graph. Providers at every stage are swappable between local (Whisper, llama.cpp, Sherpa-ONNX) and cloud (Groq, OpenAI, AWS Transcribe/Bedrock, Deepgram, AssemblyAI, Gemini Live) so you can trade off latency, cost, and privacy to match your setup.

---

## Screenshots

> TODO: screenshot. No captured screenshots or GIFs exist yet under `docs/assets/`. Contributions welcome — record a short GIF of a live capture session (knowledge graph + transcript + chat sidebar) and drop it into `apps/audio-graph/docs/assets/`, then update this section.

---

## Prerequisites

| Requirement | Version | Notes |
|---|---|---|
| **Rust** | 1.95+ | Pinned in [`rust-toolchain.toml`](src-tauri/rust-toolchain.toml). Install via [rustup](https://rustup.rs/). |
| **Bun** | latest | Used for frontend install + scripts. Prefer `bun` over `npm` in this repo. Install: `curl -fsSL https://bun.sh/install \| bash` (macOS/Linux) or `powershell -c "irm bun.sh/install.ps1 \| iex"` (Windows). |
| **CMake** | any recent | Required by `whisper-rs` and `llama-cpp-2` build scripts. |
| **C++ toolchain** | C++17 | clang 10+ or gcc 9+ (Linux/macOS); MSVC via VS Build Tools 2022 "Desktop development with C++" workload (Windows). |
| **clang / libclang** | 10+ | Required by `bindgen` for FFI. |

### Platform-specific libraries

- **Linux (Debian/Ubuntu):**
  ```bash
  sudo apt install build-essential cmake clang libclang-dev \
      libpipewire-0.3-dev libspa-0.2-dev \
      libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
  ```
- **macOS:** `xcode-select --install` then `brew install cmake`. Application-level capture requires macOS 14.4+ (Process Tap API).
- **Windows:** Install VS Build Tools 2022 (Desktop C++ workload), CMake, and LLVM via `winget` (see [Setup](#setup) section for commands).

For build/capture issues, see the rsac [troubleshooting guide](../../docs/troubleshooting.md).

---

## Quick start

```bash
# 1. Clone (with submodules if you haven't already)
git clone https://github.com/user/rust-crossplat-audio-capture.git
cd rust-crossplat-audio-capture/apps/audio-graph

# 2. Install frontend dependencies (use bun, not npm)
bun install

# 3. Download ML models (Whisper + optional extraction LLM)
./scripts/download-models.sh         # macOS / Linux
# .\scripts\download-models.ps1      # Windows PowerShell
# Or skip — models can be pulled in-app via the model manager.

# 4. Run in development mode (Tauri window + hot-reload)
bun run tauri dev
```

The canonical dev command is **`bun run tauri dev`** — this launches the Tauri shell with Vite hot-reload for the React frontend and `cargo`-rebuilds the Rust backend on change. `bun run dev` runs the Vite frontend only (no Tauri window) and is rarely what you want.

First-run workflow: pick an audio source from the dropdown, click **Start**, and watch the knowledge graph build as you speak or play audio.

---

## Configuration

### Credentials (API keys)

Cloud provider API keys are stored in a user-level config file, **not** checked into the repo:

```
~/.config/audio-graph/credentials.yaml
```

Keys for Groq, OpenAI, Deepgram, AssemblyAI, AWS (access key + secret or profile name), Gemini (API key or Vertex AI service account), etc. live here. You can edit the file directly or use the in-app **Settings** page, which reads and writes the same file.

### Gemini Live reconnect / debugging

If Gemini Live drops its WebSocket, disconnects mid-session, or fails to reconnect, follow the [Gemini reconnect runbook](docs/ops/gemini-reconnect-runbook.md). It covers the `gemini-reconnect`, `gemini-connection-state`, and `gemini-session-stats` events, backoff behavior, and the manual recovery flow.

### Pipeline config

Pipeline defaults (sample rate, VAD thresholds, ASR model path, graph parameters) are specified in [`src-tauri/config/default.toml`](src-tauri/config/default.toml). Note: runtime loading from this file is still on the roadmap — current builds use hardcoded defaults matching the spec.

### Model paths

| Model | Purpose | Size | Location |
|---|---|---|---|
| `ggml-small.en.bin` | Whisper ASR | ~500 MB | `apps/audio-graph/models/` |
| `lfm2-350m-extract-q4_k_m.gguf` | Entity extraction + chat | ~350 MB | `apps/audio-graph/models/` |
| Silero VAD v5 | Voice activity detection | ~2 MB | Auto-downloaded on first run |

---

## Platform support matrix

| Capture mode | Windows (WASAPI) | macOS (CoreAudio) | Linux (PipeWire) |
|---|---|---|---|
| System default | Yes | Yes | Yes |
| Specific device | Yes | Yes | Yes |
| Application (by PID) | Yes (process loopback) | Yes (Process Tap, macOS 14.4+) | Yes (pw-dump node) |
| Application (by name) | Yes (sysinfo → PID) | Yes (Process Tap, macOS 14.4+) | Yes (pw-dump → node serial) |
| Process tree | Yes | Yes (Process Tap, macOS 14.4+) | Yes |

| GPU acceleration | How to enable |
|---|---|
| macOS Metal | Automatic (default) |
| NVIDIA CUDA (Win/Linux) | `cargo build --features cuda` |
| Vulkan (Win/Linux, AMD/NVIDIA/Intel) | `cargo build --features vulkan` |
| CPU only | Default — no flags |

### Provider support at a glance

- **ASR:** local Whisper, local Sherpa-ONNX (Zipformer, behind `sherpa-streaming` feature flag), Groq/OpenAI, AWS Transcribe, Deepgram, AssemblyAI.
- **LLM (extraction + chat):** local llama.cpp, local Mistral.rs (Candle), OpenAI-compatible HTTP (OpenAI, OpenRouter, Ollama, LM Studio, vLLM, Together, Groq), AWS Bedrock.
- **Gemini Live:** AI Studio API key or Vertex AI service account.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and [`docs/designs/provider-architecture.md`](docs/designs/provider-architecture.md) for the full provider matrix and decision tree.

---

## Setup (detailed, per-platform)

<details>
<summary><b>Windows — step by step</b></summary>

```powershell
winget install Rustlang.Rustup
winget install Microsoft.VisualStudio.2022.BuildTools   # select "Desktop development with C++"
winget install Kitware.CMake
winget install LLVM.LLVM
powershell -c "irm bun.sh/install.ps1 | iex"

git clone https://github.com/user/rust-crossplat-audio-capture.git
cd rust-crossplat-audio-capture\apps\audio-graph
bun install
.\scripts\download-models.ps1
bun run tauri dev
```

For NVIDIA GPU acceleration: `cd src-tauri && cargo build --features cuda`.

</details>

<details>
<summary><b>macOS — step by step</b></summary>

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
xcode-select --install
brew install cmake
curl -fsSL https://bun.sh/install | bash

git clone https://github.com/user/rust-crossplat-audio-capture.git
cd rust-crossplat-audio-capture/apps/audio-graph
bun install
./scripts/download-models.sh
bun run tauri dev
```

Grant microphone permission when macOS prompts. Application-specific capture needs macOS 14.4+ (Sonoma).

</details>

<details>
<summary><b>Linux (Debian/Ubuntu) — step by step</b></summary>

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
sudo apt install build-essential cmake clang libclang-dev \
    libpipewire-0.3-dev libspa-0.2-dev \
    libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
curl -fsSL https://bun.sh/install | bash

git clone https://github.com/user/rust-crossplat-audio-capture.git
cd rust-crossplat-audio-capture/apps/audio-graph
bun install
./scripts/download-models.sh
bun run tauri dev
```

</details>

---

## Development

```bash
bun run tauri dev         # dev mode: Tauri window + hot-reload frontend + cargo rebuild
bun run tauri build       # production bundle (installer / .app / .deb)
bun run dev               # frontend only (no Tauri window)
bun run typecheck         # tsc --noEmit
bun run test              # vitest frontend tests

cd src-tauri && cargo check
cd src-tauri && cargo test
cd src-tauri && cargo clippy --all-targets -- -D warnings
```

GPU-accelerated builds:

```bash
cd apps/audio-graph/src-tauri && cargo build --features cuda       # NVIDIA CUDA 11.7+
cd apps/audio-graph/src-tauri && cargo build --features vulkan     # Vulkan SDK
# macOS Metal: automatic, no flag needed
```

---

## Documentation

The [`docs/`](docs/) directory is organized by purpose:

- **[`docs/README.md`](docs/README.md)** — documentation index (start here).
- **[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)** — full architecture overview (4-thread pipeline, event model, provider abstraction).
- **[`docs/designs/`](docs/designs/)** — design proposals (provider architecture, provider refactor, session management).
- **[`docs/ops/`](docs/ops/)** — operational runbooks ([Gemini reconnect runbook](docs/ops/gemini-reconnect-runbook.md)).
- **[`docs/reviews/`](docs/reviews/)** — loop-by-loop code review notes, gap analyses, and UX first-run review.
- **[`docs/RELEASE.md`](docs/RELEASE.md)** — release process.
- **[`docs/MODEL_MANAGEMENT_DESIGN.md`](docs/MODEL_MANAGEMENT_DESIGN.md)** — model download + management.
- **[`docs/SETTINGS_DESIGN.md`](docs/SETTINGS_DESIGN.md)** — settings page architecture.
- **[`docs/GEMINI_LANGUAGES.md`](docs/GEMINI_LANGUAGES.md)** — Gemini Live language support.
- **[`docs/SYSTEM_TRAY_WIDGET_PROPOSAL.md`](docs/SYSTEM_TRAY_WIDGET_PROPOSAL.md)** — tray widget proposal.

---

## Contributing

See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) for branch workflow, commit conventions, code review expectations, and the pre-submit checklist.

---

## License

Part of the [`rsac`](/) (Rust Cross-Platform Audio Capture) project. See the root [LICENSE](/LICENSE) for details.
