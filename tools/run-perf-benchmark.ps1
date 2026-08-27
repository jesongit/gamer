# run-perf-benchmark.ps1 —— PERF-002/003 固定夹具匹配基准
#
# 只执行被 #[ignore] 标记的 Rust 基准测试；输入来自 server/testdata/perf，
# 输出为实际测得的 p50/p95/max 微秒值，不写回仓库、不伪造目标或对比数据。
#
# 示例（仓库根目录）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\run-perf-benchmark.ps1
#   powershell ... tools\run-perf-benchmark.ps1 -Iterations 50 -FullScreen

param(
    [ValidateRange(1, 10000)]
    [int]$Iterations = 20,
    [ValidateRange(0, 1000)]
    [int]$Warmup = 3,
    [switch]$FullScreen,
    [switch]$Release
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
if ($FullScreen) {
    $env:GAMER_PERF_FULL_SCREEN = '1'
} else {
    Remove-Item Env:GAMER_PERF_FULL_SCREEN -ErrorAction SilentlyContinue
}

Write-Host "[perf] fixture=$fixture iterations=$Iterations warmup=$Warmup release=$Release full_screen=$FullScreen"
Push-Location $server
try {
    if ($Release) {
        & cargo test --release matcher::tests::fixed_fixture_benchmark_p50_p95 -- --ignored --nocapture
    } else {
        & cargo test matcher::tests::fixed_fixture_benchmark_p50_p95 -- --ignored --nocapture
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Rust 固定夹具基准失败（exit=$LASTEXITCODE）"
    }
} finally {
    Pop-Location
}
