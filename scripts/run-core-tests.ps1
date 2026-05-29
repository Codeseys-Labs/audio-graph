<#
.SYNOPSIS
  Run AudioGraph's ML-free Rust unit tests on Windows via standalone harness
  crates, bypassing the broken main test binary.

.DESCRIPTION
  The crate's own `cargo test` binary aborts at load with
  STATUS_ENTRYPOINT_NOT_FOUND (0xC0000139) on this/likely-any Windows box: the
  native ML libs (whisper-rs / llama-cpp-2 / mistralrs, all linking heavy MSVC
  C++ runtimes + OpenMP) produce a test-harness link the loader can't satisfy.
  This is pre-existing and environmental — it hits the untouched audio::pipeline
  tests identically. The proper permanent fix is ADR-0007 (feature-gate the ML
  crates so a test build doesn't link them); until then this script runs the
  pure, ML-free modules for real.

  It generates throwaway crates that #[path]-include the REAL source files (no
  copies → no drift), stub the few `crate::` deps those modules touch, and
  `cargo test` them with only their lightweight deps. Currently covers:
    - graph::temporal + graph::entities  (edge-id consistency, updated_edges,
      eviction id scheme, dedup/merge)
    - audio::mix_math + audio::mixer       (sum/scale/clamp, align, evict)
    - audio::backpressure                  (drop-oldest)

.NOTES
  Add new ML-free modules here as they gain tests. If a harness fails to
  compile because a module gained a new `crate::`/extern dep, add the dep to
  that harness's Cargo.toml or a stub to its lib.rs.
#>
[CmdletBinding()]
param(
    [string]$Root,
    [switch]$Keep
)

$ErrorActionPreference = "Stop"
if (-not $Root) { $Root = Split-Path -Parent $PSScriptRoot }
$src = (Join-Path $Root "src-tauri\src") -replace '\\', '/'
if (-not (Test-Path $src)) { Write-Host "src not found: $src" -ForegroundColor Red; exit 2 }

$work = Join-Path $env:TEMP ("ag-core-tests-{0}" -f (Get-Date -Format yyyyMMddHHmmss))
New-Item -ItemType Directory -Path $work -Force | Out-Null
$fail = 0

function New-Harness([string]$name, [string]$cargoToml, [string]$libRs) {
    $dir = Join-Path $work $name
    New-Item -ItemType Directory -Path "$dir\src" -Force | Out-Null
    Set-Content -Path "$dir\Cargo.toml" -Value $cargoToml -Encoding utf8
    Set-Content -Path "$dir\src\lib.rs" -Value $libRs -Encoding utf8
    Write-Host "=== $name ===" -ForegroundColor Cyan
    Push-Location $dir
    try {
        & cargo test --quiet 2>&1 | ForEach-Object { $_ }
        if ($LASTEXITCODE -ne 0) { $script:fail++ }
    } finally { Pop-Location }
}

# ── graph::temporal + entities ────────────────────────────────────────────
New-Harness "graph_verify" @"
[package]
name = "graph_verify"
version = "0.0.0"
edition = "2021"
[dependencies]
serde = { version = "1", features = ["derive"] }
petgraph = "0.8"
schemars = "1"
log = "0.4"
uuid = { version = "1", features = ["v4"] }
strsim = "0.11"
"@ @"
pub mod persistence {
    use std::path::Path;
    pub fn save_json<T: serde::Serialize>(_v: &T, _p: &Path) -> Result<(), String> { Ok(()) }
    pub fn load_json<T: serde::de::DeserializeOwned>(_p: &Path) -> Result<T, String> { Err(`"stub`".into()) }
}
pub mod ontology {
    pub fn entity_type_color(_t: &str) -> &'static str { `"#607D8B`" }
    pub fn relation_type_color(_t: &str) -> &'static str { `"#757575`" }
}
pub mod graph {
    #[path = "$src/graph/entities.rs"] pub mod entities;
    #[path = "$src/graph/temporal.rs"] pub mod temporal;
}
"@

# ── audio::mix_math + mixer + backpressure ─────────────────────────────────
New-Harness "audio_verify" @"
[package]
name = "audio_verify"
version = "0.0.0"
edition = "2021"
[dependencies]
crossbeam-channel = "0.5"
log = "0.4"
"@ @"
pub mod audio {
    #[path = "$src/audio/mix_math.rs"] pub mod mix_math;
    #[path = "$src/audio/backpressure.rs"] pub mod backpressure;
    pub mod pipeline {
        #[derive(Debug, Clone)]
        pub struct ProcessedAudioChunk {
            pub source_id: String,
            pub data: Vec<f32>,
            pub sample_rate: u32,
            pub num_frames: usize,
            pub timestamp: Option<std::time::Duration>,
        }
    }
    #[path = "$src/audio/mixer.rs"] pub mod mixer;
}
"@

if (-not $Keep) { Remove-Item -Recurse -Force $work -EA SilentlyContinue }
Write-Host ""
if ($fail -eq 0) { Write-Host "ALL CORE-LOGIC TESTS PASSED" -ForegroundColor Green; exit 0 }
else { Write-Host "$fail harness(es) failed" -ForegroundColor Red; exit 1 }
