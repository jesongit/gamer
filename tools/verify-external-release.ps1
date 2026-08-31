#requires -Version 5.1
<#
.SYNOPSIS
    Re-download and verify a real GitHub Release and, optionally, its GHCR image.

.DESCRIPTION
    This is an online smoke check. It must be pointed at a real Release and, for
    the full QA-008 check, a real GHCR image digest plus the tag commit SHA.
    Missing tools, authentication, assets, signatures, labels, or digests fail
    the command. No missing external source is treated as a pass.

    The Release half verifies SHA256SUMS, the full-package sums, manifest byte
    identity, both manifest trust anchors, the app artifact binding, the SBOM,
    and the two launcher doctor invocations. The GHCR half re-pulls both the
    version tag and immutable digest, then checks digest identity and OCI labels.
    Unless explicitly skipped, it also calls the existing attestation verifier.

.EXAMPLE
    .\tools\verify-external-release.ps1 -Repository OWNER/REPO -Tag v0.2.0 `
        -CommitSha <40-hex-commit> -Image ghcr.io/owner/repo `
        -Digest sha256:<64-hex-digest> -DownloadDir .\qa-008\v0.2.0

.EXAMPLE
    .\tools\verify-external-release.ps1 -Repository OWNER/REPO -Tag v0.2.0 `
        -DownloadDir .\qa-008\v0.2.0
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Repository,
    [Parameter(Mandatory = $true)][string]$Tag,
    [string]$Version = '',
    [ValidateSet('stable', 'beta')][string]$Channel = 'stable',
    [string]$CommitSha = '',
    [string]$Image = '',
    [string]$Digest = '',
    [string]$DownloadDir = '',
    [switch]$SkipLauncherDoctor,
    [switch]$SkipAttestations
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$repoRoot = Split-Path -Parent $PSScriptRoot
$validator = Join-Path $repoRoot 'release\contracts\validate-manifest.mjs'
$sbomVerifier = Join-Path $repoRoot 'release\packaging\verify-sbom.ps1'
$attestationVerifier = Join-Path $repoRoot 'release\packaging\verify-image-attestations.ps1'
$keysDir = Join-Path $repoRoot 'release\keys'
$temporaryRoot = $null
$partial = $false

function Fail {
    param([string]$Message)
    throw $Message
}

function Require-File {
    param([string]$Path, [string]$Label)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        Fail "$Label not found: $Path"
    }
}

function Require-Directory {
    param([string]$Path, [string]$Label)
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        Fail "$Label not found: $Path"
    }
}

function Resolve-Tool {
    param([string[]]$Names)
    foreach ($name in $Names) {
        $command = Get-Command $name -ErrorAction SilentlyContinue
        if ($null -ne $command) { return $command.Source }
    }
    return $null
}

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )

    $savedPreference = $ErrorActionPreference
    try {
        # PowerShell 5.1 can turn native stderr into ErrorRecord objects while
        # the process is otherwise healthy. Capture both streams and judge only
        # the exit code so a noisy docker/gh command is not misclassified.
        $ErrorActionPreference = 'Continue'
        $output = (& $FilePath @Arguments 2>&1 | Out-String)
        $exitCode = $LASTEXITCODE
    } catch {
        $output = $_ | Out-String
        $exitCode = 1
    } finally {
        $ErrorActionPreference = $savedPreference
    }
    return [pscustomobject]@{
        ExitCode = $exitCode
        Output   = $output
    }
}

function Invoke-NativeChecked {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $result = Invoke-Native -FilePath $FilePath -Arguments $Arguments
    if ($result.ExitCode -ne 0) {
        $detail = $result.Output.Trim()
        if ($detail.Length -gt 4000) { $detail = $detail.Substring(0, 4000) }
        Fail "$Label failed (exit=$($result.ExitCode)): $detail"
    }
    return $result
}

function Get-Sha256Path {
    param([string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Read-Sha256Sums {
    param(
        [Parameter(Mandatory = $true)][string]$SumsPath,
        [Parameter(Mandatory = $true)][string]$BaseDir,
        [switch]$FlatOnly
    )

    Require-File -Path $SumsPath -Label 'SHA256SUMS file'
    $values = @{}
    foreach ($line in (Get-Content -LiteralPath $SumsPath -Encoding UTF8)) {
        if ($line.Trim().Length -eq 0) { continue }
        if ($line -notmatch '^([0-9a-f]{64})  (.+)$') {
            Fail "invalid SHA256SUMS line: $line"
        }
        $hash = $Matches[1].ToLowerInvariant()
        $name = [string]$Matches[2]
        if ([string]::IsNullOrWhiteSpace($name)) {
            Fail 'SHA256SUMS entry name is empty'
        }
        if ($name -match '(^[\\/]|^[A-Za-z]:|(^|[\\/])\.\.?([\\/]|$)|:)') {
            Fail "SHA256SUMS entry contains an unsafe path: $name"
        }
        if ($FlatOnly -and $name -match '[\\/]') {
            Fail "SHA256SUMS entry is not a flat asset name: $name"
        }
        if ($values.ContainsKey($name)) { Fail "duplicate SHA256SUMS entry: $name" }
        $values[$name] = $hash

        $filePath = Join-Path $BaseDir $name
        Require-File -Path $filePath -Label "SHA256SUMS asset $name"
        $actual = Get-Sha256Path -Path $filePath
        if ($actual -cne $hash) {
            Fail "SHA256 mismatch for ${name}: expected $hash, actual $actual"
        }
    }
    if ($values.Count -eq 0) { Fail "SHA256SUMS is empty: $SumsPath" }
    return ,$values
}

function Assert-ExactNames {
    param(
        [Parameter(Mandatory = $true)][string[]]$Actual,
        [Parameter(Mandatory = $true)][string[]]$Expected,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $actualSet = @{}
    foreach ($name in $Actual) {
        if ($actualSet.ContainsKey($name)) { Fail "$Label contains duplicate name: $name" }
        $actualSet[$name] = $true
    }
    $expectedSet = @{}
    foreach ($name in $Expected) {
        if ($expectedSet.ContainsKey($name)) { Fail "$Label expected set contains duplicate name: $name" }
        $expectedSet[$name] = $true
    }
    $missing = @($Expected | Where-Object { -not $actualSet.ContainsKey($_) })
    $unexpected = @($Actual | Where-Object { -not $expectedSet.ContainsKey($_) })
    if ($missing.Count -gt 0 -or $unexpected.Count -gt 0) {
        Fail "$Label mismatch; missing=[$($missing -join ', ')], unexpected=[$($unexpected -join ', ')]"
    }
}

function Assert-BytesEqual {
    param(
        [Parameter(Mandatory = $true)][string]$Left,
        [Parameter(Mandatory = $true)][string]$Right,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $leftBytes = [IO.File]::ReadAllBytes($Left)
    $rightBytes = [IO.File]::ReadAllBytes($Right)
    if ($leftBytes.Length -ne $rightBytes.Length) {
        Fail "$Label byte lengths differ: $($leftBytes.Length) vs $($rightBytes.Length)"
    }
    for ($i = 0; $i -lt $leftBytes.Length; $i++) {
        if ($leftBytes[$i] -ne $rightBytes[$i]) {
            Fail "$Label differs at byte offset $i"
        }
    }
}

function Get-ManifestProperty {
    param(
        [Parameter(Mandatory = $true)][object]$Object,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property -or $null -eq $property.Value) { Fail "$Label is missing: $Name" }
    return $property.Value
}

function Get-JsonFile {
    param([string]$Path, [string]$Label)
    Require-File -Path $Path -Label $Label
    try {
        return (Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json)
    } catch {
        Fail "$Label is not valid JSON: $($_.Exception.Message)"
    }
}

function Test-PackageSums {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $sumsPath = Join-Path $Root 'SHA256SUMS.txt'
    $sums = Read-Sha256Sums -SumsPath $sumsPath -BaseDir $Root
    $allFiles = @(Get-ChildItem -LiteralPath $Root -Recurse -File | ForEach-Object {
        $_.FullName.Substring($Root.Length + 1) -replace '\\', '/'
    })
    foreach ($file in $allFiles) {
        if ($file -ieq 'SHA256SUMS.txt') { continue }
        if (-not $sums.ContainsKey($file)) { Fail "$Label file is not covered by SHA256SUMS: $file" }
    }
    if ($sums.Count -ne ($allFiles.Count - 1)) {
        Fail "$Label SHA256SUMS coverage count mismatch: entries=$($sums.Count), files=$($allFiles.Count - 1)"
    }
    Write-Host "[release] $Label SHA256SUMS passed ($($sums.Count) files)" -ForegroundColor Green
}

function Get-DockerInspect {
    param(
        [Parameter(Mandatory = $true)][string]$Docker,
        [Parameter(Mandatory = $true)][string]$Reference
    )
    $result = Invoke-NativeChecked -FilePath $Docker -Arguments @('image', 'inspect', $Reference) -Label "docker image inspect $Reference"
    try {
        $items = @($result.Output | ConvertFrom-Json)
    } catch {
        Fail "docker image inspect returned invalid JSON for ${Reference}: $($_.Exception.Message)"
    }
    if ($items.Count -ne 1) { Fail "docker image inspect returned $($items.Count) records for $Reference" }
    return $items[0]
}

function Get-OciLabel {
    param(
        [Parameter(Mandatory = $true)][object]$Inspect,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Reference
    )
    $labels = $Inspect.Config.Labels
    if ($null -eq $labels) { Fail "OCI labels are missing for ${Reference}" }
    $property = $labels.PSObject.Properties[$Name]
    if ($null -eq $property) { Fail "OCI label $Name is missing for $Reference" }
    return [string]$property.Value
}

function Assert-ImageLabels {
    param(
        [Parameter(Mandatory = $true)][object]$Inspect,
        [Parameter(Mandatory = $true)][string]$Reference,
        [Parameter(Mandatory = $true)][string]$ExpectedVersion,
        [Parameter(Mandatory = $true)][string]$ExpectedCommit,
        [Parameter(Mandatory = $true)][string]$ExpectedSource
    )
    $versionLabel = Get-OciLabel -Inspect $Inspect -Name 'org.opencontainers.image.version' -Reference $Reference
    $revisionLabel = Get-OciLabel -Inspect $Inspect -Name 'org.opencontainers.image.revision' -Reference $Reference
    $sourceLabel = Get-OciLabel -Inspect $Inspect -Name 'org.opencontainers.image.source' -Reference $Reference
    if ($versionLabel -cne $ExpectedVersion) {
        Fail "OCI version label=$versionLabel for $Reference, expected $ExpectedVersion"
    }
    if ($revisionLabel -ine $ExpectedCommit) {
        Fail "OCI revision label=$revisionLabel for $Reference, expected $ExpectedCommit"
    }
    if ($sourceLabel.TrimEnd('/') -ine $ExpectedSource.TrimEnd('/')) {
        Fail "OCI source label=$sourceLabel for $Reference, expected $ExpectedSource"
    }
    Write-Host "[ghcr] labels passed: version=$versionLabel revision=$revisionLabel" -ForegroundColor Green
}

function Assert-RepoDigest {
    param(
        [Parameter(Mandatory = $true)][object]$Inspect,
        [Parameter(Mandatory = $true)][string]$Reference,
        [Parameter(Mandatory = $true)][string]$ExpectedDigest
    )
    $repoDigests = @($Inspect.RepoDigests | ForEach-Object { [string]$_ })
    foreach ($repoDigest in $repoDigests) {
        $match = [regex]::Match($repoDigest, '@(?<digest>sha256:[0-9a-fA-F]{64})$')
        if ($match.Success -and $match.Groups['digest'].Value.ToLowerInvariant() -ceq $ExpectedDigest) {
            return
        }
    }
    Fail "${Reference} does not resolve to expected digest $ExpectedDigest; RepoDigests=$($repoDigests -join ', ')"
}

try {
    if ($Repository -notmatch '^[^/\s]+/[^/\s]+$') {
        Fail "Repository must be OWNER/REPO: $Repository"
    }
    if ($Tag -notmatch '^v(?<tagVersion>[^/\s]+)$') {
        Fail "Tag must be v<version>: $Tag"
    }
    $tagVersion = $Matches['tagVersion']
    if (-not $Version) { $Version = $tagVersion }
    if ($Version -cne $tagVersion) { Fail "Version $Version does not match tag $Tag" }
    $semverPattern = '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-([0-9A-Za-z-]+)(\.[0-9A-Za-z-]+)*)?(\+([0-9A-Za-z-]+)(\.[0-9A-Za-z-]+)*)?$'
    if ($Version -notmatch $semverPattern) { Fail "Version is not SemVer: $Version" }

    if ($Image) {
        if (-not $CommitSha) { Fail 'GHCR verification requires -CommitSha' }
        if (-not $Digest) { Fail 'GHCR verification requires -Digest from the published image' }
        if ($Image -notmatch '^ghcr\.io/[^:@\s]+(?:/[^:@\s]+)*$') {
            Fail "Image must be an untagged GHCR repository reference: $Image"
        }
        if ($CommitSha -notmatch '^[0-9a-fA-F]{40,64}$') { Fail "CommitSha is not a 40/64-hex SHA: $CommitSha" }
        if ($Digest -notmatch '^sha256:[0-9a-fA-F]{64}$') { Fail "Digest is not sha256:<64 hex>: $Digest" }
        $CommitSha = $CommitSha.ToLowerInvariant()
        $Digest = $Digest.ToLowerInvariant()
    } elseif ($CommitSha -or $Digest) {
        Fail '-CommitSha and -Digest are only valid together with -Image'
    }

    Require-File -Path $validator -Label 'manifest validator'
    Require-File -Path $sbomVerifier -Label 'SBOM verifier'
    Require-Directory -Path $keysDir -Label 'repository trust anchor directory'
    $gh = Resolve-Tool @('gh')
    $node = Resolve-Tool @('node')
    if (-not $gh) { Fail 'gh not found; authenticate GitHub CLI before running the external smoke' }
    if (-not $node) { Fail 'node not found; manifest verification cannot run' }
    $powerShell = Resolve-Tool @('pwsh', 'powershell')
    if (-not $powerShell) { Fail 'pwsh or powershell not found; helper verification cannot run' }
    if ($Image) {
        $docker = Resolve-Tool @('docker')
        if (-not $docker) { Fail 'docker not found; GHCR verification cannot run' }
    }

    if ([string]::IsNullOrWhiteSpace($DownloadDir)) {
        $temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('gamer-external-release-' + [guid]::NewGuid().ToString('N'))
        $DownloadDir = Join-Path $temporaryRoot 'assets'
        [IO.Directory]::CreateDirectory($DownloadDir) | Out-Null
    } else {
        $DownloadDir = [IO.Path]::GetFullPath($DownloadDir)
        if (Test-Path -LiteralPath $DownloadDir) {
            $existing = @(Get-ChildItem -LiteralPath $DownloadDir -Force)
            if ($existing.Count -ne 0) {
                Fail "DownloadDir must be empty to prevent stale external evidence: $DownloadDir"
            }
        } else {
            [IO.Directory]::CreateDirectory($DownloadDir) | Out-Null
        }
    }

    Write-Host "[release] downloading $Tag from $Repository"
    Invoke-NativeChecked -FilePath $gh -Arguments @(
        'release', 'download', $Tag, '--repo', $Repository, '--pattern', '*',
        '--dir', $DownloadDir, '--clobber'
    ) -Label 'GitHub Release download' | Out-Null

    $downloadItems = @(Get-ChildItem -LiteralPath $DownloadDir -Force)
    $downloadDirectories = @($downloadItems | Where-Object { $_.PSIsContainer })
    if ($downloadDirectories.Count -ne 0) {
        Fail "Release download directory contains unexpected directories: $($downloadDirectories.Name -join ', ')"
    }
    $downloadedNames = @($downloadItems | ForEach-Object { $_.Name })
    if ($downloadedNames.Count -ne 9) {
        Fail "Release download must contain exactly 9 files (8 assets + SHA256SUMS.txt); got $($downloadedNames.Count)"
    }

    $releaseSums = Read-Sha256Sums -SumsPath (Join-Path $DownloadDir 'SHA256SUMS.txt') -BaseDir $DownloadDir -FlatOnly
    if ($releaseSums.Count -ne 8) { Fail "Release SHA256SUMS must contain exactly 8 assets; got $($releaseSums.Count)" }

    $manifestPath = Join-Path $DownloadDir "$Version.json"
    $signaturePath = Join-Path $DownloadDir "$Version.sig"
    $manifest = Get-JsonFile -Path $manifestPath -Label 'downloaded release manifest'
    $manifestRelease = Get-ManifestProperty -Object $manifest -Name 'release' -Label 'manifest'
    $manifestVersion = [string](Get-ManifestProperty -Object $manifestRelease -Name 'version' -Label 'manifest.release')
    $manifestChannel = [string](Get-ManifestProperty -Object $manifestRelease -Name 'channel' -Label 'manifest.release')
    if ($manifestVersion -cne $Version) { Fail "manifest version=$manifestVersion, expected $Version" }
    if ($manifestChannel -cne $Channel) { Fail "manifest channel=$manifestChannel, expected $Channel" }

    $platforms = Get-ManifestProperty -Object $manifest -Name 'platforms' -Label 'manifest'
    $platform = Get-ManifestProperty -Object $platforms -Name 'windows-x86_64' -Label 'manifest.platforms'
    $app = Get-ManifestProperty -Object $platform -Name 'app' -Label 'manifest platform'
    $appArtifact = Get-ManifestProperty -Object $app -Name 'artifact' -Label 'manifest app'
    $appName = [string](Get-ManifestProperty -Object $appArtifact -Name 'name' -Label 'manifest app artifact')
    $components = @(Get-ManifestProperty -Object $platform -Name 'components' -Label 'manifest platform')
    if ($components.Count -eq 0) { Fail 'manifest has no components' }

    $expectedAssets = @(
        "GameBot-$Version-windows-x64-full.zip",
        $appName,
        "GameBot-$Version-licenses.zip",
        "$Version.json",
        "$Version.sig",
        "gamer-sbom-$Version-windows-x64.cdx.json"
    )
    foreach ($component in $components) {
        $componentArtifact = Get-ManifestProperty -Object $component -Name 'artifact' -Label 'manifest component'
        $expectedAssets += [string](Get-ManifestProperty -Object $componentArtifact -Name 'name' -Label 'manifest component artifact')
    }
    Assert-ExactNames -Actual @($releaseSums.Keys) -Expected $expectedAssets -Label 'Release SHA256SUMS'
    Assert-ExactNames -Actual $downloadedNames -Expected @($expectedAssets + 'SHA256SUMS.txt') -Label 'download directory'

    $appPath = Join-Path $DownloadDir $appName
    $declaredAppHash = [string](Get-ManifestProperty -Object $appArtifact -Name 'sha256' -Label 'manifest app artifact')
    $actualAppHash = Get-Sha256Path -Path $appPath
    if ($actualAppHash -cne $declaredAppHash.ToLowerInvariant()) {
        Fail "manifest app sha256=$declaredAppHash, downloaded app sha256=$actualAppHash"
    }
    Write-Host "[release] manifest app artifact binding passed: $appName" -ForegroundColor Green

    $fullName = "GameBot-$Version-windows-x64-full.zip"
    $fullPath = Join-Path $DownloadDir $fullName
    if ($null -eq $temporaryRoot) {
        $temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('gamer-external-release-work-' + [guid]::NewGuid().ToString('N'))
        [IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
    }
    $fullRoot = Join-Path $temporaryRoot 'full'
    [IO.Directory]::CreateDirectory($fullRoot) | Out-Null
    Expand-Archive -LiteralPath $fullPath -DestinationPath $fullRoot
    Test-PackageSums -Root $fullRoot -Label 'full package'

    foreach ($extension in @('json', 'sig')) {
        Assert-BytesEqual -Left (Join-Path $DownloadDir "$Version.$extension") `
            -Right (Join-Path $fullRoot "manifests\$Version.$extension") `
            -Label "release/package manifest .$extension"
    }
    Write-Host '[release] release manifest and package manifest bytes are identical' -ForegroundColor Green

    Invoke-NativeChecked -FilePath $node -Arguments @(
        $validator, 'check', $manifestPath, '--sig', $signaturePath,
        '--keys-dir', $keysDir, '--expect-current-version', $Version,
        '--expect-channel', $Channel
    ) -Label 'repository trust-anchor manifest verification' | Out-Null
    Invoke-NativeChecked -FilePath $node -Arguments @(
        $validator, 'check', (Join-Path $fullRoot "manifests\$Version.json"),
        '--sig', (Join-Path $fullRoot "manifests\$Version.sig"),
        '--keys-dir', (Join-Path $fullRoot 'keys'),
        '--expect-current-version', $Version, '--expect-channel', $Channel
    ) -Label 'package trust-anchor manifest verification' | Out-Null
    Write-Host '[release] manifest verification passed with repository and package trust anchors' -ForegroundColor Green

    $sbomPath = Join-Path $DownloadDir "gamer-sbom-$Version-windows-x64.cdx.json"
    Invoke-NativeChecked -FilePath $powerShell -Arguments @(
        '-NoLogo', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $sbomVerifier,
        '-SbomPath', $sbomPath, '-ExpectedVersion', $Version, '-RepoRoot', $repoRoot
    ) -Label 'downloaded SBOM contract verification' | Out-Null

    if ($SkipLauncherDoctor) {
        $partial = $true
        Write-Host '[release] launcher doctor skipped by explicit -SkipLauncherDoctor' -ForegroundColor Yellow
    } else {
        if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
            Fail 'launcher doctor requires Windows; use -SkipLauncherDoctor only for an explicitly partial asset smoke'
        }
        $launcher = Join-Path $fullRoot 'gamer-launcher.exe'
        Require-File -Path $launcher -Label 'full package launcher'
        Invoke-NativeChecked -FilePath $launcher -Arguments @(
            '--install-root', $fullRoot, 'doctor'
        ) -Label 'launcher doctor inventory smoke' | Out-Null
        Invoke-NativeChecked -FilePath $launcher -Arguments @(
            '--install-root', $fullRoot, 'doctor', '--manifest',
            (Join-Path $fullRoot "manifests\$Version.json"),
            '--expect-current-version', $Version, '--expect-channel', $Channel
        ) -Label 'launcher doctor manifest smoke' | Out-Null
        Write-Host '[release] launcher doctor inventory + manifest smoke passed' -ForegroundColor Green
    }

    if ($Image) {
        $tagReference = "$Image`:$Version"
        $digestReference = "$Image@$Digest"
        $expectedSource = "https://github.com/$Repository"

        Invoke-NativeChecked -FilePath $docker -Arguments @('pull', $tagReference) -Label "GHCR version-tag pull $tagReference" | Out-Null
        $tagInspect = Get-DockerInspect -Docker $docker -Reference $tagReference
        Assert-ImageLabels -Inspect $tagInspect -Reference $tagReference `
            -ExpectedVersion $Version -ExpectedCommit $CommitSha -ExpectedSource $expectedSource

        Assert-RepoDigest -Inspect $tagInspect -Reference $tagReference -ExpectedDigest $Digest

        Invoke-NativeChecked -FilePath $docker -Arguments @('pull', $digestReference) -Label "GHCR immutable digest pull $digestReference" | Out-Null
        $digestInspect = Get-DockerInspect -Docker $docker -Reference $digestReference
        Assert-ImageLabels -Inspect $digestInspect -Reference $digestReference `
            -ExpectedVersion $Version -ExpectedCommit $CommitSha -ExpectedSource $expectedSource
        Assert-RepoDigest -Inspect $digestInspect -Reference $digestReference -ExpectedDigest $Digest
        if ([string]$tagInspect.Id -ine [string]$digestInspect.Id) {
            Fail "version tag and digest reference resolved to different image IDs"
        }
        Write-Host "[ghcr] version tag -> digest -> labels identity passed: $Image $Version $Digest" -ForegroundColor Green

        if ($SkipAttestations) {
            $partial = $true
            Write-Host '[ghcr] attestation verification skipped by explicit -SkipAttestations' -ForegroundColor Yellow
        } else {
            Require-File -Path $attestationVerifier -Label 'image attestation verifier'
            Invoke-NativeChecked -FilePath $powerShell -Arguments @(
                '-NoLogo', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $attestationVerifier,
                '-Image', $digestReference, '-ExpectedDigest', $Digest
            ) -Label 'GHCR provenance/SBOM attestation verification' | Out-Null
            Write-Host '[ghcr] provenance + SBOM attestation verification passed' -ForegroundColor Green
        }
    } else {
        $partial = $true
        Write-Host '[ghcr] not run: no -Image/-Digest/-CommitSha supplied (release-only result)' -ForegroundColor Yellow
    }

    if ($partial) {
        Write-Host '[external-release] PASS (partial smoke; explicit skips or GHCR omission remain)' -ForegroundColor Yellow
    } else {
        Write-Host '[external-release] PASS: full QA-008 external release + GHCR smoke completed' -ForegroundColor Green
    }
    exit 0
} catch {
    Write-Error "[external-release] NOT COMPLETE: $($_.Exception.Message)"
    exit 1
} finally {
    if ($null -ne $temporaryRoot -and (Test-Path -LiteralPath $temporaryRoot)) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
