# Release workflow 本地静态/离线行为校验。
# 不访问 GitHub、生产 secrets 或真实发布资产；在线结果只由 workflow 门禁产生。

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
    'verify-key-rotation.ps1'
)) {
    $scriptPath = Join-Path $PSScriptRoot $scriptName
    if (-not (Test-Path -LiteralPath $scriptPath)) { Fail "校验脚本不存在: $scriptPath" }
    Assert-Ast -Path $scriptPath
    Assert-Text -Text $workflow -Pattern ([regex]::Escape($scriptName)) -Message "workflow 未接入 $scriptName"
}
Assert-Ast -Path $PSCommandPath
# YAML 不在 PowerShell AST 范围内，这里做发布语义的最小静态契约检查。
Assert-Text $workflow '(?ms)^\s*push:\s*$.*?tags:\s*\[.v\*.' 'workflow 必须只由 v* tag 触发'
Assert-Text $workflow 'gh release create "\$TAG" --repo "\$GH_REPO" --draft --verify-tag' 'draft Release 创建必须带 --draft 且绑定明确 repo'
Assert-Text $workflow 'gh release edit "\$TAG" --repo "\$GH_REPO" --draft=false' 'publish 必须把 draft 转正式且绑定明确 repo'
Assert-Text $workflow 'check-immutable-release\.ps1\s+-Mode\s+GitHub' 'verify 必须执行 GitHub immutable preflight'
Assert-Text $workflow '\$keyId -notmatch.*prod-ed25519-\[1-9\]' '生产签名必须拒绝 dev/fixture key，只允许 prod-ed25519-N'
Assert-True ($workflow -notmatch 'check-immutable-release\.ps1[^\r\n]*stable') 'stable 滚动别名不得走 immutable version preflight'
Assert-Text $workflow '"channel=\$channel"\s*>>\s*\$env:GITHUB_OUTPUT' 'Windows 构建必须从 tag 派生并输出 channel'
Assert-Text $workflow 'package-app\.ps1\s+-Channel \$env:CHANNEL' 'app 包构建必须消费派生 channel'
Assert-Text $workflow 'gen-manifest\.ps1\s+-SkipSign\s+-Channel \$env:CHANNEL' 'manifest 必须消费派生 channel'
Assert-Text $workflow '--expect-current-version \$env:VERSION --expect-channel \$env:CHANNEL' '签名后的 manifest 验签必须绑定派生 channel'
Assert-Text $workflow 'release/keys.*\*\.private\.pem' '生产签名必须拒绝仓库内私钥文件'

$draftPos = $workflow.IndexOf("`n  draft-release:")
$uploadPos = $workflow.IndexOf("`n  upload-assets:")
$artifactPos = $workflow.IndexOf("`n  artifact-verify:")
$smokePos = $workflow.IndexOf("`n  smoke:")
$publishPos = $workflow.IndexOf("`n  publish:")
Assert-True ($draftPos -ge 0 -and $uploadPos -gt $draftPos -and $artifactPos -gt $uploadPos -and $smokePos -gt $artifactPos -and $publishPos -gt $smokePos) 'job 顺序必须是 draft-release → upload-assets → artifact-verify → smoke → publish'
Assert-Text $workflow '(?ms)^  draft-release:.*?^\s*needs:\s*verify' 'draft-release 必须直接位于 tag/verify 后'
Assert-Text $workflow '(?ms)^  build-windows:.*?^\s*needs:\s*\[verify,\s*draft-release\]' 'build-windows 必须等待 draft 建立'
Assert-Text $workflow '(?ms)^  upload-assets:.*?^\s*needs:\s*\[draft-release,\s*build-windows\]' 'upload-assets 依赖不完整'
Assert-Text $workflow '(?ms)^  artifact-verify:.*?^\s*needs:\s*\[upload-assets,\s*build-windows\]' 'artifact-verify 依赖不完整'
Assert-Text $workflow '(?ms)^  smoke:.*?^\s*needs:\s*\[artifact-verify,\s*build-windows\]' 'smoke 必须在 artifact-verify 后运行'
Assert-Text $workflow '(?ms)^  publish:.*?^\s*needs:\s*smoke' 'publish 必须只由 smoke 放行'
Assert-Text $workflow 'verify-sbom\.ps1.*ExpectedVersion' 'SBOM 校验必须绑定发布版本'
Assert-Text $workflow 'gh release download "\$env:TAG" --repo "\$env:GH_REPO"' '跨 job 下载 Release 资产必须显式绑定 repo'

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('gamer-release-workflow-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
try {
    $immutable = Join-Path $PSScriptRoot 'check-immutable-release.ps1'
    $sbomVerifier = Join-Path $PSScriptRoot 'verify-sbom.ps1'
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
    }
    Write-Json $snapshotPath $state
    Invoke-Child -Path $immutable -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath)

    $state.release.exists = $true
    $state.release.isDraft = $false
    Write-Json $snapshotPath $state
    Invoke-Child -Path $immutable -ExpectedExit 1 -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath)

    $state.release.exists = $false
    $state.tagCommit = 'd' * 40
    Write-Json $snapshotPath $state
    Invoke-Child -Path $immutable -ExpectedExit 1 -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath)

    $state.tagCommit = $commit
    Write-Json $snapshotPath $state
    Invoke-Child -Path $immutable -ExpectedExit 1 -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0+build.1', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath)
    Invoke-Child -Path $immutable -ExpectedExit 1 -Arguments @('-Mode', 'Snapshot', '-Tag', 'v01.2.0', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath)
    Invoke-Child -Path $immutable -ExpectedExit 1 -Arguments @('-Mode', 'Snapshot', '-Tag', 'v0.2.0-01', '-CommitSha', $commit, '-SnapshotPath', $snapshotPath)

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

    Invoke-Child -Path $rotationVerifier -Arguments @('-FixtureDir', (Join-Path $RepoRoot 'release/contracts/fixtures/key-rotation'))
} finally {
    if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}

Write-Host '[release-workflow-test] PASS: workflow contract + immutable/SBOM/key-rotation offline behavior'
exit 0
