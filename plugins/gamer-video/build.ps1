#requires -Version 5.1
[CmdletBinding()]
param([string]$OutputDir, [string]$RegistryFile)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$options = @{ Plugin = 'gamer-video' }
if ($OutputDir) { $options.OutputDir = $OutputDir }
if ($RegistryFile) { $options.RegistryFile = $RegistryFile }
& (Join-Path $repo 'tools\build-plugins.ps1') @options
