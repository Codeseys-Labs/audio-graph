#!/usr/bin/env bash
# Run the Rust test suite under WSL Ubuntu (Linux) from a Windows dev box.
#
# WHY THIS EXISTS (B23 / 2.7, ADR-0007): on this Windows host the MSVC-linked
# Rust test binary aborts at load with STATUS_ENTRYPOINT_NOT_FOUND (0xC0000139)
# — a VC++ toolset/CRT version skew (binary linked by MSVC 14.50, but System32
# ships a 14.51 vcruntime140 missing an export the 14.50-linked binary imports).
# That blocks `cargo test` natively on Windows. WSL Ubuntu is a real Linux
# environment on the same box where that skew does not exist, so the tests run
# normally there. This gives genuine LOCAL test EXECUTION (not just
# clippy/compile) plus a second-platform signal (Windows-compile + Linux-run).
#
# Usage:
#   scripts/run-rust-tests-wsl.sh                 # default: cloud lib tests
#   scripts/run-rust-tests-wsl.sh cloud           # --no-default-features --features cloud
#   scripts/run-rust-tests-wsl.sh diarization     # --features diarization-clustering
#   scripts/run-rust-tests-wsl.sh local-ml        # default features (heavy: whisper/llama/mistralrs)
#
# Requires: WSL Ubuntu with a rustup toolchain (auto-installs the pinned 1.95.0
# per rust-toolchain.toml). Uses a Linux-side CARGO_TARGET_DIR so it never
# clobbers the Windows `target/`.
set -euo pipefail

FEATURE_SET="${1:-cloud}"
# Resolve this repo's path as seen from WSL (/mnt/<drive>/...).
WIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Git Bash gives /e/...; WSL wants /mnt/e/... — normalize either form.
WSL_DIR="$(printf '%s' "$WIN_DIR" | sed -E 's#^/([a-zA-Z])/#/mnt/\L\1/#')"

case "$FEATURE_SET" in
  cloud)        FEATURES=(--no-default-features --features cloud) ;;
  diarization)  FEATURES=(--no-default-features --features diarization-clustering) ;;
  local-ml)     FEATURES=(--features local-ml) ;;
  *) echo "unknown feature set '$FEATURE_SET' (cloud|diarization|local-ml)"; exit 2 ;;
esac

# Pre-flight: WSL + the Ubuntu distro must be present, else fail with a clear
# message instead of an opaque "command not found" / wsl usage dump.
if ! command -v wsl.exe >/dev/null 2>&1; then
  echo "ERROR: wsl.exe not found on PATH." >&2
  echo "  This script runs the Rust tests inside WSL from a Windows dev box." >&2
  echo "  Install WSL ('wsl --install') or run 'cargo test' natively on Linux." >&2
  exit 127
fi
# `wsl.exe -l -q` lists installed distros (one per line). Match 'Ubuntu' exactly
# or as a versioned variant (Ubuntu-22.04, etc.). Strip CR/NUL that WSL emits.
if ! wsl.exe -l -q 2>/dev/null | tr -d '\r\0' | grep -qiE '^Ubuntu(-[0-9.]+)?$'; then
  echo "ERROR: a WSL 'Ubuntu' distro was not found." >&2
  echo "  Installed distros:" >&2
  wsl.exe -l -q 2>/dev/null | tr -d '\r\0' | sed 's/^/    /' >&2 || true
  echo "  Install it with: wsl --install -d Ubuntu" >&2
  exit 1
fi

echo ">> Running Rust tests in WSL Ubuntu: feature set '$FEATURE_SET'"
echo ">> repo (WSL path): $WSL_DIR/src-tauri"

wsl.exe -d Ubuntu -- bash -lc "cd '$WSL_DIR/src-tauri' && \
  CARGO_TARGET_DIR=/tmp/ag-wsl-target cargo test ${FEATURES[*]} --lib"
