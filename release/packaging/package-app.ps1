# REL-001: 从当前工作树构建并组装 Windows app 组件包 gamer-app-<version>-windows-x64.zip。
#
# 布局 = versions/<version>/ 的内容形态（launcher 安装树的版本目录内容）:
#   gamer-server.exe            cargo build --release（构建前注入构建信息环境变量）
#   web-dist/                   pnpm build 产物（取自 server/web-dist/）
#   assets/scrcpy-server.jar    server/assets/ 原字节
#
# 产物输出到 release/dist/（gitignored）。-SkipBuild 复用既有构建产物
# （server/target/release/gamer-server.exe、server/web-dist/）。
# 组包后自动 Expand-Archive 复核布局与逐文件 sha256。
#
# 兼容 Windows PowerShell 5.1 与 pwsh。

[CmdletBinding()]
param(
    # 跳过 cargo build / pnpm build，直接复用既有产物组装
    [switch]$SkipBuild,
    # 仓库根（默认脚本位置的上两级）
    [string]$RepoRoot = '',
    # 产物输出目录（默认 <repo>/release/dist）
    [string]$DistDir = '',
    # 发布通道（注入 GAMER_CHANNEL，默认 stable）
    [ValidateSet('stable', 'beta')]
    [string]$Channel = 'stable'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

if (-not $RepoRoot) { $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot) }
if (-not $DistDir)  { $DistDir  = Join-Path $RepoRoot 'release\dist' }

$serverDir  = Join-Path $RepoRoot 'server'
$webDir     = Join-Path $RepoRoot 'web'
$serverExe  = Join-Path $serverDir 'target\release\gamer-server.exe'
$webDist    = Join-Path $serverDir 'web-dist'
$jarSrc     = Join-Path $serverDir 'assets\scrcpy-server.jar'

function Exit-Fail {
    param([string]$Message)
    Write-Host "[package-app] FAIL: $Message" -ForegroundColor Red
    exit 1
}

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function New-ZipFromDirectory {
    # 逐文件创建 zip 条目，条目名强制 '/' 分隔。PS 5.1 自带 Compress-Archive
    # 对子目录条目使用 '\' 分隔（跨工具解包损坏），故不用它。
    param(
        [Parameter(Mandatory = $true)][string]$SourceDir,
        [Parameter(Mandatory = $true)][string]$DestFile
    )
    Add-Type -AssemblyName System.IO.Compression | Out-Null
    Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null
    if (Test-Path -LiteralPath $DestFile) { Remove-Item -LiteralPath $DestFile -Force }
    $fs = [System.IO.File]::Open($DestFile, [System.IO.FileMode]::CreateNew)
    $zip = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($f in (Get-ChildItem -LiteralPath $SourceDir -Recurse -File | Sort-Object FullName)) {
            $rel = $f.FullName.Substring($SourceDir.Length + 1) -replace '\\', '/'
            [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $f.FullName, $rel, [System.IO.Compression.CompressionLevel]::Optimal)
        }
    } finally {
        $zip.Dispose()
        $fs.Dispose()
    }
}

# ---------- 版本（权威源 server/Cargo.toml [package].version）----------
$cargoToml = Join-Path $serverDir 'Cargo.toml'
$productVersion = $null
$section = ''
foreach ($line in (Get-Content -LiteralPath $cargoToml)) {
    if ($line -match '^\s*\[([^\]]+)\]\s*$') { $section = $Matches[1].Trim(); continue }
    if ($section -eq 'package' -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
        $productVersion = $Matches[1].Trim(); break
    }
}
if (-not $productVersion) { Exit-Fail "无法从 $cargoToml 读取 [package].version" }

Write-Host "[package-app] 版本: $productVersion（权威源 server/Cargo.toml）"

# ---------- 构建 ----------
if (-not $SkipBuild) {
    # 构建信息注入（server/build.rs 编译期消费，见 server/src/build_info.rs）
    $gitOk = $false
    $commit = ''
    try {
        $commit = (& git -C $RepoRoot rev-parse HEAD 2>&1 | Out-String).Trim()
        if ($LASTEXITCODE -eq 0 -and $commit -match '^[0-9a-f]{40}$') { $gitOk = $true }
    } catch { }
    if (-not $gitOk) { Exit-Fail "git rev-parse HEAD 失败：发布构建必须携带真实 commit（GAMER_GIT_COMMIT）" }

    $buildTime = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    $rustcOut = (& rustc -vV 2>&1 | Out-String)
    $targetTriple = ''
    foreach ($l in ($rustcOut -split "`n")) {
        if ($l -match '^host:\s*(\S+)') { $targetTriple = $Matches[1].Trim(); break }
    }
    if (-not $targetTriple) { Exit-Fail "无法从 rustc -vV 解析 host target triple" }

    Write-Host "[package-app] 构建信息注入: commit=$($commit.Substring(0,12)) time=$buildTime channel=$Channel target=$targetTriple"
    $env:GAMER_GIT_COMMIT  = $commit
    $env:GAMER_BUILD_TIME  = $buildTime
    $env:GAMER_CHANNEL     = $Channel
    $env:GAMER_BUILD_TARGET = $targetTriple

    Write-Host "[package-app] cargo build --release (server)..."
    Push-Location $serverDir
    try { & cargo build --release; if ($LASTEXITCODE -ne 0) { throw "cargo build --release 退出码 $LASTEXITCODE" } }
    finally { Pop-Location }

    Write-Host "[package-app] pnpm build (web)..."
    Push-Location $webDir
    try { & pnpm build; if ($LASTEXITCODE -ne 0) { throw "pnpm build 退出码 $LASTEXITCODE" } }
    finally { Pop-Location }
} else {
    Write-Host "[package-app] -SkipBuild: 复用既有产物"
}

foreach ($must in @($serverExe, (Join-Path $webDist 'index.html'), $jarSrc)) {
    if (-not (Test-Path -LiteralPath $must)) {
        Exit-Fail "缺少构建产物: $must（不带 -SkipBuild 重跑，或先完成 server/web 构建）"
    }
}

# ---------- 组装 staging（versions/<version>/ 内容形态）----------
$stage = Join-Path $DistDir ('staging-app-' + $productVersion)
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

Copy-Item -LiteralPath $serverExe -Destination (Join-Path $stage 'gamer-server.exe')
Copy-Item -Path (Join-Path $webDist '*') -Destination (Join-Path $stage 'web-dist') -Recurse -Force
New-Item -ItemType Directory -Path (Join-Path $stage 'assets') -Force | Out-Null
Copy-Item -LiteralPath $jarSrc -Destination (Join-Path $stage 'assets\scrcpy-server.jar')

$exeSha  = Get-Sha256 -Path (Join-Path $stage 'gamer-server.exe')
$jarSha  = Get-Sha256 -Path (Join-Path $stage 'assets\scrcpy-server.jar')
Write-Host "[package-app] staging 就绪: gamer-server.exe sha256=$($exeSha.Substring(0,16))... jar sha256=$($jarSha.Substring(0,16))..."

# ---------- 打 zip ----------
if (-not (Test-Path -LiteralPath $DistDir)) { New-Item -ItemType Directory -Path $DistDir -Force | Out-Null }
$zipPath = Join-Path $DistDir ('gamer-app-{0}-windows-x64.zip' -f $productVersion)
if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
Write-Host "[package-app] 压缩: $zipPath"
New-ZipFromDirectory -SourceDir $stage -DestFile $zipPath

# ---------- 复核：条目分隔符 + Expand-Archive 布局与 sha256 ----------
Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null
$zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
if ($null -eq $zip) { Exit-Fail "zip 打开失败: $zipPath" }
try {
    $badEntry = $zip.Entries | Where-Object { $_.FullName -like '*\*' } | Select-Object -First 1
    if ($null -ne $badEntry) {
        Exit-Fail "zip 条目含反斜杠分隔（跨工具解包会坏）: $($badEntry.FullName)"
    }
    $entryCount = $zip.Entries.Count
} finally { $zip.Dispose() }

$verifyDir = Join-Path $DistDir ('verify-app-' + $productVersion)
if (Test-Path -LiteralPath $verifyDir) { Remove-Item -LiteralPath $verifyDir -Recurse -Force }
try {
    Expand-Archive -LiteralPath $zipPath -DestinationPath $verifyDir -Force
    foreach ($check in @(@('gamer-server.exe', $exeSha), @('assets\scrcpy-server.jar', $jarSha))) {
        $p = Join-Path $verifyDir $check[0]
        if (-not (Test-Path -LiteralPath $p)) { Exit-Fail "复核缺失: $($check[0])" }
        if ((Get-Sha256 -Path $p) -ne $check[1]) { Exit-Fail "复核 sha256 不符: $($check[0])" }
    }
    if (-not (Test-Path -LiteralPath (Join-Path $verifyDir 'web-dist\index.html'))) {
        Exit-Fail "复核缺失: web-dist/index.html"
    }
    $zipSize = (Get-Item -LiteralPath $zipPath).Length
    Write-Host ("[package-app] PASS: {0}（{1} 字节, {2} 个条目）" -f $zipPath, $zipSize, $entryCount)
    Write-Host ("  gamer-server.exe sha256={0}" -f $exeSha)
    Write-Host ("  assets/scrcpy-server.jar sha256={0}" -f $jarSha)
} finally {
    Remove-Item -LiteralPath $verifyDir -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
exit 0
