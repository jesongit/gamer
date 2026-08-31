# REL-006：GHCR image index 的 provenance + SBOM attestation 门禁。
#
# 在线模式传 -Image，由 docker buildx imagetools inspect --raw 读取 index 及两个
# attestation manifest；离线模式传 -IndexPath/-AttestationDir，供 fixture 回归使用。

[CmdletBinding()]
param(
    [string]$Image = '',
    [string]$ExpectedDigest = '',
    [string]$IndexPath = '',
    [string]$AttestationDir = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

function Fail {
    param([string]$Message)
    Write-Error "[verify-attestations] FAIL: $Message"
    exit 1
}

function Assert-Digest {
    param([string]$Value, [string]$Label)
    if ($Value -notmatch '^sha256:[0-9a-fA-F]{64}$') { Fail "$Label 不是 sha256:<64 hex>: $Value" }
}

function Read-JsonFile {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { Fail "JSON 不存在: $Path" }
    try { return (Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json) }
    catch { Fail "JSON 解析失败 $Path：$($_.Exception.Message)" }
}

function Get-OnlineRaw {
    param([string]$Reference)
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) { Fail '在线 attestation 校验缺少 docker' }
    $raw = (& docker buildx imagetools inspect --raw $Reference 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) { Fail "无法读取 $Reference 的 raw manifest：$($raw.Trim())" }
    return $raw
}

if ([string]::IsNullOrWhiteSpace($IndexPath) -and [string]::IsNullOrWhiteSpace($Image)) {
    Fail '必须提供 -Image 或 -IndexPath'
}
if (-not [string]::IsNullOrWhiteSpace($ExpectedDigest)) { Assert-Digest -Value $ExpectedDigest -Label 'expected digest' }

if (-not [string]::IsNullOrWhiteSpace($IndexPath)) {
    $index = Read-JsonFile -Path $IndexPath
    $offline = $true
} else {
    $indexRaw = Get-OnlineRaw -Reference $Image
    try { $index = $indexRaw | ConvertFrom-Json }
    catch { Fail "image index 不是合法 JSON：$($_.Exception.Message)" }
    $offline = $false
}

if ($null -eq $index.manifests) { Fail 'image index 缺少 manifests 数组' }
$descriptors = @($index.manifests)
$attestations = @($descriptors | Where-Object {
    $annotations = $_.PSObject.Properties['annotations']
    $referenceType = if ($null -ne $annotations -and $null -ne $annotations.Value) {
        [string]$annotations.Value.'vnd.docker.reference.type'
    } else {
        ''
    }
    $referenceType -eq 'attestation-manifest'
})
if ($attestations.Count -lt 2) {
    Fail "image index 仅找到 $($attestations.Count) 个 attestation-manifest，至少需要 provenance + SBOM 两个"
}

$provenance = $false
$sbom = $false
foreach ($descriptor in $attestations) {
    $digest = [string]$descriptor.digest
    Assert-Digest -Value $digest -Label 'attestation digest'
    if (-not [string]::IsNullOrWhiteSpace($ExpectedDigest)) {
        $subject = [string]$descriptor.annotations.'vnd.docker.reference.digest'
        if ($subject.ToLowerInvariant() -ne $ExpectedDigest.ToLowerInvariant()) {
            Fail "attestation $digest 的 subject=$subject，不等于 image digest=$ExpectedDigest"
        }
    }

    if ($offline) {
        if ([string]::IsNullOrWhiteSpace($AttestationDir)) { Fail '离线模式需要 -AttestationDir' }
        $fixtureName = ($digest -replace '^sha256:', 'sha256-') + '.json'
        $attestation = Read-JsonFile -Path (Join-Path $AttestationDir $fixtureName)
    } else {
        $attestationRaw = Get-OnlineRaw -Reference ("$Image@$digest")
        try { $attestation = $attestationRaw | ConvertFrom-Json }
        catch { Fail "attestation $digest 不是合法 JSON：$($_.Exception.Message)" }
    }
    if ($null -eq $attestation.layers) { Fail "attestation $digest 缺少 layers" }
    $mediaTypes = @($attestation.layers | ForEach-Object { ([string]$_.mediaType).ToLowerInvariant() })
    if ($mediaTypes | Where-Object { $_ -match 'in-toto' }) { $provenance = $true }
    if ($mediaTypes | Where-Object { $_ -match 'spdx|cyclonedx|syft' }) { $sbom = $true }
}

if (-not $provenance) { Fail '未找到 provenance attestation（layer mediaType 未含 in-toto）' }
if (-not $sbom) { Fail '未找到 SBOM attestation（layer mediaType 未含 SPDX/CycloneDX/Syft）' }
Write-Host "[verify-attestations] PASS: $($attestations.Count) attestation manifests，provenance + SBOM 均存在"
exit 0
