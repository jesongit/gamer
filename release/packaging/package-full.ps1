# REL-002: 组装 Full bootstrap 包 GameBot-<version>-windows-x64-full.zip。
#
# 布局（解压即安装根，launcher 按此消费）:
#   gamer-launcher.exe                  cargo build --release（launcher crate 独立工作区）
#   config/config.toml                  模板（launcher 托管模式：路径留空由注入，
#                                       password_hash 占位，字段按 server/src/config.rs 写全）
#   data/<应用包名>/{yaml,func,tmpl,keymap}/ 仓库内置的脚本、函数、模板和映射种子
#   manifests/<version>.json + .sig     gen-manifest.ps1 产物
#   keys/<key_id>.pem                   dev 公钥（生产改为内置信任库）
#   seeds/                              gamer-app zip、adb zip、ffmpeg zip、scrcpy-server jar
#   SHA256SUMS.txt                      包内全部文件哈希清单
#   INSTALL.md                          解压即用说明
#   licenses/                           DEP-005 第三方声明（NOTICE + 各许可全文 + FFmpeg
#                                       源码 offer + BUILD-CONFIG，履约 dependencies.lock.toml）
#
# 组包后自动结构校验：解压到临时目录 → 文件齐全 → SHA256SUMS 逐条对 →
# manifest 用包内公钥验签。可选 -SkipSmoke 跳过 gamer-launcher.exe doctor 冒烟。
# 兼容 Windows PowerShell 5.1 与 pwsh。

[CmdletBinding()]
param(
    # 跳过 launcher 构建，复用 launcher/target/release/gamer-launcher.exe
    [switch]$SkipBuild,
    # 跳过解压后 gamer-launcher.exe doctor 冒烟
    [switch]$SkipSmoke,
    # 产品版本（默认读 server/Cargo.toml）
    [string]$Version = '',
    # 签名 key_id（默认 dev-ed25519-1）
    [string]$KeyId = 'dev-ed25519-1',
    [string]$DistDir = '',
    # manifest 目录（gen-manifest.ps1 输出，默认 <repo>/release/manifests）
    [string]$ManifestDir = '',
    [string]$KeysDir = ''
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
    $s = @'
# =============================================================================
# GameBot 配置模板（launcher 托管模式 / REL-002 Full 包）
# =============================================================================
# 本文件由 GameBot 便携包携带，运行时由 gamer-launcher.exe 管理：
# GAMER_APP_DIR / GAMER_DATA_DIR / GAMER_ADB_PATH / GAMER_FFMPEG_PATH /
# GAMER_SCRCPY_SERVER 等绝对路径由 launcher 启动 server 时注入环境变量，
# 优先级高于本文件同名字段——标注「launcher 注入」的条目留空即可，无需手改。
# 字段与 server/src/config.rs 一一对应；文件解析失败 / 校验不过进程直接退出。
# 完整字段说明见仓库 server/config.example.toml 与 server/src/config.rs。

# HTTP 监听端口 [1, 65535]
port = 8443

# 数据目录（SQLite、模板图片、脚本按应用分区存放于其下）。
# 相对路径相对本配置文件所在目录解析；launcher 托管模式会注入绝对路径覆盖。
data_dir = "./data"

# 应用资产根目录（jar / web-dist 解析基准）——launcher 注入，无需配置
# app_dir = ""

# 外部工具路径：留空 = 由 launcher 注入绝对路径（runtime/ 下的 adb、ffmpeg 与
# 版本目录内 assets/scrcpy-server.jar）。脱离 launcher 独立运行时才手填：
#   adb_path / ffmpeg_path: PATH 内命令名或绝对路径；可执行性启动探测只告警不阻断
#   scrcpy_server: 启动必检，指向的 jar 缺失直接退出
adb_path = ""
ffmpeg_path = ""
scrcpy_server = ""

# 脚本引擎默认参数（可被脚本内 config: 段覆盖）
interval = "500ms"        # 轮询与点击后等待间隔，带单位 ms/s/m/min/h/d；裸数字非法
threshold = 0.85          # 模板匹配阈值，(0, 1]
log_level = "info"        # debug / info / warn / error
judge_delay_ms = 200      # 判断类步骤命中后延迟毫秒，0 = 关闭，上限 60000
decode_frames = true      # 视频流软解码（模板匹配取帧）
max_size = 0              # scrcpy 最大分辨率，0 = 原始；非 0 须为 8 的倍数 [16, 4096]
bitrate_mbps = 12         # 码率上限 [1, 50]
fps = 15                  # 帧率上限，0 = 设备默认，≤120
encoder_name = ""         # scrcpy 编码器名，空 = 设备默认
probe_encoder = false     # 编码器质量探针（纯诊断，默认关闭）

# 空闲低功耗秒数：无 viewer 且无脚本运行持续 N 秒后拆会话/关屏；0 = 关闭
idle_power_secs = 300

# 服务端文件日志按天轮转的保留天数（GB_LOG 指向文件时生效）；0 = 永不清理
log_retain_days = 14

# 专用计算池并发上限（NCC 匹配/PNG 解码等）：0 = 按 CPU 核数自动，显式值 ≤256
compute_max_concurrency = 0

# WebRTC ICE 外部宣告（容器 / NAT 1-to-1 部署才需要；缺省 = host candidate 直连）。
# rtc_udp_port 必须与 rtc_external_ip 成对配置（启动校验强制），rtc_external_port
# 依赖 rtc_udp_port。本机/局域网使用保持全零即可。
rtc_external_ip = ""
rtc_udp_port = 0
rtc_external_port = 0

# 鉴权与会话治理。password_hash 初始留空：首次打开登录页即可设置管理员密码，
# 服务端会把密码转换为 Argon2id PHC 并保存；GAMER_ADMIN_PASSWORD 仍可供开发/自动化
# 场景使用（仅进程内生效，不落盘）。
[auth]
session_abs_secs = 43200   # 会话绝对有效期秒 [60, 2592000]
session_idle_secs = 7200   # 会话空闲有效期秒 [60, 604800]
login_max_fails = 10       # 登录限流失败次数上限 [1, 1000]
login_window_secs = 300    # 限流滑动窗口秒 [1, 86400]
password_hash = ""         # Argon2id PHC（launcher 托管模式：首次启动后设置）
'@
    return $s
}

function Get-InstallTemplate {
    $s = @'
# GameBot 安装与首次使用（Windows x64 便携包 v__VERSION__）

## 第 1 步：解压

把 `GameBot-__VERSION__-windows-x64-full.zip` 解压到本地目录（建议路径不含中文与
空格，例如 `D:\GameBot`）。**必须保持解压出的相对布局**：`gamer-launcher.exe`
与 `config\`、`data\`、`manifests\`、`keys\`、`seeds\`、`licenses\`、`SHA256SUMS.txt` 在
同一目录，不要单独把 exe 拖出去运行。

## 第 2 步：双击启动

双击解压目录中的 `gamer-launcher.exe` 即可。启动器会自动从包内 `seeds\` 安装
或修复 adb、ffmpeg、scrcpy-server 和 GameBot 本体，首次运行不需要打开命令行，
也不需要手动执行 `repair` 或 `start`；依赖已经完整时会自动跳过。

启动成功后浏览器会打开（或手动打开）`http://127.0.0.1:8443`。

## 首次设置登录密码

第一次打开登录页时会显示“设置密码并进入”：输入至少 8 位管理员密码并确认即可，
密码只以 Argon2id 不可逆哈希保存到 `config\config.toml`，设置成功后会自动登录。
以后双击 `gamer-launcher.exe` 启动，再用该管理员密码登录即可。

包内已带入仓库中的脚本、函数库、模板图片和按应用分区的按键映射，首次启动即可使用；
运行过程中新增或修改的资源会继续保存在 `data\`，升级时不会由 launcher 自动覆盖。

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
if (-not $KeysDir)     { $KeysDir     = Join-Path $repoRoot 'release\keys' }

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
$jarSrc         = Join-Path $repoRoot 'server\assets\scrcpy-server.jar'
$dataSeedDir    = Join-Path $repoRoot 'server\data'
$manifestJson   = Join-Path $ManifestDir ('{0}.json' -f $Version)
$manifestSig    = Join-Path $ManifestDir ('{0}.sig' -f $Version)
$pubKey         = Join-Path $KeysDir ('{0}.pem' -f $KeyId)
$licensesDir    = Join-Path $repoRoot 'licenses'

$components = Import-LockComponents -Path (Join-Path $repoRoot 'release\dependencies.lock.toml')
$adb    = Get-LockComponent -Components $components -Id 'adb'
$ffmpeg = Get-LockComponent -Components $components -Id 'ffmpeg'
$scrcpy = Get-LockComponent -Components $components -Id 'scrcpy-server'
$adbVersion    = [string]$adb['version']
$ffmpegVersion = [string]$ffmpeg['version']
$jarSeedName   = [string]$scrcpy['source_artifact_name']   # scrcpy-server-v3.3.3

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
    $launcherExe, $jarSrc, $dataSeedDir, $manifestJson, $manifestSig, $pubKey,
    (Join-Path $DistDir $appZipName), (Join-Path $DistDir $adbZipName), (Join-Path $DistDir $ffmpegZipName),
    (Join-Path $licensesDir 'NOTICE.md')
)) {
    if (-not (Test-Path -LiteralPath $must)) {
        Exit-Fail "缺少输入: $must（按需运行 package-app.ps1 / package-components.ps1 / gen-manifest.ps1；公钥缺失时由 gen-manifest.ps1 自动 keygen）"
    }
}

Write-Host "[package-full] 版本 $Version，key_id=$KeyId"

# ---------- 组装 staging ----------
$stage = Join-Path $DistDir ('staging-full-' + $Version)
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
foreach ($d in @('config', 'data', 'manifests', 'keys', 'seeds')) {
    New-Item -ItemType Directory -Path (Join-Path $stage $d) -Force | Out-Null
}
try {
    Copy-Item -LiteralPath $launcherExe -Destination (Join-Path $stage 'gamer-launcher.exe')
    Write-Utf8BomFile -Path (Join-Path $stage 'config\config.toml') -Text (Get-ConfigTemplate)

    # 初始业务资源随 Full 包分发，但只复制分区目录，不复制 server/data 根下的
    # gamer.db / -shm / -wal 等开发机运行时数据库文件。
    foreach ($partition in (Get-ChildItem -LiteralPath $dataSeedDir -Directory | Sort-Object Name)) {
        Copy-Item -LiteralPath $partition.FullName -Destination (Join-Path $stage 'data') -Recurse -Force
    }
    Copy-Item -LiteralPath $manifestJson -Destination (Join-Path $stage ('manifests\{0}.json' -f $Version))
    Copy-Item -LiteralPath $manifestSig  -Destination (Join-Path $stage ('manifests\{0}.sig' -f $Version))
    Copy-Item -LiteralPath $pubKey       -Destination (Join-Path $stage ('keys\{0}.pem' -f $KeyId))
    foreach ($n in @($appZipName, $adbZipName, $ffmpegZipName)) {
        Copy-Item -LiteralPath (Join-Path $DistDir $n) -Destination (Join-Path $stage ('seeds\' + $n))
    }
    Copy-Item -LiteralPath $jarSrc -Destination (Join-Path $stage ('seeds\' + $jarSeedName))

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
    $zipPath = Join-Path $DistDir ('GameBot-{0}-windows-x64-full.zip' -f $Version)
    if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
    Write-Host "[package-full] 压缩: $zipPath"
    New-ZipFromDirectory -SourceDir $stage -DestFile $zipPath
} catch {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    throw
}
Remove-Item -LiteralPath $stage -Recurse -Force

# ---------- 结构校验：解压 → 文件齐全 → SHA256SUMS → manifest 验签 ----------
Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null
$zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
if ($null -eq $zip) { Exit-Fail "zip 打开失败: $zipPath" }
try {
    $bad = $zip.Entries | Where-Object { $_.FullName -like '*\*' } | Select-Object -First 1
    if ($null -ne $bad) { Exit-Fail "zip 条目含反斜杠分隔: $($bad.FullName)" }
    $entryCount = $zip.Entries.Count
} finally { $zip.Dispose() }

$verify = Join-Path $DistDir ('verify-full-' + $Version)
if (Test-Path -LiteralPath $verify) { Remove-Item -LiteralPath $verify -Recurse -Force }
try {
    Expand-Archive -LiteralPath $zipPath -DestinationPath $verify -Force

    foreach ($rel in @(
        'gamer-launcher.exe',
        'config\config.toml',
        ('manifests\{0}.json' -f $Version), ('manifests\{0}.sig' -f $Version),
        ('keys\{0}.pem' -f $KeyId),
        ('seeds\' + $appZipName), ('seeds\' + $adbZipName), ('seeds\' + $ffmpegZipName), ('seeds\' + $jarSeedName),
        'SHA256SUMS.txt', 'INSTALL.md',
        'licenses\NOTICE.md',
        'licenses\android-platform-tools\LICENSE.txt', 'licenses\android-platform-tools\NOTICE.txt',
        'licenses\ffmpeg\COPYING.LESSER', 'licenses\ffmpeg\SOURCE-OFFER.txt', 'licenses\ffmpeg\BUILD-CONFIG.txt',
        'licenses\scrcpy\LICENSE.txt'
    )) {
        if (-not (Test-Path -LiteralPath (Join-Path $verify $rel))) { Exit-Fail "解压后缺失: $rel" }
    }

    # 每个仓库内置资源分区及其中的文件都必须进入 Full 包；数据库等运行时文件不属于种子。
    foreach ($partition in (Get-ChildItem -LiteralPath $dataSeedDir -Directory | Sort-Object Name)) {
        $expectedPartition = Join-Path $verify ('data\' + $partition.Name)
        if (-not (Test-Path -LiteralPath $expectedPartition -PathType Container)) {
            Exit-Fail "解压后缺失数据分区: data/$($partition.Name)"
        }
        foreach ($seedFile in (Get-ChildItem -LiteralPath $partition.FullName -Recurse -File)) {
            $relative = $seedFile.FullName.Substring($dataSeedDir.Length + 1) -replace '\\', '/'
            $expectedFile = Join-Path $verify ('data\' + ($relative -replace '/', '\'))
            if (-not (Test-Path -LiteralPath $expectedFile -PathType Leaf)) {
                Exit-Fail "解压后缺失种子文件: data/$relative"
            }
        }
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

    # manifest 验签（包内公钥 = 信任锚）
    $extractedManifest = Join-Path $verify ('manifests\{0}.json' -f $Version)
    & node (Join-Path $repoRoot 'release\contracts\validate-manifest.mjs') check $extractedManifest --keys-dir (Join-Path $verify 'keys') --expect-current-version $Version --expect-channel stable
    if ($LASTEXITCODE -ne 0) { Exit-Fail "包内 manifest 验签未通过（退出码 $LASTEXITCODE）" }

    # launcher doctor 冒烟（另一轨道在扩展 launcher；失败只报告不阻断组包）
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
        else { Write-Host "[package-full] doctor 冒烟: 退出码 $code（smoke 仅报告，不阻断）" -ForegroundColor Yellow }
    }

    $zipSize = (Get-Item -LiteralPath $zipPath).Length
    Write-Host ("[package-full] PASS: {0}（{1} 字节, {2} 个条目）" -f $zipPath, $zipSize, $entryCount)
} finally {
    Remove-Item -LiteralPath $verify -Recurse -Force -ErrorAction SilentlyContinue
}
exit 0
