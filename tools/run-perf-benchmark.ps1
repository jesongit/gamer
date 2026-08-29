# run-perf-benchmark.ps1 —— PERF-002/003 固定夹具匹配基准
#requires -Version 5.1
#
# 只执行被 #[ignore] 标记的 Rust 基准测试；输入来自 server/testdata/perf，
# 输出为实际测得的 p50/p95/max 微秒值，不写回仓库、不伪造目标或对比数据。
# 依次运行两个基准（独立 cargo 进程，互不竞争资源）：
#   1. matcher::tests::fixed_fixture_benchmark_p50_p95     —— decode/PNG/NCC/template/find 整体耗时
#   2. frames::tests::fixed_fixture_decode_stage_benchmark_p50_p95 —— ffmpeg 按需解码内部分段
#      （spawn/write/decode/png 四段，PERF metric=ffmpeg_start/ffmpeg_input/ffmpeg_decode/ffmpeg_png，
#      可直接被 tools/perf-stage5b-stats.mjs 的指标别名消费）
#
# 示例（仓库根目录）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\run-perf-benchmark.ps1
#   powershell ... tools\run-perf-benchmark.ps1 -Iterations 50 -FullScreen

[CmdletBinding()]
param(
    [ValidateRange(1, 10000)]
    [int]$Iterations = 20,
    [ValidateRange(0, 1000)]
    [int]$Warmup = 3,
    [ValidateRange(50, 100)]
    [int]$DecodeFreshnessMs = 75,
    [switch]$FullScreen,
    [switch]$Release,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
$server = Join-Path $root 'server'
$fixture = Join-Path $server 'testdata\perf\manifest.json'
if (-not (Test-Path -LiteralPath $fixture -PathType Leaf)) {
    throw "固定夹具不存在：$fixture。请先运行 tools\gen-perf-fixtures.ps1。"
}

$env:GAMER_PERF_ITERS = [string]$Iterations
$env:GAMER_PERF_WARMUP = [string]$Warmup
$env:GAMER_DECODE_FRESHNESS_MS = [string]$DecodeFreshnessMs
if ($FullScreen) {
    $env:GAMER_PERF_FULL_SCREEN = '1'
} else {
    Remove-Item Env:GAMER_PERF_FULL_SCREEN -ErrorAction SilentlyContinue
}

Write-Host ('{0} fixture={1} iterations={2} warmup={3} freshness_ms={4} release={5} full_screen={6}' -f '[perf]', $fixture, $Iterations, $Warmup, $DecodeFreshnessMs, $Release, $FullScreen)
if ($DryRun) {
    Write-Host '[perf] dry-run: cargo test skipped'
    return
}

Push-Location $server
try {
    $benchTests = @(
        'matcher::tests::fixed_fixture_benchmark_p50_p95',
        'frames::tests::fixed_fixture_decode_stage_benchmark_p50_p95'
    )
    foreach ($test in $benchTests) {
        if ($Release) {
            & cargo test --release $test -- --ignored --nocapture
        } else {
            & cargo test $test -- --ignored --nocapture
        }
        $exitCode = $LASTEXITCODE
        if ($exitCode -ne 0) {
            throw ('Rust 固定夹具基准失败（exit={0}）：{1}' -f $exitCode, $test)
        }
    }
} finally {
    Pop-Location
}
