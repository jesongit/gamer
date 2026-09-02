#requires -Version 5.1
<#!
.SYNOPSIS
  生成或校验 Phase 0 的机器可读基线。

.DESCRIPTION
  默认只记录当前已存在的 server release 二进制；-BuildRelease 会先用当前工作树
  执行 cargo build --release。-RunPerf 显式运行既有固定 H.264/PNG 基准并解析真实
  PERF 行。无法在无设备环境测量的指标保持 value=null，并带 status/reason，绝不填 0。

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\generate-phase0-baseline.ps1 -BuildRelease
  powershell -ExecutionPolicy Bypass -File tools\generate-phase0-baseline.ps1 -RunPerf -BuildRelease
  powershell -ExecutionPolicy Bypass -File tools\generate-phase0-baseline.ps1 -ValidateOnly
#>
[CmdletBinding()]
param(
    [switch]$BuildRelease,
    [switch]$RunPerf,
    [switch]$ValidateOnly,
    [ValidateRange(1, 10000)]
    [int]$Iterations = 20,
    [ValidateRange(0, 1000)]
    [int]$Warmup = 3
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($PSVersionTable.PSVersion.Major -ge 7) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$serverDir = Join-Path $repoRoot 'server'
$baselinePath = Join-Path $repoRoot 'benchmarks\baseline.json'
$manifestPath = Join-Path $repoRoot 'tests\fixtures\manifest.json'

function Invoke-Native([string]$Name, [string[]]$Arguments, [string]$WorkingDirectory) {
    Push-Location $WorkingDirectory
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Name @Arguments
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
        Pop-Location
    }
    if ($code -ne 0) {
        throw "$Name $($Arguments -join ' ') 失败（exit=$code）"
    }
}

function Get-NativeVersion([string]$Name) {
    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -eq $command) { return $null }
    Push-Location $repoRoot
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = @(& $Name --version 2>&1 | ForEach-Object { [string]$_ })
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
        Pop-Location
    }
    # Windows ffmpeg builds may surface the normal --version stderr exit as a
    # signed native status; the version line itself is the useful evidence.
    if ($output.Count -eq 0) { return $null }
    return ([string]$output[0]).Trim()
}

function Read-Json([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "文件不存在：$Path"
    }
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    return [IO.File]::ReadAllText($Path, $utf8) | ConvertFrom-Json
}

if ($ValidateOnly) {
    $doc = Read-Json $baselinePath
    if ($doc.schema_version -ne 1) { throw "baseline schema_version 必须为 1" }
    if ([string]::IsNullOrWhiteSpace([string]$doc.git_commit)) { throw 'baseline 缺少 git_commit' }
    foreach ($metricName in @(
        'server_idle_rss_mb', 'server_idle_cpu_percent', 'scrcpy_connect_p95_ms',
        'screenshot_p95_ms', 'find_p95_ms', 'match_many_p95_ms',
        'gop_cache_peak_bytes', 'db_log_write_p95_ms', 'webrtc_stability'
    )) {
        $metric = $doc.metrics.$metricName
        if ($null -eq $metric) { throw "baseline 缺少指标：$metricName" }
        if ($metric.status -eq 'not_measured' -and $null -ne $metric.value) {
            throw "未测指标 $metricName 不得填写 value"
        }
        if ([string]::IsNullOrWhiteSpace([string]$metric.status)) {
            throw "指标 $metricName 缺少 status"
        }
    }
    Write-Host "[baseline] schema valid: $baselinePath" -ForegroundColor Green
    return
}

if ($BuildRelease) {
    $env:GAMER_PROFILE = 'dev'
    Write-Host '[baseline] cargo build --release ...' -ForegroundColor Cyan
    Invoke-Native 'cargo' @('build', '--release') $serverDir
}

$commit = ([string](& git -C $repoRoot rev-parse HEAD)).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($commit)) {
    throw '无法读取当前 git commit'
}

$manifestHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $manifestPath).Hash.ToLowerInvariant()
$binaryCandidates = @(
    (Join-Path $serverDir 'target\release\gamer-server.exe'),
    (Join-Path $serverDir 'target\release\gamer-server')
)
$binaryPath = $binaryCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
if ($null -ne $binaryPath) {
    $relativeBinaryPath = $binaryPath.Substring($repoRoot.Length).TrimStart('\', '/')
    $binaryInfo = [ordered]@{
        status = 'available'
        path = $relativeBinaryPath.Replace('\', '/')
        bytes = (Get-Item -LiteralPath $binaryPath).Length
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $binaryPath).Hash.ToLowerInvariant()
    }
} else {
    $binaryInfo = [ordered]@{
        status = 'not_available'
        path = 'server/target/release/gamer-server[.exe]'
        bytes = $null
        sha256 = $null
        reason = '尚未执行 cargo build --release；使用 -BuildRelease 生成'
    }
}

$metricReasons = [ordered]@{
    server_idle_rss_mb = '需要启动服务并在固定空闲窗口采集进程 RSS；默认不启动服务'
    server_idle_cpu_percent = '需要启动服务并在固定空闲窗口采集 CPU；默认不启动服务'
    scrcpy_connect_p95_ms = '需要真实 Android/ADB/scrcpy 会话；默认离线测试不接触设备'
    screenshot_p95_ms = '需要 -RunPerf；其余截图链路需要真实设备'
    find_p95_ms = '需要 -RunPerf；固定 PNG 模板 NCC 仅作为离线代理'
    match_many_p95_ms = '需要 -RunPerf；以固定基准的 find_round 多模板样本为来源'
    gop_cache_peak_bytes = '需要真实 scrcpy 帧流期间采集 FrameCache 峰值'
    db_log_write_p95_ms = '需要单独的固定高压日志写入采集；当前仅有行为护栏'
    webrtc_stability = '需要浏览器 WebRTC peer/网络环境；默认 CI 不宣称投屏稳定性'
}
$metrics = [ordered]@{}
foreach ($name in $metricReasons.Keys) {
    $unit = if ($name -match 'rss') { 'MiB' } elseif ($name -match 'percent') { '%' } elseif ($name -match 'bytes') { 'bytes' } elseif ($name -match 'stability') { 'qualitative' } else { 'ms' }
    $metrics[$name] = [ordered]@{
        value = $null
        unit = $unit
        status = 'not_measured'
        reason = $metricReasons[$name]
    }
}

$perfReports = [ordered]@{}
$perfStatus = 'not_run'
$perfReason = '未执行固定基准；使用 -RunPerf 且显式具备 ffmpeg 后采集'
if ($RunPerf) {
    $ffmpeg = Get-Command ffmpeg -ErrorAction SilentlyContinue
    if ($null -eq $ffmpeg) { throw '请求 -RunPerf 但 PATH 中没有 ffmpeg；不能伪造性能值' }
    $perfStatus = 'measured'
    $perfReason = $null
    $env:GAMER_PERF_ITERS = [string]$Iterations
    $env:GAMER_PERF_WARMUP = [string]$Warmup
    $oldLocation = Get-Location
    Push-Location $serverDir
    try {
        foreach ($testName in @(
            'matcher::tests::fixed_fixture_benchmark_p50_p95',
            'frames::tests::fixed_fixture_decode_stage_benchmark_p50_p95'
        )) {
            Write-Host "[baseline] cargo test --release $testName ..." -ForegroundColor Cyan
            $lines = @(& cargo test --release $testName -- --ignored --nocapture 2>&1)
            $code = $LASTEXITCODE
            if ($code -ne 0) { throw "固定基准失败（exit=$code）：$testName" }
            foreach ($line in $lines) {
                $text = [string]$line
                if ($text -match 'PERF metric=(?<metric>\S+).*p50_us=(?<p50>\S+).*p95_us=(?<p95>\S+).*max_us=(?<max>\S+)') {
                    $perfReports[$Matches.metric] = [ordered]@{
                        p50_us = [int64]$Matches.p50
                        p95_us = [int64]$Matches.p95
                        max_us = [int64]$Matches.max
                    }
                }
            }
        }
    } finally {
        Pop-Location
    }
    if ($perfReports.Count -eq 0) {
        throw '固定基准完成但没有输出 PERF metric 行；拒绝生成无证据基线'
    }
    $metricSources = [ordered]@{
        screenshot_p95_ms = 'decode_latest_png'
        find_p95_ms = 'find_round'
        match_many_p95_ms = 'find_round'
    }
    foreach ($target in $metricSources.Keys) {
        $source = $metricSources[$target]
        if ($perfReports.Contains($source)) {
            $metrics[$target].value = [math]::Round($perfReports[$source].p95_us / 1000.0, 3)
            $metrics[$target].status = 'measured'
            $metrics[$target].source_metric = $source
            $metrics[$target].iterations = $Iterations
        } else {
            $metrics[$target].reason = "基准没有输出 PERF metric=$source；拒绝填入推测值"
        }
    }
}

$doc = [ordered]@{
    schema_version = 1
    generated_by = 'tools/generate-phase0-baseline.ps1'
    captured_at_utc = [DateTime]::UtcNow.ToString('o')
    git_commit = $commit
    toolchain = [ordered]@{
        cargo = Get-NativeVersion 'cargo'
        rustc = Get-NativeVersion 'rustc'
        node = Get-NativeVersion 'node'
        pnpm = Get-NativeVersion 'pnpm'
        ffmpeg = Get-NativeVersion 'ffmpeg'
        os = [Environment]::OSVersion.VersionString
    }
    fixture_manifest = [ordered]@{
        path = 'tests/fixtures/manifest.json'
        sha256 = $manifestHash
    }
    release_build = $binaryInfo
    benchmark = [ordered]@{
        status = $perfStatus
        iterations = if ($RunPerf) { $Iterations } else { $null }
        warmup = if ($RunPerf) { $Warmup } else { $null }
        command = 'tools/run-perf-benchmark.ps1 -Release'
        reason = $perfReason
        reports = $perfReports
    }
    metrics = $metrics
    external_boundaries = [ordered]@{
        android = 'integration_only'
        adb = 'integration_only'
        scrcpy = 'integration_only'
        ffmpeg = 'explicit_offline_benchmark'
        webrtc_peer = 'integration_only'
    }
}

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $baselinePath) | Out-Null
$json = $doc | ConvertTo-Json -Depth 12
[IO.File]::WriteAllText($baselinePath, "$json`n", (New-Object Text.UTF8Encoding($false)))
Write-Host "[baseline] wrote $baselinePath" -ForegroundColor Green
if ($binaryInfo.status -eq 'available') {
    Write-Host "[baseline] release binary: $($binaryInfo.bytes) bytes sha256=$($binaryInfo.sha256)" -ForegroundColor Green
} else {
    Write-Host "[baseline] release binary unavailable; no placeholder value written" -ForegroundColor Yellow
}
