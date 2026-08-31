# REL-003: 按 release/contracts/manifest-v1.schema.json 生成发布 manifest
# （release/manifests/<version>.json），并完成签名 + 结构/语义校验：
#   1) 从 release/dependencies.lock.toml 取 adb/ffmpeg 版本与逐文件清单；
#   2) 从 release/dist/ 取 app/组件 zip 实算 size+sha256（须先跑
#      package-app.ps1 与 package-components.ps1）；
#   3) jar 实算 sha256 并与锁 scrcpy-server 条目核对（强绑定门禁）；
#   4) dev key 缺失时自动 keygen，调用 sign-manifest.mjs 出 .sig；
#   5) 调 release/contracts/validate-manifest.mjs check 全量校验（验签→语义→结构）。
#
# 兼容 Windows PowerShell 5.1 与 pwsh。

[CmdletBinding()]
param(
    # 产品版本（默认读 server/Cargo.toml [package].version）
    [string]$Version = '',
    [ValidateSet('stable', 'beta')]
    [string]$Channel = 'stable',
    # 依赖锁文件 / dist / 输出 / 密钥目录
    [string]$LockPath = '',
    [string]$DistDir = '',
    [string]$OutDir = '',
    [string]$KeysDir = '',
    # 下载基地址（https）；GitHub Release 资产使用扁平名称，不支持目录前缀
    [string]$DownloadBaseUrl = '',
    # 发布说明 URL（https）
    [string]$ReleaseNotesUrl = '',
    # 最低 launcher / 升级起点版本（批次基线 0.1.0）
    [string]$MinLauncherVersion = '0.1.0',
    [string]$MinUpgradeVersion = '0.1.0',
    # 只生成 manifest，不签名不校验
    [switch]$SkipSign
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $LockPath) { $LockPath = Join-Path $repoRoot 'release\dependencies.lock.toml' }
if (-not $DistDir)  { $DistDir  = Join-Path $repoRoot 'release\dist' }
if (-not $OutDir)   { $OutDir   = Join-Path $repoRoot 'release\manifests' }
if (-not $KeysDir)  { $KeysDir  = Join-Path $repoRoot 'release\keys' }

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
        data_schema              = 1
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
                }
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

# ---------- 写 JSON（UTF-8 无 BOM；签名覆盖原始字节，BOM 会破坏校验）----------
if (-not (Test-Path -LiteralPath $OutDir)) { New-Item -ItemType Directory -Path $OutDir -Force | Out-Null }
$manifestPath = Join-Path $OutDir ('{0}.json' -f $Version)
$jsonText = ConvertTo-Json -InputObject $manifest -Depth 12
[System.IO.File]::WriteAllText($manifestPath, $jsonText + "`n", (New-Object System.Text.UTF8Encoding($false)))
Write-Host "[gen-manifest] 生成: $manifestPath"

if ($SkipSign) {
    Write-Host "[gen-manifest] -SkipSign: 未签名未校验（仅供检视）"
    exit 0
}

# ---------- dev key（缺则自动生成）----------
$KeyId = 'dev-ed25519-1'
$privKey = Join-Path $KeysDir ('{0}.private.pem' -f $KeyId)
$pubKey  = Join-Path $KeysDir ('{0}.pem' -f $KeyId)
if (-not (Test-Path -LiteralPath $privKey) -or -not (Test-Path -LiteralPath $pubKey)) {
    Write-Host "[gen-manifest] dev 密钥缺失，自动 keygen（$KeysDir）..."
    & node (Join-Path $PSScriptRoot 'sign-manifest.mjs') keygen --id $KeyId --out-dir $KeysDir
    if ($LASTEXITCODE -ne 0) { Exit-Fail "keygen 失败（退出码 $LASTEXITCODE）" }
}

# ---------- 签名 + 全量校验（验签 → 语义 → 结构）----------
& node (Join-Path $PSScriptRoot 'sign-manifest.mjs') sign $manifestPath --key $privKey --key-id $KeyId
if ($LASTEXITCODE -ne 0) { Exit-Fail "sign 失败（退出码 $LASTEXITCODE）" }

$sigPath = Join-Path $OutDir ('{0}.sig' -f $Version)
& node (Join-Path $repoRoot 'release\contracts\validate-manifest.mjs') check $manifestPath --sig $sigPath --keys-dir $KeysDir --expect-current-version $Version --expect-channel $Channel
if ($LASTEXITCODE -ne 0) { Exit-Fail "manifest 校验未通过（退出码 $LASTEXITCODE）" }

Write-Host "[gen-manifest] PASS: $manifestPath + $sigPath（key_id=$KeyId）"
exit 0
