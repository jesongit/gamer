# Pure fixture: extract the read helpers only, never run the application entrypoint.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$source = Join-Path $PSScriptRoot 'apply-local.ps1'
$tokens = $null; $parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($source, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
foreach ($name in @('Api','Plugin','Get-WakeupMilliseconds','Read-Idle')) {
    $definition = @($ast.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true))
    if ($definition.Count -ne 1) { throw "Expected one read helper: $name" }
    Invoke-Expression $definition[0].Extent.Text
}
function Assert-Fixture([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "Fixture failed: $Message" }
}
function Assert-Rejected([scriptblock]$Action, [string]$Expected) {
    $failure = $null
    try { $null = & $Action } catch { $failure = $_.Exception.Message }
    Assert-Fixture ($failure -and $failure.Contains($Expected)) "Expected rejection: $Expected; actual: $failure"
}
$BaseUrl = 'http://127.0.0.1:1'
$headers = @{ 'X-Admin-Token' = 'fixture-only' }
$script:requests = [Collections.Generic.List[string]]::new()
$future = [DateTime]::UtcNow.AddHours(2)
$script:responses = @{
    '/health/ready' = [pscustomobject]@{ ready = $true }
    '/api/devices' = @([pscustomobject]@{ id = 'android-one' }, [pscustomobject]@{ id = 'android-two' })
    '/api/browser-targets' = @([pscustomobject]@{ id = 'browser-one' })
    '/api/devices/android-one/run' = [pscustomobject]@{ active = $false }
    '/api/devices/android-two/run' = [pscustomobject]@{ active = $false }
    '/api/devices/browser-one/run' = [pscustomobject]@{ active = $false }
    '/api/recording' = [pscustomobject]@{ sessions = @([pscustomobject]@{ state = 'completed' }) }
    '/api/tasks' = @(
        [pscustomobject]@{ id = 'future-one'; enabled = $true; next_wakeup = $future },
        [pscustomobject]@{ id = 'future-two'; enabled = $true; next_wakeup = $future.AddMinutes(1) },
        [pscustomobject]@{ id = 'future-three'; enabled = $true; next_wakeup = $future.AddMinutes(2) }
    )
    '/api/extensions' = [pscustomobject]@{ extensions = @(
        [pscustomobject]@{ id = 'gamer-yaml'; state = 'running'; last_error = $null },
        [pscustomobject]@{ id = 'gamer-live'; state = 'running'; last_error = $null },
        [pscustomobject]@{ id = 'gamer-notify'; state = 'running'; last_error = $null }
    ) }
    '/api/extensions/gamer-live/call:live.status' = [pscustomobject]@{
        stream = $null; automatic = [pscustomobject]@{ state = 'stopped'; queue = [pscustomobject]@{ current = $null; waiting = @() } }
    }
    '/api/extensions/gamer-notify/call:channels.read' = [pscustomobject]@{ default_channel = $null; channels = @() }
    '/api/extensions/gamer-notify/call:runs.read' = [pscustomobject]@{ version = 1; rules = @() }
}
function Invoke-RestMethod {
    param($Uri, $Headers, $TimeoutSec, $Method, $ContentType, $Body)
    $route = ([uri]$Uri).AbsolutePath
    if ($Method -eq 'POST') { $route += ':' + ($Body | ConvertFrom-Json).action }
    $script:requests.Add($route)
    if (-not $script:responses.ContainsKey($route)) { throw "Unexpected fixture request: $route" }
    # Match Invoke-RestMethod's JSON root-array NoEnumerate pipeline behavior.
    $response = $script:responses[$route]
    if ($response -is [Collections.IList]) { Write-Output -NoEnumerate $response }
    else { Write-Output $response }
}

$tasks = @(Api '/api/tasks')
Assert-Fixture ($tasks.Count -eq 3 -and $tasks[0].GetType().FullName -eq 'System.Management.Automation.PSCustomObject') 'Root arrays must yield scalar PSCustomObject rows'
Assert-Fixture ($tasks[0].next_wakeup -is [DateTime]) 'Real API DateTime must remain scalar'
Assert-Fixture ((Get-WakeupMilliseconds $tasks[0].next_wakeup) -eq ([DateTimeOffset]$future).ToUnixTimeMilliseconds()) 'DateTime wakeup conversion'
$recordings = Api '/api/recording'
Assert-Fixture ($recordings.GetType().FullName -eq 'System.Management.Automation.PSCustomObject' -and $recordings.sessions.Count -eq 1) 'Object response keeps nested arrays'
$script:responses['/api/tasks'] = @()
Assert-Fixture (@(Api '/api/tasks').Count -eq 0) 'Empty root array produces no rows'
$script:responses['/api/tasks'] = $tasks

$utc = '2026-10-15T04:30:00.000Z'
$offset = '2026-10-15T12:30:00.000+08:00'
$expected = [DateTimeOffset]::Parse($utc).ToUnixTimeMilliseconds()
Assert-Fixture ((Get-WakeupMilliseconds $utc) -eq $expected -and (Get-WakeupMilliseconds $offset) -eq $expected) 'ISO UTC and offset conversion'
Assert-Fixture ((Get-WakeupMilliseconds ([DateTimeOffset]::Parse($utc))) -eq $expected) 'DateTimeOffset conversion'
Assert-Fixture ((Get-WakeupMilliseconds ([long]($expected / 1000))) -eq $expected -and (Get-WakeupMilliseconds $expected) -eq $expected) 'Numeric seconds and milliseconds conversion'
foreach ($delta in @(-1,0,1)) {
    $wakeup = [DateTimeOffset]::FromUnixTimeMilliseconds($expected + $delta).ToString('O')
    Assert-Fixture ((Get-WakeupMilliseconds $wakeup) -eq ($expected + $delta)) 'ISO one-millisecond boundary'
}

$script:requests.Clear()
$idle = Read-Idle
Assert-Fixture ($idle.devices.Count -eq 2 -and $idle.browsers.Count -eq 1 -and $idle.tasks.Count -eq 3) 'Read-Idle retains root-array rows'
foreach ($id in @('android-one','android-two','browser-one')) {
    Assert-Fixture ($script:requests.Contains("/api/devices/$id/run")) "Run guard checked scalar target $id"
}
$live = $script:responses['/api/extensions/gamer-live/call:live.status']
$live.automatic.state = 'failed'
$null = Read-Idle
$live.stream = [pscustomobject]@{ state = 'failed'; owns_output = $true }
Assert-Rejected { Read-Idle } 'Live output/automatic session/queue is active'
$live.stream = $null
$script:responses['/api/devices/browser-one/run'].active = $true
Assert-Rejected { Read-Idle } 'Active run on target browser-one'
$script:responses['/api/devices/browser-one/run'].active = $false
$tasks[0].next_wakeup = [DateTime]::UtcNow.AddMinutes(5)
Assert-Rejected { Read-Idle } 'Task future-one is due within the maintenance window'
Write-Output 'apply-local pure fixtures passed: NoEnumerate root arrays, DateTime/ISO wakeups, scalar target guards, failed idle/output and due-task guards'
