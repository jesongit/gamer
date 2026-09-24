# 独立启动器与官方插件种子；在生成发行 manifest 前运行。
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

# 直接复用插件仓库发布的字节，主发行资产与离线 seed 是同一份已校验副本。
& (Join-Path $PSScriptRoot 'fetch-plugins.ps1') -DistDir $DistDir -Version $Version
Write-Host '[package-launcher] 启动器、官方插件种子打包完成'
