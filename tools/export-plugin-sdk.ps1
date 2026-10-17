#requires -Version 7.0
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Destination, [switch]$AllowDirtySnapshot)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$destinationRoot = [IO.Path]::GetFullPath($Destination)
$files = [ordered]@{
    'sdk/ui/module-build.mjs' = 'ui/module-build.mjs'
    'sdk/ui/host-modules.json' = 'ui/host-modules.json'
    'server/wit/gamer/host.wit' = 'wit/gamer/host.wit'
    'server/wit/keymap/keymap.wit' = 'wit/keymap/keymap.wit'
}
foreach ($file in (& git -C $repo ls-files tools/plugin-packer)) {
    $files[$file] = $file.Substring('tools/'.Length)
}
$commit = (& git -C $repo rev-parse HEAD | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { throw '无法读取 SDK 来源提交' }
& git -C $repo diff --quiet HEAD -- @($files.Keys)
$sourceDirty = $LASTEXITCODE -eq 1
if ($LASTEXITCODE -gt 1) { throw '无法检查 SDK 源码状态' }
if ($sourceDirty -and -not $AllowDirtySnapshot) { throw 'SDK 源文件尚有未提交改动；正式导出须先提交，开发快照须显式 -AllowDirtySnapshot' }
$entries = @()
foreach ($source in $files.Keys) {
    $target = Join-Path $destinationRoot $files[$source]
    New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $repo $source) -Destination $target -Force
    # Normalize text so hashes also verify on Linux checkouts.
    $text = [IO.File]::ReadAllText($target).Replace("`r`n", "`n")
    [IO.File]::WriteAllText($target, $text, [Text.UTF8Encoding]::new($false))
    $entries += [ordered]@{ source = $source; path = $files[$source]; sha256 = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() }
}
$lock = [ordered]@{ schema_version = 1; host_api = '1.0.0'; repository = 'https://github.com/jesongit/gamer.git'; commit = $commit; files = $entries }
if ($sourceDirty) {
    $lock.snapshot = [ordered]@{
        kind = 'working_tree'
        base_commit = $commit
        note = 'Uncommitted source snapshot; commit is the real baseline, per-file SHA256 pins the actual bytes. Re-export from committed source before release.'
    }
}
[IO.File]::WriteAllText((Join-Path $destinationRoot 'lock.json'), ($lock | ConvertTo-Json -Depth 6) + "`n", [Text.UTF8Encoding]::new($false))
Write-Host "SDK snapshot: $commit ($($entries.Count) files; working_tree=$sourceDirty)"
# git diff reports 1 for the explicitly allowed dirty snapshot. Do not leak
# that diagnostic exit code as the successful export's process result.
$global:LASTEXITCODE = 0
