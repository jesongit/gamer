#requires -Version 5.1
<#!
.SYNOPSIS
  运行 Phase 0 离线兼容护栏；可选更新 release/性能基线。

.DESCRIPTION
  默认只运行正式 Rust/Vue 离线测试并校验已有 baseline。真实 Android、scrcpy
  会话和浏览器 WebRTC 不由默认入口伪造；固定 ffmpeg 基准使用 -RunPerf 显式开启。
#>
[CmdletBinding()]
param(
    [switch]$SkipRust,
    [switch]$SkipWeb,
    [switch]$BuildRelease,
    [switch]$RunPerf
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($PSVersionTable.PSVersion.Major -ge 7) {
    $PSNativeCommandUseErrorActionPreference = $false
}
$repoRoot = Split-Path -Parent $PSScriptRoot

function Invoke-Gate([string]$Name, [string]$Exe, [string[]]$Arguments, [string]$Dir) {
    Write-Host "`n=== $Name ===" -ForegroundColor Cyan
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    Push-Location $Dir
    try {
        & $Exe @Arguments
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
        Pop-Location
    }
    if ($code -ne 0) { throw "$Name 失败（exit=$code）" }
}

if (-not $SkipRust) {
    $env:GAMER_PROFILE = 'dev'
    Invoke-Gate 'Rust Phase 0 fixtures' 'cargo' @('test', 'phase0_', '--', '--nocapture') (Join-Path $repoRoot 'server')
}
if (-not $SkipWeb) {
    Invoke-Gate 'Vue Phase 0 + existing tests' 'pnpm' @('run', 'test:run') (Join-Path $repoRoot 'web')
}

$baseline = Join-Path $repoRoot 'tools\generate-phase0-baseline.ps1'
$baselineArgs = @()
if ($BuildRelease) { $baselineArgs += '-BuildRelease' }
if ($RunPerf) { $baselineArgs += '-RunPerf' }
if ($BuildRelease -or $RunPerf) {
    & $baseline @baselineArgs
    if ($LASTEXITCODE -ne 0) { throw "Phase 0 baseline 生成失败（exit=$LASTEXITCODE）" }
} else {
    & $baseline -ValidateOnly
    if ($LASTEXITCODE -ne 0) { throw "Phase 0 baseline 校验失败（exit=$LASTEXITCODE）" }
}

Write-Host "`nOK Phase 0 离线护栏完成。外部设备/WebRTC 集成边界见 tests\README.md。" -ForegroundColor Green
