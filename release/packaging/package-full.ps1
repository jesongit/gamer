# 完整便携包：解压后双击 Gamer.exe，设置页更新。
[CmdletBinding()]
param(
    # 跳过应用构建，复用 app 归档
    [switch]$SkipBuild,
    # 跳过真实启动冒烟
    [switch]$SkipSmoke,
    # 产品版本（默认读 server/Cargo.toml）
    [string]$Version = '',
    [ValidateSet('stable', 'beta')]
    [string]$Channel = 'stable',
    [string]$DistDir = '',
    # manifest 目录（gen-manifest.ps1 输出，默认 <repo>/release/manifests）
    [string]$ManifestDir = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

function Exit-Fail {
    param([string]$Message)
    Write-Host "[package-full] FAIL: $Message" -ForegroundColor Red
    exit 1
}

function Get-Sha256Path {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Write-Utf8BomFile {
    # 中文文本统一 UTF-8 BOM 落盘（Windows 记事本 / PS5.1 兼容）
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Text)
    [System.IO.File]::WriteAllText($Path, $Text, (New-Object System.Text.UTF8Encoding($true)))
}

function New-ZipFromDirectory {
    # 逐文件创建 zip 条目，条目名强制 '/' 分隔。PS 5.1 自带 Compress-Archive
    # 对子目录条目使用 '\' 分隔（跨工具解包损坏），故不用它。
    param(
        [Parameter(Mandatory = $true)][string]$SourceDir,
        [Parameter(Mandatory = $true)][string]$DestFile
    )
    Add-Type -AssemblyName System.IO.Compression | Out-Null
    Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null
    if (Test-Path -LiteralPath $DestFile) { Remove-Item -LiteralPath $DestFile -Force }
    $fs = [System.IO.File]::Open($DestFile, [System.IO.FileMode]::CreateNew)
    $zip = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($f in (Get-ChildItem -LiteralPath $SourceDir -Recurse -File | Sort-Object FullName)) {
            $rel = $f.FullName.Substring($SourceDir.Length + 1) -replace '\\', '/'
            [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $f.FullName, $rel, [System.IO.Compression.CompressionLevel]::Optimal)
        }
    } finally {
        $zip.Dispose()
        $fs.Dispose()
    }
}

function Get-ConfigTemplate {
    return [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../config.default.toml'), [Text.Encoding]::UTF8)
}

function Get-InstallTemplate {
    return @"
# Gamer __VERSION__
解压完整目录后双击 Gamer.exe，服务准备好后自动打开浏览器。
首次使用在网页设置管理员密码。软件更新统一在设置页进行，更新期间自动重启和恢复连接。
关闭网页不停止服务；使用托盘的退出操作结束服务。
config/ 和 data/ 是个人配置与数据，请保留。不要用新版完整包覆盖这些目录。
官方插件在插件页按需安装；软件更新不会安装未选择的插件。
"@
}

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $DistDir)     { $DistDir     = Join-Path $repoRoot 'release\dist' }
if (-not $ManifestDir) { $ManifestDir = Join-Path $repoRoot 'release\manifests' }

Import-Module (Join-Path $PSScriptRoot 'LockFile.psm1') -Force

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
if (-not $Version) { Exit-Fail "无法确定产品版本" }

# ---------- 输入清单 ----------
$serverExe = Join-Path $repoRoot 'server\target\release\gamer-server.exe'
$manifestJson   = Join-Path $ManifestDir ('{0}.json' -f $Version)
$licensesDir    = Join-Path $repoRoot 'licenses'

$components = Import-LockComponents -Path (Join-Path $repoRoot 'release\dependencies.lock.toml')
$adb    = Get-LockComponent -Components $components -Id 'adb'
$ffmpeg = Get-LockComponent -Components $components -Id 'ffmpeg'
$adbVersion    = [string]$adb['version']
$ffmpegVersion = [string]$ffmpeg['version']

$appZipName    = 'gamer-app-{0}-windows-x64.zip' -f $Version
$adbZipName    = 'gamer-adb-{0}-windows-x64.zip' -f $adbVersion
$ffmpegZipName = 'gamer-ffmpeg-{0}-windows-x64.zip' -f $ffmpegVersion

if (-not $SkipBuild) {
    & (Join-Path $PSScriptRoot 'package-app.ps1') -Channel $Channel
    if ($LASTEXITCODE -ne 0) { throw '应用构建失败' }
}

foreach ($must in @(
    $manifestJson,
    (Join-Path $DistDir $appZipName), (Join-Path $DistDir $adbZipName), (Join-Path $DistDir $ffmpegZipName),
    (Join-Path $licensesDir 'NOTICE.md')
)) {
    if (-not (Test-Path -LiteralPath $must)) {
        Exit-Fail "缺少输入: $must（按需运行 package-app.ps1 / package-components.ps1 / gen-manifest.ps1）"
    }
}

Write-Host "[package-full] 版本 $Version"

# ---------- 组装 staging ----------
$stage = Join-Path $DistDir ('staging-full-' + $Version)
$distBoundary = [IO.Path]::GetFullPath($DistDir).TrimEnd('\') + '\'
if ($Version -notmatch '^[A-Za-z0-9._+-]+$' -or $Version.Contains('..')) { Exit-Fail '版本号不能用作目录名' }
if (-not [IO.Path]::GetFullPath($stage).StartsWith($distBoundary, [StringComparison]::OrdinalIgnoreCase)) { Exit-Fail 'staging 超出输出目录' }
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
foreach ($d in @('config', 'data', 'manifests', 'state', 'versions', 'runtime')) {
    New-Item -ItemType Directory -Path (Join-Path $stage $d) -Force | Out-Null
}
try {
    Write-Utf8BomFile -Path (Join-Path $stage 'config\config.toml') -Text (Get-ConfigTemplate)

    # 数据目录留空，首次启动由 Core 播种默认配置包。
    Copy-Item -LiteralPath $manifestJson -Destination (Join-Path $stage ('manifests\{0}.json' -f $Version))
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $releaseModel = Get-Content -LiteralPath $manifestJson -Raw | ConvertFrom-Json
    $platform = $releaseModel.platforms.'windows-x86_64'
    $entries = @(@{ artifact=$platform.app.artifact; destination=(Join-Path $stage "versions/$Version") })
    foreach ($component in $platform.components) {
        $entries += @{ artifact=$component.artifact; destination=(Join-Path $stage "runtime/$($component.id)/$($component.version)") }
    }
    foreach ($entry in $entries) {
        $archive = Join-Path $DistDir $entry.artifact.name
        if ((Get-Sha256Path $archive) -ne $entry.artifact.sha256 -or (Get-Item -LiteralPath $archive).Length -ne $entry.artifact.size) { throw "发行归档校验失败: $archive" }
        [IO.Compression.ZipFile]::ExtractToDirectory($archive, $entry.destination)
    }
    # User entry is byte-identical to the manifest-verified application executable.
    Copy-Item -LiteralPath (Join-Path $stage "versions/$Version/$($platform.app.entrypoint)") -Destination (Join-Path $stage 'Gamer.exe') -Force
    $current = @{ schema_version=1; current=$Version; previous=$null; updated_at_unix_ms=0 } | ConvertTo-Json
    [IO.File]::WriteAllText((Join-Path $stage 'state/current.json'), $current, (New-Object Text.UTF8Encoding($false)))

    # licenses/（DEP-005 履约：随 full 包附第三方声明与全文）
    New-Item -ItemType Directory -Path (Join-Path $stage 'licenses\android-platform-tools') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $stage 'licenses\ffmpeg') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $stage 'licenses\scrcpy') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $licensesDir 'NOTICE.md') -Destination (Join-Path $stage 'licenses\NOTICE.md')
    Copy-Item -LiteralPath (Join-Path $licensesDir 'android-platform-tools\LICENSE.txt') -Destination (Join-Path $stage 'licenses\android-platform-tools\LICENSE.txt')
    Copy-Item -LiteralPath (Join-Path $licensesDir 'android-platform-tools\NOTICE.txt')  -Destination (Join-Path $stage 'licenses\android-platform-tools\NOTICE.txt')
    Copy-Item -LiteralPath (Join-Path $licensesDir 'ffmpeg\COPYING.LESSER') -Destination (Join-Path $stage 'licenses\ffmpeg\COPYING.LESSER')
    Copy-Item -LiteralPath (Join-Path $licensesDir 'ffmpeg\SOURCE-OFFER.txt') -Destination (Join-Path $stage 'licenses\ffmpeg\SOURCE-OFFER.txt')
    $buildConf = Join-Path $repoRoot ('release\vendor\ffmpeg\{0}\BUILD-CONFIG.txt' -f $ffmpegVersion)
    if (-not (Test-Path -LiteralPath $buildConf)) {
        Exit-Fail "BUILD-CONFIG.txt 缺失: $buildConf（先运行 fetch-ffmpeg.ps1）"
    }
    Copy-Item -LiteralPath $buildConf -Destination (Join-Path $stage 'licenses\ffmpeg\BUILD-CONFIG.txt')
    Copy-Item -LiteralPath (Join-Path $licensesDir 'scrcpy\LICENSE.txt') -Destination (Join-Path $stage 'licenses\scrcpy\LICENSE.txt')

    $installMd = (Get-InstallTemplate) -replace '__VERSION__', $Version
    Write-Utf8BomFile -Path (Join-Path $stage 'INSTALL.md') -Text $installMd

    # SHA256SUMS.txt：包内全部文件（自身除外），路径用 '/' 分隔
    $sumsLines = New-Object System.Collections.Generic.List[string]
    foreach ($f in (Get-ChildItem -LiteralPath $stage -Recurse -File | Sort-Object FullName)) {
        $rel = $f.FullName.Substring($stage.Length + 1) -replace '\\', '/'
        if ($rel -ieq 'SHA256SUMS.txt') { continue }
        $sha = Get-Sha256Path -Path $f.FullName
        # 方法调用参数列表内 -f 的逗号会被当参数分隔符，须显式 @() 打包
        $sumsLines.Add(('{0}  {1}' -f @($sha, $rel))) | Out-Null
    }
    [System.IO.File]::WriteAllLines((Join-Path $stage 'SHA256SUMS.txt'), $sumsLines, (New-Object System.Text.UTF8Encoding($false)))

    # ---------- 打 zip ----------
    $zipPath = Join-Path $DistDir ('Gamer-{0}-windows-x64-full.zip' -f $Version)
    if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
    Write-Host "[package-full] 压缩: $zipPath"
    New-ZipFromDirectory -SourceDir $stage -DestFile $zipPath
} catch {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    throw
}
Remove-Item -LiteralPath $stage -Recurse -Force

# ---------- 结构校验：解压 → 文件齐全 → SHA256SUMS → manifest 校验 ----------
Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null
$zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
if ($null -eq $zip) { Exit-Fail "zip 打开失败: $zipPath" }
try {
    $bad = $zip.Entries | Where-Object { $_.FullName -like '*\*' } | Select-Object -First 1
    if ($null -ne $bad) { Exit-Fail "zip 条目含反斜杠分隔: $($bad.FullName)" }
    $entryCount = $zip.Entries.Count
} finally { $zip.Dispose() }

$verify = Join-Path $DistDir ('verify-full-' + $Version)
if (-not [IO.Path]::GetFullPath($verify).StartsWith($distBoundary, [StringComparison]::OrdinalIgnoreCase)) { Exit-Fail 'verify 超出输出目录' }
if (Test-Path -LiteralPath $verify) { Remove-Item -LiteralPath $verify -Recurse -Force }
try {
    Expand-Archive -LiteralPath $zipPath -DestinationPath $verify -Force

    foreach ($rel in @(
        'Gamer.exe', 'state/current.json',
        'config\config.toml',
        ('manifests\{0}.json' -f $Version),
        ('versions\' + $Version + '\web-dist\index.html'),
        'SHA256SUMS.txt', 'INSTALL.md',
        'licenses\NOTICE.md',
        'licenses\android-platform-tools\LICENSE.txt', 'licenses\android-platform-tools\NOTICE.txt',
        'licenses\ffmpeg\COPYING.LESSER', 'licenses\ffmpeg\SOURCE-OFFER.txt', 'licenses\ffmpeg\BUILD-CONFIG.txt',
        'licenses\scrcpy\LICENSE.txt'
    )) {
        if (-not (Test-Path -LiteralPath (Join-Path $verify $rel))) { Exit-Fail "解压后缺失: $rel" }
    }


    if (Get-ChildItem -LiteralPath (Join-Path $verify 'data') -File -Recurse -ErrorAction SilentlyContinue) {
        Exit-Fail '发行包禁止携带个人数据'
    }

    # SHA256SUMS 逐条核对 + 完备性（除自身外每个文件都在清单里）
    $expected = @{}
    foreach ($line in (Get-Content -LiteralPath (Join-Path $verify 'SHA256SUMS.txt'))) {
        if ($line.Trim().Length -eq 0) { continue }
        if ($line -notmatch '^([0-9a-f]{64})  (.+)$') { Exit-Fail "SHA256SUMS 行格式非法: $line" }
        $expected[$Matches[2]] = $Matches[1]
    }
    if ($expected.Count -eq 0) { Exit-Fail 'SHA256SUMS 为空' }
    foreach ($rel in $expected.Keys) {
        $p = Join-Path $verify ($rel -replace '/', '\')
        if (-not (Test-Path -LiteralPath $p)) { Exit-Fail "SHA256SUMS 引用的文件缺失: $rel" }
        if ((Get-Sha256Path -Path $p) -ne $expected[$rel]) { Exit-Fail "SHA256SUMS 不符: $rel" }
    }
    $allFiles = @(Get-ChildItem -LiteralPath $verify -Recurse -File | ForEach-Object {
        $_.FullName.Substring($verify.Length + 1) -replace '\\', '/'
    })
    foreach ($f in $allFiles) {
        if ($f -ieq 'SHA256SUMS.txt') { continue }
        if (-not $expected.ContainsKey($f)) { Exit-Fail "包内文件未列入 SHA256SUMS: $f" }
    }
    Write-Host "[package-full] SHA256SUMS 校验通过（$($expected.Count) 条）"

    # manifest 结构与语义校验
    $extractedManifest = Join-Path $verify ('manifests\{0}.json' -f $Version)
    & node (Join-Path $repoRoot 'release\contracts\validate-manifest.mjs') check $extractedManifest --expect-current-version $Version --expect-channel $Channel
    if ($LASTEXITCODE -ne 0) { Exit-Fail "包内 manifest 校验未通过（退出码 $LASTEXITCODE）" }

    if (-not $SkipSmoke) {
        & (Join-Path $PSScriptRoot 'test-portable-start.ps1') -Root $verify
        if ($LASTEXITCODE -ne 0) { throw '便携版启动失败' }
    }
    # The complete layout is ready to execute without bootstrap installation.
    if ((Get-Sha256Path (Join-Path $verify 'Gamer.exe')) -ne (Get-Sha256Path (Join-Path $verify "versions/$Version/gamer-server.exe"))) { throw '入口与发行程序不一致' }

} finally {
    Remove-Item -LiteralPath $verify -Recurse -Force -ErrorAction SilentlyContinue
}
exit 0
