<#
.SYNOPSIS
  Live smoke test for the AudioGraph cloud pipeline legs: Deepgram (STT)
  and OpenRouter (LLM). Proves your API keys are valid and the request
  shapes the app uses actually work end-to-end.

.DESCRIPTION
  Secrets are NEVER hard-coded. Keys are read, in order, from:
    1. Environment variables  DEEPGRAM_API_KEY / OPENROUTER_API_KEY
    2. %APPDATA%\audio-graph\credentials.yaml  (the app's own store)

  Tested legs:
    - Deepgram STT: POST https://api.deepgram.com/v1/listen with a public
      sample audio URL; asserts a non-empty transcript comes back.
    - OpenRouter LLM: GET /api/v1/models (key validation) then POST
      /api/v1/chat/completions with a tiny prompt; asserts a reply.

.EXAMPLE
  $env:DEEPGRAM_API_KEY="dg-..."; $env:OPENROUTER_API_KEY="sk-or-..."
  pwsh scripts/test-cloud-pipeline.ps1

.EXAMPLE
  # Uses keys already saved by the app (Express Setup / Settings):
  pwsh scripts/test-cloud-pipeline.ps1
#>
[CmdletBinding()]
param(
    [string]$DeepgramModel = "nova-3",
    [string]$OpenRouterModel = "openai/gpt-4o-mini",
    [string]$SampleAudioUrl = "https://dpgr.am/spacewalk.wav"
)

$ErrorActionPreference = "Stop"
$failures = 0

function Get-CredFromYaml([string]$key) {
    $path = Join-Path $env:APPDATA "audio-graph\credentials.yaml"
    if (-not (Test-Path $path)) { return $null }
    foreach ($line in Get-Content $path) {
        if ($line -match "^\s*$([regex]::Escape($key))\s*:\s*`"?([^`"]+)`"?\s*$") {
            return $Matches[1].Trim()
        }
    }
    return $null
}

function Resolve-Key([string]$envName, [string]$yamlKey) {
    $v = [Environment]::GetEnvironmentVariable($envName)
    if ([string]::IsNullOrWhiteSpace($v)) { $v = Get-CredFromYaml $yamlKey }
    return $v
}

$dgKey = Resolve-Key "DEEPGRAM_API_KEY"   "deepgram_api_key"
$orKey = Resolve-Key "OPENROUTER_API_KEY" "openrouter_api_key"

Write-Host "=== AudioGraph cloud pipeline smoke test ===" -ForegroundColor Cyan

# ---------------------------------------------------------------- Deepgram STT
Write-Host "`n[1/2] Deepgram STT ($DeepgramModel)" -ForegroundColor Yellow
if ([string]::IsNullOrWhiteSpace($dgKey)) {
    Write-Host "  SKIP: no Deepgram key (set DEEPGRAM_API_KEY or save it in the app)" -ForegroundColor DarkYellow
    $failures++
}
else {
    try {
        $uri = "https://api.deepgram.com/v1/listen?model=$DeepgramModel&smart_format=true"
        $body = @{ url = $SampleAudioUrl } | ConvertTo-Json
        $resp = Invoke-RestMethod -Method Post -Uri $uri -Headers @{
            "Authorization" = "Token $dgKey"
            "Content-Type"  = "application/json"
        } -Body $body -TimeoutSec 60
        $transcript = $resp.results.channels[0].alternatives[0].transcript
        if ([string]::IsNullOrWhiteSpace($transcript)) { throw "empty transcript" }
        Write-Host "  OK transcript: `"$($transcript.Substring(0,[Math]::Min(80,$transcript.Length)))...`"" -ForegroundColor Green
    }
    catch {
        Write-Host "  FAIL: $($_.Exception.Message)" -ForegroundColor Red
        $failures++
    }
}

# ------------------------------------------------------------- OpenRouter LLM
Write-Host "`n[2/2] OpenRouter LLM ($OpenRouterModel)" -ForegroundColor Yellow
if ([string]::IsNullOrWhiteSpace($orKey)) {
    Write-Host "  SKIP: no OpenRouter key (set OPENROUTER_API_KEY or save it in the app)" -ForegroundColor DarkYellow
    $failures++
}
else {
    try {
        $null = Invoke-RestMethod -Method Get -Uri "https://openrouter.ai/api/v1/models" `
            -Headers @{ "Authorization" = "Bearer $orKey" } -TimeoutSec 30
        Write-Host "  OK /models reachable (key valid)" -ForegroundColor Green

        $chatBody = @{
            model    = $OpenRouterModel
            messages = @(@{ role = "user"; content = "Reply with exactly: PIPELINE_OK" })
            max_tokens = 16
        } | ConvertTo-Json -Depth 5
        $chat = Invoke-RestMethod -Method Post -Uri "https://openrouter.ai/api/v1/chat/completions" `
            -Headers @{
                "Authorization"  = "Bearer $orKey"
                "Content-Type"   = "application/json"
                "HTTP-Referer"   = "https://github.com/Codeseys-Labs/audio-graph"
                "X-Title"        = "AudioGraph"
            } -Body $chatBody -TimeoutSec 60
        $reply = $chat.choices[0].message.content
        if ([string]::IsNullOrWhiteSpace($reply)) { throw "empty completion" }
        Write-Host "  OK reply: `"$($reply.Trim())`"" -ForegroundColor Green
    }
    catch {
        Write-Host "  FAIL: $($_.Exception.Message)" -ForegroundColor Red
        $failures++
    }
}

Write-Host ""
if ($failures -eq 0) {
    Write-Host "ALL CLOUD LEGS PASSED" -ForegroundColor Green
    exit 0
}
else {
    Write-Host "$failures leg(s) failed/skipped" -ForegroundColor Red
    exit 1
}
