# REL-006：GHCR image index 的 provenance + SBOM attestation 门禁。
#
# 在线模式传不带 digest 的 -Image 与必需的 -ExpectedDigest；脚本自行读取
# <image>@<digest> 的 index，再按 attestation manifest 的 subject 和 in-toto
# predicate type 判定 provenance/SBOM。一个 attestation manifest 可包含多个 layer；
# 离线模式传 -IndexPath/-AttestationDir，
# 复用同一套解析逻辑，不访问网络、不读取生产 secrets。

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
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { Fail "JSON 不存在: $Path" }
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

function Get-PropertyValue {
    param([object]$Object, [string]$Name)
    if ($null -eq $Object) { return $null }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}

function Test-AttestationLayers {
    param(
        [Parameter(Mandatory = $true)][object]$Attestation,
        [Parameter(Mandatory = $true)][string]$AttestationDigest
    )

    $layers = @(Get-PropertyValue $Attestation 'layers')
    if ($layers.Count -eq 0) { Fail "attestation $AttestationDigest 缺少非空 layers" }

    $hasProvenance = $false
    $hasSbom = $false
    foreach ($layer in $layers) {
        $layerDigest = [string](Get-PropertyValue $layer 'digest')
        Assert-Digest -Value $layerDigest -Label "attestation $AttestationDigest layer digest"

        $mediaType = ([string](Get-PropertyValue $layer 'mediaType')).ToLowerInvariant()
        $annotations = Get-PropertyValue $layer 'annotations'
        $predicateType = ([string](Get-PropertyValue $annotations 'in-toto.io/predicate-type')).ToLowerInvariant()

        # BuildKit 的 provenance 与 SBOM 都是 application/vnd.in-toto+json layer；
        # 类型由 in-toto predicate URI 区分，而不是由 layer mediaType 区分。
        if ($mediaType -match 'in-toto' -and $predicateType -match 'slsa\.dev/provenance') { $hasProvenance = $true }
        if ($mediaType -match 'in-toto' -and $predicateType -match 'spdx\.dev/document|cyclonedx\.org/bom|cyclonedx') { $hasSbom = $true }

        # 兼容 registry/exporter 直接存放 SPDX/CycloneDX 文档的合法变体，但
        # application/vnd.in-toto+json 没有 predicate type 时不作任何猜测。
        if ($mediaType -match '(^|/)application/(?:vnd\.)?(?:spdx|cyclonedx)\+(?:json|yaml)$') {
            $hasSbom = $true
        }
    }

    return [pscustomobject]@{
        Provenance = $hasProvenance
        Sbom = $hasSbom
    }
}

function Normalize-ImageBase {
    param([string]$Value, [string]$Expected)
    if ([string]::IsNullOrWhiteSpace($Value)) { Fail '-Image 不能为空（在线模式需要镜像仓库引用）' }
    $at = $Value.IndexOf('@')
    if ($at -lt 0) { return $Value }
    $embeddedDigest = $Value.Substring($at + 1)
    Assert-Digest -Value $embeddedDigest -Label 'image digest'
    if ($embeddedDigest -ine $Expected) {
        Fail "-Image 内嵌 digest=$embeddedDigest 与 -ExpectedDigest=$Expected 不一致"
    }
    return $Value.Substring(0, $at)
}

if ([string]::IsNullOrWhiteSpace($ExpectedDigest)) { Fail '必须提供 -ExpectedDigest，attestation 必须绑定 immutable image digest' }
Assert-Digest -Value $ExpectedDigest -Label 'expected digest'

if ([string]::IsNullOrWhiteSpace($IndexPath) -and [string]::IsNullOrWhiteSpace($Image)) {
    Fail '必须提供 -Image 或 -IndexPath'
}

$offline = -not [string]::IsNullOrWhiteSpace($IndexPath)
if ($offline) {
    $index = Read-JsonFile -Path $IndexPath
} else {
    $imageBase = Normalize-ImageBase -Value $Image -Expected $ExpectedDigest
    $indexRaw = Get-OnlineRaw -Reference ('{0}@{1}' -f $imageBase, $ExpectedDigest)
    try { $index = $indexRaw | ConvertFrom-Json }
    catch { Fail "image index 不是合法 JSON：$($_.Exception.Message)" }
}

$indexMediaType = ([string](Get-PropertyValue $index 'mediaType')).ToLowerInvariant()
if ($indexMediaType -ne 'application/vnd.oci.image.index.v1+json') {
    Fail "image index mediaType 必须是 application/vnd.oci.image.index.v1+json，实际: $indexMediaType"
}

$descriptors = @(Get-PropertyValue $index 'manifests')
if ($descriptors.Count -eq 0) { Fail 'image index 缺少非空 manifests 数组' }

$imageDescriptors = @()
$attestations = @()
$seenDigests = @{}
foreach ($descriptor in $descriptors) {
    $digest = [string](Get-PropertyValue $descriptor 'digest')
    Assert-Digest -Value $digest -Label 'index descriptor digest'
    $digestKey = $digest.ToLowerInvariant()
    if ($seenDigests.ContainsKey($digestKey)) { Fail "image index 含重复 descriptor digest: $digest" }
    $seenDigests[$digestKey] = $true

    $annotations = Get-PropertyValue $descriptor 'annotations'
    $referenceType = [string](Get-PropertyValue $annotations 'vnd.docker.reference.type')
    if ($referenceType -ieq 'attestation-manifest') {
        $descriptorMediaType = ([string](Get-PropertyValue $descriptor 'mediaType')).ToLowerInvariant()
        if ($descriptorMediaType -ne 'application/vnd.oci.image.manifest.v1+json') {
            Fail "attestation descriptor $digest 的 mediaType 非 OCI image manifest: $descriptorMediaType"
        }
        $attestations += $descriptor
    } else {
        $imageDescriptors += $descriptor
    }
}

if ($imageDescriptors.Count -eq 0) { Fail 'image index 没有普通 image manifest，不能证明 attestation 属于镜像' }
if ($attestations.Count -lt 1) {
    Fail 'image index 未找到 attestation-manifest'
}

$provenance = $false
$sbom = $false
foreach ($descriptor in $attestations) {
    $digest = [string](Get-PropertyValue $descriptor 'digest')
    $annotations = Get-PropertyValue $descriptor 'annotations'

    if ($offline) {
        if ([string]::IsNullOrWhiteSpace($AttestationDir)) { Fail '离线模式需要 -AttestationDir' }
        $fixtureName = ($digest -replace '^sha256:', 'sha256-') + '.json'
        $attestation = Read-JsonFile -Path (Join-Path $AttestationDir $fixtureName)
    } else {
        $imageBase = Normalize-ImageBase -Value $Image -Expected $ExpectedDigest
        $attestationRaw = Get-OnlineRaw -Reference ('{0}@{1}' -f $imageBase, $digest)
        try { $attestation = $attestationRaw | ConvertFrom-Json }
        catch { Fail "attestation $digest 不是合法 JSON：$($_.Exception.Message)" }
    }

    # 旧式 BuildKit 通过 index descriptor annotation 绑定 subject；OCI artifact
    # 形式还会在 attestation manifest.subject.digest 中重复绑定。至少有一处必须存在，
    # 两处同时存在时必须彼此一致并等于 expected digest。
    $descriptorSubject = [string](Get-PropertyValue $annotations 'vnd.docker.reference.digest')
    $manifestSubjectNode = Get-PropertyValue $attestation 'subject'
    $manifestSubject = [string](Get-PropertyValue $manifestSubjectNode 'digest')
    if ($null -ne $manifestSubjectNode -and [string]::IsNullOrWhiteSpace($manifestSubject)) {
        Fail "attestation $digest 的 OCI subject 存在但缺少 digest"
    }
    if ([string]::IsNullOrWhiteSpace($descriptorSubject) -and [string]::IsNullOrWhiteSpace($manifestSubject)) {
        Fail "attestation $digest 缺少 subject digest（descriptor annotation 与 manifest.subject 均为空）"
    }
    if (-not [string]::IsNullOrWhiteSpace($descriptorSubject) -and -not [string]::IsNullOrWhiteSpace($manifestSubject) -and $descriptorSubject -ine $manifestSubject) {
        Fail "attestation $digest 的 descriptor subject=$descriptorSubject 与 manifest subject=$manifestSubject 不一致"
    }
    $subject = if (-not [string]::IsNullOrWhiteSpace($descriptorSubject)) { $descriptorSubject } else { $manifestSubject }
    Assert-Digest -Value $subject -Label "attestation $digest subject"
    if ($subject -ine $ExpectedDigest) {
        Fail "attestation $digest 的 subject=$subject，不等于 image digest=$ExpectedDigest"
    }

    $evidence = Test-AttestationLayers -Attestation $attestation -AttestationDigest $digest
    if ([bool]$evidence.Provenance) { $provenance = $true }
    if ([bool]$evidence.Sbom) { $sbom = $true }
}

if (-not $provenance) { Fail '未找到 provenance attestation（需要 SLSA provenance predicate）' }
if (-not $sbom) { Fail '未找到 SBOM attestation（需要 SPDX/CycloneDX predicate）' }
Write-Host "[verify-attestations] PASS: $($attestations.Count) attestation manifests 绑定 $ExpectedDigest，provenance + SBOM 均存在"
exit 0
