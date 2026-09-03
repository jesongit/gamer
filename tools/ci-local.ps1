#requires -Version 5.1
<#
.SYNOPSIS
    本地复现 .github/workflows/ci.yml 的全部质量门禁（工作流等效性验证）。

.DESCRIPTION
    按 CI 同样的顺序执行：
      rust: cargo fmt --check -> clippy -D warnings -> test -> build --release
      web : pnpm install --frozen-lockfile -> test:run -> build
    任一关卡失败立即中止、以非零码退出并标明失败关卡；全部通过打印汇总表。
    与 GitHub Actions 的差异仅剩运行平台（本机 Windows / CI ubuntu-latest）与缓存层。

.PARAMETER SkipRust
    跳过 Rust 各关卡（调试前端时用）。

.PARAMETER SkipWeb
    跳过 Web 各关卡（调试服务端时用）。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools\ci-local.ps1
#>
[CmdletBinding()]
param(
    [switch]$SkipRust,
    [switch]$SkipWeb
)

$ErrorActionPreference = 'Stop'

# ---- 定位仓库根（脚本位于 <repo>\tools\ 下）----
$RepoRoot = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path (Join-Path $RepoRoot 'web\package.json'))) {
    Write-Host "[precheck] 无法定位仓库根目录（期望包含 web\package.json）：$RepoRoot" -ForegroundColor Red
    exit 1
}

# ---- 工具存在性预检（快速失败，避免跑到一半才报 PATH 问题）----
foreach ($tool in @('cargo', 'node')) {
    if ($null -eq (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Write-Host "[precheck] 缺少工具：$tool （请安装并确保在 PATH 中）" -ForegroundColor Red
        exit 1
    }
}
if ($null -eq (Get-Command pnpm -ErrorAction SilentlyContinue)) {
    # 与 CI 的 Corepack 方案对齐：版本由 web/package.json packageManager 字段固定
    Write-Host '[precheck] 未找到 pnpm，尝试 corepack enable ...' -ForegroundColor Yellow
    corepack enable 2>$null | Out-Null
    if ($null -eq (Get-Command pnpm -ErrorAction SilentlyContinue)) {
        Write-Host '[precheck] 缺少工具：pnpm （corepack enable 后仍不可用，请手动安装）' -ForegroundColor Red
        exit 1
    }
}

# ---- 关卡定义（顺序即执行顺序；与 ci.yml 步骤一一对应）----
$gates = New-Object System.Collections.Generic.List[object]
if (-not $SkipRust) {
    $serverDir = Join-Path $RepoRoot 'server'
    # 与 ci.yml 相同：兼容即将落地的 OPS-004 配置失败即退出策略（dev 档允许无 config.toml）
    $env:GAMER_PROFILE = 'dev'
    $gates.Add([pscustomobject]@{ Group = 'rust'; Name = 'cargo fmt --check';        Exe = 'cargo'; ArgList = @('fmt', '--all', '--', '--check');                       Dir = $serverDir })
    $gates.Add([pscustomobject]@{ Group = 'rust'; Name = 'cargo clippy -D warnings'; Exe = 'cargo'; ArgList = @('clippy', '--all-targets', '--all-features', '--', '-D', 'warnings'); Dir = $serverDir })
    # 无 WASM 退出路径防退化（与 ci.yml 同步）：default 已含 wasm-runtime
    $gates.Add([pscustomobject]@{ Group = 'rust'; Name = 'cargo check --no-default-features'; Exe = 'cargo'; ArgList = @('check', '--locked', '--no-default-features'); Dir = $serverDir })
    $gates.Add([pscustomobject]@{ Group = 'rust'; Name = 'cargo test';               Exe = 'cargo'; ArgList = @('test');                                                Dir = $serverDir })
    $gates.Add([pscustomobject]@{ Group = 'rust'; Name = 'cargo build --release';    Exe = 'cargo'; ArgList = @('build', '--release');                                  Dir = $serverDir })
}
if (-not $SkipWeb) {
    $webDir = Join-Path $RepoRoot 'web'
    $gates.Add([pscustomobject]@{ Group = 'web'; Name = 'pnpm install --frozen-lockfile'; Exe = 'pnpm'; ArgList = @('install', '--frozen-lockfile'); Dir = $webDir })
    $gates.Add([pscustomobject]@{ Group = 'web'; Name = 'pnpm test:run';                  Exe = 'pnpm'; ArgList = @('test:run');                     Dir = $webDir })
    $gates.Add([pscustomobject]@{ Group = 'web'; Name = 'pnpm build';                     Exe = 'pnpm'; ArgList = @('build');                        Dir = $webDir })
}

$total     = $gates.Count
$results   = New-Object System.Collections.Generic.List[object]
$i         = 0
$failedGate = $null

Write-Host "==== GameBot 本地门禁（等效 .github/workflows/ci.yml）：共 $total 关 ====" -ForegroundColor White

foreach ($gate in $gates) {
    $i++
    Write-Host ''
    Write-Host ("===[{0}/{1}] [{2}] {3}===" -f $i, $total, $gate.Group, $gate.Name) -ForegroundColor Cyan
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Push-Location $gate.Dir
    # PS5.1 坑：$ErrorActionPreference='Stop' 时外部命令的 stderr 输出可能被包装成
    # ErrorRecord 终止脚本。门禁期间临时降为 Continue，失败与否只看 $LASTEXITCODE。
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $gate.Exe @($gate.ArgList)
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $prevEap
        Pop-Location
    }
    $sw.Stop()

    $ok = ($code -eq 0)
    $results.Add([pscustomobject]@{
        Group   = $gate.Group
        Gate    = $gate.Name
        Result  = $(if ($ok) { 'PASS' } else { "FAIL(exit=$code)" })
        Seconds = [math]::Round($sw.Elapsed.TotalSeconds, 1)
    })

    if (-not $ok) {
        $failedGate = $gate
        break   # 与 CI 的 fail-fast 一致：第一个失败关卡即中止
    }
}

Write-Host ''
Write-Host '==================== 汇总 ====================' -ForegroundColor White
$results | Format-Table -AutoSize | Out-String -Width 120 | ForEach-Object { Write-Host $_ }

if ($null -ne $failedGate) {
    Write-Host ("X 门禁失败：[{0}] {1}（exit 见上表，详见上方日志）" -f $failedGate.Group, $failedGate.Name) -ForegroundColor Red
    exit 1
}

Write-Host ("OK 全部 {0} 个门禁通过 —— 与 CI 工作流等效性验证通过。" -f $total) -ForegroundColor Green
exit 0
