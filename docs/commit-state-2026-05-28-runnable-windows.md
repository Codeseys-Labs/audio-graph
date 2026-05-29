# Commit State — 2026-05-28 — Runnable Windows build

**Baseline HEAD at start of this loop:** `480c6e4` (`ci: grant contents:write to
release.yml jobs`).

## Goal for this loop

The operative, user-stated north star (appended to the standing pipeline-
modernization prompt):

> "lets get to a point where I can run the executable on windows (current
> system) and input my api keys and start getting stuff working"

This reframes the grand pipeline backlog: the binding constraint is a
**runnable Windows executable** driven by **cloud API keys** (Deepgram STT +
OpenRouter LLM + Deepgram Aura TTS), not the long-tail S2S epics.

## Environment verified (this Windows machine)

| Tool | Version | Status |
|---|---|---|
| rustc/cargo | 1.94.0 default; **1.95.0** auto-installed via toolchain pin | OK |
| MSVC | VS 18 Community + VC x64 tools | OK |
| CMake | 4.2.3 | OK |
| clang/LLVM | 22.1.1 | OK |
| bun | 1.3.11 | OK |
| node | 24.15.0 | OK |
| rsac sibling | present at `E:\CS\github\rsac` (path dep) | OK |

## Build results

- `cargo check` (src-tauri): **PASS** in ~5 min. Toolchain 1.95.0 auto-resolved;
  rsac + whisper.cpp + llama.cpp + mistral.rs all compiled. 7 trivial warnings
  (now fixed — see below).
- Frontend `tsc --noEmit`: **PASS**.
- `vitest run`: **581 tests PASS** (72 files).
- `bun run tauri build --debug --no-bundle`: **PASS** in ~11.5 min.
  Produced standalone **`src-tauri\target\debug\audio-graph.exe`** (179.8 MB,
  valid PE, Vite frontend embedded).

## Changes landed in this commit

1. **Tauri version alignment** (`package.json`, `bun.lock`): bumped
   `@tauri-apps/api` 2.10.1 → 2.11.0 and `@tauri-apps/cli` → 2.11.2 to match the
   Rust `tauri` crate (2.11.2). The CLI's mismatch guard was blocking
   `tauri build` entirely.
2. **Warning cleanup → zero warnings** (`cargo check` clean):
   - `speak_aloud.rs`: removed unused `use std::sync::Arc;`.
   - `tts/deepgram_aura.rs`: `#[allow(dead_code)]` + rationale on
     `DisconnectKind` (String payloads are Debug-only diagnostics).
   - `playback/mod.rs`: `#[allow(deprecated)]` + rationale on the two
     `cpal::DeviceTrait::name()` call sites (name-based device matching is still
     required until the device-identity path migrates to `id()`).
3. **README**: corrected the Windows credentials path
   (`%APPDATA%\audio-graph\credentials.yaml`, not `~/.config/...`).
4. **New `docs/WINDOWS_QUICKSTART.md`**: end-to-end run + cloud-key workflow.

## Cloud-only path confirmation (no local models required)

Verified via code trace (see deep-work-log 2026-05-28):
- Deepgram STT pre-flight only checks the key is non-empty — no model download.
- OpenRouter chat streams natively (Api/OpenRouter are the streaming-enabled
  providers).
- Aura TTS reuses the Deepgram key.
- First-run `ExpressSetup` wizard collects the keys; `save_credential_cmd`
  persists them; per-provider *Test connection* commands exist.

A brand-new user can launch the exe, paste a Deepgram key + an OpenRouter key,
pick a source, Start, Transcribe, and get a transcript + streaming chat reply
with **zero** `ggml`/`gguf` downloads.
