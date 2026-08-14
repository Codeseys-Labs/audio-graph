# Contributing to AudioGraph

Developer onboarding. If you're a user, start with the [README](../README.md)
instead.

---

## 1. Quick start

### Prerequisites

- **Bun ≥ 1.0** — we use Bun, not npm/pnpm/yarn. The `bun.lock` file is the
  source of truth. Install from <https://bun.sh>.
- **Rust toolchain** — pinned to `1.95.0` by
  [`src-tauri/rust-toolchain.toml`](../src-tauri/rust-toolchain.toml).
  Rustup picks this up automatically when commands run from `src-tauri/`;
  don't override it with
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
cd audio-graph
bun install
bun run prepare:seeds-json-output
bun run tauri dev -- --locked
```

The first `tauri dev` run compiles the Rust backend from scratch — expect
several minutes. Subsequent runs are incremental.

`prepare:seeds-json-output` patches the repo-pinned `@os-eco/seeds-cli`
dependency so large `sd --format json` responses survive direct pipes on every
platform. Use `bun run check:seeds-json-output` when validating a checkout
without mutating the installed package.

Seeds JSON commands return an envelope shaped like
`{ success, command, issues, count }`. Parse issue rows from `.issues`, or use
`bun run sd:issues -- ready` / `blocked` / `list --all` when a pipeline needs a
plain JSON array of issues.

---

## 2. How the `rsac` pin works

`src-tauri/Cargo.toml` pulls rsac v0.4.4 from the official Git repository at a
full revision, with platform-specific features and default features disabled:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
rsac = { git = "https://github.com/Codeseys-Labs/rust-crossplat-audio-capture.git", rev = "ea2019bba217cab695d45696bc2ca25430b23dc2", default-features = false, features = ["feat_linux"] }
```

The manifest plus `src-tauri/Cargo.lock` is the only repository/release source
of truth. A sibling checkout is not required and is never selected implicitly.
Run Cargo with Rust 1.95 and `--locked` so a local build cannot float to another
rsac commit.

If you are deliberately developing rsac and AudioGraph together, use the
explicit, gitignored `.cargo/rsac-local.toml` override documented in the
README's Releasing section. Pass it with `cargo --config`; do not edit the
tracked manifest or commit the override or a lockfile generated while it is
active.

---

## 3. Repo layout

```
audio-graph/
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
│   │   ├── credentials/    OS keychain + legacy file credential backend
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

Run all applicable local gates before pushing. CI adds platform, packaging,
security, storage-engine, and live-audio cells that a single workstation cannot
claim; local green is necessary evidence, not a substitute for that matrix.

### Frontend

```bash
cd audio-graph
bun install --frozen-lockfile
bun run typecheck        # tsc --noEmit
bun run check            # workspace Biome gate
bun run verify:fast      # static, generated-contract, Seed, secret, diff gates
bun run test:local       # authoritative serial Vitest gate on this checkout
bun run test:focused -- src/components/MyComponent.test.tsx
bun run build            # tsc && vite build
```

`test:local` and `test:focused` run Vitest with one worker and disable Node's
experimental global Web Storage for the child process so JSDOM owns
`localStorage`. The cross-platform launcher preserves existing `NODE_OPTIONS`,
does not retry failures, and forwards Vitest's exit status.

### Backend

```bash
cd audio-graph
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --locked
cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --locked -- --test-threads=1
cargo +1.95.0 clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
cargo +1.95.0 audit --file src-tauri/Cargo.lock
```

On Windows library/test builds, set
`AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1` before Cargo so the executable under
test uses the repository's supported CRT manifest. Cloud-only focused gates use
`--no-default-features --features cloud` and must still use `--locked`.

`cargo audit` is a hard gate in CI. If it flags a new advisory, either fix
the dep or add a justified ignore entry to `.cargo/audit.toml`. Don't
silently suppress.

---

## 5. What CI runs

See `.github/workflows/ci.yml`; it is authoritative and currently contains
twelve jobs covering lint/static validation, frontend, security audit, Linux,
cloud and optional Rust features, Windows debug CRT, default Tauri packaging,
macOS, Windows, storage-engine evidence, and approval-gated live audio.

Cargo pins rsac v0.4.4 directly. CI verifies the exact package identity through
`cargo metadata --locked`; there is no sibling checkout or separately
configurable rsac SHA.

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
cd audio-graph/src-tauri
cargo +1.95.0 test --locked --lib path::to::module::test_name
# e.g.
cargo +1.95.0 test --locked --lib gemini::tests::build_setup_message_api_key
```

`--lib` restricts to the library target (skips integration tests under
`tests/`, if any). Drop `--lib` and pass a filter to run everything
matching that substring. Use `-- --test-threads=1` for tests that share process
state; the authoritative full local gate already serializes both Rust and
frontend execution.

---

## 9. Working with Seeds (the issue backlog)

The backlog lives in `.seeds/issues.jsonl` and is managed by the repo-pinned
`@os-eco/seeds-cli`. Each issue is one JSON object per line with an
`audio-graph-XXXX` `id`, a `title`, `status` (`open` / `closed`), `type`,
`priority`, and — when closed — a `closedAt` timestamp plus a free-text
`closeReason`. See §1 for the JSON-output envelope and the `bun run sd:*`
helpers.

### 9.1 Closing a duplicate Seed — canonical-ID convention

It's common for two Seeds to describe the same work (e.g. created independently
during a roadmap or audit sweep). When that happens, **keep one Seed open as the
canonical record and close the other(s) as duplicates.** The convention is:

> **A duplicate closure MUST name the canonical Seed's `id` in its
> `closeReason`.** Use the exact form `canonical linked Seed is
> audio-graph-XXXX` (or `canonical Seed: audio-graph-XXXX`) so the link is
> greppable and unambiguous.

This keeps the backlog navigable: a reader (or a duplicate-title audit) who lands
on the closed duplicate can follow the close reason straight to the surviving
canonical Seed, rather than guessing which of two same-titled issues is live.

Rules of thumb:

- **Canonical = the one that stays open.** Prefer keeping the Seed with the
  richer history (more linked work, an active assignee, or the lower-churn ID);
  close the redundant one.
- **Never delete a duplicate** — close it, so the audit trail (who flagged it,
  when, and the canonical pointer) survives.
- **Name the ID, not just "duplicate."** A bare `closeReason: "duplicate"`
  forces the next reader to hunt for the original. Always include the
  `audio-graph-XXXX` of the survivor.

### 9.2 Verified canonical example

The convention is already exercised in the live backlog. Two Seeds shared the
title *"Calendar and prior-context pre-briefs from the temporal graph"*:

- `audio-graph-53cf` — **open**, the canonical Seed.
- `audio-graph-67f9` — **closed** as a duplicate, with
  `closeReason: "Duplicate created during competitive roadmap seed creation;
  canonical linked Seed is audio-graph-53cf."`

Note that the duplicate (`67f9`) is the one closed and the close reason points
forward to the surviving canonical Seed (`53cf`) by ID — exactly the pattern new
duplicate closures should follow.

### 9.3 Finding duplicates

A quick duplicate-title scan over the backlog. `bun run sd:issues` prints a
plain JSON array of issue rows (it unwraps the `.issues` field from the Seeds
envelope — see §1), so the scan reads that array directly:

```bash
bun run sd:issues -- list --all \
  | python3 -c "import sys,json,collections; \
rows=json.load(sys.stdin); \
t=collections.Counter(r['title'] for r in rows); \
[print(k) for k,v in t.items() if v>1]"
```

When a scan turns up two same-titled Seeds, apply §9.1: pick the canonical,
close the other with the canonical ID in its `closeReason`.
