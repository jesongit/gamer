<#
.SYNOPSIS
    DEP-002: 获取并裁包 Windows adb（Android Platform-Tools 三件套）。

.DESCRIPTION
    从 release/dependencies.lock.toml 读取 adb 条目，下载官方 platform-tools
    Windows zip，校验整包 sha256，解包仅保留 adb.exe / AdbWinApi.dll /
    AdbWinUsbApi.dll（原字节提取）到 release/vendor/adb/<version>/，
    生成 SHA256SUMS.txt 清单并与锁文件 files[] 比对，不一致即失败。
    -VerifyOnly: 不下载，只校验已存在 vendor 目录与锁文件一致。

    兼容 Windows PowerShell 5.1 与 pwsh。vendor/ 目录不入 git（.gitignore）。

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File release/packaging/fetch-adb.ps1
    powershell -NoProfile -ExecutionPolicy Bypass -File release/packaging/fetch-adb.ps1 -VerifyOnly
    powershell -NoProfile -ExecutionPolicy Bypass -File release/packaging/fetch-adb.ps1 -Proxy http://127.0.0.1:7890
#>
[CmdletBinding()]
param(
    # 只校验已存在的 vendor 目录，不下载
    [switch]$VerifyOnly,
    # 依赖锁文件路径（默认 <仓库>/release/dependencies.lock.toml）
    [string]$LockPath = '',
    # vendor 输出根目录（默认 <仓库>/release/vendor）
    [string]$VendorRoot = '',
    # 下载缓存目录（默认 <VendorRoot>/_downloads；命中且 hash 一致时不重复下载）
    [string]$DownloadDir = '',
    # 代理（如 http://127.0.0.1:7890）；留空时失败会自动尝试 HTTPS_PROXY/HTTP_PROXY
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

function Exit-Fail {
    param([string]$Message)
    Write-Host "[fetch-adb] FAIL: $Message" -ForegroundColor Red
    exit 1
}

function Invoke-AdbVersion {
    # 原生命令 stderr 在 PS5.1 下被外部捕获时可能包装成 ErrorRecord，临时放宽 EAP 防误炸
    param([Parameter(Mandatory = $true)][string]$AdbExe)
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        return (& $AdbExe version 2>&1 | Out-String)
    } finally {
        $ErrorActionPreference = $prevEap
    }
}

try {
    $version = ''  # 供 catch 清理路径引用，锁解析前失败时为空
    Write-Host "[fetch-adb] 锁文件: $LockPath"
    $components = Import-LockComponents -Path $LockPath
    $c = Get-LockComponent -Components $components -Id 'adb'
    $version = [string]$c['version']
    $expectedZipSha = ([string]$c['source_sha256']).ToLowerInvariant()
    $expectedZipSize = [long]$c['source_size']
    $url = [string]$c['source_url']

    $destDir = Join-Path $VendorRoot "adb\$version"
    Write-Host "[fetch-adb] 组件 adb@$version -> $destDir"

    # ---------- VerifyOnly: 只校验已有 vendor ----------
    if ($VerifyOnly) {
        if (-not (Test-Path -LiteralPath $destDir)) {
            Exit-Fail "vendor 目录不存在: $destDir（先以默认模式运行 fetch-adb.ps1）"
        }
        Write-Host "[fetch-adb] 校验 vendor 与锁 files[] 一致性..."
        $filesOk = Test-LockFilesInDir -Files $c.files -Dir $destDir
        if (-not $filesOk) { Exit-Fail "vendor 文件与锁文件不一致" }

        $adbExe = Join-Path $destDir 'adb.exe'
        $probe = Invoke-AdbVersion -AdbExe $adbExe
        if ($LASTEXITCODE -ne 0) { Exit-Fail "adb.exe version 退出码 $LASTEXITCODE" }
        if ($probe -notmatch [regex]::Escape($version)) {
            Exit-Fail "adb version 输出未包含锁定版本 '$version': $($probe.Trim())"
        }
        Write-Host ($probe.Trim().Split("`n") | ForEach-Object { "  | $_" })
        Write-Host "[fetch-adb] PASS（VerifyOnly）" -ForegroundColor Green
        exit 0
    }

    # ---------- 默认模式: 下载 -> 校验 -> 裁包 -> 复验 -> 就位 ----------
    if (-not (Test-Path -LiteralPath $DownloadDir)) {
        New-Item -ItemType Directory -Path $DownloadDir -Force | Out-Null
    }
    $zipPath = Join-Path $DownloadDir ([string]$c['source_artifact_name'])

    $needDownload = $true
    if (Test-Path -LiteralPath $zipPath) {
        $cachedSha = Get-Sha256 -Path $zipPath
        if ($cachedSha -eq $expectedZipSha) {
            Write-Host "[fetch-adb] 缓存命中且 sha256 一致，跳过下载: $zipPath"
            $needDownload = $false
        } else {
            Write-Host "[fetch-adb] 缓存 sha256 与锁不一致（$($cachedSha.Substring(0,16))...），重新下载" -ForegroundColor Yellow
        }
    }
    if ($needDownload) {
        Write-Host "[fetch-adb] 下载: $url"
        Invoke-VerifiedDownload -Url $url -DestFile $zipPath -ExpectedSha256 $expectedZipSha -ExpectedSize $expectedZipSize -Proxy $Proxy
    }

    # 安全解包: 只取 platform-tools/ 前缀下的三个目标条目（拒绝路径穿越）
    Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null
    $wanted = @('adb.exe', 'AdbWinApi.dll', 'AdbWinUsbApi.dll')
    $staging = Join-Path $VendorRoot "adb\$version.staging"
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    New-Item -ItemType Directory -Path $staging -Force | Out-Null

    try {
        $zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
        if ($null -eq $zip) { throw "zip 打开失败: $zipPath" }
        try {
            foreach ($name in $wanted) {
                if ($name -match '[\\/]|\.\.') { throw "非法目标名: $name" }
                $entryPrefix = "platform-tools/$name"
                $entry = $zip.Entries | Where-Object { $_.FullName -ieq $entryPrefix } | Select-Object -First 1
                if ($null -eq $entry) {
                    throw "zip 中缺少条目 $entryPrefix（官方包结构可能已变化，请重新审查）"
                }
                $outPath = Join-Path $staging $name
                [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $outPath, $true)
            }
            # 版本溯源门禁: source.properties 的 Pkg.Revision 必须与锁版本一致
            $propEntry = $zip.Entries | Where-Object { $_.FullName -ieq 'platform-tools/source.properties' } | Select-Object -First 1
            if ($null -ne $propEntry) {
                $reader = New-Object System.IO.StreamReader($propEntry.Open())
                $props = $reader.ReadToEnd()
                $reader.Dispose()
                $revMatch = [regex]::Match($props, 'Pkg\.Revision\s*=\s*([^\r\n]+)')
                if ($revMatch.Success -and $revMatch.Groups[1].Value.Trim() -ne $version) {
                    throw "zip Pkg.Revision='$($revMatch.Groups[1].Value.Trim())' 与锁定版本 '$version' 不符（官方 latest 可能已滚动，请按 ARC-005 重新锁定）"
                }
            }
        } finally {
            $zip.Dispose()
        }

        # 逐文件 sha256 与锁 files[] 比对，生成 SHA256SUMS.txt 清单
        Write-Host "[fetch-adb] 逐文件校验（对照锁 files[]）..."
        $filesOk = Test-LockFilesInDir -Files $c.files -Dir $staging
        if (-not $filesOk) { throw "解包产物与锁 files[] 不一致，已放弃安装" }

        $sums = Join-Path $staging 'SHA256SUMS.txt'
        $lines = foreach ($f in $c.files) {
            '{0}  {1}' -f (Get-Sha256 -Path (Join-Path $staging ([string]$f['path']))), ([string]$f['path'])
        }
        [System.IO.File]::WriteAllLines($sums, $lines)
        Write-Host "[fetch-adb] 已生成清单: SHA256SUMS.txt"

        # 原子就位: 先删旧目录再换名，失败不碰旧目录
        if (Test-Path -LiteralPath $destDir) { Remove-Item -LiteralPath $destDir -Recurse -Force }
        Move-Item -LiteralPath $staging -Destination $destDir
    } catch {
        if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
        throw
    }

    # 功能探针: 干净 vendor 环境运行 adb version（对应验收「干净 VM 运行 adb 命令」）
    $adbExe = Join-Path $destDir 'adb.exe'
    $probe = Invoke-AdbVersion -AdbExe $adbExe
    if ($LASTEXITCODE -ne 0) { Exit-Fail "adb.exe version 退出码 $LASTEXITCODE" }
    Write-Host ($probe.Trim().Split("`n") | ForEach-Object { "  | $_" })
    if ($probe -notmatch [regex]::Escape($version)) {
        Exit-Fail "adb version 输出未包含锁定版本 '$version'"
    }

    Write-Host "[fetch-adb] PASS: $destDir" -ForegroundColor Green
    exit 0
} catch {
    if (Test-Path -LiteralPath (Join-Path $VendorRoot "adb\$version.staging")) {
        Remove-Item -LiteralPath (Join-Path $VendorRoot "adb\$version.staging") -Recurse -Force -ErrorAction SilentlyContinue
    }
    Write-Host "[fetch-adb] FAIL: $($_.Exception.Message)" -ForegroundColor Red
    Write-Host "  若为网络问题，可加 -Proxy http://<代理地址> 或设置 HTTPS_PROXY 后重试" -ForegroundColor Yellow
    exit 1
}
