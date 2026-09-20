#requires -Version 5.1
<#
.SYNOPSIS
    Release preflight checks: PowerShell parser, Cargo metadata, and dependency audit.

.DESCRIPTION
    This script only performs checks that can be reproduced inside this repo.
    It does not build images or run a real deployment.
    It verifies:
      1. PowerShell syntax for tools/*.ps1;
      3. cargo metadata --locked --no-deps;
      4. strict cargo audit (warnings denied) for the server and launcher
         lockfiles; missing cargo-audit is a hard failure.
    An optional benchmark smoke check can call tools\run-perf-benchmark.ps1.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File tools\verify-release.ps1
.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File tools\verify-release.ps1 -Benchmark
.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File tools\verify-release.ps1 -CargoAuditNoFetch
#>
[CmdletBinding()]
param(
    [switch]$Benchmark,
    [switch]$CargoAuditNoFetch,
    [ValidateRange(1, 1000)]
    [int]$BenchmarkIterations = 1,
    [ValidateRange(0, 1000)]
    [int]$BenchmarkWarmup = 0
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$RepoRoot = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path (Join-Path $RepoRoot 'web\package.json'))) {
    throw "Unable to locate repository root: $RepoRoot"
}

function Write-Step {
    param([string]$Text)
    Write-Host ("[verify] {0}" -f $Text) -ForegroundColor Cyan
}

function Test-Tool {
    param([string[]]$Names)
    foreach ($name in $Names) {
        if (Get-Command $name -ErrorAction SilentlyContinue) {
            return $name
        }
    }
    return $null
}

Write-Step 'PowerShell parser dry-run for tools/*.ps1'
$parseFailures = @()
Get-ChildItem -LiteralPath (Join-Path $RepoRoot 'tools') -Filter '*.ps1' -File | Sort-Object Name | ForEach-Object {
    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile($_.FullName, [ref]$tokens, [ref]$errors) | Out-Null
    if ($errors.Count -gt 0) {
        foreach ($err in $errors) {
            $parseFailures += [pscustomobject]@{
                File    = $_.FullName
                Message = $err.Message
                Line    = $err.Extent.StartLineNumber
                Column  = $err.Extent.StartColumnNumber
            }
        }
    }
}
if ($parseFailures.Count -gt 0) {
    $parseFailures | ForEach-Object {
        Write-Host ("[parser] {0}:{1}:{2} {3}" -f $_.File, $_.Line, $_.Column, $_.Message) -ForegroundColor Red
    }
    throw "PowerShell parse failed with $($parseFailures.Count) error(s)"
}

Write-Step 'cargo metadata --locked --no-deps (server + launcher)'
$cargo = Test-Tool @('cargo')
if ($null -eq $cargo) {
    throw 'cargo not found'
}
foreach ($cargoDir in @('server', 'launcher')) {
    Push-Location (Join-Path $RepoRoot $cargoDir)
    try {
        & $cargo metadata --format-version 1 --locked --no-deps | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "cargo metadata failed for $cargoDir (exit=$LASTEXITCODE)"
        }
    } finally {
        Pop-Location
    }
    Write-Host ("[metadata] {0}: cargo metadata passed" -f $cargoDir) -ForegroundColor Green
}

Write-Step 'cargo audit (server + launcher lockfiles, warnings denied)'
if (-not (Get-Command cargo-audit -ErrorAction SilentlyContinue)) {
    throw 'cargo-audit not found; release audit gate cannot run. Install with: cargo install cargo-audit --locked'
}
# Strict audit gate: any vulnerability or warning fails the release check.
# Documented exemption (re-evaluate on each webrtc upgrade):
#   RUSTSEC-2025-0141 — bincode 1.x unmaintained. bincode 1.3.3 is only a
#   transitive dep here: webrtc 0.13 -> webrtc-dtls 0.12 (bincode ^1), and the
#   latest webrtc-dtls 0.12.x still requires bincode ^1, so no fixed version
#   exists within the webrtc 0.13 line. webrtc >= 0.20 (rtc-dtls) drops bincode
#   but is a cross-version API migration; drop this ignore after that upgrade.
$auditIgnoreIds = @('RUSTSEC-2025-0141')
foreach ($auditDir in @('server', 'launcher')) {
    Push-Location (Join-Path $RepoRoot $auditDir)
    try {
        $auditArgs = @('audit', '--color', 'never', '-D', 'warnings')
        if ($CargoAuditNoFetch) {
            $auditArgs += '--no-fetch'
        }
        foreach ($advisoryId in $auditIgnoreIds) {
            $auditArgs += @('--ignore', $advisoryId)
        }
        & cargo @auditArgs
        $auditExit = $LASTEXITCODE
        if ($auditExit -ne 0) {
            throw "cargo audit failed for $auditDir (exit=$auditExit)"
        }
    } finally {
        Pop-Location
    }
    $auditMode = if ($CargoAuditNoFetch) { 'offline advisory DB (--no-fetch)' } else { 'fresh advisory DB fetch' }
    Write-Host ("[audit] {0}: cargo audit passed (strict; {1}; ignored: {2})" -f $auditDir, $auditMode, ($auditIgnoreIds -join ', ')) -ForegroundColor Green
}

if ($Benchmark) {
    Write-Step ("benchmark smoke via tools\run-perf-benchmark.ps1 (iterations={0}, warmup={1})" -f $BenchmarkIterations, $BenchmarkWarmup)
    $benchmarkScript = Join-Path $RepoRoot 'tools\run-perf-benchmark.ps1'
    if (-not (Test-Path -LiteralPath $benchmarkScript -PathType Leaf)) {
        throw "benchmark script not found: $benchmarkScript"
    }
    & powershell -NoProfile -ExecutionPolicy Bypass -File $benchmarkScript -Iterations $BenchmarkIterations -Warmup $BenchmarkWarmup
    if ($LASTEXITCODE -ne 0) {
        throw "benchmark smoke failed (exit=$LASTEXITCODE)"
    }
}

Write-Host '[verify] all executable release checks completed' -ForegroundColor Green
