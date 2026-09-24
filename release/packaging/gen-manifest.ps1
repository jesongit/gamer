# 生成发行清单，实算 SHA256/size 并完成结构与语义校验。
[CmdletBinding()]
param(
    # 产品版本（默认读 server/Cargo.toml [package].version）
    [string]$Version = '',
    [ValidateSet('stable', 'beta')]
    [string]$Channel = 'stable',
    # 依赖锁文件 / dist / 输出
    [string]$LockPath = '',
    [string]$DistDir = '',
    [string]$OutDir = '',
    # 下载基地址（https）；GitHub Release 资产使用扁平名称，不支持目录前缀
    [string]$DownloadBaseUrl = '',
    # 发布说明 URL（https）
    [string]$ReleaseNotesUrl = '',
    # 最低 launcher / 升级起点版本（批次基线 0.1.0）
    [string]$MinLauncherVersion = '0.2.0-beta.1',
    [string]$MinUpgradeVersion = '0.1.0'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $LockPath) { $LockPath = Join-Path $repoRoot 'release\dependencies.lock.toml' }
if (-not $DistDir)  { $DistDir  = Join-Path $repoRoot 'release\dist' }
if (-not $OutDir)   { $OutDir   = Join-Path $repoRoot 'release\manifests' }

Import-Module (Join-Path $PSScriptRoot 'LockFile.psm1') -Force

function Exit-Fail {
    param([string]$Message)
    Write-Host "[gen-manifest] FAIL: $Message" -ForegroundColor Red
    exit 1
}

function Get-Sha256Path {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

# ---------- 版本 ----------
if (-not $Version) {
    $cargoToml = Join-Path $repoRoot 'server\Cargo.toml'
    $section = ''
    foreach ($line in (Get-Content -LiteralPath $cargoToml)) {
        if ($line -match '^\s*\[([^\]]+)\]\s*$') { $section = $Matches[1].Trim(); continue }
        if ($section -eq 'package' -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
            $Version = $Matches[1].Trim(); break
        }
    }
}
if (-not $Version) { Exit-Fail "无法确定产品版本（server/Cargo.toml 与 -Version 均未提供）" }

# ---------- URL 基地址（schema 仅接受 https）----------
if (-not $DownloadBaseUrl) { $DownloadBaseUrl = 'https://example.invalid/download/v{0}' -f $Version }
if (-not $ReleaseNotesUrl) { $ReleaseNotesUrl = 'https://example.invalid/releases/v{0}' -f $Version }
if ($DownloadBaseUrl -notmatch '^https://') { Exit-Fail "DownloadBaseUrl 必须是 https URL: $DownloadBaseUrl" }
if ($ReleaseNotesUrl -notmatch '^https://') { Exit-Fail "ReleaseNotesUrl 必须是 https URL: $ReleaseNotesUrl" }

function New-Artifact {
    # 从 dist 目录取资产实算 size/sha256，返回 ordered artifact 节点
    param([string]$Name, [string]$Url)
    $p = Join-Path $DistDir $Name
    if (-not (Test-Path -LiteralPath $p)) {
        Exit-Fail "发行资产不存在: $p（先运行 package-app.ps1 / package-components.ps1）"
    }
    $item = Get-Item -LiteralPath $p
    return [ordered]@{
        name   = $Name
        url    = $Url
        size   = [long]$item.Length
        sha256 = Get-Sha256Path -Path $p
    }
}

function New-RequiredFiles {
    # 锁 files[] → manifest required_files[]
    param($Files)
    $list = @()
    foreach ($f in $Files) {
        $list += [ordered]@{
            path   = [string]$f['path']
            size   = [long]$f['size']
            sha256 = ([string]$f['sha256']).ToLowerInvariant()
        }
    }
    return ,$list
}

# ---------- 锁文件组件 ----------
$components = Import-LockComponents -Path $LockPath
$adb    = Get-LockComponent -Components $components -Id 'adb'
$ffmpeg = Get-LockComponent -Components $components -Id 'ffmpeg'
$scrcpy = Get-LockComponent -Components $components -Id 'scrcpy-server'

$adbVersion    = [string]$adb['version']
$ffmpegVersion = [string]$ffmpeg['version']
$jarVersion    = [string]$scrcpy['version']

$appZipName   = 'gamer-app-{0}-windows-x64.zip' -f $Version
$adbZipName   = 'gamer-adb-{0}-windows-x64.zip' -f $adbVersion
$ffmpegZipName = 'gamer-ffmpeg-{0}-windows-x64.zip' -f $ffmpegVersion
$scrcpyZipName = 'gamer-scrcpy-server-{0}-windows-x64.zip' -f $jarVersion
$launcherVersion = [regex]::Match([IO.File]::ReadAllText((Join-Path $repoRoot 'launcher/Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value

# 通用组件字段保持 v1；新启动器独占解释 launcher / official-plugins 的产品行为。
function New-ZipComponent {
    param([string]$Id, [string]$ComponentVersion)
    $name = "gamer-$Id-$ComponentVersion-windows-x64.zip"
    Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null
    $zip = [IO.Compression.ZipFile]::OpenRead((Join-Path $DistDir $name))
    try {
        $files = @()
        foreach ($entry in $zip.Entries) {
            if ($entry.FullName.EndsWith('/')) { continue }
            $stream = $entry.Open()
            $sha = [Security.Cryptography.SHA256]::Create()
            try { $hash = [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '').ToLowerInvariant() }
            finally { $stream.Dispose(); $sha.Dispose() }
            $files += [ordered]@{path=$entry.FullName;size=[long]$entry.Length;sha256=$hash}
        }
        $url = "$DownloadBaseUrl/$name"
        if ($Id -eq 'official-plugins') {
            $source = Get-Content (Join-Path $repoRoot 'release/plugins.lock.json') -Raw | ConvertFrom-Json
            $url = $source.bundle.url
            if ((Get-Sha256Path (Join-Path $DistDir $name)) -ne $source.bundle.sha256) { Exit-Fail '插件合集与已发布锁不一致' }
        }
        return [ordered]@{id=$Id;version=$ComponentVersion;artifact=(New-Artifact -Name $name -Url $url);required_files=$files}
    } finally { $zip.Dispose() }
}

# ---------- jar 强绑定门禁 ----------
$jarPath = Join-Path $repoRoot ('server\assets\scrcpy-server.jar')
if (-not (Test-Path -LiteralPath $jarPath)) { Exit-Fail "scrcpy-server jar 不存在: $jarPath" }
$jarSha = Get-Sha256Path -Path $jarPath
$lockJarSha = ([string]$scrcpy.files[0]['sha256']).ToLowerInvariant()
if ($jarSha -ne $lockJarSha) {
    Exit-Fail "jar sha256 与锁不一致: 实际 $jarSha / 锁 $lockJarSha（先跑 tools/check-scrcpy-binding.ps1 排查）"
}

# ---------- 组 manifest（键序与 schema 描述一致）----------
$manifest = [ordered]@{
    schema_version = 1
    product        = 'gamebot'
    release        = [ordered]@{
        version                  = $Version
        channel                  = $Channel
        published_at             = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        minimum_launcher_version = $MinLauncherVersion
        minimum_upgrade_version  = $MinUpgradeVersion
        data_schema              = [int]([regex]::Match([IO.File]::ReadAllText((Join-Path $repoRoot 'server/src/migrations.rs')), 'TARGET_SCHEMA:\s*i64\s*=\s*(\d+)').Groups[1].Value)
        rollback_floor           = 1
        release_notes_url        = $ReleaseNotesUrl
    }
    platforms = [ordered]@{
        'windows-x86_64' = [ordered]@{
            app = [ordered]@{
                artifact   = New-Artifact -Name $appZipName -Url ('{0}/{1}' -f $DownloadBaseUrl, $appZipName)
                entrypoint = 'gamer-server.exe'
            }
            components = @(
                [ordered]@{
                    id             = 'adb'
                    version        = $adbVersion
                    artifact       = New-Artifact -Name $adbZipName -Url ('{0}/{1}' -f $DownloadBaseUrl, $adbZipName)
                    required_files = New-RequiredFiles -Files $adb.files
                },
                [ordered]@{
                    id             = 'ffmpeg'
                    version        = $ffmpegVersion
                    artifact       = New-Artifact -Name $ffmpegZipName -Url ('{0}/{1}' -f $DownloadBaseUrl, $ffmpegZipName)
                    required_files = New-RequiredFiles -Files $ffmpeg.files
                },
                [ordered]@{
                    id = 'scrcpy-server'
                    version = $jarVersion
                    artifact = New-Artifact -Name $scrcpyZipName -Url "$DownloadBaseUrl/$scrcpyZipName"
                    required_files = New-RequiredFiles -Files $scrcpy.files
                },
                (New-ZipComponent -Id 'launcher' -ComponentVersion $launcherVersion),
                (New-ZipComponent -Id 'official-plugins' -ComponentVersion $Version)
            )
            resources = [ordered]@{
                scrcpy_server = [ordered]@{
                    version = $jarVersion
                    path    = 'assets/scrcpy-server.jar'
                    sha256  = $jarSha
                    binding = 'application'
                }
            }
        }
    }
}

# ---------- 写 JSON（UTF-8 无 BOM）----------
if (-not (Test-Path -LiteralPath $OutDir)) { New-Item -ItemType Directory -Path $OutDir -Force | Out-Null }
$manifestPath = Join-Path $OutDir ('{0}.json' -f $Version)
$jsonText = ConvertTo-Json -InputObject $manifest -Depth 12
[System.IO.File]::WriteAllText($manifestPath, $jsonText + "`n", (New-Object System.Text.UTF8Encoding($false)))
Write-Host "[gen-manifest] 生成: $manifestPath"

& node (Join-Path $repoRoot 'release\contracts\validate-manifest.mjs') check $manifestPath --expect-current-version $Version --expect-channel $Channel
if ($LASTEXITCODE -ne 0) { Exit-Fail "manifest 校验未通过（退出码 $LASTEXITCODE）" }
Write-Host "[gen-manifest] PASS: $manifestPath"
exit 0
