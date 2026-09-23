# 独立启动器与官方插件种子；在生成签名 manifest 前运行。
[CmdletBinding()]
param([switch]$SkipBuild, [string]$DistDir = '', [string]$Version = '')
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $DistDir) { $DistDir = Join-Path $repoRoot 'release/dist' }
if (-not $Version) { $Version = [regex]::Match([IO.File]::ReadAllText((Join-Path $repoRoot 'server/Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value }
$launcherVersion = [regex]::Match([IO.File]::ReadAllText((Join-Path $repoRoot 'launcher/Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $SkipBuild) {
    & cargo build --release --locked --manifest-path (Join-Path $repoRoot 'launcher/Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw '启动器构建失败' }
}
New-Item -ItemType Directory -Path $DistDir -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot 'launcher/target/release/gamer-launcher.exe') -Destination (Join-Path $DistDir 'gamer-launcher.exe') -Force
$launcherZip = Join-Path $DistDir "gamer-launcher-$launcherVersion-windows-x64.zip"
Compress-Archive -LiteralPath (Join-Path $DistDir 'gamer-launcher.exe') -DestinationPath $launcherZip -Force

$registryPath = Join-Path $repoRoot 'web/public/registry.json'
$registry = [IO.File]::ReadAllText($registryPath, [Text.Encoding]::UTF8) | ConvertFrom-Json
$files = @()
foreach ($plugin in $registry.plugins) {
    if ($plugin.id -notin @('gamer-yaml', 'gamer-keymap', 'gamer-video')) { continue }
    $name = "$($plugin.id)-$($plugin.version).gplugin"
    if ($name -match '[\\/]') { throw '插件安装包名称非法' }
    $path = Join-Path $repoRoot "web/public/plugins/$name"
    if ((Get-Item -LiteralPath $path).Length -ne $plugin.size -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $plugin.sha256) { throw "插件安装包与注册表不一致: $name" }
    $files += $path
}
if ($files.Count -ne 3) { throw '官方插件安装包必须包含自动化、键盘映射、视频三个插件' }
Compress-Archive -LiteralPath $files -DestinationPath (Join-Path $DistDir "gamer-official-plugins-$Version-windows-x64.zip") -Force
Write-Host '[package-launcher] 启动器、官方插件种子打包完成'
