# 只加载待测函数并模拟进程/API；不启动或停止真实服务，不读取管理令牌。
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $repoRoot 'gamer.ps1'), [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
$functions = @('Get-DataDir', 'Get-NormalizedProcessPath', 'Test-BackendProcess', 'Get-BackendProcs',
    'Test-BackendPortOwned', 'Get-FrontendProcs', 'Get-BrowserProcessProfilePath',
    'Test-BackendBrowserProfile', 'Get-BrowserProcessFamily', 'Get-BackendBrowserSnapshot', 'Close-BackendBrowsers',
    'Stop-Backend', 'Stop-Frontend', 'Start-BackgroundProcess')
foreach ($name in $functions) {
    $node = $ast.Find({ param($item)
        $item -is [Management.Automation.Language.FunctionDefinitionAst] -and $item.Name -eq $name
    }, $true)
    if (-not $node) { throw "Missing function: $name" }
    . ([scriptblock]::Create($node.Extent.Text))
}

$ServerDir = 'D:\QA 项目\gamer\server'
$DataDir = Join-Path $ServerDir 'data'
$WebDir = 'D:\QA 项目\gamer\web'
$BackendName = 'gamer-server'
$Port = 8443
$FrontendPort = 5173
$script:AdminToken = 'test-only-not-a-real-token'
$script:Processes = @()
$script:Listeners = @()
$script:CimRows = @()
$script:Stopped = @()
$script:ShutdownCalls = 0
$script:AdbResets = 0
$script:CimHook = $null
$script:AfterStop = $null
$script:UnstoppableIds = @()
$script:WaitTimeouts = @()
$script:CdpCalls = @()
$script:CdpHook = $null

function Get-Process {
    param($Name, $Id, $ErrorAction)
    if ($PSBoundParameters.ContainsKey('Id')) {
        $script:Processes | Where-Object { $_.Id -eq $Id }
    } else {
        $script:Processes | Where-Object { $_.ProcessName -like $Name }
    }
}
function Get-NetTCPConnection {
    param($LocalPort, $OwningProcess, $State, $ErrorAction)
    $script:Listeners | Where-Object {
        (-not $PSBoundParameters.ContainsKey('LocalPort') -or $_.LocalPort -eq $LocalPort) -and
        (-not $PSBoundParameters.ContainsKey('OwningProcess') -or $_.OwningProcess -eq $OwningProcess)
    }
}
function Get-CimInstance {
    param($ClassName, $Filter, $ErrorAction)
    if ($script:CimHook) { & $script:CimHook $Filter }
    if ($Filter -match '^ProcessId\s*=\s*(\d+)$') {
        $script:CimRows | Where-Object { $_.ProcessId -eq [int]$Matches[1] }
    } elseif ($Filter -match "^Name\s*=\s*'([^']+)'$") {
        $script:CimRows | Where-Object { $_.Name -eq $Matches[1] }
    } else {
        $script:CimRows
    }
}
function Stop-Process {
    param($Id, [switch]$Force, $ErrorAction)
    $script:Stopped += $Id
    if ($Id -in $script:UnstoppableIds) { return }
    $script:Processes = @($script:Processes | Where-Object { $_.Id -ne $Id })
    $script:CimRows = @($script:CimRows | Where-Object { $_.ProcessId -ne $Id })
    $script:Listeners = @($script:Listeners | Where-Object { $_.OwningProcess -ne $Id })
    if ($script:AfterStop) { & $script:AfterStop $Id }
}
function Invoke-RestMethod {
    param($Uri, $TimeoutSec, $ErrorAction)
    $script:CdpCalls += [pscustomobject]@{Uri=$Uri; TimeoutSec=$TimeoutSec}
    if ($script:CdpHook) { & $script:CdpHook }
    # 不允许测试访问真实 CDP；返回外部 URI，确认关闭逻辑会拒绝其 WebSocket。
    @{webSocketDebuggerUrl='ws://example.invalid:9222/devtools/browser/foreign'}
}
function Start-Sleep { param($Milliseconds, $Seconds) }
function Test-PortListening { param($p) @($script:Listeners | Where-Object LocalPort -eq $p).Count -gt 0 }
function Get-PortOwner { param($p) 'simulated external listener' }
function Reset-AdbServer { $script:AdbResets++ }
function curl.exe {
    $script:ShutdownCalls++
    foreach ($process in @(Get-BackendProcs)) { Stop-Process -Id $process.Id }
}
function Assert { param([bool]$Condition, [string]$Message) if (-not $Condition) { throw $Message } }
function Proc { param($Id, $Path, $Name = 'gamer-server', $Started = [datetime]'2026-10-17T08:00:00')
    $process = [pscustomobject]@{
        Id=$Id; Path=$Path; ProcessName=$Name; StartTime=$Started
        Handle=[IntPtr]1; HasExited=$false; ExitTime=$null; Disposed=$false
    }
    $process | Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {
        param($Timeout)
        $script:WaitTimeouts += $Timeout
        return $false
    }
    $process | Add-Member -MemberType ScriptMethod -Name Dispose -Value { $this.Disposed = $true }
    $process
}

$debug = Proc 1 "$ServerDir\target\debug\gamer-server.exe"
$release = Proc 2 '\\?\D:\QA 项目\GAMER\server\target\release\gamer-server.exe'
$external = Proc 3 'D:\Apps\Gamer\versions\0.2.0\gamer-server.exe'
$otherRepo = Proc 4 'D:\other\server\target\debug\gamer-server.exe'
$unknown = Proc 5 $null
$prefix = Proc 6 "$ServerDir\target\debug\gamer-server.exe.backup"
$script:Processes = @($debug, $release, $external, $otherRepo, $unknown, $prefix)
Assert ((@(Get-BackendProcs).Id -join ',') -eq '1,2') 'Only this repo debug/release belong to the script'
Assert (-not (Test-BackendPortOwned)) 'No listener must not permit shutdown API'
$script:Listeners = @([pscustomobject]@{LocalPort=8443; OwningProcess=3})
Assert (-not (Test-BackendPortOwned)) 'External port owner must not receive shutdown API'
$null = Stop-Backend
Assert (($script:Stopped -join ',') -eq '1,2') 'Stop must preserve other installs and unknown paths'
Assert ($script:ShutdownCalls -eq 0) 'Do not send the management token to an external listener'
Assert ($script:AdbResets -eq 0) 'Other Gamer instances must keep their shared ADB service'
$null = Stop-Backend
Assert (($script:Stopped -join ',') -eq '1,2') 'External-only stop must be a no-op'

$script:Processes = @($debug)
$script:Listeners = @([pscustomobject]@{LocalPort=8443; OwningProcess=1})
Assert (Test-BackendPortOwned) 'Owned port should permit graceful shutdown'
$null = Stop-Backend
Assert ($script:ShutdownCalls -eq 1) 'Owned server must use graceful shutdown'
Assert ($script:AdbResets -eq 1) 'ADB reset remains available when no other Gamer is running'

$node1 = Proc 11 'C:\node\node.exe' 'node'
$node2 = Proc 12 'C:\node\node.exe' 'node'
$node3 = Proc 13 'C:\node\node.exe' 'node'
$node4 = Proc 14 'C:\node\node.exe' 'node'
$script:Processes = @($node1, $node2, $node3, $node4)
$script:CimRows = @(
    [pscustomobject]@{ProcessId=11; Name='node.exe'; CommandLine='node.exe "D:/QA 项目/gamer/web/node_modules/vite/bin/vite.js"'},
    [pscustomobject]@{ProcessId=12; Name='node.exe'; CommandLine='node.exe "D:\other\web\node_modules\vite\bin\vite.js"'},
    [pscustomobject]@{ProcessId=13; Name='node.exe'; CommandLine='node.exe unrelated.js "D:\QA 项目\gamer\web\node_modules\vite\bin\vite.js"'},
    [pscustomobject]@{ProcessId=14; Name='node.exe'; CommandLine=$null}
)
$script:Listeners = @([pscustomobject]@{LocalPort=5173; OwningProcess=12})
Assert ((@(Get-FrontendProcs).Id -join ',') -eq '11') 'Only the exact local Vite entrypoint belongs to this repo'
$script:Stopped = @()
$null = Stop-Frontend
Assert (($script:Stopped -join ',') -eq '11') 'Foreign Vite, arbitrary node and unreadable command lines must survive'
Write-Host 'PASS: repository process ownership, shutdown API, shared ADB and Vite isolation'

function BrowserRow {
    param($Id, $Parent, $CommandLine, $Name = 'chrome.exe', $Created = [datetime]'2026-10-17T08:01:00')
    [pscustomobject]@{
        ProcessId=$Id; ParentProcessId=$Parent; Name=$Name
        CreationDate=$Created; CommandLine=$CommandLine
    }
}
function BrowserCommand {
    param($Profile)
    '"C:\Program Files\Google\Chrome\Application\chrome.exe" --headless=new --remote-debugging-port=0 --remote-debugging-address=127.0.0.1 --user-data-dir="' + $Profile + '"'
}
function Set-BrowserInventory {
    param($Rows, $Parents = @())
    $script:CimRows = @($Rows)
    $script:Processes = @($Parents) + @($Rows | ForEach-Object {
        Proc $_.ProcessId ('C:\Browsers\' + $_.Name) ([IO.Path]::GetFileNameWithoutExtension($_.Name)) $_.CreationDate
    })
    $script:Listeners = @()
    $script:Stopped = @()
    $script:CimHook = $null
    $script:AfterStop = $null
    $script:UnstoppableIds = @()
    $script:WaitTimeouts = @()
    $script:CdpCalls = @()
    $script:CdpHook = $null
}

$profileOne = Join-Path $DataDir 'browser-profiles\browser-1'
$profileTwo = Join-Path $DataDir 'browser-profiles\browser-2'
$browserRoot = BrowserRow 101 1 (BrowserCommand $profileOne)
$browserChild = BrowserRow 102 101 'chrome.exe --type=renderer --renderer-client-id=4'
Assert ((Get-BrowserProcessProfilePath $browserRoot) -eq $profileOne) 'Quoted profile values must preserve Chinese characters and spaces'
Assert (Test-BackendBrowserProfile $browserRoot) 'Only a direct profile under the configured data directory should match'
$edgeRoot = BrowserRow 103 999 ('msedge.exe --headless=new --remote-debugging-port=0 --remote-debugging-address=127.0.0.1 "--user-data-dir=\\?\D:\QA 项目\GAMER\server\data\browser-profiles\browser-2"') 'msedge.exe'
Assert ((Get-BrowserProcessProfilePath $edgeRoot) -eq $profileTwo) 'A fully quoted argument must support extended Windows paths and case differences'
Assert (Test-BackendBrowserProfile $edgeRoot) 'Normalized Edge profile must match the configured data directory'

# 后端正常退出后，仍清理停止前捕获的所属 Chrome 根进程及不带 profile 的子进程。
Set-BrowserInventory @($browserRoot, $browserChild) @($debug)
$script:Listeners = @([pscustomobject]@{LocalPort=8443; OwningProcess=1})
$shutdownBefore = $script:ShutdownCalls
$null = Stop-Backend
Assert ($script:ShutdownCalls -eq ($shutdownBefore + 1)) 'Owned backend must still receive graceful shutdown'
Assert (($script:Stopped | Sort-Object) -join ',' -eq '1,101,102') 'Backend exit must clean its captured browser family'
Assert (@($script:Processes).Count -eq 0) 'No captured browser family should survive normal backend stop'

# 后端此前已退出：严格 profile 识别 Chrome / Edge 孤儿，不依赖父 PID 仍存在。
Set-BrowserInventory @($browserRoot, $browserChild, $edgeRoot)
$snapshots = @(Get-BackendBrowserSnapshot @())
Assert ($snapshots.Count -eq 2) 'Both Chrome and Edge orphan browser roots should be recoverable'
Assert ((@($snapshots | ForEach-Object { $_.Family.ProcessId }) | Sort-Object) -join ',' -eq '101,102,103') 'Orphan snapshots must include browser descendants'
$shutdownBefore = $script:ShutdownCalls
$adbBefore = $script:AdbResets
$null = Stop-Backend
Assert (($script:Stopped | Sort-Object) -join ',' -eq '101,102,103') 'Stop without a backend must clean owned orphan browser families'
Assert ($script:ShutdownCalls -eq $shutdownBefore) 'Orphan cleanup must not send a backend shutdown request'
Assert ($script:AdbResets -eq $adbBefore) 'Orphan-only cleanup must not reset the shared ADB server'

# 根进程先退出后，带当前 profile 的 typed 子进程仍可独立作为残留清理。
$typedOrphan = BrowserRow 104 999 ('chrome.exe --type=utility --user-data-dir="' + $profileOne + '"')
Set-BrowserInventory @($typedOrphan)
$snapshots = @(Get-BackendBrowserSnapshot @())
Assert ($snapshots.Count -eq 1 -and $null -eq $snapshots[0].Root) 'A typed orphan must not be treated as a CDP browser root'
Assert (($snapshots[0].Family.ProcessId -join ',') -eq '104') 'Typed orphan snapshot must preserve its process identity'
Close-BackendBrowsers $snapshots
Assert (($script:Stopped -join ',') -eq '104') 'A typed orphan with the current profile must be cleaned'

$badDuplicate = BrowserRow 201 999 ((BrowserCommand $profileOne) + ' --user-data-dir="' + $profileTwo + '"')
$badRelative = BrowserRow 202 999 (BrowserCommand '.\browser-profiles\relative')
$badNeighbor = BrowserRow 203 999 (BrowserCommand (Join-Path $DataDir 'browser-profiles-other\other'))
$badNested = BrowserRow 204 999 (BrowserCommand (Join-Path $DataDir 'browser-profiles\parent\nested'))
$badExternal = BrowserRow 205 999 (BrowserCommand 'D:\other\data\browser-profiles\other')
$badPersonal = BrowserRow 206 999 (BrowserCommand 'C:\Users\tester\AppData\Local\Google\Chrome\User Data')
$badInteractive = BrowserRow 207 999 ((BrowserCommand $profileOne).Replace('--headless=new ', ''))
$badDebugPort = BrowserRow 208 999 ((BrowserCommand $profileOne).Replace('--remote-debugging-port=0 ', ''))
$badDebugAddress = BrowserRow 209 999 ((BrowserCommand $profileOne).Replace('--remote-debugging-address=127.0.0.1', '--remote-debugging-address=0.0.0.0'))
$badLiveParent = BrowserRow 210 700 (BrowserCommand $profileOne)
$badUnknownParentTime = BrowserRow 212 701 (BrowserCommand $profileOne)
Assert ($null -eq (Get-BrowserProcessProfilePath $badDuplicate)) 'Duplicate user-data-dir flags must fail closed'
Assert ($null -eq (Get-BrowserProcessProfilePath $badRelative)) 'A relative profile must not be resolved against the working directory'
Assert (-not (Test-BackendBrowserProfile $badNeighbor)) 'A neighboring profile prefix is outside this repository'
Assert (-not (Test-BackendBrowserProfile $badNested)) 'Only a single profile directory below browser-profiles is owned'
$foreignParent = Proc 700 'C:\Apps\launcher.exe' 'launcher'
$unknownParent = Proc 701 'C:\Apps\launcher.exe' 'launcher' $null
$negativeRows = @($badDuplicate, $badRelative, $badNeighbor, $badNested, $badExternal,
    $badPersonal, $badInteractive, $badDebugPort, $badDebugAddress, $badLiveParent,
    $badUnknownParentTime)
Set-BrowserInventory $negativeRows @($debug, $foreignParent, $unknownParent)
$snapshots = @(Get-BackendBrowserSnapshot @($debug))
Assert ($snapshots.Count -eq 0) 'Foreign profiles, invalid flags, interactive browsers and live unrelated parents must survive'
Close-BackendBrowsers $snapshots
Assert (@($script:Stopped).Count -eq 0) 'Unowned browser inventory must not be terminated'

# 父 PID 已复用且新进程创建更晚，不能阻碍真正的 profile 孤儿回收。
$reusedParent = Proc 1 'C:\Apps\launcher.exe' 'launcher' ([datetime]'2026-10-17T08:10:00')
Set-BrowserInventory @($browserRoot) @($reusedParent)
$snapshots = @(Get-BackendBrowserSnapshot @())
Assert ($snapshots.Count -eq 1) 'A newer reused parent PID must not hide an orphan browser'
Close-BackendBrowsers $snapshots
Assert (($script:Stopped -join ',') -eq '101') 'Reused parent PID must survive orphan cleanup'
Assert (@($script:Processes | Where-Object Id -eq 1).Count -eq 1) 'Cleanup must preserve the new parent process'

# 快照之后浏览器 PID 复用，创建时间或命令行任一变化都不得强杀新身份。
foreach ($identityChange in @('CreationDate', 'CommandLine')) {
    Set-BrowserInventory @($browserRoot)
    $snapshots = @(Get-BackendBrowserSnapshot @())
    $replacement = BrowserRow 101 999 $browserRoot.CommandLine
    if ($identityChange -eq 'CreationDate') {
        $replacement.CreationDate = ([datetime]$replacement.CreationDate).AddMinutes(1)
    } else {
        $replacement.CommandLine += ' --new-process-identity'
    }
    Set-BrowserInventory @($replacement)
    Close-BackendBrowsers $snapshots
    Assert (@($script:Stopped).Count -eq 0) ("Browser PID reuse must compare {0} before forced cleanup" -f $identityChange)
}

# 停机快照完成后才派生的 renderer 也要在根进程消失前重扫纳入清理。
Set-BrowserInventory @($browserRoot)
$snapshots = @(Get-BackendBrowserSnapshot @())
$lateChild = BrowserRow 105 101 'chrome.exe --type=renderer --renderer-client-id=5' 'chrome.exe' ([datetime]'2026-10-17T08:02:00')
$script:CimRows += $lateChild
$script:Processes += Proc 105 'C:\Browsers\chrome.exe' 'chrome' $lateChild.CreationDate
Close-BackendBrowsers $snapshots
Assert (($script:Stopped | Sort-Object) -join ',' -eq '101,105') 'Cleanup must re-scan descendants created after the initial snapshot'

# 外来新根 PID 的后代不能通过旧 family 快照被误认领。
Set-BrowserInventory @($browserRoot)
$snapshots = @(Get-BackendBrowserSnapshot @())
$newRoot = BrowserRow 101 999 ((BrowserCommand $profileOne) + ' --another-owner') 'chrome.exe' ([datetime]'2026-10-17T08:03:00')
$foreignChild = BrowserRow 106 101 'chrome.exe --type=renderer' 'chrome.exe' ([datetime]'2026-10-17T08:04:00')
Set-BrowserInventory @($newRoot, $foreignChild)
Close-BackendBrowsers $snapshots
Assert (@($script:Stopped).Count -eq 0) 'A reused root PID must not pull unrelated new children into the captured family'

# 已核验句柄报告旧根退出时间后，仍可补收 CDP 等待期间新生且不带 profile 的 renderer。
$exitBoundary = [datetime]'2026-10-17T08:01:10'
$duringCloseChild = BrowserRow 108 101 'chrome.exe --type=renderer --renderer-client-id=8' 'chrome.exe' ([datetime]'2026-10-17T08:01:05')
$atCloseChild = BrowserRow 109 101 'chrome.exe --type=renderer --renderer-client-id=9' 'chrome.exe' $exitBoundary
$afterCloseChild = BrowserRow 110 101 'chrome.exe --type=renderer --renderer-client-id=10' 'chrome.exe' ([datetime]'2026-10-17T08:01:11')
$exitBounds = @{101=$exitBoundary}
$family = @(Get-BrowserProcessFamily @($browserRoot) @($duringCloseChild))
Assert (($family.ProcessId -join ',') -eq '101') 'An absent parent without a verified exit boundary must not claim new descendants'
$family = @(Get-BrowserProcessFamily @($browserRoot) @($duringCloseChild, $atCloseChild, $afterCloseChild) $exitBounds)
Assert (($family.ProcessId | Sort-Object) -join ',' -eq '101,108,109') 'An exited browser may claim descendants born through its verified exit time only'
$reusedRootAfterExit = BrowserRow 101 999 ((BrowserCommand $profileOne) + ' --another-owner') 'chrome.exe' ([datetime]'2026-10-17T08:01:11')
$family = @(Get-BrowserProcessFamily @($browserRoot) @($reusedRootAfterExit, $duringCloseChild, $afterCloseChild) $exitBounds)
Assert (($family.ProcessId | Sort-Object) -join ',' -eq '101,108') 'A reused root PID may not expand the old browser family beyond its verified exit time'

# 只请求同根 PID 的 loopback CDP，并限制 HTTP 等待；外部 WebSocket 响应不能被连接。
Set-BrowserInventory @($browserRoot)
$script:Listeners = @(
    [pscustomobject]@{LocalPort=9222; OwningProcess=101; LocalAddress='127.0.0.1'},
    [pscustomobject]@{LocalPort=9223; OwningProcess=101; LocalAddress='0.0.0.0'},
    [pscustomobject]@{LocalPort=9224; OwningProcess=999; LocalAddress='127.0.0.1'}
)
Close-BackendBrowsers @(Get-BackendBrowserSnapshot @())
Assert ($script:CdpCalls.Count -eq 1 -and $script:CdpCalls[0].Uri -eq 'http://127.0.0.1:9222/json/version') 'Only the captured browser loopback CDP listener may be queried'
Assert ($script:CdpCalls[0].TimeoutSec -eq 2) 'CDP endpoint discovery must use a bounded HTTP timeout'
Assert (($script:Stopped -join ',') -eq '101') 'An invalid CDP WebSocket response must fall back to owned process cleanup'

# 模拟 CDP 查询期间根进程退出并复用 PID：补收旧根退出前新生且无 profile 的 child。
Set-BrowserInventory @($browserRoot)
$closingProcess = @($script:Processes | Where-Object Id -eq 101)[0]
$script:Listeners = @([pscustomobject]@{LocalPort=9222; OwningProcess=101; LocalAddress='127.0.0.1'})
$script:CdpHook = {
    $closingProcess.HasExited = $true
    $closingProcess.ExitTime = $exitBoundary
    $script:CimRows = @($reusedRootAfterExit, $duringCloseChild, $afterCloseChild)
    $script:Processes = @($script:CimRows | ForEach-Object {
        Proc $_.ProcessId 'C:\Browsers\chrome.exe' 'chrome' $_.CreationDate
    })
    $script:CdpHook = $null
}
Close-BackendBrowsers @(Get-BackendBrowserSnapshot @())
Assert (($script:Stopped -join ',') -eq '108') 'CDP-time root exit must clean its new renderer and preserve a reused root PID and its children'
Assert ($closingProcess.Disposed) 'The captured browser process handle must be disposed after reading its exit boundary'
Assert (($script:Processes.Id | Sort-Object) -join ',' -eq '101,110') 'New process identities after the old root exit must survive cleanup'

# 根进程正在退出时新留下的 typed profile 孤儿，通过停止流程的末次扫描补收。
Set-BrowserInventory @($browserRoot)
$script:AfterStop = {
    param($StoppedId)
    if ($StoppedId -eq 101) {
        $orphan = BrowserRow 107 101 ('chrome.exe --type=utility --user-data-dir="' + $profileOne + '"')
        $script:CimRows += $orphan
        $script:Processes += Proc 107 'C:\Browsers\chrome.exe' 'chrome' $orphan.CreationDate
        $script:AfterStop = $null
    }
}
$null = Stop-Backend
Assert (($script:Stopped -join ',') -eq '101,107') 'A typed profile orphan appearing during root exit must be cleaned by the final scan'

# 强杀失败必须显式失败，不能把旧 profile 仍占用的状态报告为已停止。
Set-BrowserInventory @($browserRoot)
$script:UnstoppableIds = @(101)
$failure = $null
try { Close-BackendBrowsers @(Get-BackendBrowserSnapshot @()) } catch { $failure = $_ }
Assert ($null -ne $failure -and $failure.Exception.Message -match '101') 'A browser that refuses termination must report its PID'
Assert (($script:WaitTimeouts -join ',') -eq '2000') 'Forced browser exit must use a bounded wait without blocking the mock test'
Set-BrowserInventory @()
Write-Host 'PASS: owned browser shutdown, orphan recovery, profile isolation, PID reuse, late descendants and bounded cleanup'

# 与服务端一致：环境目录优先；相对 data_dir 以配置文件目录为基准。
$previousDataDir = $env:GAMER_DATA_DIR
$previousConfigFile = $ConfigFile
$configTestDirectory = Join-Path ([IO.Path]::GetTempPath()) ('gamer-process-scope-' + [guid]::NewGuid().ToString('N'))
try {
    $null = New-Item -ItemType Directory -Path $configTestDirectory
    $ConfigFile = Join-Path $configTestDirectory 'custom.toml'
    Set-Content -LiteralPath $ConfigFile -Encoding UTF8 -Value 'data_dir = "custom data"'
    $env:GAMER_DATA_DIR = $null
    Assert ((Get-DataDir) -eq (Join-Path $configTestDirectory 'custom data')) 'Relative data_dir must resolve beside the selected config file'
    $env:GAMER_DATA_DIR = 'D:\isolated browser data'
    Assert ((Get-DataDir) -eq $env:GAMER_DATA_DIR) 'The server data directory environment override must take precedence'
    $env:GAMER_DATA_DIR = 'environment data'
    Assert ((Get-DataDir) -eq (Join-Path $configTestDirectory 'environment data')) 'Relative environment data directory must resolve beside the config file'
    $env:GAMER_DATA_DIR = $null
    Remove-Item -LiteralPath $ConfigFile
    Assert ((Get-DataDir) -eq (Join-Path $configTestDirectory 'data')) 'Missing config must retain the config-relative default data directory'
} finally {
    $env:GAMER_DATA_DIR = $previousDataDir
    if (Test-Path -LiteralPath $ConfigFile) { Remove-Item -LiteralPath $ConfigFile }
    if (Test-Path -LiteralPath $configTestDirectory) { Remove-Item -LiteralPath $configTestDirectory }
    $ConfigFile = $previousConfigFile
}
Write-Host 'PASS: configured and environment data directory resolution'

function New-CimInstance {
    param($ClassName, $Property, [switch]$ClientOnly)
    $Property
}
function Invoke-CimMethod {
    param($ClassName, $MethodName, $Arguments)
    $script:BackgroundArguments = $Arguments
    @{ ReturnValue = 0; ProcessId = 123 }
}
$previousProbe = $env:GAMER_ENV_PROBE
try {
    # 引号、美元符号和等号必须原样传递，不能作为命令执行。
    $env:GAMER_ENV_PROBE = 'value="quoted"; $()=''literal''=中文'
    $null = Start-BackgroundProcess -FilePath 'server.exe' -WorkingDirectory 'D:\gamer' `
        -OutputFile 'out.log' -ErrorFile 'err.log' -InputFile 'empty.txt'
    $startup = $script:BackgroundArguments.ProcessStartupInformation
    Assert ($startup.ShowWindow -eq 0) 'Background processes must remain hidden'
    Assert ($startup.EnvironmentVariables -contains "GAMER_ENV_PROBE=$env:GAMER_ENV_PROBE") 'WMI child must inherit current environment literally'
    Assert ($startup.EnvironmentVariables -contains "PATH=$env:PATH" -or $startup.EnvironmentVariables -contains "Path=$env:PATH") 'Preserve executable discovery'
    Assert (-not $script:BackgroundArguments.CommandLine.Contains($env:GAMER_ENV_PROBE)) 'Environment values must not be shell command text'
} finally {
    $env:GAMER_ENV_PROBE = $previousProbe
}
Write-Host 'PASS: hidden WMI startup inherits environment without shell interpolation'
