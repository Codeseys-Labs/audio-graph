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

### Reusable Rust build lanes

For local compile-bearing checks and tests—especially in parallel worktrees—use
the Bun tasks instead of starting uncoordinated Cargo processes:

```bash
bun run rust:check:cloud
bun run rust:test:cloud -- projection_test       # optional literal filter
bun run rust:check:full                          # exclusive/default features
bun run rust:test:full -- credential_service     # exclusive/default features
```

The cloud tasks are the fast iterative convention. They run Cargo `+1.95.0`
with `--locked -p audio-graph --lib --no-default-features --features cloud`;
test tasks also keep `--test-threads=1`. A test filter must be one literal
argument and cannot begin with `-`, so it cannot silently change the Cargo
feature set or profile. A registered child wrapper uses direct process arguments
with no shell interpolation. `rust:check:full` is the final default-feature
check and includes `--all-targets`.

Stable targets live under this ignored default base:

```text
src-tauri/target/cargo-lanes/
  worktree-<canonical-path-hash>/
    features-cloud/profile-debug/
    features-default/profile-debug/
```

`AUDIO_GRAPH_CARGO_TARGET_ROOT` overrides only the base; the worktree,
feature-set, and profile suffix remains mandatory. The default host budget is
`min(6, detected CPUs)`. Shared invocations that arrive in the same 100 ms
admission batch split it evenly. With a six-token budget, one build runs with
`--jobs 6`, two run with three jobs each, and three run with two jobs each. A
running Cargo process keeps its allocation, so an invocation arriving after a
batch has started waits if the remaining tokens cannot satisfy its own batch.

Set `AUDIO_GRAPH_CARGO_BUDGET` to a lower host-wide budget or
`AUDIO_GRAPH_CARGO_JOBS` to pin one invocation to a fixed allocation.
`AUDIO_GRAPH_CARGO_ADAPTIVE_WINDOW_MS` changes the shared admission window and
should have the same value for all cooperating workers. The budget may never
exceed detected CPUs, jobs may never exceed the budget, and active workers use
one admitted budget at a time. If the requested budget changes, the admission
lock waits for all current Cargo leases to finish and then updates the existing
pool in place; do not delete the coordination directory.

Coordination defaults to an AudioGraph token pool under the OS temporary
directory. `AUDIO_GRAPH_CARGO_COORDINATION_DIR` is available for deterministic
testing or an intentionally separate host pool; using different values for
ordinary workers bypasses mutual coordination and is unsafe. Leases heartbeat
with path/content-free PID metadata. On POSIX hosts, a detached wrapper first
persists its process-group identity into every acquired token and only then may
start Cargo. Normal exit and interruption audit that full process group before
releasing tokens. If cleanup cannot prove the group dead, the facade returns an
error and retains the leases; stale recovery still refuses to reclaim them
while the group is alive. The facade's own ordinary logs omit worktree paths and
test content; Cargo diagnostics still pass through unchanged.

The coordinated tasks currently refuse to run on Windows with
`windows_descendant_ownership_unavailable`. Windows enablement is blocked on an
auditable Job Object (or equivalent descendant-ownership strategy) plus
target-native evidence that interruption, descendant cleanup, and stale recovery
all preserve the host budget. Until then, do not use these tasks—or raw Cargo as
a substitute—for parallel local Windows builds.

There is exactly one fresh-target operation:

```bash
bun run rust:check:clean-room
```

It waits for exclusive access, creates one default-feature/debug target with
`mkdtemp` under the OS temp directory (or `AUDIO_GRAPH_CARGO_TEMP_ROOT`), prints
the resulting path, and never deletes it. Reserve this opt-in command for one
final clean-room proof. Re-running it creates another large target and is not
the normal verification path.

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

`src-tauri/Cargo.toml` pins every target-specific `rsac` dependency to the same
full Git commit and enables only that platform's desktop feature:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
rsac = { git = "https://github.com/Codeseys-Labs/rust-crossplat-audio-capture.git", rev = "ea2019bba217cab695d45696bc2ca25430b23dc2", default-features = false, features = ["feat_linux"] }
```

The application lockfile records the same resolved commit, and normal checks
must use `--locked`:

```bash
cargo metadata --manifest-path src-tauri/Cargo.toml --format-version 1 --locked
bun run rust:check:full  # POSIX; coordinated, locked full-feature check
```

If you are changing both repositories, keep the tracked manifest untouched and
use the explicit, gitignored `.cargo/rsac-local.toml` override documented in
the README's Releasing section. Pass it with `cargo --config`; do not commit the
override or a lockfile generated while it is active.

CI and release resolve rsac directly through Cargo; there is no sibling checkout
or separately configurable workflow SHA.

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
│   │   ├── credentials/    credentials.yaml management
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

Run every gate that is supported on your development host before pushing. The
coordinated compile-bearing Rust tasks are currently POSIX-only; Windows
contributors must run the frontend and non-compiling backend gates locally and
obtain the four Rust task results from a clean POSIX checkout or CI before
merge. Do not replace them with parallel raw-Cargo invocations on Windows.

### Frontend

```bash
cd audio-graph
bun run typecheck        # tsc --noEmit
bun run test             # vitest run
bun run build            # tsc && vite build
```

### Backend

On Linux or macOS:

```bash
cd audio-graph
bun run rust:check:cloud  # fast iterative compile pass
bun run rust:test:cloud   # fast iterative test pass
bun run rust:check:full   # exclusive default-feature compile pass
bun run rust:test:full    # exclusive default-feature test pass
```

On every platform:

```bash
cd audio-graph
cd src-tauri
cargo fmt --check         # non-compiling formatting gate
cargo audit               # advisory gate; see .cargo/audit.toml for ignores
```

`cargo audit` is a hard gate in CI. If it flags a new advisory, either fix
the dep or add a justified ignore entry to `.cargo/audit.toml`. Don't
silently suppress.

Clippy is not currently gated in CI but is recommended:

```bash
cargo clippy --locked --all-targets -- -D warnings
```

---

## 5. What CI runs

See `.github/workflows/ci.yml`. There are four jobs:

| Job | Runs | What |
|---|---|---|
| `frontend` | Ubuntu | `bun install`, `tsc --noEmit`, `vitest run`, `vite build` |
| `rust-linux` | Ubuntu | `cargo fmt --check`, `cargo check --locked`, `cargo test --locked`, `cargo audit` |
| `rust-macos` | macOS 15 | `cargo check --locked`, `cargo test --locked` |
| `rust-windows` | Windows 2025 | `cargo check --locked`, `cargo test --locked` |

Rust jobs use the committed application lockfile and pass `--locked`; Cargo's
resolved rsac source is the single dependency identity used by CI and release.

`cargo test --locked` runs with `--test-threads=1` because several tests touch shared
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
cd audio-graph
bun run rust:test:cloud -- path::to::module::test_name
# e.g.
bun run rust:test:cloud -- gemini::tests::build_setup_message_api_key
```

The cloud task restricts to the `audio-graph` library and keeps
`--test-threads=1`. Use `bun run rust:test:full -- <filter>` only when the test
requires default features; that task waits for exclusive build access.

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
