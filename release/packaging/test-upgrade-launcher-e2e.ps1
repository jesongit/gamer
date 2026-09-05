# 批次 3 合流门 E2E（升级 + 回滚）：M1 基线（版本取 server/Cargo.toml，随主树演进）
# 真实升级到 M2 候选（-CandidateVersion，默认 0.2.0），以及候选启动失败时的自动回滚
# （恢复 previous 程序 + 升级前数据快照）。
#
# 场景 A（upgrade）：解压 full ZIP（含中文+空格安装根）→ repair 离线首装（死代理
#   断网等效）→ launcher start（IPC 托管）→ 登录 → POST /api/system/update/check|
#   download|install（逐个 202，经 IPC 推进 journal available→staged）→ 结束 start
#   释放安装锁（server 孤儿存活）→ `launcher upgrade --manifest` 接管完成 §6.6
#   全链路（drain→stopped→snapshot→switch→候选激活闸→activate→committed）→ 逐项验收。
# 场景 B（rollback）：同基线重建，但候选 zip 内 gamer-server.exe 为「启动即退出」的
#   故障构建（保留 maintenance inspect 子命令，快照 schema 门禁可过）→ 升级在
#   candidate 阶段失败 → 验证自动回滚：快照恢复 + current.json 切回 0.1.0 + 旧版
#   重新 ready + 数据完整（升级前写入的设备仍在、候选期间零业务写入）+ journal 失败记录。
# 场景 C（identity，LCH-008）：候选身份探针携带 X-Admin-Token 的真实进程回归——
#   manifest 声明版本（候选补丁位 +1）与候选二进制真实版本不符 → /api/system/info
#   （回环管理通道鉴权）观测到版本差异 → commit 前拒绝并自动回滚到基线。
#
# 与冻结契约的两个边界（详见 docs/evidence/UPDATE_M2_EVIDENCE.md）：
# - manifest 内 artifact.url 契约强制 https（launcher model 与 JSON Schema 双重门禁），
#   本机临时 HTTP 服务只承载 manifest 本体（引擎 fetch_remote_manifest 接受 http://
#   并按 <url>.sig 拉分离签名）；候选 app zip 经 cache/artifacts 种子命中
#   （seeds→cache→remote 链路的 cache 级；远端下载路径由 QA-002 专项测试覆盖）。
# - server 侧 install API 的 IPC 链路止于 prepare_install（复验 staging、驻留 staged）；
#   drain/快照/切换/候选/commit 的接管入口当前只有 `launcher upgrade` CLI（安装锁
#   与 start 互斥，先结束 start 再接管）。证据文档有完整原因说明。
#
# 幂等：每次运行删除并重建两个安装根；-SkipBuild 复用既有 exe/zip/manifest。
# 兼容 Windows PowerShell 5.1 与 pwsh（不使用 ArgumentList/.NET Core API）。

[CmdletBinding()]
param(
    # 仓库根（默认脚本位置上两级）
    [string]$RepoRoot = '',
    # 工作区（安装根/产物/日志都在其下）
    [string]$WorkDir = 'D:\e2e-upgrade-tmp\m2e2e',
    # all = 构建+两场景；build = 只构建打包；upgrade / rollback = 单场景（要求产物已就绪）
    # identity = LCH-008 真实进程回归（候选身份校验 + 身份不符回滚负例）
    [ValidateSet('all', 'build', 'upgrade', 'rollback', 'identity')]
    [string]$Scenario = 'all',
    # 跳过 cargo/pnpm/打包（复用工作区既有产物；两个场景仍会重建安装根）
    [switch]$SkipBuild,
    # manifest 本机 HTTP 服务端口
    [int]$HttpPort = 18630,
    # 场景 A / B 安装根的 server 端口（错峰：并行测试可能占用 8443）
    [int]$PortA = 18443,
    [int]$PortB = 18444,
    # 场景 C（identity）安装根的 server 端口
    [int]$PortC = 18445,
    # 候选版本（须高于 server/Cargo.toml 的当前版本；strict upgrade 语义）
    [string]$CandidateVersion = '0.2.0',
    # QA-005：可将安装根放到工作目录之外（例如 C:），以覆盖真实路径长度
    # 与跨盘 data 物理存储；未设置时保持原有 WorkDir/<中文+空格> 行为。
    [string]$InstallRootA = '',
    [string]$InstallRootB = '',
    [string]$DataRootA = '',
    [string]$DataRootB = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

if (-not $RepoRoot) { $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot) }

# 基线版本权威源 = server/Cargo.toml [package].version（与 package-app.ps1 同规则）；
# 身份不符负例版本 = 候选补丁位 +1（仅 manifest 声明，制品仍是真实候选二进制）。
$script:BaselineVersion = ''
foreach ($line in (Get-Content -LiteralPath (Join-Path $RepoRoot 'server\Cargo.toml'))) {
    if ($line -match '^\s*version\s*=\s*"([^"]+)"') { $script:BaselineVersion = $Matches[1].Trim(); break }
}
if (-not $script:BaselineVersion) { throw '无法从 server/Cargo.toml 读取 [package].version' }
$wrongParts = $CandidateVersion.Split('.')
$script:WrongVersion = '{0}.{1}.{2}' -f $wrongParts[0], $wrongParts[1], ([int]$wrongParts[2] + 1)
$script:Failures = New-Object System.Collections.Generic.List[string]
$script:CleanupTargets = New-Object System.Collections.Generic.List[object]
$script:Roots = New-Object System.Collections.Generic.List[string]

function Write-Step { param([string]$Message) Write-Host "`n===== $Message" -ForegroundColor Cyan }
function Write-Ok { param([string]$Message) Write-Host "  [PASS] $Message" -ForegroundColor Green }
function Write-Bad { param([string]$Message) Write-Host "  [FAIL] $Message" -ForegroundColor Red; $script:Failures.Add($Message) | Out-Null }
function Write-Note { param([string]$Message) Write-Host "  | $Message" -ForegroundColor DarkGray }
function Assert-True { param([bool]$Condition, [string]$Message)
    if ($Condition) { Write-Ok $Message } else { Write-Bad $Message }
}

function ConvertTo-ExtendedPath {
    # 绝对路径 → \\?\ 扩展长度形态。LongPathsEnabled=0 的主机上 >260 字符路径
    # 只有 verbatim 形态可用（QA-005 长路径缺陷）；已是 verbatim / 空 值原样返回。
    param([string]$LiteralPath)
    if ([string]::IsNullOrWhiteSpace($LiteralPath)) { return $LiteralPath }
    if ($LiteralPath -like '\\?\*') { return $LiteralPath }
    if ($LiteralPath -like '\\*') { return ('\\?\UNC\' + $LiteralPath.TrimStart('\')) }
    return ('\\?\' + $LiteralPath)
}

function Join-Path {
    # PS 5.1 内建 Join-Path 解析不了 \\?\ verbatim 路径（drive 解析失败，实测）。
    # 长路径安装根场景下文件系统调用统一走 verbatim 形态：verbatim 输入退化为
    # 纯字符串拼接，其余输入转发内建实现。仅覆盖本脚本用到的位置参数形态。
    param([string]$Path, [string]$ChildPath)
    if ($Path -like '\\?\*') {
        return ($Path.TrimEnd('\') + '\' + $ChildPath.TrimStart('\'))
    }
    return (Microsoft.PowerShell.Management\Join-Path -Path $Path -ChildPath $ChildPath)
}

function Expand-InstallZip {
    # 解压 full ZIP 到安装根。PS 5.1 的 Expand-Archive（.NET ZipFile）在
    # LongPathsEnabled=0 下无法创建 >260 字符目标（QA-005 实测缺陷）；
    # System32\tar.exe（bsdtar）的 -C 同样进不了长目录（chdir 受 260 限制，
    # verbatim 也不行，实测）。故先解到短路径 staging，再用 robocopy 搬入
    # 安装根——robocopy 内部自带长路径处理，不依赖注册表开关。
    param([Parameter(Mandatory = $true)][string]$ZipPath, [Parameter(Mandatory = $true)][string]$DestinationPath)
    $stage = Join-Path $WorkDir ('expand-stage-' + [System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    try {
        & "$env:SystemRoot\System32\tar.exe" -xf $ZipPath -C $stage
        if ($LASTEXITCODE -ne 0) { throw "tar.exe 解压失败（exit $LASTEXITCODE）: $ZipPath" }
        & robocopy $stage $DestinationPath '/E' '/NFL' '/NDL' '/NJH' '/NJS' '/NP' | Out-Null
        if ($LASTEXITCODE -ge 8) { throw "robocopy 搬入安装根失败（exit $LASTEXITCODE）: $DestinationPath" }
    } finally {
        if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function ConvertTo-QuotedArguments {
    # PS 5.1 兼容：ProcessStartInfo.ArgumentList 在 .NET Framework 不存在，
    # 用 Windows 命令行引号规则拼 Arguments 字符串。
    param([string[]]$Items)
    $sb = New-Object System.Text.StringBuilder
    foreach ($item in $Items) {
        if ($sb.Length -gt 0) { [void]$sb.Append(' ') }
        [void]$sb.Append('"').Append(($item -replace '"', '\"')).Append('"')
    }
    return $sb.ToString()
}

function Start-E2EProcess {
    # 启动受控子进程（显式环境 + stdout/stderr 异步落盘），返回 Process。
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$ArgumentList = @(),
        [hashtable]$EnvMap = @{},
        [string]$WorkingDirectory,
        [string]$StdoutLog,
        [string]$StderrLog
    )
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $FilePath
    $psi.Arguments = ConvertTo-QuotedArguments -Items $ArgumentList
    $psi.UseShellExecute = $false
    if ($WorkingDirectory) { $psi.WorkingDirectory = $WorkingDirectory }
    if ($StdoutLog) { $psi.RedirectStandardOutput = $true }
    if ($StderrLog) { $psi.RedirectStandardError = $true }
    # 先清掉与本 E2E 相关的继承变量，再注入显式值
    foreach ($k in @('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy', 'NO_PROXY', 'no_proxy', 'GAMER_ADMIN_PASSWORD', 'GAMER_ADMIN_TOKEN', 'GAMER_LAUNCHER_RELEASE_MANIFEST')) {
        [void]$psi.EnvironmentVariables.Remove($k)
    }
    foreach ($k in $EnvMap.Keys) { $psi.EnvironmentVariables[$k] = [string]$EnvMap[$k] }
    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = $psi
    [void]$proc.Start()
    # 不用事件回调（PS 脚本块委托在 .NET 线程池线程上并发回调会崩掉整个
    # 解释器，实测 2026-08-31）：两流 ReadToEndAsync 后台收集，进程退出后由
    # Save-ProcessOutput 落盘。长驻进程运行期输出先驻内存（launcher/server
    # 另有自己的文件日志）。
    $outTask = $proc.StandardOutput.ReadToEndAsync()
    $errTask = $proc.StandardError.ReadToEndAsync()
    Add-Member -InputObject $proc -MemberType NoteProperty -Name OutTask -Value $outTask -Force
    Add-Member -InputObject $proc -MemberType NoteProperty -Name ErrTask -Value $errTask -Force
    Add-Member -InputObject $proc -MemberType NoteProperty -Name E2EStdoutLog -Value $StdoutLog -Force
    Add-Member -InputObject $proc -MemberType NoteProperty -Name E2EStderrLog -Value $StderrLog -Force
    return $proc
}

function Save-ProcessOutput {
    # 进程退出后把收集到的 stdout/stderr 写日志。注意：子进程（如 CLI 拉起的
    # 候选 server）会继承父进程的管道写句柄，ReadToEndAsync 任务可能永不完成
    # （管道读端等不到 EOF）——只刷已完成任务，未完成的注明并跳过，绝不阻塞。
    param($Process)
    if (-not $Process) { return }
    foreach ($pair in @(@('E2EStdoutLog', 'OutTask'), @('E2EStderrLog', 'ErrTask'))) {
        try {
            $logPath = $Process.($pair[0])
            $task = $Process.($pair[1])
            if ($logPath -and $task) {
                if (-not $task.IsCompleted) {
                    Set-Content -LiteralPath $logPath -Value "(输出流仍被存活子进程持有，本次不落盘)" -Encoding UTF8
                    continue
                }
                $text = $task.Result
                if ($text -and $text.Trim().Length -gt 0) { Set-Content -LiteralPath $logPath -Value $text -Encoding UTF8 }
            }
        } catch { }
    }
}

function Stop-E2EProcess {
    param($Process, [string]$Label, [switch]$NoTree)
    if ($Process -and -not $Process.HasExited) {
        try {
            # -NoTree：只杀 launcher 本身，子进程（server）成孤儿存活——升级接管
            # 场景需要 CLI upgrade 去真实 drain 这个孤儿 server
            if ($NoTree) {
                & taskkill.exe /F /PID $Process.Id 2>$null | Out-Null
            } else {
                & taskkill.exe /F /T /PID $Process.Id 2>$null | Out-Null
            }
            if (-not $Process.WaitForExit(5000)) { $Process.Kill() }
            Write-Note "$Label 已停止 (pid=$($Process.Id))"
        } catch { Write-Note "$Label 停止异常: $_" }
    }
}

function Stop-RootServers {
    # 只按命令行匹配本 E2E 安装根下的进程（不误杀其他进程）。adb.exe 守护进程
    # 也要清——它锁住 runtime\adb\...\adb.exe 会让整根 Remove-Item 失败；
    # launcher 不清会继续持有单实例锁，导致下一轮 repair/start 失败。
    param([string]$Root)
    try {
        $procs = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
            Where-Object {
                (($_.Name -in @('gamer-server.exe', 'gamer-launcher.exe')) -and $_.CommandLine -like "*$Root*") -or
                (($_.Name -eq 'adb.exe') -and $_.ExecutablePath -like "*$Root*")
            }
        foreach ($p in $procs) {
            & taskkill.exe /F /T /PID $p.ProcessId 2>$null | Out-Null
            Write-Note ("已停止安装根内进程 {0} pid={1}" -f $p.Name, $p.ProcessId)
        }
    } catch { }
}

function Stop-StaleHttpServer {
    # 清掉上一次运行遗留的 manifest HTTP 服务（同端口会让本轮 python 绑定失败）
    param([int]$Port)
    try {
        $procs = Get-CimInstance Win32_Process -Filter "Name='python.exe'" -ErrorAction SilentlyContinue |
            Where-Object { $_.CommandLine -like "*http.server*$Port*" }
        foreach ($p in $procs) {
            & taskkill.exe /F /PID $p.ProcessId 2>$null | Out-Null
            Write-Note ("已停止遗留 http.server pid={0}" -f $p.ProcessId)
        }
    } catch { }
}

function Wait-HttpReady {
    param([int]$Port, [int]$TimeoutSec = 120)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $resp = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$Port/health/ready" -TimeoutSec 3
            if ($resp.StatusCode -eq 200) { return $true }
        } catch { }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

function Invoke-Login {
    # curl 同源语义：不带 Origin 头（Origin 缺失放行），返回 WebSession。
    param([int]$Port, [string]$Password)
    $body = @{ username = 'admin'; password = $Password } | ConvertTo-Json -Compress
    $resp = Invoke-WebRequest -UseBasicParsing -Method Post -Uri "http://127.0.0.1:$Port/api/login" `
        -ContentType 'application/json' -Body $body -SessionVariable sess -TimeoutSec 10
    if ($resp.StatusCode -ne 200) { throw "login 失败: HTTP $($resp.StatusCode)" }
    return $sess
}

function Get-UpdateState {
    param([Microsoft.PowerShell.Commands.WebRequestSession]$Session, [int]$Port)
    return (Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/system/update" -WebSession $Session -TimeoutSec 10)
}

function Wait-UpdateState {
    param([Microsoft.PowerShell.Commands.WebRequestSession]$Session, [int]$Port, [string[]]$Want, [int]$TimeoutSec = 120)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $last = ''
    while ((Get-Date) -lt $deadline) {
        try {
            $st = Get-UpdateState -Session $Session -Port $Port
            $last = "$($st.state)/$($st.detail)"
            if ($Want -contains $st.state) { return $st }
        } catch { $last = "req-error" }
        Start-Sleep -Milliseconds 500
    }
    throw "等待更新状态 [$($Want -join ',')] 超时（最后: $last）"
}

function Read-Journal {
    param([string]$Root)
    $p = Join-Path $Root 'state\update-journal.json'
    if (-not (Test-Path -LiteralPath $p)) { return $null }
    return (Get-Content -LiteralPath $p -Raw -Encoding UTF8 | ConvertFrom-Json)
}

function Remove-DataDirectoryLink {
    # 跨盘 QA 用的是目录 junction。删除安装根前必须只移除 link 本身，不能让
    # Remove-Item -Recurse 误触物理 data 目标目录。
    param([string]$Root)
    $link = Join-Path $Root 'data'
    if (-not (Test-Path -LiteralPath $link)) { return }
    try {
        $item = Get-Item -LiteralPath $link -Force -ErrorAction Stop
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            & cmd.exe /d /c ('rmdir "{0}"' -f $link) 2>$null | Out-Null
            if ($LASTEXITCODE -ne 0 -and (Test-Path -LiteralPath $link)) {
                throw "目录 junction 删除失败: $link"
            }
        }
    } catch {
        throw "清理 data junction 失败: $($_.Exception.Message)"
    }
}

function New-DataDirectoryLink {
    # launcher/server 仍看到稳定契约中的 <install-root>\data 路径，但文件实际
    # 落在 target 所在卷；这样不改生产代码即可实证跨卷快照/恢复。
    param([string]$Root, [string]$Target)
    $link = Join-Path $Root 'data'
    Remove-DataDirectoryLink -Root $Root
    if (Test-Path -LiteralPath $link) {
        Remove-Item -LiteralPath $link -Recurse -Force -ErrorAction Stop
    }
    New-Item -ItemType Directory -Path $Target -Force | Out-Null
    $mklinkOutput = @(& cmd.exe /d /c ('mklink /J "{0}" "{1}"' -f $link, $Target) 2>&1)
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $link)) {
        throw "创建 data junction 失败: $link -> $Target; $($mklinkOutput -join ' ')"
    }
    return $link
}

function Invoke-UpgradeWithJournalTrace {
    # 后台运行 `launcher upgrade`，前台 50ms 轮询 journal 记录 (state|last_step) 迁移。
    param(
        [string]$LauncherExe, [string]$Root, [string]$ManifestPath,
        [hashtable]$EnvMap, [string]$TraceFile, [int]$TimeoutSec = 300,
        # 长路径场景：launcher exe 从短路径 staging 中转启动，cwd 不能是长根
        # （.NET ProcessStartInfo.WorkingDirectory 对 >260 路径会失败）
        [string]$WorkingDirectory
    )
    Set-Content -LiteralPath $TraceFile -Value "# journal transitions" -Encoding UTF8
    $proc = Start-E2EProcess -FilePath $LauncherExe `
        -ArgumentList @('--install-root', $Root, 'upgrade', '--manifest', $ManifestPath) `
        -EnvMap $EnvMap -WorkingDirectory $(if ($WorkingDirectory) { $WorkingDirectory } else { $Root }) `
        -StdoutLog "$TraceFile.stdout.log" -StderrLog "$TraceFile.stderr.log"
    $script:CleanupTargets.Add($proc) | Out-Null
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $prev = ''
    while (-not $proc.HasExited -and (Get-Date) -lt $deadline) {
        try {
            $j = Read-Journal -Root $Root
            if ($null -ne $j) {
                $cur = "{0}|{1}" -f $j.state, $j.last_step
                if ($cur -ne $prev) {
                    Add-Content -LiteralPath $TraceFile -Value ("{0:HH:mm:ss.fff}  {1}" -f (Get-Date), $cur) -Encoding UTF8
                    Write-Note ("journal → " + $cur)
                    $prev = $cur
                }
            }
        } catch { }
        Start-Sleep -Milliseconds 15
    }
    if (-not $proc.HasExited) { Stop-E2EProcess -Process $proc -Label 'upgrade(超时强杀)' }
    Save-ProcessOutput -Process $proc
    $exitCode = if ($proc.HasExited) { $proc.ExitCode } else { -1 }
    return @{ ExitCode = $exitCode; TraceFile = $TraceFile }
}

function Verify-Snapshot {
    # 复核 backups/<update-id>/manifest.json：逐文件 size+sha256 与快照副本一致。
    param([string]$Root, [string]$UpdateId)
    $snapDir = Join-Path $Root "backups\$UpdateId"
    $manifestPath = Join-Path $snapDir 'manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath)) { throw "快照清单不存在: $manifestPath" }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $bad = @()
    foreach ($f in $manifest.files) {
        $p = Join-Path $snapDir ($f.path -replace '/', '\')
        if (-not (Test-Path -LiteralPath $p)) { $bad += "$($f.path): 缺失"; continue }
        if ((Get-Item -LiteralPath $p).Length -ne $f.size) { $bad += "$($f.path): size 不符"; continue }
        if ((Get-Sha256 -Path $p) -ne $f.sha256) { $bad += "$($f.path): sha256 不符" }
    }
    return @{ Manifest = $manifest; Bad = $bad }
}

# ---------------------------------------------------------------------------
# 场景执行体
# ---------------------------------------------------------------------------
function Invoke-Scenario {
    param([bool]$CandidateMustFail)

    $tag = if ($CandidateMustFail) { 'B-rollback' } else { 'A-upgrade' }
    $port = if ($CandidateMustFail) { $PortB } else { $PortA }
    $rootName = if ($CandidateMustFail) { 'GameBot E2E 升级验证_B' } else { 'GameBot E2E 升级验证_A' }
    $rootOverride = if ($CandidateMustFail) { $InstallRootB } else { $InstallRootA }
    $root = if ([string]::IsNullOrWhiteSpace($rootOverride)) {
        Join-Path $WorkDir $rootName
    } else {
        [System.IO.Path]::GetFullPath($rootOverride)
    }
    $dataOverride = if ($CandidateMustFail) { $DataRootB } else { $DataRootA }
    if (-not [string]::IsNullOrWhiteSpace($dataOverride)) {
        $dataOverride = [System.IO.Path]::GetFullPath($dataOverride)
    }
    $script:Roots.Add($root) | Out-Null
    # 长路径安装根（QA-005）：>240 字符的根在 LongPathsEnabled=0 下所有文件
    # 系统调用必须走 \\?\ verbatim 形态（$rootFs）；$root 保持普通形态，用于
    # --install-root 参数、进程命令行匹配与 robocopy/robocopy 类原生工具。
    $longRootMode = $root.Length -gt 240
    $rootFs = if ($longRootMode) { ConvertTo-ExtendedPath $root } else { $root }
    $manifestName = if ($CandidateMustFail) { "$CandidateVersion-broken.json" } else { "$CandidateVersion.json" }
    $appZipName = if ($CandidateMustFail) { "gamer-app-$CandidateVersion-broken-windows-x64.zip" } else { "gamer-app-$CandidateVersion-windows-x64.zip" }
    # launcher exe 在 full ZIP 解压根（= 安装根）下；长路径场景从解压后的
    # 根中转复制到短路径 staging 再启动（见下方解压完成后的 staging 块）。
    # 复制源必须用 verbatim 形态（$rootFs）——PS 解析不了普通 >260 路径。
    $launcherExe = Join-Path $rootFs 'gamer-launcher.exe'
    $launcherCwd = $root
    $adminPass = 'e2e-admin-pass'

    Write-Step "[$tag] 解压 full ZIP → 安装根（中文+空格路径）"
    # 先清残留进程（server exe 被锁会让整根删除失败）；再删根重建。taskkill 是
    # 异步的，杀完等句柄真正释放（进程列表清空）再删，Remove-Item 带一次重试
    Stop-RootServers -Root $root
    $waitDeadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $waitDeadline) {
        $left = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
                (($_.Name -in @('gamer-server.exe', 'gamer-launcher.exe')) -and $_.CommandLine -like "*$Root*") -or
                (($_.Name -eq 'adb.exe') -and $_.ExecutablePath -like "*$Root*")
            })
        if ($left.Count -eq 0) { break }
        Start-Sleep -Milliseconds 500
    }
    $removed = $false
    foreach ($attempt in @(1, 2, 3)) {
        try {
            if (Test-Path -LiteralPath $rootFs) { Remove-Item -LiteralPath $rootFs -Recurse -Force -ErrorAction Stop }
            $removed = $true
            break
        } catch {
            Write-Note ("删除安装根重试 {0}: {1}" -f $attempt, $_.Exception.Message)
            Stop-RootServers -Root $root
            Start-Sleep -Seconds 2
        }
    }
    if (-not $removed -and (Test-Path -LiteralPath $rootFs)) { throw "安装根删除失败（仍有进程占用）: $root" }
    Expand-InstallZip -ZipPath (Join-Path $WorkDir "dist-m1\GameBot-$BaselineVersion-windows-x64-full.zip") -DestinationPath $root
    Write-Ok "解压完成: $root"

    if ($longRootMode) {
        # .NET ProcessStartInfo 无法启动 >260 字符的 exe（verbatim 也被归一化
        # 拒绝，实测）；launcher 自身从短路径 staging 中转启动，--install-root
        # 仍指向真实长安装根，被测行为不变。
        $launcherStage = Join-Path $WorkDir 'launcher-stage'
        New-Item -ItemType Directory -Path $launcherStage -Force | Out-Null
        Copy-Item -LiteralPath $launcherExe -Destination (Join-Path $launcherStage 'gamer-launcher.exe') -Force
        $launcherExe = Join-Path $launcherStage 'gamer-launcher.exe'
        $launcherCwd = $launcherStage
        Write-Ok "launcher 已中转到短路径 staging（长路径根 spawn 由产品侧 verbatim 路径承担）"
    }

    if (-not [string]::IsNullOrWhiteSpace($dataOverride)) {
        $dataLink = New-DataDirectoryLink -Root $root -Target $dataOverride
        $junctionListing = @(& cmd.exe /d /c ('dir /al "{0}"' -f $root) 2>&1) -join "`n"
        Set-Content -LiteralPath (Join-Path $WorkDir "logs\$tag-data-junction.txt") `
            -Value ("link=$dataLink`ntarget=$dataOverride`nroot_length=$($root.Length)`ndata_target_length=$($dataOverride.Length)`n$junctionListing") -Encoding UTF8
        $linkItem = Get-Item -LiteralPath $dataLink -Force
        Assert-True (($linkItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) `
            "data junction 创建成功（逻辑路径=$dataLink，物理目标=$dataOverride）"
    }

    # 端口错峰：改写用户配置（user data，升级纳入快照）。注意 .NET 正则 $ 不匹配
    # \r 之前，CRLF 文件必须显式 \r?
    $cfgPath = Join-Path $rootFs 'config\config.toml'
    (Get-Content -LiteralPath $cfgPath -Raw -Encoding UTF8) -replace '(?m)^port = 8443\r?$', "port = $port" |
        Set-Content -LiteralPath $cfgPath -Encoding UTF8 -NoNewline
    $portLine = (Get-Content -LiteralPath $cfgPath | Select-String -Pattern '^port').Line
    Assert-True ($portLine -eq "port = $port") "config.toml 端口改写（$portLine）"

    Write-Step "[$tag] repair 首装（死代理模拟断网，seeds 安装）"
    $repairOut = Join-Path $WorkDir "logs\$tag-repair.log"
    $rp = Start-E2EProcess -FilePath $launcherExe `
        -ArgumentList @('--install-root', $root, '--keys-dir', (Join-Path $rootFs 'keys'), 'repair', '--manifest', (Join-Path $rootFs "manifests\$BaselineVersion.json")) `
        -EnvMap @{ HTTP_PROXY = 'http://127.0.0.1:9'; HTTPS_PROXY = 'http://127.0.0.1:9'; ALL_PROXY = 'http://127.0.0.1:9' } `
        -WorkingDirectory $launcherCwd -StdoutLog $repairOut -StderrLog "$repairOut.err"
    $rp.WaitForExit(180000) | Out-Null
    Save-ProcessOutput -Process $rp
    $repairExit = if ($rp.HasExited) { $rp.ExitCode } else { -1 }
    Assert-True ($repairExit -eq 0) "repair 退出码 0（实际 $repairExit）"
    foreach ($line in (Get-Content -LiteralPath $repairOut -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -Last 4)) { Write-Note $line }
    $current = Get-Content -LiteralPath (Join-Path $rootFs 'state\current.json') -Raw | ConvertFrom-Json
    Assert-True ($current.current -eq $BaselineVersion -and $null -eq $current.previous) "current.json 首装指针 current=$BaselineVersion previous=null"

    # 缓存种子：候选 app zip 进 cache/artifacts（seeds→cache→remote 的 cache 级命中）
    $artifactsDir = Join-Path $rootFs 'cache\artifacts'
    New-Item -ItemType Directory -Path $artifactsDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $WorkDir "dist-m2\$appZipName") -Destination (Join-Path $artifactsDir $appZipName) -Force
    Write-Ok "cache/artifacts 已种子候选包 $appZipName"

    Write-Step "[$tag] launcher start（IPC 托管 + check 源 = 本机 HTTP manifest）"
    $startProc = Start-E2EProcess -FilePath $launcherExe `
        -ArgumentList @('--install-root', $root, 'start') `
        -EnvMap @{ GAMER_ADMIN_PASSWORD = $adminPass; GAMER_LAUNCHER_RELEASE_MANIFEST = "http://127.0.0.1:$HttpPort/$manifestName" } `
        -WorkingDirectory $launcherCwd -StdoutLog (Join-Path $WorkDir "logs\$tag-start.log") -StderrLog (Join-Path $WorkDir "logs\$tag-start.log.err")
    $script:CleanupTargets.Add($startProc) | Out-Null
    $ready = Wait-HttpReady -Port $port -TimeoutSec 120
    Assert-True $ready "/health/ready 200（port $port）"
    if ($ready) {
        $health = Invoke-RestMethod -Uri "http://127.0.0.1:$port/health/ready" -TimeoutSec 5
        Write-Note ("health: " + ($health | ConvertTo-Json -Compress -Depth 5))
    }

    $sess = Invoke-Login -Port $port -Password $adminPass
    Write-Ok 'POST /api/login 200（GAMER_ADMIN_PASSWORD 经 launcher 透传）'

    # 关闭协调器自动流程（默认 notify 会在后台自动 check+download；改为 off 后
    # 由本脚本显式驱动 check/download/install，时间线确定可断言）。PUT 尽力而为：
    # 失败不影响正确性，后续动作 POST 带一次重试兜底协调器 30s tick 的竞态。
    try {
        $st0 = Get-UpdateState -Session $sess -Port $port
        $policyOff = $st0.policy
        $policyOff.strategy = 'off'
        $putResp = Invoke-WebRequest -UseBasicParsing -Method Put -Uri "http://127.0.0.1:$port/api/system/update/policy" `
            -WebSession $sess -ContentType 'application/json' -Body ($policyOff | ConvertTo-Json -Compress) -TimeoutSec 10
        Assert-True ($putResp.StatusCode -eq 200) "PUT policy strategy=off（阻断协调器自动流程，显式驱动）"
    } catch {
        Write-Note "PUT policy off 未成功（尽力而为）: $_"
    }

    $info = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/system/info" -WebSession $sess -TimeoutSec 10
    $info | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $WorkDir "logs\$tag-info-before.json") -Encoding UTF8
    Assert-True ($info.app.version -eq $BaselineVersion) "system/info app.version=$BaselineVersion（实际 $($info.app.version)）"
    Assert-True ($info.deployment.mode -eq 'launcher' -and $info.deployment.update_strategy -eq 'managed') "deployment mode=launcher / update_strategy=managed"
    Assert-True ($info.capabilities.check -and $info.capabilities.download -and $info.capabilities.install -and $info.capabilities.rollback) "capabilities check/download/install/rollback 全 true（IPC 已建立）"

    # 业务数据标记（数据完整性锚点）
    $devBody = @{ name = "e2e-marker-$tag"; kind = '--'; addr = 'e2e://marker' } | ConvertTo-Json -Compress
    $devResp = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$port/api/devices" -WebSession $sess -ContentType 'application/json' -Body $devBody -TimeoutSec 10
    Assert-True ($devResp.ok -eq $true) "业务写入：创建标记设备 id=$($devResp.id)"
    # 数据完整性比对用稳定业务字段投影（id/name/kind/addr 排序拼接）；
    # DeviceView 还含 status/error/帧宽高等运行时动态字段，重启后天然可能不同
    function Get-DeviceFingerprint {
        param($Devices)
        return (($Devices | Sort-Object id | ForEach-Object { "$($_.id)|$($_.name)|$($_.kind)|$($_.addr)" }) -join ';')
    }
    $devicesBefore = Get-DeviceFingerprint (Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/devices" -WebSession $sess -TimeoutSec 10)

    Write-Step "[$tag] server API：check / download / install（202 + journal 推进）"
    foreach ($action in @('check', 'download', 'install')) {
        try {
            $resp = $null
            foreach ($attempt in @(1, 2)) {
                try {
                    $resp = Invoke-WebRequest -UseBasicParsing -Method Post -Uri "http://127.0.0.1:$port/api/system/update/$action" `
                        -WebSession $sess -ContentType 'application/json' -Body '{}' -TimeoutSec 60
                } catch {
                    # 协调器并发 tick 可能短暂 update_busy（409）：2s 后重试一次
                    if ($attempt -eq 1) { Start-Sleep -Seconds 2; continue }
                    throw
                }
                break
            }
            Assert-True ($resp.StatusCode -eq 202) "POST /api/system/update/$action → 202（body $($resp.Content)）"
            $want = switch ($action) {
                'check' { @('available') }
                'download' { @('staged') }
                'install' { @('staged') }   # install 受理 → IPC prepare_install 复验后驻留 staged
            }
            $st = Wait-UpdateState -Session $sess -Port $port -Want $want -TimeoutSec 120
            Write-Ok "更新状态推进至 $($st.state)/$($st.detail)（update_id=$($st.update_id)）"
        } catch {
            Write-Bad "动作 $action 未完成: $_"
        }
    }
    $stagedState = Get-UpdateState -Session $sess -Port $port
    $stagedState | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $WorkDir "logs\$tag-staged.json") -Encoding UTF8
    Write-Note ("staged 聚合: " + ($stagedState | ConvertTo-Json -Compress -Depth 6))

    Write-Step "[$tag] 结束 start（释放安装锁；孤儿 server 应存活）"
    Stop-E2EProcess -Process $startProc -Label 'launcher start' -NoTree
    Start-Sleep -Seconds 1
    $orphanAlive = $false
    try { $orphanAlive = ((Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$port/health/ready" -TimeoutSec 3).StatusCode -eq 200) } catch { }
    Assert-True $orphanAlive "孤儿 server（$BaselineVersion）仍存活且 ready（CLI upgrade 将真实 drain 它）"

    Write-Step "[$tag] launcher upgrade 接管（§6.6 全链路 + journal 50ms 轨迹）"
    $up = Invoke-UpgradeWithJournalTrace -LauncherExe $launcherExe -Root $rootFs `
        -ManifestPath (Join-Path $WorkDir "manifests\$manifestName") `
        -EnvMap @{ GAMER_ADMIN_PASSWORD = $adminPass } -TraceFile (Join-Path $WorkDir "logs\$tag-journal-trace.log") `
        -WorkingDirectory $launcherCwd
    $upgradeExit = $up.ExitCode
    $traceText = Get-Content -LiteralPath $up.TraceFile -Encoding UTF8 -Raw
    foreach ($line in (Get-Content -LiteralPath "$($up.TraceFile).stdout.log" -Encoding UTF8 -ErrorAction SilentlyContinue)) { Write-Note "upgrade stdout: $line" }
    foreach ($line in (Get-Content -LiteralPath "$($up.TraceFile).stderr.log" -Encoding UTF8 -ErrorAction SilentlyContinue)) { Write-Note "upgrade stderr: $line" }

    Write-Step "[$tag] 验收"
    $newServerReady = Wait-HttpReady -Port $port -TimeoutSec 90
    Assert-True $newServerReady "升级/回滚后 /health/ready 200（port $port）"

    $pointer = Get-Content -LiteralPath (Join-Path $rootFs 'state\current.json') -Raw | ConvertFrom-Json
    $journal = Read-Journal -Root $rootFs
    # CLI upgrade 会开自己的升级事务（phase_check 分配新 update_id），快照归属
    # 以 journal.snapshot 为准，而不是 API 阶段（IPC check）的事务 id
    $updateId = if ($journal -and $journal.snapshot -and $journal.snapshot.id) { $journal.snapshot.id } else { $stagedState.update_id }
    $sess2 = Invoke-Login -Port $port -Password $adminPass
    $info2 = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/system/info" -WebSession $sess2 -TimeoutSec 10
    $info2 | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $WorkDir "logs\$tag-info-after.json") -Encoding UTF8
    $devicesAfter = Get-DeviceFingerprint (Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/devices" -WebSession $sess2 -TimeoutSec 10)

    $snap = Verify-Snapshot -Root $rootFs -UpdateId $updateId
    Assert-True ($snap.Bad.Count -eq 0) "backups/$updateId 快照逐文件 sha256 复核全对（$($snap.Manifest.file_count) 文件 / $($snap.Manifest.total_bytes) 字节）"
    if ($snap.Bad.Count -gt 0) { $snap.Bad | ForEach-Object { Write-Note $_ } }

    if (-not $CandidateMustFail) {
        Assert-True ($upgradeExit -eq 0) "launcher upgrade 退出码 0（实际 $upgradeExit）"
        Assert-True ($pointer.current -eq $CandidateVersion) "current.json current=$CandidateVersion（实际 $($pointer.current)）"
        Assert-True ($pointer.previous -eq $BaselineVersion) "current.json previous=$BaselineVersion（实际 $($pointer.previous)）"
        Assert-True ($journal.state -eq 'idle') "journal 终态 idle（committed → cleaning → idle 复位）"
        Assert-True ($info2.app.version -eq $CandidateVersion) "登录后 system/info app.version=$CandidateVersion（实际 $($info2.app.version)）"
        Assert-True ($devicesAfter.Contains("e2e-marker-$tag")) "升级后业务数据仍在（标记设备可查）"
    } else {
        Assert-True ($upgradeExit -eq 1) "launcher upgrade 退出码 1（FailedOldHealthy，实际 $upgradeExit）"
        Assert-True ($pointer.current -eq $BaselineVersion) "current.json 回到 current=$BaselineVersion（实际 $($pointer.current)）"
        Assert-True ($null -eq $pointer.previous) "current.json previous=null（回滚到基线，previous 链正确）"
        Assert-True ($journal.state -eq 'idle' -and $journal.last_step -eq 'failed') "journal idle/failed（失败记录落盘；实际 $($journal.state)/$($journal.last_step)）"
        Assert-True ($null -ne $journal.error -and $journal.error.code -eq 'artifact_invalid') "journal.error.code=artifact_invalid（实际 $(if ($journal.error) { $journal.error.code } else { 'null' })）"
        if ($null -ne $journal.error) { Write-Note "journal.error.message: $($journal.error.message)" }
        Assert-True ($info2.app.version -eq $BaselineVersion) "旧版本程序恢复：system/info app.version=$BaselineVersion（实际 $($info2.app.version)）"
        Assert-True ($devicesAfter.Contains("e2e-marker-$tag")) "升级前数据仍在（标记设备可查）"
        Set-Content -LiteralPath (Join-Path $WorkDir "logs\$tag-devices-before.txt") -Value $devicesBefore -Encoding UTF8
        Set-Content -LiteralPath (Join-Path $WorkDir "logs\$tag-devices-after.txt") -Value $devicesAfter -Encoding UTF8
        # 「候选期间零业务写入」：升级前设备必须原样保留；新增设备仅允许来自
        # 旧版 server 重启后的 adb 自动扫描自举（kind=usb/emu 的真实环境设备），
        # 候选在激活闸内无任何业务写入路径
        $beforeIds = ($devicesBefore -split ';') | ForEach-Object { ($_ -split '\|')[0] }
        $extraDevs = ($devicesAfter -split ';') | Where-Object { $beforeIds -notcontains ($_ -split '\|')[0] }
        $unexpected = @($extraDevs | Where-Object { ($_.Split('|')[2]) -notin @('usb', 'emu', 'emu-wifi') })
        foreach ($d in $extraDevs) { Write-Note "新增设备（adb 扫描自举）: $d" }
        Assert-True ($unexpected.Count -eq 0) "无候选期间业务写入（升级前设备原样保留；新增仅 adb 扫描设备）"
        $qEntries = @(Get-ChildItem -LiteralPath (Join-Path $rootFs 'quarantine') -ErrorAction SilentlyContinue)
        Assert-True ($qEntries.Count -gt 0) "quarantine/ 保留失败阶段数据（$($qEntries.Count) 项）"
        Write-Note "quarantine 条目: $($qEntries.Name -join ', ')"
        # switched 的确定性工件佐证（journal 轨迹 50ms 采样可能漏掉该快边）
        Assert-True (Test-Path -LiteralPath (Join-Path $rootFs "versions\$CandidateVersion\gamer-server.exe")) "switched 工件：versions/$CandidateVersion/ 已安装（候选确实换入过）"
    }

    # journal 轨迹（15ms 轮询）：journal 状态机严格顺序推进，candidate_starting
    # 出现即证明 switched/committed 等前序边全部发生过；switched/migrating 是
    # 亚 15ms 快边（migrating 只跨一次 rename 的两次 journal 写），采样缺漏时
    # 由后继态出现按状态机顺序性判定通过，不视为产品缺陷
    if (-not $traceText.Contains('migrating|migrating') -and $traceText.Contains('switched|switched')) {
        Write-Note "migrating 快边未被 15ms 采样捕获（透传态）；switched 已出现，按状态机严格顺序判定该边发生过"
    } else {
        Assert-True ($traceText.Contains('migrating|migrating')) "journal 轨迹含 migrating|migrating"
    }
    $mandatory = @('waiting_idle|waiting_idle', 'snapshotting|snapshotting', 'candidate_starting|candidate_starting')
    foreach ($key in $mandatory) {
        Assert-True ($traceText.Contains($key)) "journal 轨迹含 $key"
    }
    if ($CandidateMustFail) {
        Assert-True ($traceText.Contains('candidate_starting|rolling_back')) "journal 轨迹含 candidate_starting|rolling_back（回滚确实接管）"
    }

    Stop-RootServers -Root $root
    Write-Ok "[$tag] 场景完成"
}

# ---------------------------------------------------------------------------
# 场景 C：LCH-008 候选身份校验真实进程回归（X-Admin-Token 接线 + 负例回滚）
# ---------------------------------------------------------------------------
function Invoke-IdentityScenario {
    # 缺陷 #5（UPDATE_M2_EVIDENCE §E-6）收口回归：候选身份探针携带
    # state/admin-token 派生的 X-Admin-Token → 受保护组 /api/system/info 返回
    # 200 → version/schema/boot_id 真实比对。负例：manifest 声明版本
    # （$WrongVersion = 候选补丁位 +1）与候选二进制真实版本（$CandidateVersion）
    # 不符 → 引擎必须在 commit 前拒绝并完整回滚。
    # 该负例同时是 (a) 的正证：/health/ready 回退 body 无版本字段，唯有带令牌
    # 的 info 探测成功才可能观测到「版本不符」——若接线失效，升级会照常
    # committed，本场景必然失败。
    $tag = 'C-identity'
    $port = $PortC
    $root = Join-Path $WorkDir 'GameBot E2E 升级验证_C'
    $script:Roots.Add($root) | Out-Null
    $rootFs = $root
    $launcherExe = Join-Path $rootFs 'gamer-launcher.exe'
    $launcherCwd = $root
    $adminPass = 'e2e-admin-pass'

    foreach ($required in @(
            (Join-Path $WorkDir "dist-m1\GameBot-$BaselineVersion-windows-x64-full.zip"),
            (Join-Path $WorkDir "dist-m2\gamer-app-$CandidateVersion-windows-x64.zip"),
            (Join-Path $WorkDir "manifests\$CandidateVersion.json"),
            (Join-Path $WorkDir 'keys\dev-ed25519-1.private.pem'))) {
        if (-not (Test-Path -LiteralPath $required)) {
            throw "identity 场景缺少构建产物: $required（先以 -Scenario all/build 构建一次）"
        }
    }

    Write-Step "[$tag] 解压 full ZIP → 安装根"
    Stop-RootServers -Root $root
    $waitDeadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $waitDeadline) {
        $left = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
                (($_.Name -in @('gamer-server.exe', 'gamer-launcher.exe')) -and $_.CommandLine -like "*$Root*") -or
                (($_.Name -eq 'adb.exe') -and $_.ExecutablePath -like "*$Root*")
            })
        if ($left.Count -eq 0) { break }
        Start-Sleep -Milliseconds 500
    }
    foreach ($attempt in @(1, 2, 3)) {
        try {
            if (Test-Path -LiteralPath $rootFs) { Remove-Item -LiteralPath $rootFs -Recurse -Force -ErrorAction Stop }
            break
        } catch {
            Write-Note ("删除安装根重试 {0}: {1}" -f $attempt, $_.Exception.Message)
            Stop-RootServers -Root $root
            Start-Sleep -Seconds 2
        }
    }
    Expand-InstallZip -ZipPath (Join-Path $WorkDir "dist-m1\GameBot-$BaselineVersion-windows-x64-full.zip") -DestinationPath $root
    Write-Ok "解压完成: $root"

    $cfgPath = Join-Path $rootFs 'config\config.toml'
    (Get-Content -LiteralPath $cfgPath -Raw -Encoding UTF8) -replace '(?m)^port = 8443\r?$', "port = $port" |
        Set-Content -LiteralPath $cfgPath -Encoding UTF8 -NoNewline

    Write-Step "[$tag] repair 首装 + 缓存候选包"
    $repairOut = Join-Path $WorkDir "logs\$tag-repair.log"
    $rp = Start-E2EProcess -FilePath $launcherExe `
        -ArgumentList @('--install-root', $root, '--keys-dir', (Join-Path $rootFs 'keys'), 'repair', '--manifest', (Join-Path $rootFs "manifests\$BaselineVersion.json")) `
        -EnvMap @{ HTTP_PROXY = 'http://127.0.0.1:9'; HTTPS_PROXY = 'http://127.0.0.1:9'; ALL_PROXY = 'http://127.0.0.1:9' } `
        -WorkingDirectory $launcherCwd -StdoutLog $repairOut -StderrLog "$repairOut.err"
    $rp.WaitForExit(180000) | Out-Null
    Save-ProcessOutput -Process $rp
    $repairExit = if ($rp.HasExited) { $rp.ExitCode } else { -1 }
    Assert-True ($repairExit -eq 0) "repair 退出码 0（实际 $repairExit）"
    $artifactsDir = Join-Path $rootFs 'cache\artifacts'
    New-Item -ItemType Directory -Path $artifactsDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $WorkDir "dist-m2\gamer-app-$CandidateVersion-windows-x64.zip") `
        -Destination (Join-Path $artifactsDir "gamer-app-$CandidateVersion-windows-x64.zip") -Force
    Write-Ok "cache/artifacts 已种子候选包 gamer-app-$CandidateVersion-windows-x64.zip"

    Write-Step "[$tag] launcher start（IPC 托管；无 manifest 源 → 协调器空转）"
    $startProc = Start-E2EProcess -FilePath $launcherExe `
        -ArgumentList @('--install-root', $root, 'start') `
        -EnvMap @{ GAMER_ADMIN_PASSWORD = $adminPass } `
        -WorkingDirectory $launcherCwd -StdoutLog (Join-Path $WorkDir "logs\$tag-start.log") -StderrLog (Join-Path $WorkDir "logs\$tag-start.log.err")
    $script:CleanupTargets.Add($startProc) | Out-Null
    $ready = Wait-HttpReady -Port $port -TimeoutSec 120
    Assert-True $ready "/health/ready 200（port $port）"
    $sess = Invoke-Login -Port $port -Password $adminPass
    Write-Ok 'POST /api/login 200'

    # 业务数据标记（快照恢复完整性锚点）
    $devBody = @{ name = "e2e-marker-$tag"; kind = '--'; addr = 'e2e://marker' } | ConvertTo-Json -Compress
    $devResp = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$port/api/devices" -WebSession $sess -ContentType 'application/json' -Body $devBody -TimeoutSec 10
    Assert-True ($devResp.ok -eq $true) "业务写入：创建标记设备 id=$($devResp.id)"

    Write-Step "[$tag] 结束 start（孤儿 0.1.0 server 存活，供真实 drain / boot_id 锚定）"
    Stop-E2EProcess -Process $startProc -Label 'launcher start' -NoTree
    Start-Sleep -Seconds 1
    $orphanAlive = $false
    try { $orphanAlive = ((Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$port/health/ready" -TimeoutSec 3).StatusCode -eq 200) } catch { }
    Assert-True $orphanAlive "孤儿 server（0.1.0）仍存活且 ready"

    Write-Step "[$tag] 构造身份不符 manifest（声明 0.2.1，制品仍为真实 0.2.0 二进制）"
    $wrongManifestPath = Join-Path $WorkDir 'manifests\0.2.1-wrongver.json'
    $good = Get-Content -LiteralPath (Join-Path $WorkDir 'manifests\0.2.0.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $good.release.version = '0.2.1'
    [System.IO.File]::WriteAllText($wrongManifestPath, ($good | ConvertTo-Json -Depth 12) + "`n", (New-Object System.Text.UTF8Encoding($false)))
    $pack = Join-Path $RepoRoot 'release\packaging'
    $r = Invoke-Native -FilePath 'node' -Arguments ('"{0}" sign "{1}" --key "{2}" --key-id dev-ed25519-1' -f (Join-Path $pack 'sign-manifest.mjs'), $wrongManifestPath, (Join-Path $WorkDir 'keys\dev-ed25519-1.private.pem')) -TailLines 1
    if ($r.ExitCode -ne 0) { throw '身份不符 manifest 签名失败' }
    $r = Invoke-Native -FilePath 'node' -Arguments ('"{0}" check "{1}" --keys-dir "{2}" --expect-current-version 0.1.0 --expect-channel stable' -f (Join-Path $RepoRoot 'release\contracts\validate-manifest.mjs'), $wrongManifestPath, (Join-Path $WorkDir 'keys')) -TailLines 2
    foreach ($line in $r.Tail) { Write-Note $line }
    if ($r.ExitCode -ne 0) { throw '身份不符 manifest 校验未通过' }
    Write-Ok '0.2.1-wrongver manifest 签名 + 校验通过（0.2.1 > 0.1.0，候选二进制实际 0.2.0）'

    Write-Step "[$tag] launcher upgrade（身份校验必须在 commit 前拒绝并回滚）"
    $up = Invoke-UpgradeWithJournalTrace -LauncherExe $launcherExe -Root $rootFs `
        -ManifestPath $wrongManifestPath `
        -EnvMap @{ GAMER_ADMIN_PASSWORD = $adminPass } -TraceFile (Join-Path $WorkDir "logs\$tag-journal-trace.log") `
        -WorkingDirectory $launcherCwd
    $upgradeExit = $up.ExitCode
    $traceText = Get-Content -LiteralPath $up.TraceFile -Encoding UTF8 -Raw
    foreach ($line in (Get-Content -LiteralPath "$($up.TraceFile).stdout.log" -Encoding UTF8 -ErrorAction SilentlyContinue)) { Write-Note "upgrade stdout: $line" }
    foreach ($line in (Get-Content -LiteralPath "$($up.TraceFile).stderr.log" -Encoding UTF8 -ErrorAction SilentlyContinue)) { Write-Note "upgrade stderr: $line" }

    Write-Step "[$tag] 验收"
    $restoredReady = Wait-HttpReady -Port $port -TimeoutSec 90
    Assert-True $restoredReady "回滚后 /health/ready 200（port $port）"

    $pointer = Get-Content -LiteralPath (Join-Path $rootFs 'state\current.json') -Raw | ConvertFrom-Json
    $journal = Read-Journal -Root $rootFs

    Assert-True ($upgradeExit -eq 1) "launcher upgrade 退出码 1（FailedOldHealthy，实际 $upgradeExit）"
    Assert-True ($null -ne $journal.error -and $journal.error.code -eq 'artifact_invalid') "journal.error.code=artifact_invalid（实际 $(if ($journal.error) { $journal.error.code } else { 'null' })）"
    Assert-True ($null -ne $journal.error -and $journal.error.message -like '*版本不符*') `
        "身份门禁触发：journal.error.message 含「版本不符」（实际 $(if ($journal.error) { $journal.error.message } else { 'null' })）"
    Assert-True ($null -ne $journal.error -and $journal.error.message -like '*0.2.1*') `
        "期望版本 0.2.1 与观测版本进入同一诊断（实际 $(if ($journal.error) { $journal.error.message } else { 'null' })）"

    # (a) 探针携带 token 的直接证据：身份观测日志落盘（logs/launcher.log 双写）
    $launcherLog = Join-Path $rootFs 'logs\launcher.log'
    $identityObserved = $false
    if (Test-Path -LiteralPath $launcherLog) {
        $identityObserved = @((Get-Content -LiteralPath $launcherLog -Encoding UTF8 -ErrorAction SilentlyContinue) -match '候选身份已由 /api/system/info 观测').Count -gt 0
    }
    Assert-True $identityObserved "launcher.log 含「候选身份已由 /api/system/info 观测」（X-Admin-Token 鉴权探针真实执行）"

    Assert-True ($pointer.current -eq '0.1.0') "current.json 回到 current=0.1.0（实际 $($pointer.current)）"
    Assert-True ($null -eq $pointer.previous) "current.json previous=null（回滚到基线）"
    Assert-True ($journal.state -eq 'idle' -and $journal.last_step -eq 'failed') "journal idle/failed（实际 $($journal.state)/$($journal.last_step)）"

    $sess2 = Invoke-Login -Port $port -Password $adminPass
    $info2 = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/system/info" -WebSession $sess2 -TimeoutSec 10
    Assert-True ($info2.app.version -eq '0.1.0') "旧版本程序恢复：system/info app.version=0.1.0（实际 $($info2.app.version)）"
    $devs2 = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/devices" -WebSession $sess2 -TimeoutSec 10
    $markerAlive = @($devs2 | Where-Object { $_.name -eq "e2e-marker-$tag" }).Count -gt 0
    Assert-True $markerAlive "升级前数据仍在（标记设备可查）"

    $qEntries = @(Get-ChildItem -LiteralPath (Join-Path $rootFs 'quarantine') -ErrorAction SilentlyContinue)
    Assert-True ($qEntries.Count -gt 0) "quarantine/ 保留失败阶段数据（$($qEntries.Count) 项）"
    Assert-True (Test-Path -LiteralPath (Join-Path $rootFs 'versions\0.2.1\gamer-server.exe')) "switched 工件：versions/0.2.1/ 已安装（候选确实换入过，回滚才有效力）"

    if ($journal -and $journal.snapshot -and $journal.snapshot.id) {
        $snap = Verify-Snapshot -Root $rootFs -UpdateId $journal.snapshot.id
        Assert-True ($snap.Bad.Count -eq 0) "backups/$($journal.snapshot.id) 快照逐文件 sha256 复核全对（$($snap.Manifest.file_count) 文件）"
    }
    Assert-True ($traceText.Contains('candidate_starting|candidate_starting')) "journal 轨迹含 candidate_starting|candidate_starting"
    Assert-True ($traceText.Contains('candidate_starting|rolling_back')) "journal 轨迹含 candidate_starting|rolling_back（回滚确实接管）"

    Stop-RootServers -Root $root
    Write-Ok "[$tag] 场景完成"
}

# ---------------------------------------------------------------------------
# 构建 / 打包
# ---------------------------------------------------------------------------
function Invoke-Native {
    # PS 5.1 安全的原生调用：stderr 不与 $ErrorActionPreference='Stop' 相互作用
    # （2>&1 会把 stderr 行变成终止性 ErrorRecord），输出落临时文件、退出码可靠。
    # 返回 @{ ExitCode; Tail }（Tail = 输出末尾若干行，诊断用）。
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string]$Arguments = '',
        [string]$WorkingDirectory,
        [hashtable]$EnvMap = @{},
        [string[]]$EnvRemove = @(),
        [int]$TailLines = 3
    )
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $FilePath
    $psi.Arguments = $Arguments
    $psi.UseShellExecute = $false
    if ($WorkingDirectory) { $psi.WorkingDirectory = $WorkingDirectory }
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    foreach ($k in $EnvRemove) { [void]$psi.EnvironmentVariables.Remove($k) }
    foreach ($k in $EnvMap.Keys) { $psi.EnvironmentVariables[$k] = [string]$EnvMap[$k] }
    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = $psi
    [void]$proc.Start()
    # ReadToEndAsync：不用事件回调（PS 脚本块委托在线程池线程被回调时可能静默
    # 崩溃整个解释器，实测 2026-08-31）；两流并行异步读避免缓冲区死锁。
    $outTask = $proc.StandardOutput.ReadToEndAsync()
    $errTask = $proc.StandardError.ReadToEndAsync()
    $proc.WaitForExit() | Out-Null
    $all = @()
    $all += (($outTask.Result -split "`r?`n") | Where-Object { $_.Trim().Length -gt 0 })
    $all += (($errTask.Result -split "`r?`n") | Where-Object { $_.Trim().Length -gt 0 })
    $tail = @($all | Select-Object -Last $TailLines)
    return @{ ExitCode = $proc.ExitCode; Tail = $tail }
}

function Invoke-BuildAndPackage {
    Write-Step '构建：主树 release（server + launcher）与 web'
    $headSha = (& git -C $RepoRoot rev-parse HEAD).Trim()
    $buildEnv = @{ GAMER_GIT_COMMIT = $headSha; GAMER_BUILD_TIME = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'); GAMER_CHANNEL = 'stable'; GAMER_BUILD_TARGET = 'x86_64-pc-windows-gnu' }
    foreach ($build in @('server', 'launcher')) {
        $r = Invoke-Native -FilePath 'cargo' -Arguments 'build --release' -WorkingDirectory (Join-Path $RepoRoot $build) -TailLines 1
        if ($r.ExitCode -ne 0) { throw "cargo build --release（$build）退出码 $($r.ExitCode)" }
        Write-Ok "$build cargo build --release OK"
    }
    $r = Invoke-Native -FilePath 'pnpm' -Arguments 'build' -WorkingDirectory (Join-Path $RepoRoot 'web') -TailLines 1
    if ($r.ExitCode -ne 0) { throw "pnpm build 失败（退出码 $($r.ExitCode)）" }
    Write-Ok 'web pnpm build OK'

    Write-Step '打包：M1 full 包（工作区自有 dist/keys/manifests，不触碰主仓 release/dist）'
    $pack = Join-Path $RepoRoot 'release\packaging'
    $distM1 = Join-Path $WorkDir 'dist-m1'
    $keys = Join-Path $WorkDir 'keys'
    $manifests = Join-Path $WorkDir 'manifests'
    foreach ($d in @($distM1, $manifests)) { if (-not (Test-Path -LiteralPath $d)) { New-Item -ItemType Directory -Path $d -Force | Out-Null } }
    if (-not (Test-Path -LiteralPath (Join-Path $keys 'dev-ed25519-1.private.pem'))) {
        $r = Invoke-Native -FilePath 'node' -Arguments ('"{0}" keygen --id dev-ed25519-1 --out-dir "{1}"' -f (Join-Path $pack 'sign-manifest.mjs'), $keys) -TailLines 1
        if ($r.ExitCode -ne 0) { throw 'keygen 失败' }
        Write-Ok '签名密钥对生成（工作区 keys/，私钥不入库）'
    }
    function Invoke-Pack { param([string]$Script, [string]$Arguments, [string]$Label)
        $r = Invoke-Native -FilePath 'powershell.exe' -Arguments ('-NoProfile -ExecutionPolicy Bypass -File "{0}" {1}' -f $Script, $Arguments) -TailLines 2
        foreach ($line in $r.Tail) { Write-Note $line }
        if ($r.ExitCode -ne 0) { throw "$Label 失败（退出码 $($r.ExitCode)）" }
        Write-Ok $Label
    }
    Invoke-Pack (Join-Path $pack 'package-components.ps1') ('-DistDir "{0}"' -f $distM1) 'package-components.ps1（adb/ffmpeg zip，vendor 逐 hash 校验）'
    Invoke-Pack (Join-Path $pack 'package-app.ps1') ('-SkipBuild -DistDir "{0}"' -f $distM1) 'package-app.ps1（gamer-app-0.1.0 zip）'
    Invoke-Pack (Join-Path $pack 'gen-manifest.ps1') ('-Version 0.1.0 -DistDir "{0}" -OutDir "{1}" -KeysDir "{2}"' -f $distM1, $manifests, $keys) 'gen-manifest.ps1（0.1.0 签名 manifest）'
    Invoke-Pack (Join-Path $pack 'package-full.ps1') ('-SkipBuild -Version 0.1.0 -DistDir "{0}" -ManifestDir "{1}" -KeysDir "{2}" -KeyId dev-ed25519-1' -f $distM1, $manifests, $keys) 'package-full.ps1（full ZIP + SHA256SUMS + 包内验签）'

    Write-Step '候选构建：隔离副本（排除 .git/target/node_modules）+ 版本 0.2.0'
    $copy = Join-Path $WorkDir 'candidate-copy'
    $distM2 = Join-Path $WorkDir 'dist-m2'
    if (-not (Test-Path -LiteralPath $distM2)) { New-Item -ItemType Directory -Path $distM2 -Force | Out-Null }
    & robocopy $RepoRoot $copy /E /XD .git target node_modules dist vendor baseline-backups /XF *.log /NFL /NDL /NJH /NP /MT:8 /IS /IT 2>$null | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy 失败（退出码 $LASTEXITCODE）" }
    $cargoToml = Join-Path $copy 'server\Cargo.toml'
    (Get-Content -LiteralPath $cargoToml -Raw -Encoding UTF8) -replace '(?m)^version = "0\.1\.0"\r?', 'version = "0.2.0"' |
        Set-Content -LiteralPath $cargoToml -Encoding UTF8 -NoNewline
    Write-Ok '副本 server/Cargo.toml version → 0.2.0（主树未动）'
    $r = Invoke-Native -FilePath 'pnpm' -Arguments 'install --prefer-offline' -WorkingDirectory (Join-Path $copy 'web') -TailLines 1
    if ($r.ExitCode -ne 0) { throw "副本 pnpm install 失败（退出码 $($r.ExitCode)）" }
    $r = Invoke-Native -FilePath 'pnpm' -Arguments 'build' -WorkingDirectory (Join-Path $copy 'web') -TailLines 1
    if ($r.ExitCode -ne 0) { throw "副本 pnpm build 失败（退出码 $($r.ExitCode)）" }
    $r = Invoke-Native -FilePath 'cargo' -Arguments 'build --release' -WorkingDirectory (Join-Path $copy 'server') -EnvMap $buildEnv -TailLines 1
    if ($r.ExitCode -ne 0) { throw '副本 cargo build --release 失败' }
    Write-Ok '副本 server cargo build --release OK（0.2.0，真实 commit 注入）'
    Invoke-Pack (Join-Path $copy 'release\packaging\package-app.ps1') ('-SkipBuild -RepoRoot "{0}" -DistDir "{1}"' -f $copy, $distM2) 'package-app.ps1（gamer-app-0.2.0 zip，副本）'
    foreach ($n in @('gamer-adb-37.0.1-windows-x64.zip', 'gamer-ffmpeg-N-126335-gb32f8d1c23-20260830-windows-x64.zip')) {
        Copy-Item -LiteralPath (Join-Path $distM1 $n) -Destination (Join-Path $distM2 $n) -Force
    }
    Invoke-Pack (Join-Path $copy 'release\packaging\gen-manifest.ps1') ('-Version 0.2.0 -DistDir "{0}" -OutDir "{1}" -KeysDir "{2}"' -f $distM2, $manifests, $keys) 'gen-manifest.ps1（0.2.0 签名 manifest，dev-ed25519-1）'
    # HTTP 分离签名约定 URL+.sig（fetch_remote_manifest）；gen-manifest 落盘名是
    # <v>.sig，HTTP 服务侧补一份 <v>.json.sig（每次重签后同步，防陈旧签名）
    foreach ($v in @('0.2.0')) {
        Copy-Item -LiteralPath (Join-Path $manifests "$v.sig") -Destination (Join-Path $manifests "$v.json.sig") -Force
    }

    Write-Step '故障候选构建（场景 B）：main.rs 注入「启动即退出」缺陷（仅副本）'
    $mainRs = Join-Path $copy 'server\src\main.rs'
    $mainText = [System.IO.File]::ReadAllText($mainRs, (New-Object System.Text.UTF8Encoding($false)))
    $markerBegin = '// [E2E-SABOTAGE-BEGIN]'
    if (-not $mainText.Contains($markerBegin)) {
        $sabotage = @(
            "$markerBegin 场景 B 故障候选：无子命令（= 正常 server 启动）时 3 秒后退出。",
            '    // maintenance 子命令（inspect/migrate）不受影响——快照 schema 门禁照常可用。',
            '    if argv.len() == 1 {',
            '        std::thread::sleep(std::time::Duration::from_secs(3));',
            '        eprintln!("E2E sabotaged candidate: fatal startup defect, exiting");',
            '        std::process::exit(1);',
            '    }',
            '// [E2E-SABOTAGE-END]'
        ) -join "`r`n"
        $anchor = 'let argv: Vec<String> = std::env::args().collect();'
        if (-not $mainText.Contains($anchor)) { throw 'main.rs 注入点未找到（argv 收集语句）' }
        $mainText = $mainText.Replace($anchor, "$anchor`r`n$sabotage")
        [System.IO.File]::WriteAllText($mainRs, $mainText, (New-Object System.Text.UTF8Encoding($false)))
        Write-Ok 'main.rs 故障补丁注入（仅副本，主树未动）'
    } else {
        Write-Note '故障补丁已存在（幂等跳过）'
    }
    $r = Invoke-Native -FilePath 'cargo' -Arguments 'build --release' -WorkingDirectory (Join-Path $copy 'server') -EnvMap $buildEnv -TailLines 1
    if ($r.ExitCode -ne 0) { throw '故障候选 cargo build 失败' }
    # 故障 app zip（独立 artifact 名，避免与正常候选 cache 冲突）
    $stageBroken = Join-Path $WorkDir 'stage-broken'
    if (Test-Path -LiteralPath $stageBroken) { Remove-Item -LiteralPath $stageBroken -Recurse -Force }
    New-Item -ItemType Directory -Path $stageBroken -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $copy 'server\target\release\gamer-server.exe') -Destination (Join-Path $stageBroken 'gamer-server.exe')
    Copy-Item -Path (Join-Path $copy 'server\web-dist\*') -Destination (Join-Path $stageBroken 'web-dist') -Recurse -Force
    New-Item -ItemType Directory -Path (Join-Path $stageBroken 'assets') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $copy 'server\assets\scrcpy-server.jar') -Destination (Join-Path $stageBroken 'assets\scrcpy-server.jar')
    $brokenZip = Join-Path $distM2 'gamer-app-0.2.0-broken-windows-x64.zip'
    if (Test-Path -LiteralPath $brokenZip) { Remove-Item -LiteralPath $brokenZip -Force }
    Add-Type -AssemblyName System.IO.Compression | Out-Null
    Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null
    $fs = [System.IO.File]::Open($brokenZip, [System.IO.FileMode]::CreateNew)
    $zip = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($f in (Get-ChildItem -LiteralPath $stageBroken -Recurse -File | Sort-Object FullName)) {
            $rel = $f.FullName.Substring($stageBroken.Length + 1) -replace '\\', '/'
            [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $f.FullName, $rel, [System.IO.Compression.CompressionLevel]::Optimal)
        }
    } finally { $zip.Dispose(); $fs.Dispose() }
    Remove-Item -LiteralPath $stageBroken -Recurse -Force
    $brokenExeSha = Get-Sha256 -Path (Join-Path $copy 'server\target\release\gamer-server.exe')
    Write-Ok "gamer-app-0.2.0-broken-windows-x64.zip（故障 exe sha256=$($brokenExeSha.Substring(0, 16))…）"
    # 故障 manifest（0.2.0 版本号不变，app.artifact 指向 broken zip，重签名 + 校验）
    $goodManifest = Get-Content -LiteralPath (Join-Path $manifests '0.2.0.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $goodManifest.platforms.'windows-x86_64'.app.artifact.name = 'gamer-app-0.2.0-broken-windows-x64.zip'
    $goodManifest.platforms.'windows-x86_64'.app.artifact.url = 'https://mirror.e2e.invalid/gamer-app-0.2.0-broken-windows-x64.zip'
    $goodManifest.platforms.'windows-x86_64'.app.artifact.size = (Get-Item -LiteralPath $brokenZip).Length
    $goodManifest.platforms.'windows-x86_64'.app.artifact.sha256 = Get-Sha256 -Path $brokenZip
    $brokenManifestPath = Join-Path $manifests '0.2.0-broken.json'
    [System.IO.File]::WriteAllText($brokenManifestPath, ($goodManifest | ConvertTo-Json -Depth 12) + "`n", (New-Object System.Text.UTF8Encoding($false)))
    $r = Invoke-Native -FilePath 'node' -Arguments ('"{0}" sign "{1}" --key "{2}" --key-id dev-ed25519-1' -f (Join-Path $pack 'sign-manifest.mjs'), $brokenManifestPath, (Join-Path $keys 'dev-ed25519-1.private.pem')) -TailLines 1
    if ($r.ExitCode -ne 0) { throw '故障 manifest 签名失败' }
    $r = Invoke-Native -FilePath 'node' -Arguments ('"{0}" check "{1}" --keys-dir "{2}" --expect-current-version 0.1.0 --expect-channel stable' -f (Join-Path $RepoRoot 'release\contracts\validate-manifest.mjs'), $brokenManifestPath, $keys) -TailLines 2
    foreach ($line in $r.Tail) { Write-Note $line }
    if ($r.ExitCode -ne 0) { throw '故障 manifest 校验未通过' }
    # HTTP 分离签名约定 URL+.sig：broken manifest 的签名同步补 <名>.json.sig
    Copy-Item -LiteralPath (Join-Path $manifests '0.2.0-broken.sig') -Destination (Join-Path $manifests '0.2.0-broken.json.sig') -Force
    Write-Ok '0.2.0-broken manifest 签名 + 校验通过（0.2.0 > 0.1.0，严格升级语义成立）'
}

# ===========================================================================
# 主流程
# ===========================================================================
Write-Host "=== GameBot M2 升级/回滚 E2E（批次 3 合流门） ===" -ForegroundColor White
Write-Host "RepoRoot=$RepoRoot"; Write-Host "WorkDir =$WorkDir"
Write-Host "Scenario=$Scenario SkipBuild=$SkipBuild HttpPort=$HttpPort PortA=$PortA PortB=$PortB"
Write-Host "InstallRootA=$InstallRootA InstallRootB=$InstallRootB DataRootA=$DataRootA DataRootB=$DataRootB"

if (-not (Test-Path -LiteralPath $WorkDir)) { New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null }
New-Item -ItemType Directory -Path (Join-Path $WorkDir 'logs') -Force | Out-Null

if (-not $SkipBuild -and ($Scenario -eq 'all' -or $Scenario -eq 'build')) {
    Invoke-BuildAndPackage
}
if ($Scenario -eq 'build') {
    Write-Step '仅构建模式，结束'
    if ($script:Failures.Count -gt 0) { exit 1 } else { exit 0 }
}

# ---------- manifest 本机 HTTP 服务（python http.server，--directory） ----------
Write-Step "启动 manifest 本机 HTTP 服务（127.0.0.1:$HttpPort → WorkDir\manifests）"
Stop-StaleHttpServer -Port $HttpPort
$httpProc = Start-E2EProcess -FilePath 'python' `
    -ArgumentList @('-m', 'http.server', "$HttpPort", '--bind', '127.0.0.1', '--directory', (Join-Path $WorkDir 'manifests')) `
    -WorkingDirectory (Join-Path $WorkDir 'manifests') `
    -StdoutLog (Join-Path $WorkDir 'logs\http-server.log') -StderrLog (Join-Path $WorkDir 'logs\http-server.log')
$script:CleanupTargets.Add($httpProc) | Out-Null
$probeOk = $false
foreach ($attempt in @(1, 2, 3, 4, 5, 6, 7, 8, 9, 10)) {
    try {
        if ((Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$HttpPort/0.2.0.json" -TimeoutSec 2).StatusCode -eq 200) { $probeOk = $true; break }
    } catch { Start-Sleep -Seconds 1 }
}
Assert-True $probeOk "manifest HTTP 服务就绪（python http.server 绑定 127.0.0.1:$HttpPort）"
foreach ($f in @('0.2.0.json.sig', '0.2.0-broken.json', '0.2.0-broken.json.sig')) {
    $ok = $false
    try { $ok = ((Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$HttpPort/$f" -TimeoutSec 5).StatusCode -eq 200) } catch { }
    Assert-True $ok "manifest HTTP 服务可取 /$f（fetch_remote_manifest 按 URL 与 URL+.sig 成对获取）"
}

try {
    if ($Scenario -eq 'all' -or $Scenario -eq 'upgrade') { Invoke-Scenario -CandidateMustFail $false }
    if ($Scenario -eq 'all' -or $Scenario -eq 'rollback') { Invoke-Scenario -CandidateMustFail $true }
    if ($Scenario -eq 'all' -or $Scenario -eq 'identity') { Invoke-IdentityScenario }
} finally {
    Write-Step '清理（仅本 E2E 的进程与安装根内 server）'
    foreach ($p in $script:CleanupTargets) {
        Stop-E2EProcess -Process $p -Label 'cleanup'
        # 进程终止后管道读端才会 EOF：flush 已完成的输出任务到证据日志
        Save-ProcessOutput -Process $p
    }
    foreach ($r in $script:Roots) { Stop-RootServers -Root $r }
}

Write-Host "`n=== E2E 结果 ===" -ForegroundColor White
if ($script:Failures.Count -eq 0) {
    Write-Host 'ALL PASS' -ForegroundColor Green
    exit 0
} else {
    Write-Host "$($script:Failures.Count) FAIL：" -ForegroundColor Red
    $script:Failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}
