#requires -Version 5.1
# Verify downloaded Release hashes, manifests, SBOM and launcher.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Repository,
    [Parameter(Mandatory = $true)][string]$Tag,
    [string]$Version = '',
    [ValidateSet('stable', 'beta')][string]$Channel = 'stable',
    [string]$DownloadDir = '',
    [switch]$SkipLauncherDoctor
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$repoRoot = Split-Path -Parent $PSScriptRoot
$validator = Join-Path $repoRoot 'release\contracts\validate-manifest.mjs'
$sbomVerifier = Join-Path $repoRoot 'release\packaging\verify-sbom.ps1'
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
        # the exit code so a noisy gh command is not misclassified.
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

    Require-File -Path $validator -Label 'manifest validator'
    Require-File -Path $sbomVerifier -Label 'SBOM verifier'
    $gh = Resolve-Tool @('gh')
    $node = Resolve-Tool @('node')
    if (-not $gh) { Fail 'gh not found; authenticate GitHub CLI before running the external smoke' }
    if (-not $node) { Fail 'node not found; manifest verification cannot run' }
    $powerShell = Resolve-Tool @('pwsh', 'powershell')
    if (-not $powerShell) { Fail 'pwsh or powershell not found; helper verification cannot run' }
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
    if ($downloadedNames.Count -ne 13) {
        Fail "Release download must contain exactly 13 files (12 assets + SHA256SUMS.txt); got $($downloadedNames.Count)"
    }

    $releaseSums = Read-Sha256Sums -SumsPath (Join-Path $DownloadDir 'SHA256SUMS.txt') -BaseDir $DownloadDir -FlatOnly
    if ($releaseSums.Count -ne 12) { Fail "Release SHA256SUMS must contain exactly 12 assets; got $($releaseSums.Count)" }

    $manifestPath = Join-Path $DownloadDir "$Version.json"
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
        "Gamer-$Version-windows-x64-full.zip",
        $appName,
        "Gamer-$Version-licenses.zip",
        "$Version.json",
        "gamer-release.json",
        "gamer-launcher.exe",
        "gamer-sbom-$Version-windows-x64.cdx.json"
    )
    foreach ($component in $components) {
        $componentArtifact = Get-ManifestProperty -Object $component -Name 'artifact' -Label 'manifest component'
        $expectedAssets += [string](Get-ManifestProperty -Object $componentArtifact -Name 'name' -Label 'manifest component artifact')
    }
    Assert-ExactNames -Actual @($releaseSums.Keys) -Expected $expectedAssets -Label 'Release SHA256SUMS'
    Assert-ExactNames -Actual $downloadedNames -Expected @($expectedAssets + 'SHA256SUMS.txt') -Label 'download directory'
    Assert-BytesEqual -Left $manifestPath -Right (Join-Path $DownloadDir 'gamer-release.json') -Label 'versioned and discovery manifests'
    foreach ($artifact in @($appArtifact) + @($components | ForEach-Object { $_.artifact })) {
        $asset = Get-Item -LiteralPath (Join-Path $DownloadDir $artifact.name)
        if ($asset.Length -ne $artifact.size -or (Get-Sha256Path $asset.FullName) -cne $artifact.sha256) {
            Fail "artifact does not match manifest: $($artifact.name)"
        }
    }

    $appPath = Join-Path $DownloadDir $appName
    $declaredAppHash = [string](Get-ManifestProperty -Object $appArtifact -Name 'sha256' -Label 'manifest app artifact')
    $actualAppHash = Get-Sha256Path -Path $appPath
    if ($actualAppHash -cne $declaredAppHash.ToLowerInvariant()) {
        Fail "manifest app sha256=$declaredAppHash, downloaded app sha256=$actualAppHash"
    }
    Write-Host "[release] manifest app artifact binding passed: $appName" -ForegroundColor Green

    $fullName = "Gamer-$Version-windows-x64-full.zip"
    $fullPath = Join-Path $DownloadDir $fullName
    if ($null -eq $temporaryRoot) {
        $temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('gamer-external-release-work-' + [guid]::NewGuid().ToString('N'))
        [IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
    }
    $fullRoot = Join-Path $temporaryRoot 'full'
    [IO.Directory]::CreateDirectory($fullRoot) | Out-Null
    Expand-Archive -LiteralPath $fullPath -DestinationPath $fullRoot
    Test-PackageSums -Root $fullRoot -Label 'full package'

    foreach ($extension in @('json')) {
        Assert-BytesEqual -Left (Join-Path $DownloadDir "$Version.$extension") `
            -Right (Join-Path $fullRoot "manifests\$Version.$extension") `
            -Label "release/package manifest .$extension"
    }
    Write-Host '[release] release manifest and package manifest bytes are identical' -ForegroundColor Green

    Invoke-NativeChecked -FilePath $node -Arguments @(
        $validator, 'check', $manifestPath,
        '--expect-current-version', $Version,
        '--expect-channel', $Channel
    ) -Label 'release manifest verification' | Out-Null
    Invoke-NativeChecked -FilePath $node -Arguments @(
        $validator, 'check', (Join-Path $fullRoot "manifests\$Version.json"),
        '--expect-current-version', $Version, '--expect-channel', $Channel
    ) -Label 'package manifest verification' | Out-Null
    Write-Host '[release] manifest verification passed for release and package copies' -ForegroundColor Green

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

    if ($partial) {
        Write-Host '[external-release] PASS (partial smoke; explicit skips remain)' -ForegroundColor Yellow
    } else {
        Write-Host '[external-release] PASS: full QA-008 external release smoke completed' -ForegroundColor Green
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
