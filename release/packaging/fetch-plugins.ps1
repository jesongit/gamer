# 发行只消费已公开发布且固定 SHA256 的插件，不重新构建另一份字节。
[CmdletBinding()]
param([string]$DistDir = '', [string]$Version = '', [switch]$TestFixturesOnly, [string]$FixturesDir = '')
$ErrorActionPreference = 'Stop'
function Resolve-PluginAssetDestinations {
    param([string]$RepoRoot, [string]$DistDir, [switch]$TestFixturesOnly, [string]$FixturesDir)
    if (-not $TestFixturesOnly) {
        if ($FixturesDir) { throw '-FixturesDir 仅允许与 -TestFixturesOnly 一起使用' }
        if (-not $DistDir) { $DistDir = Join-Path $RepoRoot 'release/dist' }
        return @{ Assets = (Join-Path $RepoRoot 'web/public'); Dist = $DistDir; Cache = (Join-Path $RepoRoot 'release/cache/published-plugins') }
    }
    if (-not $FixturesDir) { $FixturesDir = Join-Path $RepoRoot 'server/target/plugin-test-fixtures' }
    $fixtureRoot = [IO.Path]::GetFullPath($FixturesDir).TrimEnd([char[]]'\/')
    $allowedRoots = @((Join-Path $RepoRoot 'server/target'), [IO.Path]::GetTempPath())
    $allowed = $false
    foreach ($root in $allowedRoots) {
        $boundary = [IO.Path]::GetFullPath($root).TrimEnd([char[]]'\/') + [IO.Path]::DirectorySeparatorChar
        if ($fixtureRoot.StartsWith($boundary, [StringComparison]::OrdinalIgnoreCase)) { $allowed = $true }
    }
    if (-not $allowed) { throw '历史测试 fixture 必须隔离在 server/target 或系统临时目录的子目录' }
    # A checkout itself may live in TEMP. Explicitly protect its publication,
    # configuration, source and business-data paths even in that case.
    foreach ($protected in @('web/public', 'server/web-dist', 'server/data', 'release/dist', 'plugins')) {
        $protectedPath = [IO.Path]::GetFullPath((Join-Path $RepoRoot $protected)).TrimEnd([char[]]'\/')
        if ($fixtureRoot -eq $protectedPath -or $fixtureRoot.StartsWith($protectedPath + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or $protectedPath.StartsWith($fixtureRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw '历史测试 fixture 不能写入发行或业务目录' }
    }
    if (-not $DistDir) { $DistDir = Join-Path $fixtureRoot 'bundles' }
    $distPath = [IO.Path]::GetFullPath($DistDir)
    if (-not $distPath.StartsWith($fixtureRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw '历史测试合集输出必须留在 fixture 目录内' }
    foreach ($path in @($fixtureRoot, $distPath)) {
        $ancestor = $path
        while ($ancestor) {
            if ((Test-Path -LiteralPath $ancestor) -and ((Get-Item -LiteralPath $ancestor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw '历史测试 fixture 路径不能经过重解析点' }
            $ancestor = Split-Path -Parent $ancestor
        }
    }
    return @{ Assets = $fixtureRoot; Dist = $distPath; Cache = (Join-Path $fixtureRoot 'cache') }
}
function Assert-PublishedPluginSet {
    param([object[]]$Plugins, [string[]]$RequiredIds, [switch]$TestFixturesOnly)
    if (-not $TestFixturesOnly -and @($Plugins | Where-Object { $_.id -in @('gamer-ai', 'gamer-ocr') }).Count -gt 0) {
        throw '发布锁仍包含已退休的 gamer-ai 或 gamer-ocr；请先发布新官方插件并更新 release/plugins.lock.json 的真实资产与 SHA256，不能将旧合集作为新发行。历史基线仅允许 -TestFixturesOnly 回归使用。'
    }
    if (-not $TestFixturesOnly -and @($Plugins | Where-Object {
        ($_.PSObject.Properties['execution'] -and $_.execution.PSObject.Properties['builtin_id'] -and $_.execution.builtin_id -in @('gamer-ai', 'gamer-ocr')) -or ($_.PSObject.Properties['permissions'] -and @($_.permissions | Where-Object { $_ -is [string] -and $_.Trim() -eq 'ai.connect' }).Count -gt 0)
    }).Count -gt 0) { throw '发布锁包含已退休 ai.connect 权限；请先发布新官方插件并更新 release/plugins.lock.json，历史版本仅允许隔离 fixture 回归' }
    $allowedIds = $RequiredIds + @('gamer-package-publisher', 'gamer-live', 'gamer-notify')
    if ($TestFixturesOnly) { $allowedIds += @('gamer-ai', 'gamer-ocr') }
    if (@($Plugins.id | Sort-Object -Unique).Count -ne $Plugins.Count -or
        @($RequiredIds | Where-Object { $_ -notin $Plugins.id }).Count -gt 0 -or
        @($Plugins.id | Where-Object { $_ -notin $allowedIds }).Count -gt 0) { throw '官方插件目录缺项、重复或含未知插件' }
}
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$destinations = Resolve-PluginAssetDestinations -RepoRoot $repo -DistDir $DistDir -TestFixturesOnly:$TestFixturesOnly -FixturesDir $FixturesDir
$DistDir = $destinations.Dist
if (-not $Version) { $Version = [regex]::Match([IO.File]::ReadAllText((Join-Path $repo 'server/Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value }
$lock = Get-Content (Join-Path $repo 'release/plugins.lock.json') -Raw | ConvertFrom-Json
$commit = (& git -C (Join-Path $repo 'plugins') rev-parse HEAD | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or (-not $TestFixturesOnly -and $commit -ne $lock.plugin_commit)) { throw '插件源码提交与发布锁不一致；请先发布新官方插件并更新 release/plugins.lock.json 的真实来源与 SHA256' }
# Tests exercise the new host against the pinned published baseline, even when
# plugin source is under development. Release packaging keeps strict binding.
$base = "https://github.com/jesongit/gamer-plugins/releases/download/$($lock.tag)/"
$cache = $destinations.Cache
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
$registry = Get-Content -LiteralPath $registryPath -Raw | ConvertFrom-Json
if ($registry.provenance.plugin_commit -ne $lock.plugin_commit) { throw '插件 registry 来源提交不一致' }
$plugins = @($registry.plugins)
$requiredIds = @('gamer-keymap', 'gamer-video', 'gamer-yaml')
Assert-PublishedPluginSet -Plugins $plugins -RequiredIds $requiredIds -TestFixturesOnly:$TestFixturesOnly
$bundlePath = Get-VerifiedAsset $lock.bundle
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
    $public = $destinations.Assets
    New-Item -ItemType Directory -Force -Path (Join-Path $public 'plugins') | Out-Null
    foreach ($p in $plugins) {
        $name = "$($p.id)-$($p.version).gplugin"
        [IO.Compression.ZipFileExtensions]::ExtractToFile($zip.GetEntry($name), (Join-Path $public "plugins/$name"), $true)
        # Console 的共享组件在插件未安装/停用时仍需这些发行自带的 UI。
        # 从已校验归档提取同一份字节，普通 Web build 只负责复制静态快照。
        if ($p.id -in $requiredIds) {
            $uiRoot = [IO.Path]::GetFullPath((Join-Path $public "plugin-ui/$($p.id)"))
            $archive = [IO.Compression.ZipFile]::OpenRead((Join-Path $public "plugins/$name"))
            try {
                if (-not $archive.GetEntry('ui/plugin.js') -or -not $archive.GetEntry('ui/style.css')) { throw "插件缺少共享 UI 入口或样式: $name" }
                foreach ($entry in $archive.Entries) {
                    if (-not $entry.FullName.StartsWith('ui/', [StringComparison]::Ordinal) -or $entry.FullName.EndsWith('/')) { continue }
                    $relative = $entry.FullName.Substring(3)
                    if ($relative -match '\\|(^|/)\.\.?(/|$)') { throw "非法插件 UI 路径: $($entry.FullName)" }
                    $target = [IO.Path]::GetFullPath((Join-Path $uiRoot $relative))
                    if (-not $target.StartsWith($uiRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw '插件 UI 路径越界' }
                    New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($target)) | Out-Null
                    [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $target, $true)
                }
            } finally { $archive.Dispose() }
        }
        # 市场使用发布字节的同源副本，离线可用且无需浏览器跨域 GitHub。
        $p.download_url = "/plugins/$name"
    }
    [IO.File]::WriteAllText((Join-Path $public 'registry.json'), ($registry | ConvertTo-Json -Depth 30) + "`n", [Text.UTF8Encoding]::new($false))
} finally { $zip.Dispose() }
Copy-Item -LiteralPath $bundlePath -Destination (Join-Path $DistDir "gamer-official-plugins-$Version-windows-x64.zip") -Force
Write-Host "[fetch-plugins] 已校验并复用发布 $($lock.tag)"
if ($TestFixturesOnly) { Write-Host "[fetch-plugins] 历史回归目录（不可发行）: $public" }
