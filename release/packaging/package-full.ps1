# REL-002: 组装 Full bootstrap 包 Gamer-<version>-windows-x64-full.zip。
#
# 布局（解压即安装根，launcher 按此消费）:
#   gamer-launcher.exe                  cargo build --release（launcher crate 独立工作区）
#   config/config.toml                  模板（launcher 托管模式：路径留空由注入，
#                                       password_hash 占位，字段按 server/src/config.rs 写全）
#   data/                              空目录，不携带个人配置或运行数据
#   manifests/<version>.json     gen-manifest.ps1 产物
#   seeds/                              发行清单锁定的本体、运行依赖、启动器、官方插件 ZIP
#   SHA256SUMS.txt                      包内全部文件哈希清单
#   INSTALL.md                          解压即用说明
#   licenses/                           DEP-005 第三方声明（NOTICE + 各许可全文 + FFmpeg
#                                       源码 offer + BUILD-CONFIG，履约 dependencies.lock.toml）
#
# 组包后自动结构校验：解压到临时目录 → 文件齐全 → SHA256SUMS 逐条对 →
# manifest 结构与语义校验。可选 -SkipSmoke 跳过 gamer-launcher.exe doctor 冒烟。
# 兼容 Windows PowerShell 5.1 与 pwsh。

[CmdletBinding()]
param(
    # 跳过 launcher 构建，复用 launcher/target/release/gamer-launcher.exe
    [switch]$SkipBuild,
    # 跳过解压后 gamer-launcher.exe doctor 冒烟
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
    $s = @'
# Gamer 安装与首次使用（Windows x64 便携包 v__VERSION__）

## 第 1 步：解压

把 `Gamer-__VERSION__-windows-x64-full.zip` 解压到本地目录（建议路径不含中文与
空格，例如 `D:\Gamer`）。**必须保持解压出的相对布局**：`gamer-launcher.exe`
与 `config\`、`data\`、`manifests\`、`seeds\`、`licenses\`、`SHA256SUMS.txt` 在
同一目录，不要单独把 exe 拖出去运行。

## 第 2 步：双击启动

双击解压目录中的 `gamer-launcher.exe`，首次在小窗口点击“安装”，再从包内
`seeds\` 安装 ADB、FFmpeg/FFprobe、scrcpy-server 与 Gamer 本体。安装支持暂停
和恢复；已校验且未改变的文件不会重复计算哈希。有可用更新时显示启动器，
其余情况下在后台检查后直接启动并留在托盘，不先弹出窗口。更新窗口只提供“更新 / 取消”。
关闭窗口保留托盘，右键仅“打开 Gamer / 退出 Gamer”；双击托盘或再次双击 EXE，运行时打开网页，安装或更新时显示窗口。检查更新在工作台“设置”里，文件在启动时自动校验修复。

启动成功后浏览器会打开（或手动打开）`http://127.0.0.1:8443`。

## 首次设置登录密码

第一次打开登录页时会显示“设置密码并进入”：输入至少 8 位管理员密码并确认即可，
密码只以 Argon2id 不可逆哈希保存到 `config\config.toml`，设置成功后会自动登录。
以后双击 `gamer-launcher.exe` 启动，再用该管理员密码登录即可。

包内携带自动化、键盘映射、视频工作台三个官方插件安装包，首次启动展示权限并
选择安装（点“继续”，也可“跳过”）。包内没有个人脚本、配置、媒体或数据库；需要的配置包可在软件中导入。
运行数据保存在 `data\`，更新时由启动器备份与保护。

## 其他

- 配置模板 `config\config.toml` 为 launcher 托管模式：`adb_path`/`ffmpeg_path`/
  `scrcpy_server`/`data_dir` 等路径留空即可，由 launcher 注入绝对路径，无需手改。
- 高级维护仍可在命令行运行 `gamer-launcher.exe doctor`、`repair` 或 `upgrade`，
  日常使用不需要这些命令。
- 第三方组件许可声明见 `licenses\NOTICE.md`（Apache-2.0 / LGPL-3.0 履约文本）。
- 升级：`gamer-launcher.exe upgrade`（检查 manifest 并原子升级；离线环境把新版
  full 包解压覆盖即可，数据目录不受影响）。
'@
    return $s
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
$launcherExe    = Join-Path $repoRoot 'launcher\target\release\gamer-launcher.exe'
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
    Write-Host "[package-full] cargo build --release（launcher crate，独立工作区）..."
    Push-Location (Join-Path $repoRoot 'launcher')
    try { & cargo build --release; if ($LASTEXITCODE -ne 0) { throw "cargo build 退出码 $LASTEXITCODE" } }
    finally { Pop-Location }
}

foreach ($must in @(
    $launcherExe, $manifestJson,
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
foreach ($d in @('config', 'data', 'manifests', 'seeds')) {
    New-Item -ItemType Directory -Path (Join-Path $stage $d) -Force | Out-Null
}
try {
    Copy-Item -LiteralPath $launcherExe -Destination (Join-Path $stage 'gamer-launcher.exe')
    Write-Utf8BomFile -Path (Join-Path $stage 'config\config.toml') -Text (Get-ConfigTemplate)

    # 数据目录留空，首次启动由 Core 播种默认配置包。
    Copy-Item -LiteralPath $manifestJson -Destination (Join-Path $stage ('manifests\{0}.json' -f $Version))
    $releaseModel = Get-Content -LiteralPath $manifestJson -Raw | ConvertFrom-Json
    $seedNames = @($releaseModel.platforms.'windows-x86_64'.app.artifact.name) + @($releaseModel.platforms.'windows-x86_64'.components | ForEach-Object { $_.artifact.name })
    foreach ($n in $seedNames) {
        Copy-Item -LiteralPath (Join-Path $DistDir $n) -Destination (Join-Path $stage ('seeds\' + $n))
    }

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
        'gamer-launcher.exe',
        'config\config.toml',
        ('manifests\{0}.json' -f $Version),
        ('seeds\' + $appZipName), ('seeds\' + $adbZipName), ('seeds\' + $ffmpegZipName),
        'SHA256SUMS.txt', 'INSTALL.md',
        'licenses\NOTICE.md',
        'licenses\android-platform-tools\LICENSE.txt', 'licenses\android-platform-tools\NOTICE.txt',
        'licenses\ffmpeg\COPYING.LESSER', 'licenses\ffmpeg\SOURCE-OFFER.txt', 'licenses\ffmpeg\BUILD-CONFIG.txt',
        'licenses\scrcpy\LICENSE.txt'
    )) {
        if (-not (Test-Path -LiteralPath (Join-Path $verify $rel))) { Exit-Fail "解压后缺失: $rel" }
    }
    foreach ($seed in $seedNames) {
        if (-not (Test-Path -LiteralPath (Join-Path $verify ('seeds\' + $seed)))) { Exit-Fail "解压后缺少清单种子: $seed" }
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

    # launcher doctor 冒烟失败必须阻断组包。
    if (-not $SkipSmoke) {
        Write-Host '[package-full] launcher doctor 冒烟...'
        $prevEap = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $out = & (Join-Path $verify 'gamer-launcher.exe') --install-root $verify doctor 2>&1 | Out-String
            $code = $LASTEXITCODE
        } finally { $ErrorActionPreference = $prevEap }
        foreach ($l in ($out.Trim() -split "`r?`n")) { Write-Host "  | $l" }
        if ($code -eq 0) { Write-Host '[package-full] doctor 冒烟: 退出码 0' -ForegroundColor Green }
        else { Exit-Fail "doctor 冒烟失败，退出码 $code" }
    }

    $zipSize = (Get-Item -LiteralPath $zipPath).Length
    Write-Host ("[package-full] PASS: {0}（{1} 字节, {2} 个条目）" -f $zipPath, $zipSize, $entryCount)
} finally {
    Remove-Item -LiteralPath $verify -Recurse -Force -ErrorAction SilentlyContinue
}
exit 0
