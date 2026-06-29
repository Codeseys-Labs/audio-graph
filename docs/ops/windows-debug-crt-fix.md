# Windows `--debug` CRT heap-mismatch — cause and fix (seed audio-graph-d47b)

**Symptom.** A `--debug` Windows build (`tauri dev`, `tauri build --debug`)
aborts at runtime with:

```
Debug Assertion Failed!
Program: ...\src-tauri\target\debug\audio-graph.exe
File: minkernel\crts\ucrt\src\appcrt\heap\debug_heap.cpp
Line: 908
Expression: is_block_type_valid(header->_block_use)
```

Release builds (`tauri build`) run fine. This note explains why and how to run a
working `--debug` build.

## Root cause — two C runtimes in one process

A debug Rust build links the MSVC **debug** CRT (`/MDd`: `msvcrtd` / `ucrtbased`).
The bundled native ML libraries (`whisper.cpp`, `llama.cpp`, `ggml`) are built by
`cmake-rs` + `cc` and compile against the **release** CRT (`/MD`). A heap block
allocated by one CRT and freed by the other trips the debug CRT's heap validator.

Traced through the pinned sources (`docs/reviews/_d47b-crt-2026-06-28/research-findings.md`):

- **`cc` 1.2.62** has no `/MDd` code path at all — it only chooses between `/MT`
  (static release) and `/MD` (dynamic release).
- **`cmake-rs` 0.1.58** never sets `CMAKE_MSVC_RUNTIME_LIBRARY`; it injects `cc`'s
  `/MD` into `CMAKE_*_FLAGS`.
- **`whisper-rs-sys` / `llama-cpp-sys-2`** force a Release-family
  `CMAKE_BUILD_TYPE` (`RelWithDebInfo` / `Release`) even in a Rust debug build,
  so even modern CMake's runtime expression picks `/MD`.
- **`mistralrs` / `candle`** are pure Rust on the default Windows CPU build — NOT
  part of the mismatch.

### The CMP0091 asymmetry (why one env var isn't enough)

`CMAKE_MSVC_RUNTIME_LIBRARY` only works when CMake policy **CMP0091 is NEW**:

- **llama.cpp** (`cmake_minimum_required(... 3.28)`) → CMP0091 **NEW** → honors
  `CMAKE_MSVC_RUNTIME_LIBRARY`.
- **whisper.cpp** (`cmake_minimum_required(3.5)`, no policy-max) → CMP0091
  **OLD** → ignores it; the runtime flag comes from `CMAKE_*_FLAGS`.

So a robust fix sets **both** the runtime-library property (for llama) and the
flag vars (for whisper). Both `-sys` build scripts pass `CMAKE_*` / `CFLAGS` /
`CXXFLAGS` env through to the build, and setting `CMAKE_C_FLAGS`/`CMAKE_CXX_FLAGS`
also pre-defines the var so `cmake-rs` skips its own `/MD` injection.

## The fix — force the C++ deps to `/MDd` (Windows debug only)

Set these env vars for a Windows debug build (the canonical set, after the
first CI run surfaced an `LNK2038` on `llama-sampler.obj` — the base flag vars
alone did not cover the per-CONFIG `/MD` injection or the `_ITERATOR_DEBUG_LEVEL`
axis):

```
CMAKE_MSVC_RUNTIME_LIBRARY     = MultiThreadedDebugDLL   # llama (CMP0091 NEW)
LLAMA_LIB_PROFILE              = Debug                   # llama: Debug config -> /MDd + _ITERATOR_DEBUG_LEVEL=2
CFLAGS                         = /MDd                    # cc::Build wrapper objects in llama-cpp-sys-2
CXXFLAGS                       = /MDd
CMAKE_C_FLAGS                  = /MDd                    # base flags (whisper CMP0091 OLD + pre-define guard)
CMAKE_CXX_FLAGS                = /MDd
CMAKE_C_FLAGS_RELEASE          = /MDd                    # per-config: beats cmake-rs's per-config /MD injection
CMAKE_CXX_FLAGS_RELEASE        = /MDd
CMAKE_C_FLAGS_RELWITHDEBINFO   = /MDd                    # whisper hardcodes CMAKE_BUILD_TYPE=RelWithDebInfo
CMAKE_CXX_FLAGS_RELWITHDEBINFO = /MDd
```

**Why so many.** `LNK2038 RuntimeLibrary: MD_DynamicRelease ≠ MDd_DynamicDebug`
(and the matching `_ITERATOR_DEBUG_LEVEL 0 ≠ 2`) on `llama-sampler.obj` proved
the base `CMAKE_*_FLAGS` did NOT reach the per-config flags cmake-rs injects for
the VS generator. llama responds cleanly to `LLAMA_LIB_PROFILE=Debug`; whisper
(which hardcodes `CMAKE_BUILD_TYPE=RelWithDebInfo` via `config.define`, beating a
`CMAKE_BUILD_TYPE` env) needs the per-config flag vars forced.

**Why not `config.toml [env]`?** Cargo's `[env]` table cannot `cfg()`-scope
(rust-lang/cargo#10273), so a committed key would force `/MDd` onto the **release**
Windows build too — corrupting the shipping `tauri build` path. So the override is
delivered only via the run paths below, never committed globally.

**Why not `crt-static`?** `-C target-feature=+crt-static` flips
dynamic→static but stays the *release* CRT (wrong axis), and Tauri/wry fight the
static CRT. Rejected.

## How to run a working `--debug` build

- **Local wrapper (recommended):** `scripts/run-windows-debug.ps1`
  - `./scripts/run-windows-debug.ps1` — `tauri dev` with the override
  - `./scripts/run-windows-debug.ps1 -Build` — standalone `--debug` exe
  - `./scripts/run-windows-debug.ps1 -Cloud` — cloud-only (no native ML; sidesteps the issue entirely, fastest)
- **Or a release build:** `bun run tauri build` (single CRT, always works).
- **Or cloud-only debug:** `tauri dev -- --no-default-features --features cloud`
  (ADR-0007 — no native ML, single CRT).

`src-tauri/build.rs` emits a `cargo::warning` when it detects a Windows debug
local-ML build without the override, so the failure is self-explaining.

## CI

The `rust-windows-debug-crt-smoke` job (`.github/workflows/ci.yml`, push/dispatch/
nightly only — the local-ML compile is ~13-15 min) sets the override env, builds
the `--debug` local-ml app, and runs the debug whisper lib tests so the heap
validator actually executes. A green run is the authoritative proof that the
`--debug` build no longer aborts. (This was authored on Linux and cannot be run
locally there — the Windows runtime proof comes from this job.)
