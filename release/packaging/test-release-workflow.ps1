# Release workflow 本地静态/离线行为校验。
# 不访问 GitHub、GHCR、生产 secrets 或真实发布资产；在线结果只由 workflow 门禁产生。

[CmdletBinding()]
param(
    [string]$RepoRoot = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

if (-not $RepoRoot) { $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot) }
$RepoRoot = [IO.Path]::GetFullPath($RepoRoot)

function Fail {
    param([string]$Message)
    Write-Error "[release-workflow-test] FAIL: $Message"
    exit 1
}

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { Fail $Message }
}

function Assert-Text {
    param([string]$Text, [string]$Pattern, [string]$Message)
    if ($Text -notmatch $Pattern) { Fail $Message }
}

function Assert-Ast {
    param([string]$Path)
    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors) | Out-Null
    if ($errors.Count -ne 0) {
        Fail "PowerShell AST 解析失败: $Path`n$($errors -join "`n")"
    }
    Write-Host "[release-workflow-test] AST OK: $Path"
}

function Write-Json {
    param([string]$Path, [object]$Value)
    $json = $Value | ConvertTo-Json -Depth 30
    [IO.File]::WriteAllText($Path, $json + "`n", (New-Object Text.UTF8Encoding($false)))
}

function Invoke-Child {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [int]$ExpectedExit = 0
    )
    $pwsh = Get-Command pwsh -ErrorAction SilentlyContinue
    if (-not $pwsh) { $pwsh = Get-Command powershell -ErrorAction SilentlyContinue }
    if (-not $pwsh) { Fail '本地行为校验需要 pwsh 或 powershell' }
    $savedErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $output = (& $pwsh.Source -NoLogo -NoProfile -File $Path @Arguments 2>&1 | Out-String)
    } finally {
        $ErrorActionPreference = $savedErrorActionPreference
    }
    $code = $LASTEXITCODE
    if ($code -ne $ExpectedExit) {
        Fail "子校验退出码=$code，期望=$ExpectedExit：$Path`n$output"
    }
    if ($ExpectedExit -eq 0) { Write-Host "[release-workflow-test] PASS: $(Split-Path -Leaf $Path)" }
    else { Write-Host "[release-workflow-test] expected reject: $(Split-Path -Leaf $Path)" }
}

$workflowPath = Join-Path $RepoRoot '.github/workflows/release.yml'
if (-not (Test-Path -LiteralPath $workflowPath)) { Fail "workflow 不存在: $workflowPath" }
$workflow = Get-Content -LiteralPath $workflowPath -Raw

foreach ($scriptName in @(
    'check-immutable-release.ps1',
    'verify-sbom.ps1',
    'verify-image-attestations.ps1',
    'verify-key-rotation.ps1'
)) {
    $scriptPath = Join-Path $PSScriptRoot $scriptName
    if (-not (Test-Path -LiteralPath $scriptPath)) { Fail "校验脚本不存在: $scriptPath" }
    Assert-Ast -Path $scriptPath
    Assert-Text -Text $workflow -Pattern ([regex]::Escape($scriptName)) -Message "workflow 未接入 $scriptName"
}
Assert-Ast -Path $PSCommandPath
foreach ($scriptName in @('upgrade-release.ps1', 'mock-docker.ps1', 'test-upgrade-release.ps1')) {
    $scriptPath = Join-Path $PSScriptRoot $scriptName
    if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) { Fail "升级离线测试文件不存在: $scriptPath" }
    Assert-Ast -Path $scriptPath
}

# YAML 不在 PowerShell AST 范围内，这里做发布语义的最小静态契约检查。
Assert-Text $workflow '(?ms)^\s*push:\s*$.*?tags:\s*\[.v\*.' 'workflow 必须只由 v* tag 触发'
Assert-Text $workflow '(?ms)^\s*provenance:\s*mode=max\s*$' 'docker 构建缺少 provenance mode=max'
Assert-Text $workflow '(?ms)^\s*sbom:\s*true\s*$' 'docker 构建缺少 SBOM attestation'
Assert-Text $workflow 'gh release create "\$TAG" --draft' 'draft Release 创建必须带 --draft'
Assert-Text $workflow 'gh release edit "\$TAG" --draft=false' 'publish 必须把 draft 转正式'
Assert-Text $workflow 'check-immutable-release\.ps1\s+-Mode\s+GitHub' 'verify 必须执行 GitHub immutable preflight'
Assert-Text $workflow 'check-immutable-release\.ps1\s+-Mode\s+Registry' 'docker 必须执行 registry immutable preflight'

$draftPos = $workflow.IndexOf("`n  draft-release:")
$artifactPos = $workflow.IndexOf("`n  artifact-verify:")
$smokePos = $workflow.IndexOf("`n  smoke:")
$publishPos = $workflow.IndexOf("`n  publish:")
Assert-True ($draftPos -ge 0 -and $artifactPos -gt $draftPos -and $smokePos -gt $artifactPos -and $publishPos -gt $smokePos) 'job 顺序必须是 draft-release → artifact-verify → smoke → publish'
Assert-Text $workflow '(?ms)^  draft-release:.*?^\s*needs:\s*\[build-windows,\s*docker\]' 'draft-release 必须等待 Windows artifact 与镜像 verify'
Assert-Text $workflow '(?ms)^  artifact-verify:.*?^\s*needs:\s*\[draft-release,\s*docker,\s*build-windows\]' 'artifact-verify 依赖不完整'
Assert-Text $workflow '(?ms)^  smoke:.*?^\s*needs:\s*\[artifact-verify,\s*docker,\s*build-windows\]' 'smoke 必须在 artifact-verify 后运行'
Assert-Text $workflow '(?ms)^  publish:.*?^\s*needs:\s*smoke' 'publish 必须只由 smoke 放行'
Assert-Text $workflow 'verify-sbom\.ps1.*ExpectedVersion' 'SBOM 校验必须绑定发布版本'
Assert-Text $workflow 'verify-image-attestations\.ps1.*ExpectedDigest' '镜像 attestation 校验必须绑定 digest'

$composePath = Join-Path $RepoRoot 'docker-compose.release.yml'
if (-not (Test-Path -LiteralPath $composePath -PathType Leaf)) { Fail "release compose 不存在: $composePath" }
$compose = Get-Content -LiteralPath $composePath -Raw
Assert-Text $compose '(?m)^\s*image:\s+\$\{GAMER_IMAGE:\?' 'release compose 必须显式消费 GAMER_IMAGE'
Assert-True ($compose -notmatch '(?m)^\s*build\s*:') 'release compose 不得包含 build'
Assert-Text $compose '/health/ready' 'release compose 必须保留 /health/ready healthcheck'
Assert-Text $compose '(?m)^\s*-\s*\$\{GAMER_DATA_DIR:' 'release compose 必须绑定 data 目录'
Assert-Text $compose '(?m)^\s*-\s*\$\{GAMER_CONFIG_DIR:' 'release compose 必须绑定 config 目录'
Assert-Text $compose '(?m)^\s*-\s*\$\{GAMER_LOG_DIR:' 'release compose 必须绑定 log 目录'
Write-Host "[release-workflow-test] compose static contract OK: $composePath"

$upgradeBehaviorTest = Join-Path $PSScriptRoot 'test-upgrade-release.ps1'
Invoke-Child -Path $upgradeBehaviorTest -Arguments @('-RepoRoot', $RepoRoot)

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('gamer-release-workflow-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
try {
    $immutable = Join-Path $PSScriptRoot 'check-immutable-release.ps1'
    $sbomVerifier = Join-Path $PSScriptRoot 'verify-sbom.ps1'
    $attestationVerifier = Join-Path $PSScriptRoot 'verify-image-attestations.ps1'
    $rotationVerifier = Join-Path $PSScriptRoot 'verify-key-rotation.ps1'
    $commit = 'a' * 40
    $digest = 'sha256:' + ('b' * 64)
    $otherDigest = 'sha256:' + ('c' * 64)
    $snapshotPath = Join-Path $testRoot 'immutable.json'

    $state = [ordered]@{
        schemaVersion = 1
        tag = 'v0.2.0'
        tagCommit = $commit
        release = [ordered]@{ exists = $false; isDraft = $false }
        image = [ordered]@{ exists = $false; digest = '' }
    }
    Write-Json $snapshotPath $state
    Invoke-Child -Path $immutable -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath)

    $state.release.exists = $true
    $state.release.isDraft = $false
    Write-Json $snapshotPath $state
    Invoke-Child -Path $immutable -ExpectedExit 1 -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath)

    $state.release.exists = $false
    $state.image.exists = $true
    $state.image.digest = $digest
    Write-Json $snapshotPath $state
    Invoke-Child -Path $immutable -ExpectedExit 1 -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath)
    Invoke-Child -Path $immutable -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath, '-ExpectedDigest', $digest)
    Invoke-Child -Path $immutable -ExpectedExit 1 -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath, '-ExpectedDigest', $otherDigest)

    $state.image.exists = $false
    $state.image.digest = ''
    $state.tagCommit = 'd' * 40
    Write-Json $snapshotPath $state
    Invoke-Child -Path $immutable -ExpectedExit 1 -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath)

    Import-Module (Join-Path $PSScriptRoot 'LockFile.psm1') -Force
    $locked = Import-LockComponents -Path (Join-Path $RepoRoot 'release/dependencies.lock.toml')
    $sbomComponents = @()
    foreach ($component in $locked) {
        $id = [string]$component['id']
        $version = [string]$component['version']
        $properties = @()
        foreach ($file in $component.files) {
            $packageFile = '{0}={1}' -f @([string]$file['path'], ([string]$file['sha256']).ToLowerInvariant())
            $properties += [ordered]@{
                name = 'gamebot:packaged-file-sha256'
                value = $packageFile
            }
        }
        $componentRef = 'pkg:generic/{0}@{1}' -f @($id, $version)
        $sbomComponents += [ordered]@{
            type = 'library'
            'bom-ref' = $componentRef
            name = $id
            version = $version
            purl = $componentRef
            scope = 'required'
            hashes = @([ordered]@{ alg = 'SHA-256'; content = ([string]$component['source_sha256']).ToLowerInvariant() })
            properties = $properties
        }
    }
    $bom = [ordered]@{
        bomFormat = 'CycloneDX'
        specVersion = '1.5'
        metadata = [ordered]@{ component = [ordered]@{ type = 'application'; version = '0.2.0' } }
        components = $sbomComponents
    }
    $sbomPath = Join-Path $testRoot 'fixture.cdx.json'
    Write-Json $sbomPath $bom
    Invoke-Child -Path $sbomVerifier -Arguments @('-SbomPath', $sbomPath, '-ExpectedVersion', '0.2.0', '-RepoRoot', $RepoRoot, '-LockPath', (Join-Path $RepoRoot 'release/dependencies.lock.toml'))

    $imageDigest = 'sha256:' + ('1' * 64)
    $provenanceDigest = 'sha256:' + ('2' * 64)
    $sbomDigest = 'sha256:' + ('3' * 64)
    $attestationDir = Join-Path $testRoot 'attestations'
    New-Item -ItemType Directory -Path $attestationDir -Force | Out-Null
    $index = [ordered]@{
        schemaVersion = 2
        manifests = @(
            [ordered]@{ mediaType = 'application/vnd.oci.image.manifest.v1+json'; digest = 'sha256:' + ('4' * 64); size = 1 },
            [ordered]@{ mediaType = 'application/vnd.oci.image.manifest.v1+json'; digest = 'sha256:' + ('5' * 64); size = 1 },
            [ordered]@{ mediaType = 'application/vnd.oci.image.manifest.v1+json'; digest = $provenanceDigest; size = 1; annotations = [ordered]@{ 'vnd.docker.reference.type' = 'attestation-manifest'; 'vnd.docker.reference.digest' = $imageDigest } },
            [ordered]@{ mediaType = 'application/vnd.oci.image.manifest.v1+json'; digest = $sbomDigest; size = 1; annotations = [ordered]@{ 'vnd.docker.reference.type' = 'attestation-manifest'; 'vnd.docker.reference.digest' = $imageDigest } }
        )
    }
    $indexPath = Join-Path $testRoot 'index.json'
    Write-Json $indexPath $index
    Write-Json (Join-Path $attestationDir ('sha256-' + ('2' * 64) + '.json')) ([ordered]@{ schemaVersion = 2; layers = @([ordered]@{ mediaType = 'application/vnd.in-toto+json'; digest = 'sha256:' + ('6' * 64); size = 1 }) })
    Write-Json (Join-Path $attestationDir ('sha256-' + ('3' * 64) + '.json')) ([ordered]@{ schemaVersion = 2; layers = @([ordered]@{ mediaType = 'application/vnd.cyclonedx+json'; digest = 'sha256:' + ('7' * 64); size = 1 }) })
    Invoke-Child -Path $attestationVerifier -Arguments @('-IndexPath', $indexPath, '-AttestationDir', $attestationDir, '-ExpectedDigest', $imageDigest)

    $badIndex = [ordered]@{ schemaVersion = 2; manifests = @($index.manifests[0], $index.manifests[1], $index.manifests[2]) }
    $badIndexPath = Join-Path $testRoot 'bad-index.json'
    Write-Json $badIndexPath $badIndex
    Invoke-Child -Path $attestationVerifier -ExpectedExit 1 -Arguments @('-IndexPath', $badIndexPath, '-AttestationDir', $attestationDir, '-ExpectedDigest', $imageDigest)

    Invoke-Child -Path $rotationVerifier -Arguments @('-FixtureDir', (Join-Path $RepoRoot 'release/contracts/fixtures/key-rotation'))
} finally {
    if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}

Write-Host '[release-workflow-test] PASS: workflow contract + immutable/SBOM/attestation/key-rotation offline behavior'
exit 0
