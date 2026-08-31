<#
.SYNOPSIS
    DEP-004: scrcpy 协议版本与 jar 三方强绑定门禁。

.DESCRIPTION
    校验以下一致性，任一硬门禁失败即非零退出（发布阻断）:
      1) server/src/device/scrcpy.rs 的 SCRCPY_VERSION 常量 = -ExpectedVersion（默认 3.3.3）
      2) server/assets/scrcpy-server.jar 实算 sha256/size = release/dependencies.lock.toml 条目
      3) lock 条目 version = 代码常量
      4) manifest fixture（release/contracts/fixtures/manifest/valid/manifest-valid-full.json）
         的 resources.scrcpy_server.version = 代码常量（硬门禁）；
         其 sha256 若与 jar/lock 一致则全量通过，若不一致视为占位样例值
         （fixture 的 hash/URL 本为示例数据），降级为警告、只强校验 jar↔lock，
         输出中明确提示发布打包时必须以真实 hash 生成 manifest。

    兼容 Windows PowerShell 5.1 与 pwsh。

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-scrcpy-binding.ps1
    powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-scrcpy-binding.ps1 -ExpectedVersion 3.4.0
#>
[CmdletBinding()]
param(
    # 期望的协议版本（当前基线 3.3.3；未来协议升级时显式传入新值）
    [string]$ExpectedVersion = '3.3.3',
    [string]$LockPath = '',
    [string]$RsPath = '',
    [string]$JarPath = '',
    [string]$ManifestPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $LockPath)    { $LockPath    = Join-Path $repoRoot 'release\dependencies.lock.toml' }
if (-not $RsPath)      { $RsPath      = Join-Path $repoRoot 'server\src\device\scrcpy.rs' }
if (-not $JarPath)     { $JarPath     = Join-Path $repoRoot 'server\assets\scrcpy-server.jar' }
if (-not $ManifestPath) { $ManifestPath = Join-Path $repoRoot 'release\contracts\fixtures\manifest\valid\manifest-valid-full.json' }

Import-Module (Join-Path $repoRoot 'release\packaging\LockFile.psm1') -Force

$failed = $false
function Assert-Check {
    param([string]$Name, [bool]$Condition, [string]$Detail, [switch]$WarnOnly)
    if ($Condition) {
        Write-Host "  [OK] $Name$Detail"
    } elseif ($WarnOnly) {
        Write-Host "  [警告] $Name$Detail" -ForegroundColor Yellow
    } else {
        Write-Host "  [失败] $Name$Detail" -ForegroundColor Red
        $script:failed = $true
    }
}

try {
    Write-Host "[scrcpy-binding] 期望协议版本: $ExpectedVersion"

    # ① 源码协议常量
    if (-not (Test-Path -LiteralPath $RsPath)) { throw "源码不存在: $RsPath" }
    $rsText = [System.IO.File]::ReadAllText($RsPath)
    $m = [regex]::Match($rsText, 'SCRCPY_VERSION\s*:\s*&str\s*=\s*"([^"]+)"')
    if (-not $m.Success) { throw "未能从 scrcpy.rs 提取 SCRCPY_VERSION 常量（正则失配，可能常量已改名）" }
    $codeVersion = $m.Groups[1].Value
    Assert-Check "代码常量 = $ExpectedVersion" ($codeVersion -eq $ExpectedVersion) "（scrcpy.rs SCRCPY_VERSION = '$codeVersion'）"

    # ② lock 条目
    $components = Import-LockComponents -Path $LockPath
    $c = Get-LockComponent -Components $components -Id 'scrcpy-server'
    $lockVersion = [string]$c['version']
    $lockSha = ([string]$c['source_sha256']).ToLowerInvariant()
    $lockSize = [long]$c['source_size']
    Assert-Check "lock version = 代码常量" ($lockVersion -eq $codeVersion) "（lock = '$lockVersion'）"

    # ③ jar 实算
    if (-not (Test-Path -LiteralPath $JarPath)) { throw "jar 不存在: $JarPath" }
    $jarSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $JarPath).Hash.ToLowerInvariant()
    $jarSize = (Get-Item -LiteralPath $JarPath).Length
    Assert-Check "jar sha256 = lock" ($jarSha -eq $lockSha) "（jar = $jarSha, lock = $lockSha）" 
    Assert-Check "jar size = lock" ($jarSize -eq $lockSize) "（jar = $jarSize, lock = $lockSize）"

    # ④ manifest fixture（resources.scrcpy_server）
    if (Test-Path -LiteralPath $ManifestPath) {
        $manifest = [System.IO.File]::ReadAllText($ManifestPath) | ConvertFrom-Json
        $res = $manifest.platforms.'windows-x86_64'.resources.scrcpy_server
        if ($null -eq $res) {
            Assert-Check "manifest resources.scrcpy_server 存在" $false "（fixture 缺少该节点）"
        } else {
            Assert-Check "manifest version = 代码常量" ([string]$res.version -eq $codeVersion) "（manifest = '$($res.version)'）"
            $manifestSha = [string]$res.sha256
            if ($manifestSha.ToLowerInvariant() -eq $jarSha) {
                Assert-Check "manifest sha256 = jar/lock" $true "（$jarSha）"
            } else {
                Assert-Check "manifest sha256 = jar/lock" $false "（manifest fixture 为 '$manifestSha'，与 jar/lock 不一致——判定为契约样例占位值，本门禁仅强校验 jar<->lock；正式发布 manifest 必须以真实 jar hash 生成）" -WarnOnly
            }
        }
    } else {
        Assert-Check "manifest fixture 存在" $false "（未找到: $ManifestPath）"
    }

    if ($failed) {
        Write-Host "[scrcpy-binding] FAIL: 三方绑定不一致，阻断发布" -ForegroundColor Red
        exit 1
    }
    Write-Host "[scrcpy-binding] PASS: 常量=$codeVersion, jar sha256=$($jarSha.Substring(0,16))..., jar=$jarSize 字节, lock/manifest 校验通过" -ForegroundColor Green
    exit 0
} catch {
    Write-Host "[scrcpy-binding] FAIL: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
