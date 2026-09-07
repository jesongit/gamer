# Build script for the "vision-probe" example plugin (Windows PowerShell).
# Produces dist/com.example.visionprobe-<version>.gplugin using the repo's own
# tools/plugin-signer CLI. No signing key is needed (unsigned by design).
#
# Usage:
#   pwsh ./build.ps1                    # from a gamer repo checkout
#   pwsh ./build.ps1 -Signer <exe>      # standalone copy: point at plugin-signer
#
# Requires: cargo with the wasm32-unknown-unknown target
#   rustup target add wasm32-unknown-unknown
param(
    [string]$Signer = ""
)
$ErrorActionPreference = "Stop"
$exampleDir = $PSScriptRoot
$manifest = Join-Path $exampleDir "manifest.toml"

# 1. Locate (or build) the repo's plugin-signer CLI.
if ($Signer -eq "") {
    # sdk/examples/vision-probe -> repo root is three levels up.
    $repoRoot = (Resolve-Path (Join-Path $exampleDir "..\..\..")).Path
    $Signer = Join-Path $repoRoot "tools\plugin-signer\target\release\gamer-plugin-signer.exe"
    if (-not (Test-Path $Signer)) {
        Write-Host "[build] building tools/plugin-signer (one-time)..."
        cargo build --release --manifest-path (Join-Path $repoRoot "tools\plugin-signer\Cargo.toml")
        if ($LASTEXITCODE -ne 0) { exit 1 }
    }
}

# 2. Guest: core module (wasm32) -> WASM Component.
$targetDir = Join-Path $exampleDir "target"
cargo build --release --lib --target wasm32-unknown-unknown `
    --manifest-path (Join-Path $exampleDir "Cargo.toml") --target-dir $targetDir
if ($LASTEXITCODE -ne 0) { exit 1 }
$module = Join-Path $targetDir "wasm32-unknown-unknown\release\vision_probe_plugin_guest.wasm"
$component = Join-Path $targetDir "plugin.component.wasm"
cargo run --release --bin componentize `
    --manifest-path (Join-Path $exampleDir "Cargo.toml") --target-dir $targetDir `
    -- $module $component
if ($LASTEXITCODE -ne 0) { exit 1 }

# 3. Read plugin id/version from the manifest via the signer (single source of truth).
$metaOutput = & $Signer inspect --manifest $manifest
if ($LASTEXITCODE -ne 0) { exit 1 }
$metaOutput
$pluginId = (($metaOutput | Where-Object { $_ -match '^id=' }) -replace '^id=', '')
$version = (($metaOutput | Where-Object { $_ -match '^version=' }) -replace '^version=', '')
if (-not $pluginId -or -not $version) { Write-Error "signer inspect did not report id/version"; exit 1 }

# 4. Pack (unsigned) + verify.
$dist = Join-Path $exampleDir "dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$out = Join-Path $dist ("{0}-{1}.gplugin" -f $pluginId.Trim(), $version.Trim())
& $Signer pack --manifest $manifest --wasm $component --out $out
if ($LASTEXITCODE -ne 0) { exit 1 }
& $Signer verify --archive $out
if ($LASTEXITCODE -ne 0) { exit 1 }
Write-Host ""
Write-Host "OK: $out"
