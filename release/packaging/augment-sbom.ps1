# REL-006：把发行运行依赖补入 CycloneDX SBOM。
#
# tools/gen-sbom.ps1 负责从两个 Cargo.lock 收集 Rust 依赖；本脚本在同一份
# SBOM 上追加 dependencies.lock.toml 中锁定的 adb、ffmpeg、scrcpy-server，
# 使发布 SBOM 同时覆盖 Rust crate 与实际随包分发的运行依赖。所有版本、来源、
# hash、许可和逐文件 hash 均从 release/ 事实源读取，不依赖网络或凭据。

[CmdletBinding()]
param(
    [string]$SbomPath = '',
    [string]$LockPath = '',
    [string]$RepoRoot = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

if (-not $RepoRoot) { $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot) }
if (-not $SbomPath) { $SbomPath = Join-Path $RepoRoot 'release\sbom' }
if (-not $LockPath) { $LockPath = Join-Path $RepoRoot 'release\dependencies.lock.toml' }

function Fail {
    param([string]$Message)
    Write-Host "[augment-sbom] FAIL: $Message" -ForegroundColor Red
    exit 1
}

if (-not (Test-Path -LiteralPath $SbomPath)) { Fail "SBOM 不存在: $SbomPath（先运行 tools/gen-sbom.ps1）" }
if (-not (Test-Path -LiteralPath $LockPath)) { Fail "依赖锁文件不存在: $LockPath" }

Import-Module (Join-Path $PSScriptRoot 'LockFile.psm1') -Force

try {
    $bom = Get-Content -LiteralPath $SbomPath -Raw | ConvertFrom-Json
} catch {
    Fail "SBOM 不是合法 JSON: $SbomPath（$($_.Exception.Message)）"
}
if ($null -eq $bom.components) { Fail "SBOM 缺少 components 数组: $SbomPath" }

$components = Import-LockComponents -Path $LockPath
$seen = @{}
foreach ($c in @($bom.components)) {
    if ($c.'bom-ref') { $seen[[string]$c.'bom-ref'] = $true }
}

$added = 0
foreach ($id in @('adb', 'ffmpeg', 'scrcpy-server')) {
    $c = Get-LockComponent -Components $components -Id $id
    $version = [string]$c['version']
    $bomRef = 'pkg:generic/{0}@{1}' -f $id, $version
    if ($seen.ContainsKey($bomRef)) { continue }

    $hashes = @()
    if ($c.ContainsKey('source_sha256')) {
        $hashes += [ordered]@{ alg = 'SHA-256'; content = ([string]$c['source_sha256']).ToLowerInvariant() }
    }
    $properties = @()
    foreach ($f in $c.files) {
        $properties += [ordered]@{
            name  = 'gamebot:packaged-file-sha256'
            value = '{0}={1}' -f ([string]$f['path']), ([string]$f['sha256']).ToLowerInvariant()
        }
    }
    $licenses = @([ordered]@{ license = [ordered]@{ id = [string]$c['license'] } })
    $external = @()
    if ($c.ContainsKey('source_url')) {
        $external += [ordered]@{ type = 'distribution'; url = [string]$c['source_url'] }
    }
    if ($c.ContainsKey('source_offer')) {
        $external += [ordered]@{ type = 'vcs'; url = [string]$c['source_offer'] }
    }
    if ($c.ContainsKey('license_url')) {
        $external += [ordered]@{ type = 'website'; url = [string]$c['license_url'] }
    }

    $entry = [ordered]@{
        type              = 'library'
        'bom-ref'         = $bomRef
        name              = $id
        version           = $version
        purl              = $bomRef
        scope             = 'required'
        hashes            = $hashes
        licenses          = $licenses
        externalReferences = $external
        properties        = $properties
    }
    $bom.components += $entry
    $seen[$bomRef] = $true
    $added++
}

$json = $bom | ConvertTo-Json -Depth 20
[System.IO.File]::WriteAllText($SbomPath, $json + "`n", (New-Object System.Text.UTF8Encoding($false)))
Write-Host "[augment-sbom] OK: 追加 $added 个运行依赖条目，覆盖 adb/ffmpeg/scrcpy-server"
exit 0
