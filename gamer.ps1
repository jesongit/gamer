<#
.SYNOPSIS
  GameBot (gamer) 项目启动管理脚本：同时管理后端 gamer-server 与前端 Vite。

.DESCRIPTION
  start / stop / restart / rebuild / status 默认同时作用于前后端：
  - 后端：Rust gamer-server（端口从 server/config.toml 读取，默认 8443）
  - 前端：Vite dev server（端口 5173，代理 /api、/ws 到后端）
  停止时均通过「端口 + 进程名」定位进程，避免误杀其他程序。
  rebuild：停止前后端 → 重新编译（后端 cargo build，前端 vite build 产物输出到
  server/web-dist 由后端静态托管）→ 再启动（-Release 可指定 release 构建）。

.EXAMPLE
  .\gamer.ps1 start              # 启动后端 + 前端（依赖缺失时自动安装/构建）
  .\gamer.ps1 start -Build       # 后端强制重新构建后启动
  .\gamer.ps1 start -BackendOnly # 只启动后端
  .\gamer.ps1 start -FrontendOnly# 只启动前端
  .\gamer.ps1 stop               # 停止后端 + 前端（端口+进程名定位）
  .\gamer.ps1 restart            # 重启前后端
  .\gamer.ps1 rebuild            # 重新编译前后端（cargo build + vite build）并重启
  .\gamer.ps1 status             # 查看前后端运行状态、最近日志
  .\gamer.ps1 test_adb           # USB/adb 链路体检（选择性暂停 / 空闲稳定性 / push 突发流量）
  .\gamer.ps1 test_adb -IdleSeconds 60   # 空闲观察延长到 60 秒
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('start', 'stop', 'restart', 'rebuild', 'status', 'test_adb', 'help')]
    [string]$Command = 'status',

    # 后端端口，0 = 自动从 server/config.toml 读取（默认 8443）
    [int]$Port = 0,

    # 只操作后端（默认前后端一起）
    [switch]$BackendOnly,

    # 只操作前端（默认前后端一起）
    [switch]$FrontendOnly,

    # 启动后端前强制重新构建
    [switch]$Build,

    # 后端使用 release 构建（默认 debug）
    [switch]$Release,

    # test_adb：空闲稳定性测试时长（秒），建议 ≥ 30（历史病例空闲 15~25s 掉线）
    [int]$IdleSeconds = 30
)

$ErrorActionPreference = 'Stop'
# PS 7.3+ 默认会把原生命令（cargo/npm）的 stderr 输出当成错误记录，
# 在 EAP=Stop 下直接抛 NativeCommandError 中断脚本（cargo 编译进度就是走 stderr 的）。
# 显式关闭，让原生命令的 stderr 只作普通输出显示。
$PSNativeCommandUseErrorActionPreference = $false

$Root         = $PSScriptRoot
$ServerDir    = Join-Path $Root 'server'
$WebDir       = Join-Path $Root 'web'
$ConfigFile   = Join-Path $ServerDir 'config.toml'

# 后端
$BackendName   = 'gamer-server'                             # 进程名匹配模式
$BackendLog    = Join-Path $ServerDir 'gamer-server.log'    # 主日志（追加，GB_LOG 文件模式）
$BackendOutLog = Join-Path $ServerDir 'gamer-server.out.log' # stdout 重定向（GB_LOG 模式下通常为空）
$BackendErrLog = Join-Path $ServerDir 'gamer-server.err.log'

# 子进程 stdin 统一重定向到空文件：防止 server / vite 继承控制台键盘输入，
# 也避免任何继承的 stdout/stderr 句柄让父控制台/管道永远等不到 EOF（表现为控制台卡死）
$NullInputFile = Join-Path $env:TEMP 'gamer-stdin-empty.txt'
if (-not (Test-Path $NullInputFile)) { New-Item -ItemType File -Path $NullInputFile -Force | Out-Null }

# 前端（端口与 vite.config.js 保持一致）
$FrontendPort  = 5173
$FrontendLog   = Join-Path $WebDir 'vite.log'
$FrontendErrLog = Join-Path $WebDir 'vite.err.log'
$WebDistDir    = Join-Path $ServerDir 'web-dist'  # 前端构建产物（vite build 输出），由后端静态托管

# ---------- 基础工具 ----------

function Get-ServerPort {
    if ($Port -gt 0) { return $Port }
    if (Test-Path $ConfigFile) {
        $m = Select-String -Path $ConfigFile -Pattern '^\s*port\s*=\s*(\d+)' | Select-Object -First 1
        if ($m) { return [int]$m.Matches[0].Groups[1].Value }
    }
    return 8443
}

function Get-BinaryPath {
    $profile = if ($Release) { 'release' } else { 'debug' }
    return Join-Path $ServerDir "target\$profile\$BackendName.exe"
}

function Test-PortListening([int]$p) {
    return [bool](Get-NetTCPConnection -LocalPort $p -State Listen -ErrorAction SilentlyContinue)
}

function Get-PortOwner([int]$p) {
    Get-NetTCPConnection -LocalPort $p -State Listen -ErrorAction SilentlyContinue |
        ForEach-Object {
            $o = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
            "    PID $($_.OwningProcess) $($o.ProcessName)"
        } | Select-Object -Unique
}

# ---------- 后台进程启动（句柄隔离） ----------

<#
  Start-BackgroundProcess：以「句柄隔离」方式启动后台服务进程（gamer-server / vite）。

  背景：直接在本进程里 Start-Process 时，子进程会继承父进程句柄表中所有「可继承」句柄。
  当脚本输出被管道 / 终端捕获时（cmd 管道、GUI 终端、任务系统等），管道写端句柄会被
  gamer-server / node 继承且永不关闭 → 读端永远等不到 EOF → 控制台看起来卡死。

  方案：通过 WMI（Win32_Process.Create）启动一个隐藏的 powershell 包装进程，再由它
  Start-Process 真正的服务。WMI 创建进程时由 wmiprvse 服务代为创建，进程句柄表干净，
  没有任何管道句柄可继承；包装进程退出后，服务进程与父控制台完全脱离。
#>
function Start-BackgroundProcess {
    param(
        [Parameter(Mandatory)][string]$FilePath,      # 服务可执行文件
        [string]$ArgumentList,                        # 附加参数（可空）
        [Parameter(Mandatory)][string]$WorkingDirectory,
        [Parameter(Mandatory)][string]$OutputFile,
        [Parameter(Mandatory)][string]$ErrorFile,
        [Parameter(Mandatory)][string]$InputFile,
        [string]$EnvBlock = ''                        # 形如 "GB_LOG=D:\...\x.log" 的环境变量（可选）
    )
    $inner = "Start-Process -FilePath '{0}' -WorkingDirectory '{1}' -WindowStyle Hidden -RedirectStandardOutput '{2}' -RedirectStandardError '{3}' -RedirectStandardInput '{4}' -PassThru | Out-Null" -f `
        $FilePath, $WorkingDirectory, $OutputFile, $ErrorFile, $InputFile
    if ($ArgumentList) { $inner = "Start-Process -FilePath '{0}' -ArgumentList '{1}' -WorkingDirectory '{2}' -WindowStyle Hidden -RedirectStandardOutput '{3}' -RedirectStandardError '{4}' -RedirectStandardInput '{5}' -PassThru | Out-Null" -f `
        $FilePath, $ArgumentList, $WorkingDirectory, $OutputFile, $ErrorFile, $InputFile }
    $cmd = if ($EnvBlock) { "$EnvBlock; $inner" } else { $inner }
    $wmiCmd = 'powershell -NoProfile -WindowStyle Hidden -Command "' + $cmd + '"'
    # 必须通过 Win32_ProcessStartup 显式传 ShowWindow=0（SW_HIDE）：
    # WMI 创建的控制台进程默认 SW_SHOWNORMAL，黑框会在包装进程处理
    # "-WindowStyle Hidden" 之前先弹出来（前后端各一个，即启动时闪两个黑框的来源）。
    # 从创建时刻就隐藏，窗口完全不出现。
    # 注意 ShowWindow 必须是 UInt16：直接写 0 会被推断成 Int32，WMI 报「类型不匹配」
    $startup = New-CimInstance -ClassName Win32_ProcessStartup -Property @{ ShowWindow = [UInt16]0 } -ClientOnly
    $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
        CommandLine               = $wmiCmd
        ProcessStartupInformation = $startup
    }
    if ($r.ReturnValue -ne 0) {
        throw "后台进程启动失败（WMI returnValue=$($r.ReturnValue)），命令: $wmiCmd"
    }
    return $r.ProcessId  # 包装进程 PID（退出很快，仅用于诊断）
}

# 通过「端口 + 进程名」定位后端 gamer-server 进程
function Get-BackendProcs {
    # 1) 先查监听端口的进程，再按名字过滤（端口优先，精确锁定）
    $portPids = @(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty OwningProcess -Unique)
    $found = @()
    foreach ($procId in $portPids) {
        $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
        if ($p -and $p.ProcessName -like "$BackendName*") { $found += $p }
    }
    # 2) 再按进程名补充（覆盖端口已释放但仍存活的残留进程）
    foreach ($p in @(Get-Process -Name "$BackendName*" -ErrorAction SilentlyContinue)) {
        if ($found.Id -notcontains $p.Id) { $found += $p }
    }
    return @($found | Sort-Object Id -Unique)
}

# 通过「端口 + 名字/命令行」定位前端 vite 进程（node 名太通用，须端口优先）
function Get-FrontendProcs {
    $found = @()
    # 1) 端口 5173 优先：监听进程必须为 node
    $portPids = @(Get-NetTCPConnection -LocalPort $FrontendPort -State Listen -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty OwningProcess -Unique)
    foreach ($procId in $portPids) {
        $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
        if ($p -and $p.ProcessName -eq 'node') { $found += $p }
    }
    # 2) 兜底：命令行含 vite / npm run dev 的残留进程（node 与 npm/cmd 包装）
    $cims = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -match 'vite' -or $_.CommandLine -match 'npm run dev' }
    foreach ($c in $cims) {
        $p = Get-Process -Id $c.ProcessId -ErrorAction SilentlyContinue
        if ($p -and $found.Id -notcontains $p.Id) { $found += $p }
    }
    return @($found | Sort-Object Id -Unique)
}

# ---------- 状态 ----------

function Show-Status {
    $beProcs = @(Get-BackendProcs)
    $feProcs = @(Get-FrontendProcs)

    # --- 后端 ---
    Write-Host "【后端】gamer-server（端口 $Port）"
    if ($beProcs.Count -eq 0) {
        if (Test-PortListening $Port) {
            Write-Host "  未运行，但端口 $Port 被其他进程占用：" -ForegroundColor Red
            Get-PortOwner $Port | ForEach-Object { Write-Host $_ }
        } else {
            Write-Host "  未运行" -ForegroundColor Yellow
        }
    } else {
        Write-Host "  运行中  http://localhost:$Port" -ForegroundColor Green
        foreach ($p in $beProcs) {
            Write-Host ("  进程: {0} (PID {1})" -f $p.ProcessName, $p.Id)
            if ($p.StartTime) {
                $up = (Get-Date) - $p.StartTime
                Write-Host ("  启动于: {0:yyyy-MM-dd HH:mm:ss}，已运行 {1:dd\.hh\:mm\:ss}" -f $p.StartTime, $up)
            }
        }
    }
    if (Test-Path $BackendLog) {
        Write-Host "  最近日志:"
        Get-Content $BackendLog -Tail 3 | ForEach-Object { Write-Host "    $_" }
    }

    # --- 前端 ---
    Write-Host ""
    Write-Host "【前端】Vite dev（端口 $FrontendPort）"
    if ($feProcs.Count -eq 0) {
        if (Test-PortListening $FrontendPort) {
            Write-Host "  未运行，但端口 $FrontendPort 被其他进程占用：" -ForegroundColor Red
            Get-PortOwner $FrontendPort | ForEach-Object { Write-Host $_ }
        } else {
            Write-Host "  未运行" -ForegroundColor Yellow
        }
    } else {
        Write-Host "  运行中  http://localhost:$FrontendPort" -ForegroundColor Green
        foreach ($p in $feProcs) {
            Write-Host ("  进程: {0} (PID {1})" -f $p.ProcessName, $p.Id)
            if ($p.StartTime) {
                $up = (Get-Date) - $p.StartTime
                Write-Host ("  启动于: {0:yyyy-MM-dd HH:mm:ss}，已运行 {1:dd\.hh\:mm\:ss}" -f $p.StartTime, $up)
            }
        }
    }
    if (Test-Path $FrontendLog) {
        Write-Host "  最近日志:"
        Get-Content $FrontendLog -Tail 3 | ForEach-Object { Write-Host "    $_" }
    }
}

# ---------- 停止 ----------

function Stop-Backend {
    $procs = @(Get-BackendProcs)
    if ($procs.Count -eq 0) {
        Write-Host "后端: 没有运行中的 gamer-server 进程（端口 $Port 无监听且无匹配进程名）" -ForegroundColor Yellow
        return $false
    }
    foreach ($p in $procs) {
        Write-Host ("后端: 停止进程 {0} (PID {1}) ..." -f $p.ProcessName, $p.Id)
        Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
    }
    # 等待优雅退出（最多 10 秒），仍未退出则强制结束
    $deadline = (Get-Date).AddSeconds(10)
    while (@(Get-BackendProcs).Count -gt 0 -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
    foreach ($p in @(Get-BackendProcs)) {
        Write-Host ("后端: 进程未退出，强制结束 {0} (PID {1})" -f $p.ProcessName, $p.Id) -ForegroundColor Yellow
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 500
    if (Test-PortListening $Port) {
        Write-Host "后端: 端口 $Port 仍被占用（可能为其他程序）：" -ForegroundColor Red
        Get-PortOwner $Port | ForEach-Object { Write-Host $_ }
    } else {
        Write-Host "后端: 已停止。" -ForegroundColor Green
    }
    return $true
}

function Stop-Frontend {
    $procs = @(Get-FrontendProcs)
    if ($procs.Count -eq 0) {
        Write-Host "前端: 没有运行中的 vite 进程（端口 $FrontendPort 无监听且无匹配进程）" -ForegroundColor Yellow
        return $false
    }
    foreach ($p in $procs) {
        Write-Host ("前端: 停止进程 {0} (PID {1}) ..." -f $p.ProcessName, $p.Id)
        Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
    }
    $deadline = (Get-Date).AddSeconds(10)
    while (@(Get-FrontendProcs).Count -gt 0 -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
    foreach ($p in @(Get-FrontendProcs)) {
        Write-Host ("前端: 进程未退出，强制结束 {0} (PID {1})" -f $p.ProcessName, $p.Id) -ForegroundColor Yellow
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 500
    if (Test-PortListening $FrontendPort) {
        Write-Host "前端: 端口 $FrontendPort 仍被占用（可能为其他程序）：" -ForegroundColor Red
        Get-PortOwner $FrontendPort | ForEach-Object { Write-Host $_ }
    } else {
        Write-Host "前端: 已停止。" -ForegroundColor Green
    }
    return $true
}

# ---------- 构建 ----------

<#
  构建期间临时把 $ErrorActionPreference 切回 Continue：
  cargo / npm 的编译进度输出走 stderr，在 EAP=Stop 下会被当成错误中断脚本
 （Windows PowerShell 5.1 与 PS 7.3+ 通病），成败只看 $LASTEXITCODE。
#>
function Invoke-NativeChecked {
    param([string]$Desc, [scriptblock]$Cmd)
    Write-Host $Desc
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $Cmd } finally { $ErrorActionPreference = $prev }
    if ($LASTEXITCODE -ne 0) { throw "$Desc 失败（exit code $LASTEXITCODE）" }
}

function Build-Backend {
    $exe = Get-BinaryPath
    Invoke-NativeChecked -Desc "后端: 构建 $exe ..." -Cmd {
        Push-Location $ServerDir
        try {
            if ($Release) { & cargo build --release } else { & cargo build }
        } finally {
            Pop-Location
        }
    }
}

function Ensure-FrontendDeps {
    if (Test-Path (Join-Path $WebDir 'node_modules')) { return }
    Invoke-NativeChecked -Desc "前端: node_modules 不存在，执行 npm install ..." -Cmd {
        Push-Location $WebDir
        try {
            & npm install --no-audit --no-fund
        } finally {
            Pop-Location
        }
    }
}

function Build-Frontend {
    Ensure-FrontendDeps
    Invoke-NativeChecked -Desc "前端: vite build → $WebDistDir（后端静态托管目录）..." -Cmd {
        Push-Location $WebDir
        try {
            & npm run build
        } finally {
            Pop-Location
        }
    }
}

# ---------- 启动 ----------

function Start-Backend {
    # 已在运行：直接报状态（区分是否被其他程序占端口）
    if (@(Get-BackendProcs).Count -gt 0) {
        Write-Host "后端: 已在运行，无需重复启动" -ForegroundColor Yellow
        return
    }
    if (Test-PortListening $Port) {
        Write-Host "后端: 端口 $Port 已被其他进程占用，无法启动：" -ForegroundColor Red
        Get-PortOwner $Port | ForEach-Object { Write-Host $_ }
        return
    }

    $exe = Get-BinaryPath
    if (-not (Test-Path $exe) -or $Build) { Build-Backend }
    if (-not (Test-Path $exe)) { throw "后端: 未找到服务端二进制: $exe" }

    Write-Host ("后端: 启动 {0}（端口 {1}）..." -f $exe, $Port)
    Write-Host ("后端日志: {0}（追加），stdout: {1}，stderr: {2}" -f $BackendLog, $BackendOutLog, $BackendErrLog)

    # 服务端支持 GB_LOG=<文件> 自带文件日志（追加模式），不依赖 stdout 重定向；
    # GB_LOG 在 WMI 包装进程内设置（包装进程与当前控制台无句柄关联）
    $launchTime = Get-Date
    $null = Start-BackgroundProcess -FilePath $exe `
        -WorkingDirectory $ServerDir `
        -OutputFile $BackendOutLog `
        -ErrorFile $BackendErrLog `
        -InputFile $NullInputFile `
        -EnvBlock ("`$env:GB_LOG='{0}'" -f $BackendLog)

    # 等待端口监听（最多 60 秒）
    $deadline = (Get-Date).AddSeconds(60)
    while (-not (Test-PortListening $Port) -and (Get-Date) -lt $deadline) {
        # 启动后 5 秒仍无 gamer-server 进程 → 判定为启动后立即退出
        if ((Get-Date) - $launchTime -gt [TimeSpan]::FromSeconds(5)) {
            $be = @(Get-Process -Name "$BackendName*" -ErrorAction SilentlyContinue |
                Where-Object { $_.StartTime -ge $launchTime })
            if ($be.Count -eq 0) {
                throw "后端: 进程启动后立即退出，请查看日志: $BackendLog / $BackendErrLog"
            }
        }
        Start-Sleep -Seconds 1
    }
    if (-not (Test-PortListening $Port)) {
        throw "后端: 等待超时，端口 $Port 未监听，请查看日志: $BackendLog / $BackendErrLog"
    }
    Write-Host "后端: 启动成功  http://localhost:$Port" -ForegroundColor Green
}

function Start-Frontend {
    if (@(Get-FrontendProcs).Count -gt 0) {
        Write-Host "前端: 已在运行，无需重复启动" -ForegroundColor Yellow
        return
    }
    if (Test-PortListening $FrontendPort) {
        Write-Host "前端: 端口 $FrontendPort 已被其他进程占用，无法启动：" -ForegroundColor Red
        Get-PortOwner $FrontendPort | ForEach-Object { Write-Host $_ }
        return
    }

    # 依赖缺失时自动安装
    Ensure-FrontendDeps

    Write-Host "前端: 启动 vite dev（端口 $FrontendPort）..."
    Write-Host ("前端日志: {0}，stderr: {1}" -f $FrontendLog, $FrontendErrLog)

    # 直接启动 node 运行 vite（等价于 npm run dev，但少一层 cmd 包装进程，
    # 进程树更干净、stop 时不需要额外杀 cmd）
    $nodeExe = (Get-Command node.exe -ErrorAction Stop).Source
    $viteJs  = Join-Path $WebDir (Join-Path 'node_modules' (Join-Path 'vite' (Join-Path 'bin' 'vite.js')))
    if (-not (Test-Path $viteJs)) { throw "前端: 未找到 vite 入口: $viteJs" }

    $launchTime = Get-Date
    $null = Start-BackgroundProcess -FilePath $nodeExe -ArgumentList $viteJs `
        -WorkingDirectory $WebDir `
        -OutputFile $FrontendLog `
        -ErrorFile $FrontendErrLog `
        -InputFile $NullInputFile

    $deadline = (Get-Date).AddSeconds(60)
    while (-not (Test-PortListening $FrontendPort) -and (Get-Date) -lt $deadline) {
        # 启动后 5 秒仍无 node 进程 → 判定为启动后立即退出
        if ((Get-Date) - $launchTime -gt [TimeSpan]::FromSeconds(5)) {
            $fe = @(Get-Process -Name 'node' -ErrorAction SilentlyContinue |
                Where-Object { $_.StartTime -ge $launchTime })
            if ($fe.Count -eq 0) {
                throw "前端: vite 启动后立即退出，请查看日志: $FrontendLog / $FrontendErrLog"
            }
        }
        Start-Sleep -Seconds 1
    }
    if (-not (Test-PortListening $FrontendPort)) {
        throw "前端: 启动失败（60 秒内端口未监听），请查看日志: $FrontendLog / $FrontendErrLog"
    }
    Write-Host "前端: 启动成功  http://localhost:$FrontendPort" -ForegroundColor Green
}

# ---------- ADB / USB 链路诊断（test_adb） ----------

<#
  USB/adb 链路体检，覆盖两类历史真凶（完整排障记录见 AGENTS.md 已知坑）：
  ① Windows「USB 选择性暂停」：USB 空闲 15~25s 后被系统挂起 → adb 掉线
     （设备从 adb devices 消失或转 offline）
  ② USB 口/线接触不良：大流量传输（push/pull）瞬间断开（push 报
     "failed to read copy response: EOF"），手机 adbd 常楔死 offline 只能拔插

  判定逻辑：空闲期掉线 → 指向①；传输中掉线 → 指向②。
#>
function Test-AdbUsb {
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'   # adb/powercfg 的 stderr 不当作异常，成败看 exit code/输出

    $idleFail = $false
    $burstFail = $false

    # ---- 1/5 adb 可用性 ----
    Write-Host ""
    Write-Host "【1/5】adb 可用性" -ForegroundColor Cyan
    $adbVer = (& adb version 2>$null | Select-Object -First 1)
    if ($LASTEXITCODE -ne 0 -or -not $adbVer) {
        Write-Host "  ✗ adb 不可用（不在 PATH），无法测试" -ForegroundColor Red
        $ErrorActionPreference = $prevEap
        return
    }
    Write-Host "  ✓ $adbVer"

    # ---- 2/5 Windows USB 选择性暂停 ----
    Write-Host ""
    Write-Host "【2/5】Windows USB 选择性暂停（空闲掉线真凶①）" -ForegroundColor Cyan
    $usbSub = '2a737441-1930-4402-8d77-b2bebba308a3'
    $usbSet = '48e6b7a6-50f5-4782-a5d4-53bb8f07e226'
    $suspendOn = $false
    try {
        $pq = (powercfg /q SCHEME_CURRENT $usbSub $usbSet 2>$null) -join "`n"
        $acOn = $pq -match '交流电源设置索引:\s*0x0*1\b'
        $dcOn = $pq -match '直流电源设置索引:\s*0x0*1\b'
        $suspendOn = $acOn -or $dcOn
    } catch { $suspendOn = $false }
    if (-not $suspendOn) {
        Write-Host "  ✓ 已禁用" -ForegroundColor Green
    } else {
        Write-Host "  ✗ 已启用（AC=$(if($acOn){'开'}else{'关'}) DC=$(if($dcOn){'开'}else{'关'})）—— 空闲 15~25 秒 Windows 会挂起 USB 设备导致 adb 掉线" -ForegroundColor Red
        $ans = Read-Host "  现在禁用它？（修改电源计划，需要管理员权限）[Y/n]"
        if ($ans -eq '' -or $ans -match '^[yY]') {
            powercfg /SETACVALUEINDEX SCHEME_CURRENT $usbSub $usbSet 0 2>$null | Out-Null
            powercfg /SETDCVALUEINDEX SCHEME_CURRENT $usbSub $usbSet 0 2>$null | Out-Null
            powercfg /SETACTIVE SCHEME_CURRENT 2>$null | Out-Null
            $pq2 = (powercfg /q SCHEME_CURRENT $usbSub $usbSet 2>$null) -join "`n"
            if ($pq2 -match '设置索引:\s*0x0*1\b') {
                Write-Host "  ✗ 自动修复失败（大概率权限不足）。请用管理员 PowerShell 执行：" -ForegroundColor Red
                Write-Host "    powercfg /SETACVALUEINDEX SCHEME_CURRENT $usbSub $usbSet 0"
                Write-Host "    powercfg /SETDCVALUEINDEX SCHEME_CURRENT $usbSub $usbSet 0"
                Write-Host "    powercfg /SETACTIVE SCHEME_CURRENT"
            } else {
                Write-Host "  ✓ 已禁用" -ForegroundColor Green
            }
        } else {
            Write-Host "  ! 跳过修复（保持启用），空闲稳定性测试大概率会失败" -ForegroundColor Yellow
        }
    }

    # ---- 3/5 设备检测 ----
    Write-Host ""
    Write-Host "【3/5】设备检测" -ForegroundColor Cyan
    $devLines = @(& adb devices -l 2>$null | Select-Object -Skip 1 | Where-Object { $_.Trim() })
    if ($devLines.Count -eq 0) {
        Write-Host "  ✗ 无设备。插好 USB（必要时换口/换线）后重试；手机熄屏不影响本测试" -ForegroundColor Red
        $ErrorActionPreference = $prevEap
        return
    }
    $targets = @()
    foreach ($l in $devLines) {
        $parts = $l -split '\s+'
        $serial = $parts[0]; $state = $parts[1]
        if ($state -eq 'device') {
            $targets += $serial
            Write-Host "  ✓ $serial" -ForegroundColor Green
        } elseif ($state -eq 'offline') {
            Write-Host "  ✗ $serial offline —— adbd 楔死，adb reconnect 通常救不回，只能拔插 USB" -ForegroundColor Red
        } else {
            Write-Host "  ! $serial（$state）—— 手机上确认 USB 调试授权弹窗" -ForegroundColor Yellow
        }
    }
    if ($targets.Count -eq 0) {
        Write-Host "  ✗ 无可用（device 状态）设备，测试终止" -ForegroundColor Red
        $ErrorActionPreference = $prevEap
        return
    }

    # ---- 4/5 突发流量（push/pull 分级） ----
    Write-Host ""
    Write-Host "【4/5】突发流量测试（口/线接触不良真凶②，逐级加量）..." -ForegroundColor Cyan
    $tmpDir = Join-Path $env:TEMP 'gamer-adbtest'
    New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null
    $f1k   = Join-Path $tmpDir 't1k.bin';   [IO.File]::WriteAllBytes($f1k, (New-Object byte[] 1024))
    $f100k = Join-Path $tmpDir 't100k.bin'; [IO.File]::WriteAllBytes($f100k, (New-Object byte[] 102400))
    foreach ($s in $targets) {
        if ($burstFail) { break }
        Write-Host "  设备 $s："
        $steps = @(
            @{ n = 'push 1KB x2';   cmd = { 1..2 | ForEach-Object { adb -s $s push $f1k /data/local/tmp/gamer-t1k.bin } } },
            @{ n = 'push 100KB x2'; cmd = { 1..2 | ForEach-Object { adb -s $s push $f100k /data/local/tmp/gamer-t100k.bin } } },
            @{ n = 'pull 100KB';    cmd = { adb -s $s pull /data/local/tmp/gamer-t100k.bin (Join-Path $tmpDir 'back.bin') } }
        )
        foreach ($st in $steps) {
            $null = (& $st.cmd) 2>&1
            $code = $LASTEXITCODE
            $map = @{}
            foreach ($l in @(& adb devices 2>$null | Select-Object -Skip 1 | Where-Object { $_.Trim() })) {
                $p = $l -split '\s+'; $map[$p[0]] = $p[1]
            }
            if ($code -ne 0 -or $map[$s] -ne 'device') {
                Write-Host ("    ✗ {0} 失败（exit={1}, 状态={2}）—— 传输中掉线，指向「口/线接触不良」：换口/换线后重测" -f $st.n, $code, $map[$s]) -ForegroundColor Red
                $burstFail = $true
                break
            }
            Write-Host ("    ✓ {0}" -f $st.n) -ForegroundColor Green
        }
        # 清理设备端测试文件（连接已死则跳过）
        if ($map[$s] -eq 'device') { $null = (& adb -s $s shell rm -f /data/local/tmp/gamer-t1k.bin /data/local/tmp/gamer-t100k.bin) 2>&1 }
    }
    Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue

    # ---- 5/5 空闲稳定性 ----
    if ($burstFail) {
        Write-Host ""
        Write-Host "【5/5】空闲稳定性测试跳过（突发流量已打死连接，先解决硬件问题）" -ForegroundColor Yellow
    } else {
        Write-Host ""
        Write-Host ("【5/5】空闲稳定性测试（{0}s，每 5s 检查一次；历史病例：空闲 15~25s 掉线）..." -f $IdleSeconds) -ForegroundColor Cyan
        $deadline = (Get-Date).AddSeconds($IdleSeconds)
        while ((Get-Date) -lt $deadline -and -not $idleFail) {
            Start-Sleep -Seconds 5
            $map = @{}
            foreach ($l in @(& adb devices 2>$null | Select-Object -Skip 1 | Where-Object { $_.Trim() })) {
                $p = $l -split '\s+'; $map[$p[0]] = $p[1]
            }
            foreach ($s in $targets) {
                if ($map[$s] -ne 'device') {
                    Write-Host ("  ✗ {0} 于 {1} 掉线（状态: {2}）—— 空闲期断开，指向「USB 选择性暂停」" -f $s, (Get-Date -Format HH:mm:ss), $map[$s]) -ForegroundColor Red
                    $idleFail = $true
                    break
                }
            }
            if (-not $idleFail) { Write-Host ("  · {0} 在线" -f (Get-Date -Format HH:mm:ss)) -ForegroundColor DarkGray }
        }
        if (-not $idleFail) { Write-Host "  ✓ 空闲 $IdleSeconds 秒稳定" -ForegroundColor Green }
    }

    # ---- 结论 ----
    Write-Host ""
    Write-Host "==== 诊断结论 ====" -ForegroundColor Cyan
    if (-not $idleFail -and -not $burstFail) {
        Write-Host "  ✓ USB/adb 链路健康（空闲稳定 + 突发流量稳定），可以正常跑挂机" -ForegroundColor Green
    }
    if ($idleFail) {
        Write-Host "  → 空闲期掉线：检查第 2 步「USB 选择性暂停」是否已禁用（换电脑/重装系统后该设置会丢失需重做）" -ForegroundColor Yellow
    }
    if ($burstFail) {
        Write-Host "  → 传输中掉线：USB 口/线接触不良。换口换线后重测；若手机停在 offline，只能拔插" -ForegroundColor Yellow
    }
    $ErrorActionPreference = $prevEap
}

# ---------- 入口 ----------

$Port = Get-ServerPort
$Both = -not $BackendOnly -and -not $FrontendOnly
Write-Host ("== gamer.ps1：GameBot 前后端管理（后端端口 {0} / 前端端口 {1}）==" -f $Port, $FrontendPort)

switch ($Command) {
    'start' {
        if ($Both -or $BackendOnly) { Start-Backend }
        if ($Both -or $FrontendOnly) { Start-Frontend }
        Write-Host ""
        Show-Status
    }
    'stop' {
        if ($Both -or $FrontendOnly) { Stop-Frontend | Out-Null }
        if ($Both -or $BackendOnly) { Stop-Backend | Out-Null }
    }
    'restart' {
        if ($Both -or $FrontendOnly) { Stop-Frontend | Out-Null }
        if ($Both -or $BackendOnly) { Stop-Backend | Out-Null }
        Write-Host ""
        if ($Both -or $BackendOnly) { Start-Backend }
        if ($Both -or $FrontendOnly) { Start-Frontend }
        Write-Host ""
        Show-Status
    }
    'rebuild' {
        # Windows 锁定运行中的可执行文件，必须先停后端 cargo 才能覆盖 exe
        if ($Both -or $FrontendOnly) { Stop-Frontend | Out-Null }
        if ($Both -or $BackendOnly) { Stop-Backend | Out-Null }
        Write-Host ""
        if ($Both -or $BackendOnly) { Build-Backend }
        if ($Both -or $FrontendOnly) { Build-Frontend }
        Write-Host ""
        if ($Both -or $BackendOnly) { Start-Backend }
        if ($Both -or $FrontendOnly) { Start-Frontend }
        Write-Host ""
        Show-Status
    }
    'status' { Show-Status }
    'test_adb' { Test-AdbUsb }
    'help'   { Get-Help -Path $MyInvocation.MyCommand.Path -Detailed }
}
