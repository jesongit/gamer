# 只加载待测函数并模拟进程/API；不启动或停止真实服务，不读取管理令牌。
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $repoRoot 'gamer.ps1'), [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
$functions = @('Get-NormalizedProcessPath', 'Test-BackendProcess', 'Get-BackendProcs',
    'Test-BackendPortOwned', 'Get-FrontendProcs', 'Stop-Backend', 'Stop-Frontend')
foreach ($name in $functions) {
    $node = $ast.Find({ param($item)
        $item -is [Management.Automation.Language.FunctionDefinitionAst] -and $item.Name -eq $name
    }, $true)
    if (-not $node) { throw "Missing function: $name" }
    . ([scriptblock]::Create($node.Extent.Text))
}

$ServerDir = 'D:\QA 项目\gamer\server'
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

function Get-Process {
    param($Name, $Id, $ErrorAction)
    if ($PSBoundParameters.ContainsKey('Id')) {
        $script:Processes | Where-Object { $_.Id -eq $Id }
    } else {
        $script:Processes | Where-Object { $_.ProcessName -like $Name }
    }
}
function Get-NetTCPConnection {
    param($LocalPort, $State, $ErrorAction)
    $script:Listeners | Where-Object { $_.LocalPort -eq $LocalPort }
}
function Get-CimInstance { param($ClassName, $Filter, $ErrorAction) $script:CimRows }
function Stop-Process {
    param($Id, [switch]$Force, $ErrorAction)
    $script:Stopped += $Id
    $script:Processes = @($script:Processes | Where-Object { $_.Id -ne $Id })
    $script:Listeners = @($script:Listeners | Where-Object { $_.OwningProcess -ne $Id })
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
function Proc { param($Id, $Path, $Name = 'gamer-server')
    [pscustomobject]@{Id=$Id; Path=$Path; ProcessName=$Name}
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
    [pscustomobject]@{ProcessId=11; CommandLine='node.exe "D:/QA 项目/gamer/web/node_modules/vite/bin/vite.js"'},
    [pscustomobject]@{ProcessId=12; CommandLine='node.exe "D:\other\web\node_modules\vite\bin\vite.js"'},
    [pscustomobject]@{ProcessId=13; CommandLine='node.exe unrelated.js "D:\QA 项目\gamer\web\node_modules\vite\bin\vite.js"'},
    [pscustomobject]@{ProcessId=14; CommandLine=$null}
)
$script:Listeners = @([pscustomobject]@{LocalPort=5173; OwningProcess=12})
Assert ((@(Get-FrontendProcs).Id -join ',') -eq '11') 'Only the exact local Vite entrypoint belongs to this repo'
$script:Stopped = @()
$null = Stop-Frontend
Assert (($script:Stopped -join ',') -eq '11') 'Foreign Vite, arbitrary node and unreadable command lines must survive'
Write-Host 'PASS: repository process ownership, shutdown API, shared ADB and Vite isolation'
