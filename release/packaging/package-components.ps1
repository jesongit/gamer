# REL-003: 把 release/vendor/ 已校验的组件解包产物按 manifest required_files
# 形态打包为发行组件 zip（zip 内平铺: adb.exe+AdbWinApi.dll+AdbWinUsbApi.dll /
# ffmpeg.exe），输出到 release/dist/。
#
# 命名: gamer-<组件id>-<版本>-windows-x64.zip（与 manifest fixture 命名惯例一致）。
# 打包前先按 dependencies.lock.toml 逐文件校验 vendor；打包后解包回读并逐条目
# 复核 sha256/size 与条目分隔符。
#
# 兼容 Windows PowerShell 5.1 与 pwsh。

[CmdletBinding()]
param(
    # 依赖锁文件（默认 <repo>/release/dependencies.lock.toml）
    [string]$LockPath = '',
    # vendor 根目录（默认 <repo>/release/vendor）
    [string]$VendorRoot = '',
    # 产物输出目录（默认 <repo>/release/dist）
    [string]$DistDir = '',
    # 只打包指定组件（默认 adb + ffmpeg）
    [ValidateSet('adb', 'ffmpeg')]
    [string[]]$ComponentIds = @('adb', 'ffmpeg')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $LockPath)   { $LockPath   = Join-Path $repoRoot 'release\dependencies.lock.toml' }
if (-not $VendorRoot) { $VendorRoot = Join-Path $repoRoot 'release\vendor' }
if (-not $DistDir)    { $DistDir    = Join-Path $repoRoot 'release\dist' }

Import-Module (Join-Path $PSScriptRoot 'LockFile.psm1') -Force

function Exit-Fail {
    param([string]$Message)
    Write-Host "[package-components] FAIL: $Message" -ForegroundColor Red
    exit 1
}

function Get-Sha256Path {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

$components = Import-LockComponents -Path $LockPath
if (-not (Test-Path -LiteralPath $DistDir)) { New-Item -ItemType Directory -Path $DistDir -Force | Out-Null }

foreach ($id in $ComponentIds) {
    $c = Get-LockComponent -Components $components -Id $id
    $version = [string]$c['version']
    $vendorDir = Join-Path $VendorRoot "$id\$version"

    # ---------- 前置: vendor 与锁 files[] 逐文件一致 ----------
    if (-not (Test-Path -LiteralPath $vendorDir)) {
        Exit-Fail "vendor 目录不存在: $vendorDir（先运行 release/packaging/fetch-$id.ps1）"
    }
    Write-Host "[package-components] 校验 vendor/$id@$version 与锁 files[] 一致性..."
    $ok = Test-LockFilesInDir -Files $c.files -Dir $vendorDir
    if (-not $ok) { Exit-Fail "vendor/$id@$version 与锁文件不一致，拒绝打包" }

    # ---------- staging: 平铺所需文件 ----------
    $stage = Join-Path $DistDir ("staging-{0}-{1}" -f $id, $version)
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    try {
        foreach ($f in $c.files) {
            $name = [string]$f['path']
            if ($name -match '[\\/]|\.\.') { Exit-Fail "锁 files[] 含非法路径: $name" }
            Copy-Item -LiteralPath (Join-Path $vendorDir $name) -Destination (Join-Path $stage $name)
        }

        $zipPath = Join-Path $DistDir ('gamer-{0}-{1}-windows-x64.zip' -f $id, $version)
        if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
        Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zipPath -CompressionLevel Optimal
    } catch {
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
        throw
    }
    Remove-Item -LiteralPath $stage -Recurse -Force

    # ---------- 复核: zip 条目集合、分隔符、逐条目 sha256/size 与锁一致 ----------
    Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null
    $zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
    if ($null -eq $zip) { Exit-Fail "zip 打开失败: $zipPath" }
    try {
        $expectedNames = @($c.files | ForEach-Object { [string]$_['path'] })
        $actualNames = @($zip.Entries | ForEach-Object { $_.FullName })
        foreach ($n in $actualNames) {
            if ($n -like '*\*') { Exit-Fail "zip 条目含反斜杠分隔: $n" }
        }
        if (Compare-Object -ReferenceObject ($expectedNames | Sort-Object) -DifferenceObject ($actualNames | Sort-Object)) {
            Exit-Fail ("zip 条目集合与锁 files[] 不符: 期望 [{0}] 实际 [{1}]" -f ($expectedNames -join ', '), ($actualNames -join ', '))
        }
        $tmpDir = Join-Path $DistDir ("verify-{0}-{1}" -f $id, $version)
        if (Test-Path -LiteralPath $tmpDir) { Remove-Item -LiteralPath $tmpDir -Recurse -Force }
        New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null
        foreach ($f in $c.files) {
            $name = [string]$f['path']
            $entry = $zip.Entries | Where-Object { $_.FullName -ieq $name } | Select-Object -First 1
            if ($entry.Length -ne [long]$f['size']) {
                Remove-Item -LiteralPath $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
                Exit-Fail "zip 条目大小不符: $name（期望 $($f['size'])，实际 $($entry.Length)）"
            }
            $outP = Join-Path $tmpDir $name
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $outP, $true)
            if ((Get-Sha256Path -Path $outP) -ne ([string]$f['sha256']).ToLowerInvariant()) {
                Remove-Item -LiteralPath $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
                Exit-Fail "zip 条目 sha256 不符: $name"
            }
        }
        Remove-Item -LiteralPath $tmpDir -Recurse -Force
        $zipSize = (Get-Item -LiteralPath $zipPath).Length
        Write-Host ("[package-components] PASS: {0}（{1} 字节, 条目: {2}）" -f $zipPath, $zipSize, ($actualNames -join ', '))
    } finally { $zip.Dispose() }
}
exit 0
