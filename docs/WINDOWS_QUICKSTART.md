# Windows Quickstart — run the app and start with cloud API keys

This is the fastest path to a running AudioGraph on Windows using **cloud
providers only** (no local ML model downloads required):

- **STT (speech-to-text):** Deepgram
- **LLM (chat + entity extraction):** OpenRouter
- **TTS (speak-aloud):** Deepgram Aura (reuses the same Deepgram key)

With these three, you do **not** need to download Whisper (`ggml-*.bin`) or the
LFM2 extraction model (`*.gguf`). Those are only required for the fully-local
ASR/LLM paths.

---

## 1. Build the executable

There is no pre-signed installer yet (Authenticode signing is still a blocked
backlog item — see `docs/backlog/pipeline-modernization.md` AG-P4-005). You
build the binary locally once.

### One-time prerequisites

```powershell
winget install Rustlang.Rustup
winget install Microsoft.VisualStudio.2022.BuildTools   # select "Desktop development with C++"
winget install Kitware.CMake
winget install LLVM.LLVM
powershell -c "irm bun.sh/install.ps1 | iex"
```

CMake + the MSVC C++ toolchain + LLVM/clang are required because the build
compiles the bundled native ML libraries (whisper.cpp, llama.cpp, mistral.rs)
from source. The pinned Rust toolchain (`1.95.0`) is installed automatically by
rustup on first build via `src-tauri/rust-toolchain.toml`.

### Clone audio-graph + the rsac sibling

AudioGraph consumes the `rsac` audio-capture library as a **path dependency**
(`src-tauri/Cargo.toml` points at `../../rsac`). The two repos must sit side by
side under the same parent directory:

```
<parent>\
  audio-graph\
  rsac\
```

```powershell
git clone https://github.com/Codeseys-Labs/audio-graph.git
cd audio-graph
git clone https://github.com/Codeseys-Labs/rust-crossplat-audio-capture.git ..\rsac
bun install
```

### Build

**Build in release mode — this is the version you actually run.**

```powershell
# Optimized standalone .exe (recommended for running):
bun run tauri build --no-bundle
# -> src-tauri\target\release\audio-graph.exe   (~82 MB)

# Optimized .exe + installers (slower; needs NSIS/WiX bundlers):
bun run tauri build
# -> src-tauri\target\release\bundle\...
```

> **Do NOT run the `--debug` build.** A debug build
> (`tauri build --debug` / `tauri dev`) links the MSVC **debug CRT** (`/MDd`),
> but the bundled native ML libraries (whisper.cpp, llama.cpp, mistral.rs) are
> compiled by `cc`/CMake against the **release CRT** (`/MD`). At runtime the
> debug CRT's `_CrtIsValidHeapPointer` check fires a "Debug Assertion Failed"
> dialog (`debug_heap.cpp` line 904) when a buffer allocated in one CRT is
> freed by the other. This is a **debug-only** artifact — the release build
> shares one CRT and does not trip it. Use debug only with a debugger attached
> and "Ignore" the assertion, or prefer release. (Tracked: seeds issue for a
> proper debug-CRT fix.)

The first release build takes ~13-15 min (it compiles whisper.cpp / llama.cpp /
mistral.rs with optimizations). Subsequent builds are incremental.

> Want a much faster cloud-only build? Gating the local ML crates behind cargo
> feature flags is a tracked backlog item (see the deep-work log entry for
> 2026-05-28). Until that lands, the native ML crates always compile.

---

## 2. Launch + enter your API keys

Run `src-tauri\target\release\audio-graph.exe` (double-click, or launch from a
terminal to see logs).

On first launch — with no credentials found — the app shows the **Express
Setup** wizard. You can also reach the same fields later via the **Settings**
page.

| Provider | Key format | Where to get it |
|---|---|---|
| Deepgram (STT + Aura TTS) | `dg-...` (a raw token is fine) | https://console.deepgram.com/ |
| OpenRouter (LLM) | `sk-or-...` | https://openrouter.ai/keys |

Keys are written to (Windows):

```
%APPDATA%\audio-graph\credentials.yaml
```

(owner-only permissions, written atomically, zeroized in memory). You can edit
that file directly, but using the in-app Settings page is recommended.

Each provider panel has a **Test connection** button:
- Deepgram STT — Settings → ASR provider → Deepgram → *Test*
- OpenRouter — Settings → LLM provider → OpenRouter → *Test* (then pick a model)
- Aura TTS — Settings → Text-to-speech → `deepgram_aura` → *Test*

---

## 3. Run a session (cloud-only)

1. **Pick a source** in the left panel: System, a specific Device, an
   Application (by name/PID), a Process, or a Process tree.
2. Click **Start** — this begins audio capture from that source.
3. Click **Transcribe** — with Deepgram selected this is fully cloud; it needs
   only your Deepgram key (no model download).
4. The transcript stream, the temporal knowledge graph, and per-stage latency
   appear as audio flows.
5. Use the **chat sidebar** to ask questions — replies stream from OpenRouter.
6. Enable **Speak aloud** (Settings → Text-to-speech) to hear replies via Aura.

> "Start" = capture. "Transcribe" (or "Gemini") = the processing path that
> actually produces the graph + chat. Capture alone is silent until you start
> one of those.

---

## 4. What still needs local models (optional)

| Feature | Model | Needed only if |
|---|---|---|
| Local ASR | `ggml-small.en.bin` (Whisper) | ASR provider = Local Whisper |
| Local streaming ASR | Sherpa Zipformer | ASR provider = Sherpa (feature-gated) |
| Local LLM / extraction | `lfm2-350m-extract-q4_k_m.gguf` | LLM provider = Local llama / mistral.rs |
| Speaker diarization | Sortformer ONNX | Optional; missing => "Simple" fallback |

Download them in-app via the model badges in each Settings provider panel, or
with `scripts\download-models.ps1`. None are required for the
Deepgram + OpenRouter + Aura cloud path.

---

## 4b. Verify it works (test scripts)

Two PowerShell scripts validate the moving parts in isolation from the GUI.
Neither contains secrets — keys are read from environment variables or from
`%APPDATA%\audio-graph\credentials.yaml`.

**Cloud pipeline (Deepgram STT + OpenRouter LLM):**

```powershell
pwsh scripts/test-cloud-pipeline.ps1
# [1/2] Deepgram STT   -> OK transcript: "..."
# [2/2] OpenRouter LLM -> OK reply: "PIPELINE_OK"
```

**rsac audio capture (devices, formats, loopback record):**

```powershell
pwsh scripts/test-rsac-windows.ps1
# info   -> platform capabilities
# list   -> every device + the exact formats it advertises
# record -> captures system loopback to a WAV (plays a sound so there's signal)
```

> **Capture-format note (Windows):** devices advertise fixed formats — a
> virtual surround endpoint may only offer `8ch/96000`, a USB mic only
> `1ch/48000`. AudioGraph and rsac now **negotiate** to a format the device
> actually supports (the pipeline resamples to 16 kHz mono downstream), so you
> won't hit "Unsupported audio format" anymore. If you capture your default
> output for loopback, remember WASAPI only delivers audio **while something is
> playing** — silence produces zero frames, which is expected.

---

## 5. Troubleshooting

- **Sharing logs as feedback** — file logging is on by default. Find the log at
  `%APPDATA%\audio-graph\logs\audio-graph.log` (Settings → Logging →
  *Open logs folder*). Each launch archives the previous log to
  `audio-graph-<timestamp>.log` and starts a fresh one; Settings → Logging lets
  you switch to *overwrite*, change the level, or *purge archived logs*. Attach
  the active log when reporting an issue.

- **"Debug Assertion Failed! `_CrtIsValidHeapPointer(block)`" dialog on
  launch** — you ran a `--debug` build. Rebuild in release
  (`bun run tauri build --no-bundle`) and run
  `src-tauri\target\release\audio-graph.exe`. See the build note above for why.
- **`Found version mismatched Tauri packages`** — the npm `@tauri-apps/*`
  packages must match the Rust `tauri` crate's minor version. Run
  `bun add @tauri-apps/api@^2.11.0 && bun add -d @tauri-apps/cli@^2.11.0`
  (or whatever minor the Rust crate resolved to) and rebuild.
- **Linker / `link.exe` errors** — make sure the MSVC "Desktop development with
  C++" workload is installed; cargo locates MSVC's linker automatically when VS
  Build Tools are present.
- **`could not find rsac`** — the `rsac` repo must be cloned as a sibling of
  `audio-graph` (see step 1).
- **Transcribe button does nothing / errors** — confirm the Deepgram key is set
  and the *Test connection* button is green.
