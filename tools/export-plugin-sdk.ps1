#requires -Version 7.0
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Destination)
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
if ($LASTEXITCODE -ne 0) { throw 'SDK 源文件尚有未提交改动，请先提交再导出，以保证来源提交可复现' }
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
[IO.File]::WriteAllText((Join-Path $destinationRoot 'lock.json'), ($lock | ConvertTo-Json -Depth 6) + "`n", [Text.UTF8Encoding]::new($false))
Write-Host "SDK snapshot: $commit ($($entries.Count) files)"
