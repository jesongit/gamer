<#
.SYNOPSIS
    DEP-003: 获取 Windows ffmpeg（BtbN win64-lgpl 静态构建）并跑许可红线与真实冒烟。

.DESCRIPTION
    从 release/dependencies.lock.toml 读取 ffmpeg 条目，下载 BtbN win64-lgpl
    构建 zip 并校验 sha256（锁定时点），解包仅保留 bin/ffmpeg.exe 到
    release/vendor/ffmpeg/<version>/。验收门禁（任一失败即整体失败）:
      1) ffmpeg.exe -version  版本串必须与锁一致
      2) ffmpeg.exe -buildconf 不得出现 --enable-gpl / --enable-nonfree（许可红线）
      3) ffmpeg.exe -L         必须为 GNU Lesser General Public License
      4) 真实冒烟: H.264 Annex-B stdin 管道 -> PNG stdout（模拟服务端 frames.rs
         解码用法），输出必须是合法 PNG
    归档: -buildconf 完整输出写入 vendor BUILD-CONFIG.txt（LGPL「解释如何编译」要求）。
    -VerifyOnly: 不下载，只校验已有 vendor 与锁一致并重跑红线检查。

    兼容 Windows PowerShell 5.1 与 pwsh。vendor/ 目录不入 git（.gitignore）。

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File release/packaging/fetch-ffmpeg.ps1
    powershell -NoProfile -ExecutionPolicy Bypass -File release/packaging/fetch-ffmpeg.ps1 -VerifyOnly
    powershell -NoProfile -ExecutionPolicy Bypass -File release/packaging/fetch-ffmpeg.ps1 -Proxy http://127.0.0.1:7890
#>
[CmdletBinding()]
param(
    [switch]$VerifyOnly,
    [string]$LockPath = '',
    [string]$VendorRoot = '',
    [string]$DownloadDir = '',
    [string]$Proxy = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $LockPath)    { $LockPath    = Join-Path $repoRoot 'release\dependencies.lock.toml' }
if (-not $VendorRoot)  { $VendorRoot  = Join-Path $repoRoot 'release\vendor' }
if (-not $DownloadDir) { $DownloadDir = Join-Path $VendorRoot '_downloads' }

Import-Module (Join-Path $PSScriptRoot 'LockFile.psm1') -Force
Initialize-DownloadEnvironment

# 统一用 .NET Process 捕获子进程输出，规避 PS5.1 下 2>&1 与 $ErrorActionPreference=Stop
# 组合把 stderr 包装成 ErrorRecord 误触发异常的坑。
function Invoke-ProcessCapture {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string]$Arguments,
        [byte[]]$StdInBytes = $null,
        [int]$TimeoutMs = 120000
    )
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $FilePath
    $psi.Arguments = $Arguments
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.RedirectStandardInput = ($null -ne $StdInBytes)
    $psi.WorkingDirectory = Split-Path -Parent $FilePath
    $p = [System.Diagnostics.Process]::Start($psi)

    if ($psi.RedirectStandardInput) {
        # 输入为约 1-2KB 的单帧流，先写完关 stdin，再排空 stdout，避免管道互锁
        $p.StandardInput.BaseStream.Write($StdInBytes, 0, $StdInBytes.Length)
        $p.StandardInput.BaseStream.Flush()
        $p.StandardInput.Close()
    }

    # 必须在 WaitForExit 之前持续排空 stdout/stderr，否则大输出（如 -encoders 约 30KB）
    # 撑满管道缓冲后子进程阻塞在写、父进程阻塞在等退出，互相死锁。
    $outMs = New-Object System.IO.MemoryStream
    $p.StandardOutput.BaseStream.CopyTo($outMs)
    $errText = $p.StandardError.ReadToEnd()
    if (-not $p.WaitForExit($TimeoutMs)) {
        try { $p.Kill() } catch { }
        throw "进程超时被终止: $FilePath $Arguments"
    }

    return @{
        ExitCode    = $p.ExitCode
        StdOut      = [System.Text.Encoding]::UTF8.GetString($outMs.ToArray())
        StdOutBytes = $outMs.ToArray()
        StdErr      = $errText
    }
}

function Exit-Fail {
    param([string]$Message)
    Write-Host "[fetch-ffmpeg] FAIL: $Message" -ForegroundColor Red
    exit 1
}

# 许可红线: buildconf 不得出现 gpl/nonfree；-L 必须为 LGPL
function Test-LicenseGates {
    param([Parameter(Mandatory = $true)][string]$Exe)

    $bc = Invoke-ProcessCapture -FilePath $Exe -Arguments '-hide_banner -buildconf'
    $bcText = $bc.StdOut + $bc.StdErr
    if ($bcText -match '--enable-gpl\b') { throw "许可红线: -buildconf 出现 --enable-gpl（GPL 构建，禁止分发）" }
    if ($bcText -match '--enable-nonfree\b') { throw "许可红线: -buildconf 出现 --enable-nonfree（禁止分发）" }
    if ($bcText -notmatch '--enable-version3\b') {
        Write-Host "[fetch-ffmpeg] 警告: buildconf 无 --enable-version3，许可表述需按 dependency-licensing.md §2.3 复核" -ForegroundColor Yellow
    }

    $lic = Invoke-ProcessCapture -FilePath $Exe -Arguments '-hide_banner -L'
    $licText = $lic.StdOut + $lic.StdErr
    if ($licText -notmatch 'Lesser General Public License') {
        throw "许可红线: -L 输出不含 Lesser General Public License（实际: $($licText.Trim() -split "`n" | Select-Object -First 2)）"
    }
    Write-Host "  [红线OK] buildconf 无 --enable-gpl / --enable-nonfree; -L 为 LGPL"
    return $bcText
}

try {
    $version = ''
    Write-Host "[fetch-ffmpeg] 锁文件: $LockPath"
    $components = Import-LockComponents -Path $LockPath
    $c = Get-LockComponent -Components $components -Id 'ffmpeg'
    $version = [string]$c['version']
    $expectedZipSha = ([string]$c['source_sha256']).ToLowerInvariant()
    $expectedZipSize = [long]$c['source_size']
    $url = [string]$c['source_url']

    $destDir = Join-Path $VendorRoot "ffmpeg\$version"
    Write-Host "[fetch-ffmpeg] 组件 ffmpeg@$version -> $destDir"

    # ---------- VerifyOnly: 只校验已有 vendor + 红线 ----------
    if ($VerifyOnly) {
        if (-not (Test-Path -LiteralPath $destDir)) {
            Exit-Fail "vendor 目录不存在: $destDir（先以默认模式运行 fetch-ffmpeg.ps1）"
        }
        Write-Host "[fetch-ffmpeg] 校验 vendor 与锁 files[] 一致性..."
        $filesOk = Test-LockFilesInDir -Files $c.files -Dir $destDir
        if (-not $filesOk) { Exit-Fail "vendor 文件与锁文件不一致" }

        $exe = Join-Path $destDir 'ffmpeg.exe'
        $ver = Invoke-ProcessCapture -FilePath $exe -Arguments '-version'
        $vMatch = [regex]::Match($ver.StdOut, 'ffmpeg version (\S+)')
        if (-not $vMatch.Success -or $vMatch.Groups[1].Value -ne $version) {
            Exit-Fail "ffmpeg 版本串 '$($vMatch.Groups[1].Value)' 与锁定版本 '$version' 不符"
        }
        Write-Host "  [版本OK] $($vMatch.Groups[1].Value)"
        Test-LicenseGates -Exe $exe | Out-Null
        Write-Host "[fetch-ffmpeg] PASS（VerifyOnly）" -ForegroundColor Green
        exit 0
    }

    # ---------- 默认模式: 下载 -> 校验 -> 裁包 -> 门禁 -> 冒烟 -> 就位 ----------
    if (-not (Test-Path -LiteralPath $DownloadDir)) {
        New-Item -ItemType Directory -Path $DownloadDir -Force | Out-Null
    }
    $zipPath = Join-Path $DownloadDir ([string]$c['source_artifact_name'])

    $needDownload = $true
    if (Test-Path -LiteralPath $zipPath) {
        $cachedSha = Get-Sha256 -Path $zipPath
        if ($cachedSha -eq $expectedZipSha) {
            Write-Host "[fetch-ffmpeg] 缓存命中且 sha256 一致，跳过下载: $zipPath"
            $needDownload = $false
        } else {
            Write-Host "[fetch-ffmpeg] 缓存 sha256 与锁不一致（$($cachedSha.Substring(0,16))...），重新下载" -ForegroundColor Yellow
        }
    }
    if ($needDownload) {
        Write-Host "[fetch-ffmpeg] 下载（约 140MB，耐心等待）: $url"
        Invoke-VerifiedDownload -Url $url -DestFile $zipPath -ExpectedSha256 $expectedZipSha -ExpectedSize $expectedZipSize -Proxy $Proxy
    }

    # 解包: 只取 */bin/ffmpeg.exe（原字节）
    Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null
    $staging = Join-Path $VendorRoot "ffmpeg\$version.staging"
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    New-Item -ItemType Directory -Path $staging -Force | Out-Null

    try {
        $zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
        if ($null -eq $zip) { throw "zip 打开失败: $zipPath" }
        try {
            $entries = @($zip.Entries | Where-Object {
                    $_.FullName -notmatch '(\.\.|^/)' -and $_.FullName -match '/bin/ffmpeg\.exe$'
                })
            if ($entries.Count -ne 1) { throw "zip 内未找到唯一 bin/ffmpeg.exe（匹配数: $($entries.Count)）" }
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entries[0], (Join-Path $staging 'ffmpeg.exe'), $true)
        } finally {
            $zip.Dispose()
        }

        $exe = Join-Path $staging 'ffmpeg.exe'

        # 门禁 1: 版本串与锁一致
        $ver = Invoke-ProcessCapture -FilePath $exe -Arguments '-version'
        $vMatch = [regex]::Match($ver.StdOut, 'ffmpeg version (\S+)')
        if (-not $vMatch.Success) { throw "无法从 -version 输出解析版本串" }
        if ($vMatch.Groups[1].Value -ne $version) {
            throw "ffmpeg 版本串 '$($vMatch.Groups[1].Value)' 与锁定版本 '$version' 不符（BtbN latest 可能已滚动，请按 ARC-005 §2.2 重新锁定并重走门禁）"
        }
        Write-Host "  [版本OK] $($vMatch.Groups[1].Value)"

        # 门禁 2/3: 许可红线（gpl/nonfree、LGPL），并取得 buildconf 全文
        $bcText = Test-LicenseGates -Exe $exe

        # 归档 BUILD-CONFIG.txt（LGPL: “解释如何编译”）
        $buildConfig = @(
            "# GameBot ffmpeg BUILD-CONFIG（由 release/packaging/fetch-ffmpeg.ps1 自动归档）",
            "# 锁定日期: $([string]$c['locked_at'])",
            "# 版本串: $version",
            "# 来源: $url",
            "# zip sha256: $expectedZipSha",
            "# ffmpeg.exe sha256: $((Get-Sha256 -Path $exe))",
            "# 源码 offer: $([string]$c['source_offer'])",
            "",
            "----- ffmpeg -hide_banner -buildconf -----",
            $bcText.TrimEnd()
        )
        [System.IO.File]::WriteAllLines((Join-Path $staging 'BUILD-CONFIG.txt'), $buildConfig)

        # 门禁 4: 真实冒烟 —— H.264 Annex-B stdin 管道 -> PNG stdout
        # 生成流: lgpl 构建无 libx264，按 libopenh264(BSD) -> libx264 -> 硬件编码器 顺序选可用者
        $encOut = Invoke-ProcessCapture -FilePath $exe -Arguments '-hide_banner -encoders'
        $candidates = @('libopenh264', 'libx264', 'h264_qsv', 'h264_nvenc', 'h264_amf', 'h264_mf')
        $encoder = $null
        foreach ($cand in $candidates) {
            if ($encOut.StdOut -match ('\b' + [regex]::Escape($cand) + '\b')) { $encoder = $cand; break }
        }
        if ($null -eq $encoder) { throw "构建内无可用 H.264 编码器，无法自产冒烟流（-encoders 输出中无: $($candidates -join ', ')）" }

        $smokeH264 = Join-Path $staging 'smoke-tmp.h264'
        $genArgs = "-hide_banner -loglevel error -f lavfi -i testsrc=size=160x120:rate=10 -frames:v 1 -pix_fmt yuv420p -c:v $encoder -f h264 -y `"$smokeH264`""
        $gen = Invoke-ProcessCapture -FilePath $exe -Arguments $genArgs
        if ($gen.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $smokeH264)) {
            throw "用 $encoder 生成冒烟 H.264 流失败: $($gen.StdErr.Trim())"
        }
        Write-Host "  [冒烟] 冒烟流已生成（编码器 $encoder, $((Get-Item -LiteralPath $smokeH264).Length) 字节, 1 帧 Annex-B）"

        # 与服务端 frames.rs 用法同构: -f h264 -i pipe:0 ... -f image2pipe -c:v png pipe:1
        $h264Bytes = [System.IO.File]::ReadAllBytes($smokeH264)
        $decArgs = '-hide_banner -loglevel error -f h264 -i pipe:0 -frames:v 1 -f image2pipe -c:v png pipe:1'
        $dec = Invoke-ProcessCapture -FilePath $exe -Arguments $decArgs -StdInBytes $h264Bytes
        Remove-Item -LiteralPath $smokeH264 -Force
        $png = $dec.StdOutBytes
        if ($dec.ExitCode -ne 0) { throw "管道解码退出码 $($dec.ExitCode): $($dec.StdErr.Trim())" }
        if ($png.Length -lt 100) { throw "管道解码 PNG 输出过小（$($png.Length) 字节）" }
        $magicOk = ($png.Length -ge 8 -and $png[0] -eq 0x89 -and $png[1] -eq 0x50 -and $png[2] -eq 0x4E -and $png[3] -eq 0x47 -and $png[4] -eq 0x0D -and $png[5] -eq 0x0A -and $png[6] -eq 0x1A -and $png[7] -eq 0x0A)
        if (-not $magicOk) { throw "输出不是合法 PNG（魔数不符）" }
        $tail = [System.Text.Encoding]::ASCII.GetString($png[($png.Length - 12)..($png.Length - 1)])
        if (-not $tail.Contains('IEND')) { throw "PNG 缺少 IEND 块（输出被截断）" }
        [System.IO.File]::WriteAllBytes((Join-Path $staging 'smoke.png'), $png)
        Write-Host "  [冒烟OK] H.264 Annex-B stdin -> PNG stdout: $($png.Length) 字节, PNG 魔数/IEND 有效（证据 smoke.png）"

        # 逐文件校验 + SHA256SUMS.txt
        Write-Host "[fetch-ffmpeg] 逐文件校验（对照锁 files[]）..."
        $filesOk = Test-LockFilesInDir -Files $c.files -Dir $staging
        if (-not $filesOk) { throw "解包产物与锁 files[] 不一致，已放弃安装" }
        $sums = Join-Path $staging 'SHA256SUMS.txt'
        $lines = foreach ($f in $c.files) {
            '{0}  {1}' -f (Get-Sha256 -Path (Join-Path $staging ([string]$f['path']))), ([string]$f['path'])
        }
        [System.IO.File]::WriteAllLines($sums, $lines)

        # 原子就位
        if (Test-Path -LiteralPath $destDir) { Remove-Item -LiteralPath $destDir -Recurse -Force }
        Move-Item -LiteralPath $staging -Destination $destDir
    } catch {
        if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
        throw
    }

    Write-Host "[fetch-ffmpeg] PASS: $destDir" -ForegroundColor Green
    exit 0
} catch {
    Write-Host "[fetch-ffmpeg] FAIL: $($_.Exception.Message)" -ForegroundColor Red
    Write-Host "  若为网络问题，可加 -Proxy http://<代理地址> 或设置 HTTPS_PROXY 后重试" -ForegroundColor Yellow
    exit 1
}
