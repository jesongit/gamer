# VER-001: product version single-source check.
# Authoritative source: `version` under [package] in server/Cargo.toml.
# Compares (when present): web/package.json version, optional -Tag (v<semver>),
# optional -Manifest <file> (release.release.version). Any mismatch => non-zero exit.
#
# Cross-platform: plain PowerShell (pwsh on Linux CI / Windows PowerShell 5.1+),
# ASCII-only output, no external module dependencies.

[CmdletBinding()]
param(
    # Expected release tag, e.g. "v0.2.0". Must equal "v" + Cargo package version.
    [string]$Tag,
    # Path to a release manifest JSON; its release.release.version must equal the
    # Cargo package version.
    [string]$Manifest
)

$ErrorActionPreference = 'Stop'

function Fail {
    param([string[]]$Messages)
    foreach ($m in $Messages) {
        Write-Host "[version-check] MISMATCH: $m" -ForegroundColor Red
    }
    Write-Host "[version-check] FAILED: $($Messages.Count) version mismatch(es)." -ForegroundColor Red
    exit 1
}

# Repo root = parent of tools/ where this script lives.
$repoRoot = (Split-Path -Parent $PSScriptRoot)
$cargoPath = Join-Path $repoRoot 'server/Cargo.toml'
$webPkgPath = Join-Path $repoRoot 'web/package.json'

foreach ($f in @($cargoPath, $webPkgPath)) {
    if (-not (Test-Path -LiteralPath $f)) {
        Fail @("required file not found: $f")
    }
}

# ---- Parse Cargo.toml [package].version (section-scoped regex, no TOML module) ----
$cargoVersion = $null
$section = ''
foreach ($line in Get-Content -LiteralPath $cargoPath) {
    if ($line -match '^\s*\[([^\]]+)\]\s*$') {
        $section = $Matches[1].Trim()
        continue
    }
    if ($section -eq 'package' -and $line -match '^\s*version\s*=\s*"([^"]+)"\s*(#.*)?$') {
        $cargoVersion = $Matches[1].Trim()
        break
    }
}
if ([string]::IsNullOrEmpty($cargoVersion)) {
    Fail @("cannot read package.version from $cargoPath")
}

# ---- Parse web/package.json version ----
try {
    $webJson = Get-Content -LiteralPath $webPkgPath -Raw | ConvertFrom-Json
} catch {
    Fail @("cannot parse $webPkgPath as JSON: $($_.Exception.Message)")
}
$webVersion = $webJson.version
if ([string]::IsNullOrEmpty($webVersion)) {
    Fail @("cannot read version from $webPkgPath")
}

$errors = New-Object System.Collections.Generic.List[string]

# web/package.json is package metadata only; it must stay in lockstep with Cargo.
if ($webVersion -ne $cargoVersion) {
    $errors.Add("web/package.json version '$webVersion' != Cargo package.version '$cargoVersion'")
}

# ---- Optional: release tag ----
if (-not [string]::IsNullOrWhiteSpace($Tag)) {
    $tagNorm = $Tag.Trim()
    if ($tagNorm -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+([\-+].*)?$') {
        $errors.Add("tag '$tagNorm' is not in v<semver> form (e.g. v0.2.0)")
    } elseif ($tagNorm -ne "v$cargoVersion") {
        $errors.Add("tag '$tagNorm' != 'v$cargoVersion' (v + Cargo package.version)")
    }
}

# ---- Optional: release manifest ----
if (-not [string]::IsNullOrWhiteSpace($Manifest)) {
    $manifestPath = $Manifest.Trim()
    if (-not (Test-Path -LiteralPath $manifestPath)) {
        $errors.Add("manifest file not found: $manifestPath")
    } else {
        try {
            $manifestJson = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        } catch {
            $manifestJson = $null
            $errors.Add("cannot parse manifest $manifestPath as JSON: $($_.Exception.Message)")
        }
        if ($null -ne $manifestJson) {
            $manifestVersion = $manifestJson.release.version
            if ([string]::IsNullOrEmpty($manifestVersion)) {
                $errors.Add("manifest $manifestPath has no release.release.version")
            } elseif ($manifestVersion -ne $cargoVersion) {
                $errors.Add("manifest release.release.version '$manifestVersion' != Cargo package.version '$cargoVersion'")
            }
        }
    }
}

if ($errors.Count -gt 0) {
    Fail $errors.ToArray()
}

Write-Host "[version-check] OK: product version '$cargoVersion' (Cargo == web$(
    if ($Tag) { ", tag '$($Tag.Trim())'" }
)$(if ($Manifest) { ", manifest '$($Manifest.Trim())'" }))."
exit 0
