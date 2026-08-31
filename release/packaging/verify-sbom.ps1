# REL-006：发布 SBOM 内容门禁。
# 校验 CycloneDX 结构、产品版本以及 dependencies.lock.toml 中实际随包分发的
# adb/ffmpeg/scrcpy-server 条目、来源 hash 和逐文件 hash。全程离线。

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$SbomPath,
    [Parameter(Mandatory = $true)][string]$ExpectedVersion,
    [string]$LockPath = '',
    [string]$RepoRoot = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

if (-not $RepoRoot) { $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot) }
if (-not $LockPath) { $LockPath = Join-Path $RepoRoot 'release\dependencies.lock.toml' }

function Fail {
    param([string]$Message)
    Write-Error "[verify-sbom] FAIL: $Message"
    exit 1
}

if (-not (Test-Path -LiteralPath $SbomPath)) { Fail "SBOM 不存在: $SbomPath" }
if (-not (Test-Path -LiteralPath $LockPath)) { Fail "依赖锁文件不存在: $LockPath" }
try { $bom = Get-Content -LiteralPath $SbomPath -Raw | ConvertFrom-Json }
catch { Fail "SBOM 不是合法 JSON: $($_.Exception.Message)" }

if ([string]$bom.bomFormat -cne 'CycloneDX') { Fail "bomFormat 不是 CycloneDX" }
if ([string]$bom.specVersion -cne '1.5') { Fail "specVersion=$($bom.specVersion)，期望 1.5" }
if ($null -eq $bom.metadata -or $null -eq $bom.metadata.component) { Fail 'SBOM 缺少 metadata.component' }
if ([string]$bom.metadata.component.version -cne $ExpectedVersion) {
    Fail "SBOM 产品版本=$($bom.metadata.component.version)，期望 $ExpectedVersion"
}
if ($null -eq $bom.components) { Fail 'SBOM 缺少 components 数组' }

Import-Module (Join-Path $PSScriptRoot 'LockFile.psm1') -Force
$components = Import-LockComponents -Path $LockPath
$bomComponents = @($bom.components)
$seen = @{}
foreach ($entry in $bomComponents) {
    $ref = [string]$entry.'bom-ref'
    if ([string]::IsNullOrWhiteSpace($ref)) { Fail 'SBOM 存在缺少 bom-ref 的 component' }
    if ($seen.ContainsKey($ref)) { Fail "SBOM 存在重复 bom-ref: $ref" }
    $seen[$ref] = $true
}

foreach ($id in @('adb', 'ffmpeg', 'scrcpy-server')) {
    $lock = Get-LockComponent -Components $components -Id $id
    $version = [string]$lock['version']
    $expectedRef = 'pkg:generic/{0}@{1}' -f $id, $version
    $matches = @($bomComponents | Where-Object { [string]$_.'bom-ref' -eq $expectedRef })
    if ($matches.Count -ne 1) { Fail "SBOM 缺少唯一锁定条目: $expectedRef" }
    $entry = $matches[0]
    if ([string]$entry.name -cne $id -or [string]$entry.version -cne $version -or [string]$entry.purl -cne $expectedRef) {
        Fail "SBOM 条目字段不匹配: $expectedRef"
    }
    if ([string]$entry.scope -cne 'required') { Fail "SBOM 条目 scope 不是 required: $id" }

    $sourceHash = [string]$lock['source_sha256']
    $hashes = @($entry.hashes | Where-Object {
        [string]$_.alg -ieq 'SHA-256' -and [string]$_.content -ieq $sourceHash
    })
    if ($hashes.Count -ne 1) { Fail "SBOM 未包含 $id 的 source_sha256" }

    $properties = @($entry.properties)
    foreach ($file in $lock.files) {
        $expected = '{0}={1}' -f ([string]$file['path']), ([string]$file['sha256']).ToLowerInvariant()
        $found = @($properties | Where-Object {
            [string]$_.name -eq 'gamebot:packaged-file-sha256' -and [string]$_.value -ieq $expected
        })
        if ($found.Count -ne 1) { Fail "SBOM 未包含 $id/$($file['path']) 的逐文件 sha256" }
    }
    Write-Host "[verify-sbom] OK: $id@$version"
}

Write-Host "[verify-sbom] PASS: CycloneDX 1.5 / $ExpectedVersion / 3 个锁定运行依赖"
exit 0
