# 把宿主 adb server 切换到「监听所有接口」模式（-a -P 5037）
#
# 用途：Docker 并存部署（docker-compose.local.yml，容器 8444）的容器内 adb 客户端经
#       ADB_SERVER_SOCKET=tcp:host.docker.internal:5037 复用宿主 adb server，
#       USB / 无线设备对容器天然全可见，无需在容器内单独跑 server 或配对密钥。
#       标准方式拉起的 adb server 只听 127.0.0.1，容器访问不到，必须以 -a 重新拉起。
#       完整方案与验证步骤见 docs/reference/DEVICE_ACCESS.md。
#
# 注意：adb server 被 kill（gamer.ps1 rebuild/restart 内部 Reset-AdbServer、服务端
#       Adb::reset_server 自愈）或重启机器后会回到标准模式，需重跑本脚本。
#
# 用法：powershell -ExecutionPolicy Bypass -File tools\adb-share-start.ps1
# 代价：kill-server 会让 USB 设备断连几秒，运行中的 GameBot 实例（8443/8444）会自动重连恢复。

$ErrorActionPreference = 'Stop'

$ok = $false
$proc = $null
foreach ($attempt in 1..3) {
    # 1) 停掉旧 server（标准模式只听 127.0.0.1）
    adb kill-server 2>$null | Out-Null

    # 2) 以 -a 模式拉起新 server：nodaemon 不 fork，由隐藏窗口进程托管常驻
    #    （不要对 adb 用 Start-Process -Wait：会永久挂起，见 docs/PITFALLS.md）
    $proc = Start-Process -FilePath adb -ArgumentList '-a', '-P', '5037', 'nodaemon', 'server' `
        -WindowStyle Hidden -PassThru

    # 3) 轮询 server 就绪
    $ready = $false
    foreach ($i in 1..20) {
        Start-Sleep -Milliseconds 500
        $out = adb devices 2>$null
        if ($LASTEXITCODE -eq 0 -and ($out -match 'List of devices attached')) { $ready = $true; break }
    }
    if (-not $ready) { Write-Host "attempt ${attempt}: server not ready, retrying..."; continue }

    # 4) 必须监听 0.0.0.0:5037；若被竞速拉起的标准 server（127.0.0.1）抢占则重试
    $listen = netstat -ano | Select-String 'TCP\s+0\.0\.0\.0:5037\s+\S+\s+LISTENING\s+(\d+)'
    if ($listen -and $listen.Matches[0].Groups[1].Value -eq [string]$proc.Id) {
        $ok = $true
        break
    }
    Write-Host "attempt ${attempt}: 5037 not listening on 0.0.0.0 with pid $($proc.Id), retrying..."
}

if (-not $ok) { throw 'adb server failed to listen on 0.0.0.0:5037 after 3 attempts' }

Write-Host "adb server listening on 0.0.0.0:5037 (pid=$($proc.Id))"
adb devices
