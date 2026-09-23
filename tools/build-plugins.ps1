#requires -Version 7.0
<# Main-repository convenience entry. Plugin build recipes belong to gamer-plugins. #>
[CmdletBinding()]
param(
    [string]$OutputDir,
    [string]$RegistryFile,
    [string]$ManifestsRoot,
    [string]$ChecksumsFile,
    [string]$Publisher = 'gamer.dev',
    [string]$Plugin,
    [switch]$KeepStaleArtifacts
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $ManifestsRoot) { $ManifestsRoot = Join-Path $repo 'plugins' }
if (-not (Test-Path -LiteralPath (Join-Path $ManifestsRoot 'build.ps1'))) {
    throw '插件源码未初始化，请运行 git submodule update --init --recursive'
}
if (-not $OutputDir) { $OutputDir = Join-Path $repo 'web/public/plugins' }
if (-not $RegistryFile) { $RegistryFile = Join-Path $repo 'web/public/registry.json' }
& (Join-Path $ManifestsRoot 'build.ps1') -OutputDir $OutputDir -RegistryFile $RegistryFile `
    -ChecksumsFile $ChecksumsFile -Publisher $Publisher -Plugin $Plugin `
    -TargetRoot (Join-Path $repo 'server/target/plugin-build') -KeepStaleArtifacts:$KeepStaleArtifacts
