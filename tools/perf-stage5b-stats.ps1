#requires -Version 5.1
<#
.SYNOPSIS
    Stage5 B benchmark statistics wrapper.

.DESCRIPTION
    Pure stats entry point for JSONL/CSV benchmark samples. It only parses inputs
    and forwards to the Node implementation so Windows and Linux can share the same
    aggregation logic.
.#>
[CmdletBinding()]
param(
    [string[]]$InputPath = @(),
    [switch]$Json,
    [switch]$IncludeResource,
    [switch]$DryRun,
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
$script = Join-Path $root 'tools\perf-stage5b-stats.mjs'
if (-not (Test-Path -LiteralPath $script -PathType Leaf)) {
    throw "统计入口不存在：$script"
}

$node = Get-Command node -ErrorAction SilentlyContinue
if ($null -eq $node) {
    throw 'node not found on PATH'
}

$nodeArgs = @($script)
foreach ($p in $InputPath) {
    foreach ($item in ($p -split ',')) {
        $trimmed = $item.Trim()
        if ($trimmed.Length -eq 0) { continue }
        $nodeArgs += '--input'
        $nodeArgs += $trimmed
    }
}
if ($Json) { $nodeArgs += '--json' }
if ($IncludeResource) { $nodeArgs += '--include-resource' }
if ($DryRun) { $nodeArgs += '--dry-run' }
if ($SelfTest) { $nodeArgs += '--self-test' }

& node @nodeArgs
exit $LASTEXITCODE
