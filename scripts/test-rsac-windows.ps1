<#
.SYNOPSIS
  Exercises the rsac audio-capture functionality that AudioGraph depends on,
  on Windows (WASAPI). Use this to diagnose capture problems like
  "Unsupported audio format" or "Audio device not found".

.DESCRIPTION
  AudioGraph captures audio through the sibling `rsac` crate. This script
  drives rsac's own CLI to verify, in isolation from the GUI:
    1. info   - platform capabilities (system/app/process-tree capture)
    2. list   - every device + the exact formats it advertises
                (this is what reveals e.g. "device only supports 8ch/96000")
    3. record - capture system-default loopback to a WAV for N seconds and
                confirm a non-empty file is produced

  AudioGraph itself negotiates a supported capture format per device
  (see src-tauri/src/audio/capture.rs::choose_capture_format), so a device
  that only advertises 8ch/96000 or 2ch/44100 still works in the app even
  though a naive 48000/1/F32 request would be rejected by rsac.

.PARAMETER RsacDir
  Path to the rsac checkout. Defaults to ..\rsac relative to this repo.

.PARAMETER Seconds
  Record duration for the loopback capture test (default 5).

.EXAMPLE
  pwsh scripts/test-rsac-windows.ps1
#>
[CmdletBinding()]
param(
    [string]$RsacDir,
    [int]$Seconds = 5
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $RsacDir) { $RsacDir = Join-Path (Split-Path -Parent $repoRoot) "rsac" }

if (-not (Test-Path (Join-Path $RsacDir "Cargo.toml"))) {
    Write-Host "rsac not found at '$RsacDir'. Pass -RsacDir <path>." -ForegroundColor Red
    Write-Host "AudioGraph expects rsac as a sibling checkout (../rsac)." -ForegroundColor Red
    exit 2
}

Write-Host "=== rsac Windows capture diagnostic ===" -ForegroundColor Cyan
Write-Host "rsac: $RsacDir`n" -ForegroundColor DarkGray

function Invoke-Rsac([string[]]$rsacArgs, [string]$label) {
    Write-Host "----- $label -----" -ForegroundColor Yellow
    Write-Host "  cargo run --quiet --bin rsac -- $($rsacArgs -join ' ')" -ForegroundColor DarkGray
    Push-Location $RsacDir
    try {
        & cargo run --quiet --bin rsac -- @rsacArgs
        return $LASTEXITCODE
    }
    finally { Pop-Location }
}

$fail = 0

# 1. Platform capabilities
if ((Invoke-Rsac @("info") "Platform capabilities") -ne 0) { $fail++ }

# 2. Devices + advertised formats (the key diagnostic for format errors)
Write-Host ""
if ((Invoke-Rsac @("list") "Devices and supported formats") -ne 0) { $fail++ }

# 3. Loopback capture to WAV (proves capture actually produces audio)
Write-Host ""
$wav = Join-Path $env:TEMP ("rsac-loopback-{0}.wav" -f (Get-Date -Format HHmmss))
# WASAPI loopback only delivers packets while audio is actually playing, so
# play a short system sound for the duration to give the capture real signal.
$playJob = $null
$sound = "C:\Windows\Media\Alarm01.wav"
if (Test-Path $sound) {
    Write-Host "Playing a system sound through the default device so loopback has signal..." -ForegroundColor DarkGray
    $playJob = Start-Job -ArgumentList $sound, $Seconds {
        param($s, $secs)
        $deadline = (Get-Date).AddSeconds($secs + 1)
        while ((Get-Date) -lt $deadline) {
            (New-Object System.Media.SoundPlayer $s).PlaySync()
            Start-Sleep -Milliseconds 150
        }
    }
    Start-Sleep -Milliseconds 400
}
else {
    Write-Host "Tip: play some audio so loopback has signal to capture." -ForegroundColor DarkGray
}
$rc = Invoke-Rsac @("record", $wav, "--duration", "$Seconds") "Record system loopback ($Seconds s)"
if ($playJob) { Stop-Job $playJob -EA SilentlyContinue; Remove-Job $playJob -Force -EA SilentlyContinue }
if ($rc -ne 0) {
    Write-Host "  record exited with code $rc" -ForegroundColor Red
    $fail++
}
elseif (Test-Path $wav) {
    $sizeKB = [math]::Round((Get-Item $wav).Length / 1KB, 1)
    if ($sizeKB -gt 1) {
        Write-Host "  OK wrote $wav ($sizeKB KB)" -ForegroundColor Green
    }
    else {
        Write-Host "  WARN wrote $wav but it is tiny ($sizeKB KB) - capture may have produced no frames" -ForegroundColor DarkYellow
    }
}
else {
    Write-Host "  FAIL: no WAV produced" -ForegroundColor Red
    $fail++
}

Write-Host ""
if ($fail -eq 0) {
    Write-Host "rsac capture diagnostic PASSED" -ForegroundColor Green
    exit 0
}
else {
    Write-Host "$fail rsac check(s) failed" -ForegroundColor Red
    exit 1
}
