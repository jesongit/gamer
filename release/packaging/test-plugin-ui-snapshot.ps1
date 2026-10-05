[CmdletBinding()]
param([string]$SnapshotRoot = '')
$ErrorActionPreference = 'Stop'
if (-not $SnapshotRoot) { $SnapshotRoot = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'web/public' }
$SnapshotRoot = (Resolve-Path -LiteralPath $SnapshotRoot).Path
$registry = Get-Content -LiteralPath (Join-Path $SnapshotRoot 'registry.json') -Raw | ConvertFrom-Json
Add-Type -AssemblyName System.IO.Compression.FileSystem
foreach ($id in @('gamer-yaml', 'gamer-keymap', 'gamer-video')) {
    $plugin = @($registry.plugins | Where-Object id -EQ $id)
    if ($plugin.Count -ne 1) { throw "共享 UI 插件缺失或重复: $id" }
    $archive = [IO.Compression.ZipFile]::OpenRead((Join-Path $SnapshotRoot "plugins/$id-$($plugin[0].version).gplugin"))
    try {
        if (-not $archive.GetEntry('ui/plugin.js') -or -not $archive.GetEntry('ui/style.css')) { throw "共享 UI 入口或样式缺失: $id" }
        $count = 0
        foreach ($entry in $archive.Entries) {
            if (-not $entry.FullName.StartsWith('ui/') -or $entry.FullName.EndsWith('/')) { continue }
            $file = Join-Path $SnapshotRoot "plugin-ui/$id/$($entry.FullName.Substring(3))"
            if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "发行 UI 静态文件缺失: $file" }
            $stream = $entry.Open()
            $sha = [Security.Cryptography.SHA256]::Create()
            try { $expected = [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '').ToLowerInvariant() }
            finally { $stream.Dispose(); $sha.Dispose() }
            if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw "发行 UI 与发布归档字节不同: $file" }
            $count++
        }
        Write-Host "PASS verified deployment UI snapshot: $id ($count files)"
    } finally { $archive.Dispose() }
}
exit 0
