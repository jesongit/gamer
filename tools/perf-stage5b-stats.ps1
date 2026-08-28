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

$args = @($script)
foreach ($p in $InputPath) {
    foreach ($item in ($p -split ',')) {
        $trimmed = $item.Trim()
        if ($trimmed.Length -eq 0) { continue }
        $args += '--input'
        $args += $trimmed
    }
}
if ($Json) { $args += '--json' }
if ($DryRun) { $args += '--dry-run' }
if ($SelfTest) { $args += '--self-test' }

& node @args
exit $LASTEXITCODE
