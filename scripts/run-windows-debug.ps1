# Run a Windows --debug build of AudioGraph WITHOUT the debug-CRT heap assertion.
#
# WHY (seed audio-graph-d47b): a plain `tauri dev` / `tauri build --debug` on
# Windows links the MSVC DEBUG CRT (/MDd), but the bundled native ML libs
# (whisper.cpp / llama.cpp / ggml, built by cmake-rs + cc) compile against the
# RELEASE CRT (/MD) — cc has no /MDd path and cmake-rs never sets the runtime
# library, and the -sys scripts force a Release-family CMAKE_BUILD_TYPE. Memory
# allocated by one CRT's heap and freed by the other trips the debug heap
# validator:  "Debug Assertion Failed! is_block_type_valid(header->_block_use)"
# at debug_heap.cpp:908. (Release builds share one CRT and never trip it.)
#
# This wrapper exports the CRT-override env vars that force the C++ ML deps to
# the DEBUG dynamic CRT (/MDd) so both sides share one heap, then runs the debug
# build. It does NOT touch your release build or any other platform — the env is
# set only for the process this script spawns. See docs/ops/windows-debug-crt-fix.md.
#
# Usage (from the repo root, in PowerShell):
#   ./scripts/run-windows-debug.ps1            # tauri dev (hot-reload)
#   ./scripts/run-windows-debug.ps1 -Build     # tauri build --debug --no-bundle (standalone debug exe)
#   ./scripts/run-windows-debug.ps1 -Cloud     # cloud-only --debug (no native ML at all — also avoids the issue)
[CmdletBinding()]
param(
    # Produce a standalone --debug exe instead of running `tauri dev`.
    [switch]$Build,
    # Build the cloud-only feature set (no whisper/llama/mistralrs). This sidesteps
    # the CRT mismatch entirely (single CRT) and is the fastest debug loop when you
    # don't need local ML. ADR-0007.
    [switch]$Cloud
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot

# --- The d47b CRT override: force the cmake/cc-built ML deps to /MDd ----------
# Two mechanisms, because whisper.cpp and llama.cpp differ on CMake policy
# CMP0091: llama (CMP0091 NEW) honors CMAKE_MSVC_RUNTIME_LIBRARY; whisper
# (CMP0091 OLD) reads the runtime flag from CMAKE_*_FLAGS instead. Setting both
# CMAKE_C_FLAGS/CMAKE_CXX_FLAGS also pre-defines the var so cmake-rs skips its
# own /MD injection. Cloud builds have no native ML, so the override is a no-op
# there but harmless to set.
$env:CMAKE_MSVC_RUNTIME_LIBRARY = 'MultiThreadedDebugDLL'
$env:CFLAGS = '/MDd'
$env:CXXFLAGS = '/MDd'
$env:CMAKE_C_FLAGS = '/MDd'
$env:CMAKE_CXX_FLAGS = '/MDd'
# A --debug Windows exe also needs the SxS Common-Controls manifest to load (B23).
$env:AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST = '1'

Push-Location $repoRoot
try {
    if ($Cloud) {
        Write-Host 'Running cloud-only --debug build (no native ML; single CRT) ...'
        if ($Build) {
            bun run tauri build --debug --no-bundle --ci -- --no-default-features --features cloud
        } else {
            bun run tauri dev -- --no-default-features --features cloud
        }
    }
    elseif ($Build) {
        Write-Host 'Building standalone --debug exe (local-ml, /MDd-forced native deps) ...'
        bun run tauri build --debug --no-bundle
        Write-Host 'Built: src-tauri\target\debug\audio-graph.exe'
    }
    else {
        Write-Host 'Running `tauri dev` --debug (local-ml, /MDd-forced native deps) ...'
        bun run tauri dev
    }
}
finally {
    Pop-Location
}
