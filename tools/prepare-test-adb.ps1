# USB / explicit-address acceptance: disable wireless auto-discovery before
# starting ADB. Never silently restart a shared daemon with active devices.
[CmdletBinding()]
param([Parameter(Mandatory)][string]$AdbPath)
$ErrorActionPreference = 'Stop'
$adb = (Resolve-Path -LiteralPath $AdbPath).Path
$previousMdns = $env:ADB_MDNS
$env:ADB_MDNS = '0'
try {
    & $adb start-server
    if ($LASTEXITCODE -ne 0) { throw '测试 ADB 启动失败' }
    $discovery = (& $adb mdns check 2>&1 | Out-String).Trim()
    if ($discovery -notmatch 'mdns discovery disabled') {
        throw '已有 ADB 服务仍开启无线自动发现；先安全停止该服务再重试。测试不会自动中断其他设备会话。'
    }
    Write-Host 'PASS: test ADB mDNS disabled (USB and explicit IP connections remain available)'
} finally {
    $env:ADB_MDNS = $previousMdns
}
