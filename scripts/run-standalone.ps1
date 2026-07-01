# Launch the standalone release build of AudioGraph (no installer needed).
#
# WHY: the standalone `audio-graph.exe` is the better dev-loop artifact than the
# NSIS installer — it runs in place (no install/uninstall churn per rebuild) and
# reads the SAME per-user state the installed app would:
#   - settings: %APPDATA%\com.rsac.audiograph\settings.json
#   - credentials: %APPDATA%\audio-graph\credentials.yaml   (your API keys)
#   - models: %APPDATA%\com.rsac.audiograph\models
#   - logs: %APPDATA%\audio-graph\logs\audio-graph.log
# The only thing the installer additionally provisions is the WebView2 Evergreen
# runtime, which Windows 11 already ships (verified present), so the standalone is
# fully self-sufficient on this machine.
#
# Build it first if needed:
#   bunx tauri build --no-bundle          # just the exe (fastest)
#   bunx tauri build --bundles nsis       # exe + installer
#
# Usage (from the repo root, in PowerShell):
#   ./scripts/run-standalone.ps1            # launch detached
#   ./scripts/run-standalone.ps1 -Tail      # launch, then tail the log
[CmdletBinding()]
param(
    # After launching, follow the live log (Ctrl+C to stop tailing; the app keeps running).
    [switch]$Tail
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $repoRoot 'src-tauri\target\release\audio-graph.exe'

if (-not (Test-Path $exe)) {
    Write-Error "Standalone build not found at $exe`nBuild it first: bunx tauri build --no-bundle"
}

# Start-Process fully detaches from this shell, so the app survives the script
# exiting (unlike `cmd /c start` launched from a transient subshell).
Write-Host "Launching $exe ..."
Start-Process -FilePath $exe -WorkingDirectory (Split-Path -Parent $exe)
Write-Host "Launched (detached). Window should appear shortly."

$log = Join-Path $env:APPDATA 'audio-graph\logs\audio-graph.log'
Write-Host "Log: $log"

if ($Tail) {
    Start-Sleep -Seconds 2
    if (Test-Path $log) {
        Write-Host "--- tailing log (Ctrl+C stops the tail; the app keeps running) ---"
        Get-Content -Path $log -Tail 20 -Wait
    }
    else {
        Write-Host "Log not created yet; check $log shortly."
    }
}
