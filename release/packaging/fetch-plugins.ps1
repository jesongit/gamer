# 发行只消费已公开发布且固定 SHA256 的插件，不重新构建另一份字节。
[CmdletBinding()]
param([string]$DistDir = '', [string]$Version = '', [switch]$TestFixturesOnly)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $DistDir) { $DistDir = Join-Path $repo 'release/dist' }
if (-not $Version) { $Version = [regex]::Match([IO.File]::ReadAllText((Join-Path $repo 'server/Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value }
$lock = Get-Content (Join-Path $repo 'release/plugins.lock.json') -Raw | ConvertFrom-Json
$commit = (& git -C (Join-Path $repo 'plugins') rev-parse HEAD | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or (-not $TestFixturesOnly -and $commit -ne $lock.plugin_commit)) { throw '插件源码提交与发布锁不一致' }
# Tests exercise the new host against the pinned published baseline, even when
# plugin source is under development. Release packaging keeps strict binding.
$base = "https://github.com/jesongit/gamer-plugins/releases/download/$($lock.tag)/"
$cache = Join-Path $repo 'release/cache/published-plugins'
New-Item -ItemType Directory -Force -Path $cache, $DistDir | Out-Null
function Get-VerifiedAsset($asset) {
    if ($asset.name -notmatch '^[A-Za-z0-9._-]+$' -or $asset.url -ne ($base + $asset.name) -or $asset.sha256 -notmatch '^[a-f0-9]{64}$') { throw '插件发布锁资产非法' }
    $path = Join-Path $cache $asset.name
    $valid = (Test-Path -LiteralPath $path) -and (Get-Item -LiteralPath $path).Length -eq $asset.size -and (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -eq $asset.sha256
    if (-not $valid) {
        $part = "$path.part"
        Invoke-WebRequest -Uri $asset.url -OutFile $part
        if ((Get-Item -LiteralPath $part).Length -ne $asset.size -or (Get-FileHash -LiteralPath $part -Algorithm SHA256).Hash.ToLowerInvariant() -ne $asset.sha256) { throw "插件发布资产校验失败: $($asset.name)" }
        Move-Item -LiteralPath $part -Destination $path -Force
    }
    return $path
}
$registryPath = Get-VerifiedAsset $lock.registry
$bundlePath = Get-VerifiedAsset $lock.bundle
$registry = Get-Content -LiteralPath $registryPath -Raw | ConvertFrom-Json
if ($registry.provenance.plugin_commit -ne $lock.plugin_commit) { throw '插件 registry 来源提交不一致' }
$plugins = @($registry.plugins)
if ($plugins.Count -ne 3 -or (@($plugins.id | Sort-Object -Unique) -join ',') -ne 'gamer-keymap,gamer-video,gamer-yaml') { throw '插件目录必须包含三款官方插件各一份' }
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead($bundlePath)
try {
    if ($zip.Entries.Count -ne $plugins.Count) { throw '插件合集条目数量不一致' }
    foreach ($p in $plugins) {
        $name = "$($p.id)-$($p.version).gplugin"
        if ($name -notmatch '^[A-Za-z0-9._-]+$') { throw '非法插件文件名' }
        $entries = @($zip.Entries | Where-Object FullName -EQ $name)
        if ($entries.Count -ne 1) { throw "插件合集缺少或重复条目: $name" }
        $stream = $entries[0].Open()
        $hash = [Security.Cryptography.SHA256]::Create()
        try { $digest = [BitConverter]::ToString($hash.ComputeHash($stream)).Replace('-', '').ToLowerInvariant() }
        finally { $stream.Dispose(); $hash.Dispose() }
        if ($digest -ne $p.sha256 -or $entries[0].Length -ne $p.size) { throw "插件文件与 registry 不符: $name" }
    }
    $public = Join-Path $repo 'web/public'
    New-Item -ItemType Directory -Force -Path (Join-Path $public 'plugins') | Out-Null
    foreach ($p in $plugins) {
        $name = "$($p.id)-$($p.version).gplugin"
        [IO.Compression.ZipFileExtensions]::ExtractToFile($zip.GetEntry($name), (Join-Path $public "plugins/$name"), $true)
        # 市场使用发布字节的同源副本，离线可用且无需浏览器跨域 GitHub。
        $p.download_url = "/plugins/$name"
    }
    [IO.File]::WriteAllText((Join-Path $public 'registry.json'), ($registry | ConvertTo-Json -Depth 30) + "`n", [Text.UTF8Encoding]::new($false))
} finally { $zip.Dispose() }
Copy-Item -LiteralPath $bundlePath -Destination (Join-Path $DistDir "gamer-official-plugins-$Version-windows-x64.zip") -Force
Write-Host "[fetch-plugins] 已校验并复用发布 $($lock.tag)"
