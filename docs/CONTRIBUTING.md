# Contributing to AudioGraph

Developer onboarding. If you're a user, start with the [README](../README.md)
instead.

---

## 1. Quick start

### Prerequisites

- **Bun ≥ 1.0** — we use Bun, not npm/pnpm/yarn. The `bun.lock` file is the
  source of truth. Install from <https://bun.sh>.
- **Rust toolchain** — pinned to `1.95.0` by `/rust-toolchain.toml` in the
  parent repo. Rustup picks this up automatically; don't override it with
  `+stable` or similar. `rustfmt` and `clippy` components are required
  (also listed in `rust-toolchain.toml`).
- **Platform dependencies for Tauri v2:**
  - **Linux:** GTK3, WebKit2GTK, libayatana-appindicator, librsvg, plus
    PipeWire headers for rsac audio capture:
    ```bash
    sudo add-apt-repository ppa:pipewire-debian/pipewire-upstream -y
    sudo apt-get install -y \
      libpipewire-0.3-dev libspa-0.2-dev \
      libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
      librsvg2-dev cmake clang libclang-dev pkg-config
    ```
  - **macOS:** Xcode Command Line Tools (`xcode-select --install`) plus
    CMake (`brew install cmake`). Metal GPU acceleration for Whisper and
    llama.cpp is enabled by default on macOS targets.
  - **Windows:** MSVC Build Tools (Visual Studio 2022 Build Tools with the
    "Desktop development with C++" workload), plus CMake and LLVM
    (`choco install cmake llvm -y`).

### Running in dev mode

```bash
cd apps/audio-graph
bun install
bun run tauri dev
```

The first `tauri dev` run compiles the Rust backend from scratch — expect
several minutes. Subsequent runs are incremental.

---

## 2. How the `rsac` path dep works

`src-tauri/Cargo.toml` pulls in the `rsac` audio-capture crate via a relative
path:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
rsac = { path = "../../../", features = ["feat_linux"] }
```

That `../../../` assumes the standard dev layout where `audio-graph` lives
at `rsac/apps/audio-graph/` inside the parent `rust-crossplat-audio-capture`
repo. `audio-graph` is a git submodule of that repo; clone the parent with
`--recurse-submodules` and you'll get the layout for free:

```bash
git clone --recurse-submodules \
  https://github.com/Codeseys-Labs/rust-crossplat-audio-capture.git
cd rust-crossplat-audio-capture/apps/audio-graph
```

If you're working in a standalone checkout of just `audio-graph/`, you'll
need to either (a) check the parent repo out one level up, or (b) swap the
path dep for a git dep:

```toml
rsac = { git = "https://github.com/Codeseys-Labs/rust-crossplat-audio-capture.git", tag = "v0.1.0", features = ["feat_linux"] }
```

CI uses approach (a) — see `.github/workflows/ci.yml` for the "Fetch rsac
parent" step that stages the parent repo at the expected path.

---

## 3. Repo layout

```
apps/audio-graph/
├── src/                    React + TypeScript + Vite frontend
│   ├── components/         UI components
│   ├── hooks/              React hooks (audio sources, graph, transcript, …)
│   ├── store/              Zustand stores
│   ├── i18n/               i18next locale resources
│   └── test/               Vitest setup + RTL tests
├── src-tauri/              Rust backend (Tauri v2)
│   ├── src/
│   │   ├── lib.rs          Tauri builder + command registration
│   │   ├── commands.rs     #[tauri::command] IPC wrappers
│   │   ├── events.rs       Event name constants + emit helper
│   │   ├── state.rs        AppState managed by Tauri
│   │   ├── audio/          Capture + pipeline plumbing
│   │   ├── asr/            Speech recognition providers
│   │   ├── diarization/    Speaker diarization
│   │   ├── gemini/         Gemini Live WebSocket client
│   │   ├── graph/          Knowledge graph + entity extraction
│   │   ├── llm/            Local + API LLM engines
│   │   ├── speech/         VAD + segment assembly
│   │   ├── settings/       Persisted user settings
│   │   ├── sessions/       Session persistence
│   │   ├── persistence/    File-based graph/transcript storage
│   │   ├── models/         Whisper/LLM model download + management
│   │   ├── credentials/    OS keyring integration
│   │   ├── aws_util/       AWS SDK helpers
│   │   ├── crash_handler/  Panic → user dialog bridge
│   │   └── logging/        Tracing setup
│   ├── Cargo.toml          Backend deps (rsac, whisper-rs, tauri, …)
│   └── tauri.conf.json     Tauri app config
├── docs/                   User + dev documentation
│   ├── ARCHITECTURE.md     System architecture deep dive
│   ├── RELEASE.md          Release process
│   ├── reviews/            Ongoing review + gap-analysis notes
│   └── designs/            Design docs for larger features
├── scripts/                Helper scripts (version bump, model download)
├── .github/workflows/      CI (ci.yml) and release (release.yml)
├── package.json            Frontend deps + Bun scripts
└── vite.config.ts          Vite config
```

---

## 4. Gates before pushing

Run **all** of these locally before pushing. CI runs the same set across
Linux / macOS / Windows — a PR that's green locally but flags something on
another OS is fine, but a PR that doesn't pass on your own box wastes
everyone's time.

### Frontend

```bash
cd apps/audio-graph
bun run typecheck        # tsc --noEmit
bun run test             # vitest run
bun run build            # tsc && vite build
```

### Backend

```bash
cd apps/audio-graph/src-tauri
cargo fmt --check        # hard gate — CI fails on unformatted code
cargo check              # cheap compile pass
cargo test --lib         # unit tests only; integration tests need models/devices
cargo audit              # advisory check — see .cargo/audit.toml for ignores
```

`cargo audit` is a hard gate in CI. If it flags a new advisory, either fix
the dep or add a justified ignore entry to `.cargo/audit.toml`. Don't
silently suppress.

Clippy is not currently gated in CI but is recommended:

```bash
cargo clippy --all-targets -- -D warnings
```

---

## 5. What CI runs

See `.github/workflows/ci.yml`. There are four jobs:

| Job | Runs | What |
|---|---|---|
| `frontend` | Ubuntu | `bun install`, `tsc --noEmit`, `vitest run`, `vite build` |
| `rust-linux` | Ubuntu | `cargo fmt --check`, `cargo check`, `cargo test`, `cargo audit` |
| `rust-macos` | macOS 15 | `cargo check`, `cargo test` |
| `rust-windows` | Windows 2025 | `cargo check`, `cargo test` |

All three Rust jobs stage the parent `rsac` repo into the expected relative
path before running cargo — see the "Fetch rsac parent" step. The parent
ref is pinned via `RSAC_REPO_REF` in the workflow env.

`cargo test` runs with `--test-threads=1` because several tests touch shared
audio state.

Release artifacts are built by `.github/workflows/release.yml` on tag push.
See [RELEASE.md](RELEASE.md).

---

## 6. Commit style

Match the existing style on `master`. Recent commits (`git log --oneline`)
show the pattern:

- One-line summary under 72 chars, descriptive, in the imperative.
- Explain **why**, not just **what** — the diff shows the what.
- If the commit touches a subsystem, mention it (`Windows audio CI:`,
  `Update audio-graph submodule:`, `Fix wasapi_session_test …`).
- In the body, note any gate results you ran (CI passes / tests green /
  clippy clean), especially for anything non-trivial.
- Don't skip hooks (`--no-verify`) unless you're fixing the hook itself.

Example from `master`:

```
Fix wasapi_session_test cross-platform build + update audio-graph submodule
```

---

## 7. Where to learn more

- [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) — full system architecture:
  pipeline stages, threading model, event flow, module boundaries.
- [`docs/RELEASE.md`](RELEASE.md) — how to cut a signed release.
- [`docs/reviews/gap-analysis.md`](reviews/gap-analysis.md) — the open work
  list, annotated with resolved / partial / open status. Good place to find
  a first issue.
- [`docs/SETTINGS_DESIGN.md`](SETTINGS_DESIGN.md) — settings persistence
  design.
- [`docs/MODEL_MANAGEMENT_DESIGN.md`](MODEL_MANAGEMENT_DESIGN.md) — Whisper
  / LLM model download + management.

---

## 8. How do I…

### …add a new ASR provider?

1. Add a new file under `src-tauri/src/asr/` (e.g. `myprovider.rs`) and
   re-export it in `src-tauri/src/asr/mod.rs` alongside the existing
   `deepgram`, `assemblyai`, `aws_transcribe`, `sherpa_streaming` modules.
2. Add a new variant to the `AsrProvider` enum in
   `src-tauri/src/settings/mod.rs` with `#[serde(rename = "my_provider")]`
   and any config fields. Pick defaults so existing `settings.json` files
   keep parsing.
3. Wire the new variant into the transcribe startup logic in
   `src-tauri/src/commands.rs` (`start_transcribe`) and
   `src-tauri/src/speech/`.
4. Add a frontend settings UI entry in `src/components/` and translations
   in `src/i18n/`.
5. Document the provider in the README's provider table.

### …add a new Tauri command?

1. Write an `#[tauri::command] pub async fn my_command(...)` in
   `src-tauri/src/commands.rs`. Return `Result<T, String>`.
2. Register it in the `tauri::generate_handler![...]` list in
   `src-tauri/src/lib.rs`.
3. Call it from the frontend with `@tauri-apps/api/core`'s `invoke`:
   ```ts
   import { invoke } from "@tauri-apps/api/core";
   const result = await invoke<MyType>("my_command", { arg: "value" });
   ```
   Argument names on the Rust side use `snake_case`; the frontend uses
   `camelCase` — Tauri bridges them.

### …add a new event?

1. Add a `pub const MY_EVENT: &str = "my-event";` to
   `src-tauri/src/events.rs`.
2. Emit it from wherever the source is, using the `emit_or_log` helper so
   failures surface in logs:
   ```rust
   use crate::events::{self, emit_or_log};
   emit_or_log(&app_handle, events::MY_EVENT, payload);
   ```
3. Subscribe on the frontend:
   ```ts
   import { listen } from "@tauri-apps/api/event";
   const unlisten = await listen<MyPayload>("my-event", (e) => { … });
   ```
4. Add a TypeScript type for the payload in `src/types/`.

### …run a single backend test?

```bash
cd apps/audio-graph/src-tauri
cargo test --lib path::to::module::test_name
# e.g.
cargo test --lib gemini::tests::build_setup_message_api_key
```

`--lib` restricts to the library target (skips integration tests under
`tests/`, if any). Drop `--lib` and pass a filter to run everything
matching that substring. `--test-threads=1` is set in CI for isolation;
locally you can usually leave the default.
