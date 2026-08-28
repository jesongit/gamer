#requires -Version 5.1
<#
.SYNOPSIS
    Release preflight checks: compose config, PowerShell parser, Cargo metadata, and dependency audit.

.DESCRIPTION
    This script only performs checks that can be reproduced inside this repo.
    It does not build images or run a real deployment.
    It verifies:
      1. PowerShell syntax for tools/*.ps1;
      2. docker compose config for the base file and the USB override;
      3. cargo metadata --locked --no-deps;
      4. cargo audit, or a clear missing-tool report if unavailable.
    An optional benchmark smoke check can call tools\run-perf-benchmark.ps1.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File tools\verify-release.ps1
.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File tools\verify-release.ps1 -Benchmark
#>
[CmdletBinding()]
param(
    [switch]$Benchmark,
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

Write-Step 'docker compose config'
$docker = Test-Tool @('docker')
if ($null -eq $docker) {
    Write-Host '[compose] docker not found; skipping compose config check' -ForegroundColor Yellow
} else {
    Push-Location $RepoRoot
    try {
        & docker compose -f docker-compose.yml config | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "docker compose -f docker-compose.yml config failed (exit=$LASTEXITCODE)"
        }
        & docker compose -f docker-compose.yml -f docker-compose.usb.yml config | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "docker compose USB override config failed (exit=$LASTEXITCODE)"
        }
    } finally {
        Pop-Location
    }
    Write-Host '[compose] config check passed' -ForegroundColor Green
}

Write-Step 'cargo metadata --locked --no-deps'
$cargo = Test-Tool @('cargo')
if ($null -eq $cargo) {
    throw 'cargo not found'
}
Push-Location (Join-Path $RepoRoot 'server')
try {
    & cargo metadata --format-version 1 --locked --no-deps | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed (exit=$LASTEXITCODE)"
    }
} finally {
    Pop-Location
}

Write-Step 'cargo audit'
$auditTool = Test-Tool @('cargo-audit', 'cargo')
if ($null -ne $auditTool -and (Get-Command cargo-audit -ErrorAction SilentlyContinue)) {
    Push-Location (Join-Path $RepoRoot 'server')
    try {
        & cargo audit --color never
        $auditExit = $LASTEXITCODE
        if ($auditExit -ne 0) {
            throw "cargo audit failed (exit=$auditExit)"
        }
    } finally {
        Pop-Location
    }
    Write-Host '[audit] cargo audit passed' -ForegroundColor Green
} else {
    Write-Host '[audit] cargo-audit / cargo audit subcommand not found; reported without fake success' -ForegroundColor Yellow
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
