# Windows local `cargo test` — CRT skew + the WSL workaround (B23 / 2.7, ADR-0007)

**Status: diagnosed + worked around.** Local Rust *test execution* on this
Windows host is restored via WSL (449 + 58 tests green, see below). The
Windows-native `cargo test` failure is a VC++ toolset/CRT version skew that needs
a toolchain repair (a system-config action), not a code change.

## Symptom

`cargo test` (any feature set) builds fine, then the test harness aborts at
process load:

```
process didn't exit successfully: ...\audio_graph-<hash>.exe
  (exit code: 0xc0000139, STATUS_ENTRYPOINT_NOT_FOUND)
```

No Rust code runs — the failure is in the Windows loader resolving the C runtime.

## Root cause (diagnosed 2026-05-31)

Two MSVC toolsets are installed on this box:

- **VS 18 (2026) Community — MSVC 14.50.35717** (a preview toolset). Rust's
  `link.exe` resolves to this; the test binary is linked against its
  `vcruntime140.dll` / `vcruntime140_1.dll` (file version 14.50.35730).
- **VS 2022 BuildTools — MSVC 14.44.35207** (+ older 14.16 / 14.29 redists).

At runtime the loader resolves `VCRUNTIME140.dll` from `C:\Windows\System32`,
which is **14.51.36231** (shipped by a Windows/VC++ redist update). The
14.50-linked binary imports an export the 14.51 System32 vcruntime/ucrtbase chain
does not satisfy → `STATUS_ENTRYPOINT_NOT_FOUND`.

`dumpbin /imports` on the harness shows ordinary imports (`__CxxFrameHandler3/4`,
`__std_exception_*`, `memcpy`, math fns) from `VCRUNTIME140.dll` + `VCRUNTIME140_1.dll`
+ the `api-ms-win-crt-*` UCRT forwarders. **App-local CRT deployment** (copying
the matching 14.50 `vcruntime140*.dll` next to the binary) was tried and does
**not** fix it — the copied DLLs export every needed symbol, so the unresolved
entrypoint is deeper in the vcruntime→`ucrtbase.dll` chain, where the 14.50
vcruntime meets the 14.51 System32 UCRT. That gap can't be patched app-locally.

## Workaround (in use): run the tests under WSL

WSL Ubuntu is a real Linux environment on this same box; the MSVC CRT stack is
irrelevant there, so the tests run normally. This gives genuine **local test
execution** (not just `clippy`/compile) plus a **second-platform signal**
(Windows-compile + Linux-run).

```bash
scripts/run-rust-tests-wsl.sh cloud        # 449 passed / 0 failed (2026-05-31)
scripts/run-rust-tests-wsl.sh diarization  #  58 passed / 0 failed (2026-05-31)
scripts/run-rust-tests-wsl.sh local-ml     # full default suite (whisper/llama/mistralrs)
```

The script uses a Linux-side `CARGO_TARGET_DIR=/tmp/ag-wsl-target` so it never
clobbers the Windows `target/`. Requires a rustup toolchain in WSL (auto-installs
the pinned 1.95.0 from `rust-toolchain.toml`).

## Permanent Windows-native fix (toolchain repair — not done here)

Pick one (all are host-config actions, outside the repo):

1. **Align the linker toolset with the deployed CRT.** Force Rust to link with
   the VS 2022 BuildTools **14.44** toolset (whose redist matches a broadly
   deployed System32 CRT) instead of the 14.50 preview — e.g. launch the build
   from the 14.44 *Developer Command Prompt*, or set the MSVC toolset version so
   `link.exe` 14.44 is selected. Then the binary imports the stable 14.44 ABI.
2. **Install the matching VC++ Redistributable** for the 14.50 toolset
   system-wide so System32 carries a vcruntime/ucrtbase that satisfies the
   14.50-linked imports.
3. **Uninstall/repair the 14.50 preview toolset** so only the stable 14.44
   toolset remains (removes the skew at the source).

CI is unaffected (Linux runners; the Windows CI runner uses a clean stable
toolset). This note exists so the skew is understood, not rediscovered.
