# Manual application draft. Default is read-only preflight; only a human should use -Apply.
# Do not execute shutdown/restart through Codex after an approval/policy denial.
[CmdletBinding()]
param(
    [switch]$Apply,
    [string]$CandidatePath,
    [string]$EvidencePath = (Join-Path $PSScriptRoot '../../server/target/live-daily-qa/apply-plan.json'),
    [string]$BaseUrl = 'http://127.0.0.1:8443',
    [string]$DataDirectory,
    [string]$TokenFile = (Join-Path $env:LOCALAPPDATA 'gamer/admin-token')
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$dataRoot = if ($DataDirectory) { [IO.Path]::GetFullPath($DataDirectory) } else { Join-Path $repoRoot 'server/data' }
$workflowRoot = Join-Path $repoRoot 'server/data/live-workflow-20261015'
$binaryPath = Join-Path $repoRoot 'server/target/debug/gamer-server.exe'
$controller = Join-Path $repoRoot 'gamer.ps1'
$configure = Join-Path $PSScriptRoot 'configure-local.mjs'
$uri = [uri]$BaseUrl
if ($uri.Scheme -ne 'http' -or $uri.Host -ne '127.0.0.1' -or $uri.UserInfo -or $uri.AbsolutePath -ne '/' -or $uri.Query) { throw 'Only a plain loopback HTTP base URL is allowed' }
$BaseUrl = $BaseUrl.TrimEnd('/')
$adminToken = if ($env:GAMER_ADMIN_TOKEN) { $env:GAMER_ADMIN_TOKEN.Trim() } else { [IO.File]::ReadAllText($TokenFile).Trim() }
if (-not $adminToken) { throw 'Missing existing administrator token' }
$headers = @{ 'X-Admin-Token' = $adminToken }
function Assert-WorkspacePath([string]$Path) {
    $absolute = [IO.Path]::GetFullPath($Path)
    if (-not $absolute.StartsWith($repoRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw "Path escapes workspace: $absolute" }
    $cursor = $absolute
    while ($cursor -and $cursor.Length -ge $repoRoot.Length) {
        if (Test-Path -LiteralPath $cursor) {
            if ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Reparse path is not allowed: $cursor" }
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
    return $absolute
}
function Assert-Tree([string]$Path) {
    $null = Assert-WorkspacePath $Path
    if (Test-Path -LiteralPath $Path) {
        $links = @(Get-ChildItem -LiteralPath $Path -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint })
        if ($links.Count) { throw "Reparse entries in backup/restore tree: $Path" }
    }
}
function Api([string]$Path, $Body = $null) {
    $args = @{ Uri = $BaseUrl + $Path; Headers = $headers; TimeoutSec = 60; Method = 'GET' }
    if ($null -ne $Body) { $args.Method = 'POST'; $args.ContentType = 'application/json; charset=utf-8'; $args.Body = ConvertTo-Json -InputObject $Body -Depth 50 -Compress }
    $response = Invoke-RestMethod @args
    # Invoke-RestMethod writes JSON root arrays with NoEnumerate. Explicitly
    # emit their elements so @(Api ...) has one row per task/target.
    if ($response -is [System.Collections.IList]) {
        foreach ($row in $response) { Write-Output $row }
    } else { return $response }
}
function Plugin([string]$Id, [string]$Action, $Values = @{}) { return Api "/api/extensions/$Id/call" @{ action = $Action; values = $Values } }
function Get-OwnedListener {
    $listeners = @(Get-NetTCPConnection -LocalPort $uri.Port -State Listen -ErrorAction SilentlyContinue)
    if (-not $listeners.Count) { throw 'Expected service is not listening' }
    foreach ($listener in $listeners) {
        $process = Get-Process -Id $listener.OwningProcess -ErrorAction Stop
        if (-not [string]::Equals([IO.Path]::GetFullPath($process.Path), $binaryPath, [StringComparison]::OrdinalIgnoreCase)) { throw 'Listener is not this checkout debug server; do not stop it' }
    }
}
function Stable-Targets($Devices, $Browsers) {
    return @{ devices = @($Devices | Sort-Object id | Select-Object id,name,addr,screen_mode,vd_res,vd_dpi,pkg,fps); browsers = @($Browsers | Sort-Object id | Select-Object id,name,url,profile_id,width,height) } | ConvertTo-Json -Depth 20 -Compress
}
function Get-WakeupMilliseconds($Value) {
    if ($Value -is [DateTimeOffset]) { return $Value.ToUnixTimeMilliseconds() }
    if ($Value -is [datetime]) {
        if ($Value.Kind -eq [DateTimeKind]::Unspecified) { $Value=[datetime]::SpecifyKind($Value,[DateTimeKind]::Utc) }
        return ([DateTimeOffset]$Value).ToUnixTimeMilliseconds()
    }
    if ($Value -is [string]) { return [DateTimeOffset]::Parse($Value,[Globalization.CultureInfo]::InvariantCulture,[Globalization.DateTimeStyles]::AssumeUniversal).ToUnixTimeMilliseconds() }
    $number=[long]$Value
    if ($number -lt 100000000000) { $number *= 1000 }
    return $number
}
function Read-Idle {
    $null = Api '/health/ready'
    $devices = @(Api '/api/devices')
    $browsers = @(Api '/api/browser-targets')
    foreach ($target in @($devices) + @($browsers)) {
        if ((Api "/api/devices/$([uri]::EscapeDataString($target.id))/run").active) { throw "Active run on target $($target.id); wait for completion" }
    }
    $recordings = Api '/api/recording'
    if (@($recordings.sessions | Where-Object { $_.state -in @('recording','finalizing') }).Count) { throw 'Recording is active/finalizing; wait for completion' }
    $live = Plugin 'gamer-live' 'live.status'
    if ($live.stream -or $live.automatic.state -notin @('stopped','failed') -or $live.automatic.queue.current -or @($live.automatic.queue.waiting).Count) { throw 'Live output/automatic session/queue is active; stop it manually before maintenance' }
    $tasks = @(Api '/api/tasks')
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    foreach ($task in $tasks | Where-Object { $_.enabled -and $null -ne $_.next_wakeup }) {
        $next = Get-WakeupMilliseconds $task.next_wakeup
        if ($next -le $now + 900000) { throw "Task $($task.id) is due within the maintenance window; wait or adjust its schedule manually" }
    }
    $extensions = (Api '/api/extensions').extensions
    foreach ($id in @('gamer-yaml','gamer-live','gamer-notify')) {
        $extension = $extensions | Where-Object id -eq $id
        if (-not $extension -or $extension.state -ne 'running' -or $extension.last_error) { throw "Required installed plugin is not healthy: $id" }
    }
    return @{ devices=$devices; browsers=$browsers; live=$live; tasks=$tasks; extensions=$extensions; channels=(Plugin 'gamer-notify' 'channels.read'); notifications=(Plugin 'gamer-notify' 'runs.read') }
}
function Run-Configuration([bool]$DryRun, [string]$BackupDirectory) {
    $oldBase = $env:GAMER_WORKFLOW_BASE; $oldTokenFile = $env:GAMER_WORKFLOW_TOKEN_FILE; $oldDry = $env:GAMER_WORKFLOW_DRY_RUN; $oldBackup = $env:GAMER_WORKFLOW_BACKUP_DIR; $oldAdmin = $env:GAMER_ADMIN_TOKEN; $oldData = $env:GAMER_WORKFLOW_DATA_DIR; $oldWork = $env:GAMER_WORKFLOW_DIR
    try {
        $env:GAMER_WORKFLOW_BASE = $BaseUrl; $env:GAMER_WORKFLOW_TOKEN_FILE = $TokenFile
        $env:GAMER_ADMIN_TOKEN = $adminToken
        $env:GAMER_WORKFLOW_DATA_DIR=$dataRoot; $env:GAMER_WORKFLOW_DIR=$workflowRoot
        $env:GAMER_WORKFLOW_DRY_RUN = if ($DryRun) { '1' } else { '0' }
        $env:GAMER_WORKFLOW_BACKUP_DIR = $BackupDirectory
        & node $configure
        if ($LASTEXITCODE -ne 0) { throw 'Workflow configuration/preflight failed' }
    } finally { $env:GAMER_WORKFLOW_BASE=$oldBase; $env:GAMER_WORKFLOW_TOKEN_FILE=$oldTokenFile; $env:GAMER_WORKFLOW_DRY_RUN=$oldDry; $env:GAMER_WORKFLOW_BACKUP_DIR=$oldBackup; $env:GAMER_ADMIN_TOKEN=$oldAdmin; $env:GAMER_WORKFLOW_DATA_DIR=$oldData; $env:GAMER_WORKFLOW_DIR=$oldWork }
}
function Copy-Overlay([string]$Source, [string]$Destination) {
    Assert-Tree $Source; $null = Assert-WorkspacePath $Destination
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    foreach ($item in Get-ChildItem -LiteralPath $Source -Force) {
        $target = Assert-WorkspacePath (Join-Path $Destination $item.Name)
        if ($item.PSIsContainer) { Copy-Overlay $item.FullName $target } else { Copy-Item -LiteralPath $item.FullName -Destination $target -Force }
    }
}
function Controller([string]$Command, [string]$Log) {
    $previousAdmin = $env:GAMER_ADMIN_TOKEN
    try { $env:GAMER_ADMIN_TOKEN=$adminToken; & $controller $Command -BackendOnly -Port $uri.Port *> $Log }
    finally { $env:GAMER_ADMIN_TOKEN=$previousAdmin }
}
function Wait-Ready {
    for ($attempt=0; $attempt -lt 30; $attempt++) { try { $null=Api '/health/ready'; Get-OwnedListener; return } catch { Start-Sleep -Seconds 1 } }
    throw 'Candidate readiness/identity did not become healthy'
}
function Assert-InstalledBytes($Artifact) {
    $installed = Assert-WorkspacePath (Join-Path $dataRoot "extensions/$($Artifact.id)/$($Artifact.version)")
    Assert-Tree $installed
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($Artifact.path)
    try {
        $fileCount = 0
        foreach ($entry in $archive.Entries) {
            if ($entry.FullName.EndsWith('/')) { continue }
            $fileCount++
            $file = Assert-WorkspacePath (Join-Path $installed $entry.FullName)
            if (-not $file.StartsWith($installed + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase) -or -not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Installed payload does not match pinned archive: $($Artifact.id)" }
            $stream=$entry.Open()
            try { $expected=(Get-FileHash -InputStream $stream -Algorithm SHA256).Hash } finally { $stream.Dispose() }
            if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $expected) { throw "Same plugin version has different immutable bytes: $($Artifact.id)" }
        }
        if (@(Get-ChildItem -LiteralPath $installed -Recurse -File -Force).Count -ne $fileCount) { throw "Unexpected files in immutable payload: $($Artifact.id)" }
    } finally { $archive.Dispose() }
}
$before = Read-Idle
Run-Configuration $true ''
if (-not $Apply -and -not (Test-Path -LiteralPath $EvidencePath)) { Write-Output 'Read-only service preflight passed; final candidate evidence is not present. No changes were made.'; exit 0 }
$EvidencePath = Assert-WorkspacePath $EvidencePath
$evidence = Get-Content -LiteralPath $EvidencePath -Raw | ConvertFrom-Json
if (-not $CandidatePath) { $CandidatePath=$evidence.candidate.path }
$CandidatePath = Assert-WorkspacePath $CandidatePath
if ($CandidatePath -eq $binaryPath) { throw 'Candidate must be a separate immutable artifact' }
if ($evidence.passed -ne $true -or @($evidence.unresolved).Count) { throw 'Final evidence must pass with no unresolved blockers' }
if (-not [string]::Equals((Assert-WorkspacePath $evidence.candidate.path),$CandidatePath,[StringComparison]::OrdinalIgnoreCase) -or (Get-FileHash -LiteralPath $CandidatePath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $evidence.candidate.sha256) { throw 'Candidate identity/hash differs from final evidence' }
if ((Get-FileHash -LiteralPath $configure -Algorithm SHA256).Hash.ToLowerInvariant() -ne $evidence.configure_sha256) { throw 'Reviewed configuration script hash changed' }
if ((Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $evidence.apply_sha256) { throw 'Reviewed manual application script hash changed' }
function Assert-Inputs {
    $paths=@()
    foreach ($inputArtifact in $evidence.inputs) {
        $path=Assert-WorkspacePath $inputArtifact.path
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $inputArtifact.sha256) { throw "Prepared workflow input changed: $path" }
        if ($paths -contains $path) { throw 'Duplicate evidence input path' }
        $paths+=$path
    }
    foreach ($required in @('staging/daily-notify.yaml','staging/daily-test.yaml','staging/live-cloud.yaml','backup/云游戏日常.before-notify.yaml')) {
        if ($paths -notcontains (Join-Path $workflowRoot $required)) { throw "Missing pinned input: $required" }
    }
    if ($paths -notcontains (Join-Path $repoRoot 'server/target/live-daily-qa/rules.example.json')) { throw 'Missing pinned danmaku rules' }
    $videoRoot=Join-Path $repoRoot 'live-videos'
    if (@($paths | Where-Object { $_.StartsWith($videoRoot + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase) }).Count -lt 2) { throw 'Pin both video source files in evidence.inputs' }
}
Assert-Inputs
$expectedVersions = @{ 'gamer-yaml'='0.3.18'; 'gamer-live'='0.2.9'; 'gamer-notify'='0.1.1' }
foreach ($id in $expectedVersions.Keys) {
    $artifact = @($evidence.plugins | Where-Object id -eq $id)
    if ($artifact.Count -ne 1 -or $artifact[0].version -ne $expectedVersions[$id]) { throw "Missing pinned plugin evidence: $id" }
    $artifact[0].path = Assert-WorkspacePath $artifact[0].path
    if ((Get-FileHash -LiteralPath $artifact[0].path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $artifact[0].sha256) { throw "Plugin hash changed: $id" }
    $existing=$before.extensions | Where-Object id -eq $id
    if ($Apply -and $existing.active_version -eq $artifact[0].version) { Assert-InstalledBytes $artifact[0] }
    elseif ($Apply -and $existing.installed_versions -contains $artifact[0].version) { throw "Pinned version is already installed but inactive: $id. Preserve it; review this version conflict before applying" }
}
if (-not $Apply) { Write-Output 'Read-only preflight passed with final artifact hashes. No shutdown, copy, plugin update, configuration write or restart was performed.'; exit 0 }
Get-OwnedListener
if (-not [string]::Equals($dataRoot,(Join-Path $repoRoot 'server/data'),[StringComparison]::OrdinalIgnoreCase)) { throw '-Apply requires the standard checkout data directory; custom directories are read-only QA inputs' }
$config = Get-Content -LiteralPath (Join-Path $repoRoot 'server/config.toml') -Raw
if ($config -notmatch '(?m)^\s*data_dir\s*=\s*["'']\./data["'']\s*$' -or $config -notmatch "(?m)^\s*port\s*=\s*$($uri.Port)\s*$") { throw 'This draft only applies to the standard checkout data directory/configured port' }
$backupRoot = Assert-WorkspacePath (Join-Path $workflowRoot ('applications/' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N')))
New-Item -ItemType Directory -Path $backupRoot | Out-Null
$before | ConvertTo-Json -Depth 60 | Set-Content -LiteralPath (Join-Path $backupRoot 'api-before.json') -Encoding utf8
Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $backupRoot 'gamer-server.before.exe')
Copy-Item -LiteralPath (Join-Path $repoRoot 'server/config.toml') -Destination (Join-Path $backupRoot 'config.before.toml')
$trees = @('extensions','packages/hsr','extension-data/gamer-yaml','extension-data/gamer-notify','extension-data/gamer-live')
foreach ($tree in $trees) { $source=Assert-WorkspacePath (Join-Path $dataRoot $tree); Assert-Tree $source; if (Test-Path -LiteralPath $source) { Copy-Overlay $source (Join-Path $backupRoot "online-data/$tree") } }
$stopped = $false; $coldBackup = $false
try {
    $null = Read-Idle
    Controller 'stop' (Join-Path $backupRoot 'stop.log')
    if (@(Get-NetTCPConnection -LocalPort $uri.Port -State Listen -ErrorAction SilentlyContinue).Count -or @(Get-Process -Name 'gamer-server' -ErrorAction SilentlyContinue | Where-Object { [string]::Equals($_.Path,$binaryPath,[StringComparison]::OrdinalIgnoreCase) }).Count) { throw 'Controller did not stop the expected checkout server; do not replace files' }
    $stopped=$true
    # Re-copy after shutdown for a coherent durable backup, including SQLite WAL.
    foreach ($tree in $trees) { $source=Join-Path $dataRoot $tree; if (Test-Path -LiteralPath $source) { Copy-Overlay $source (Join-Path $backupRoot "data/$tree") } }
    foreach ($name in @('gamer.db','gamer.db-wal','gamer.db-shm')) { $source=Join-Path $dataRoot $name; if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination (Join-Path $backupRoot $name) } }
    $coldBackup=$true
    Copy-Item -LiteralPath $CandidatePath -Destination $binaryPath -Force
    Controller 'start' (Join-Path $backupRoot 'start-candidate.log'); Wait-Ready
    if ((Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $evidence.candidate.sha256) { throw 'Running checkout binary differs from the reviewed candidate' }
    foreach ($id in @('gamer-yaml','gamer-live','gamer-notify')) {
        $artifact=$evidence.plugins | Where-Object id -eq $id
        $extension=(Api '/api/extensions').extensions | Where-Object id -eq $id
        if ($extension.active_version -eq $artifact.version) { Assert-InstalledBytes $artifact; continue }
        $uploadHeaders=$headers.Clone(); $uploadHeaders['X-Expected-Sha256']=$artifact.sha256
        $inspect=Invoke-RestMethod -Uri ($BaseUrl + '/api/extensions/inspect') -Method Post -Headers $uploadHeaders -ContentType 'application/octet-stream' -InFile $artifact.path
        if ($inspect.id -ne $id -or $inspect.version -ne $artifact.version -or @($inspect.permission_diff.added).Count) { throw "Plugin inspection requires further review: $id" }
        $null=Invoke-RestMethod -Uri ($BaseUrl + "/api/extensions/$id/update") -Method Post -Headers $uploadHeaders -ContentType 'application/octet-stream' -InFile $artifact.path
        Assert-InstalledBytes $artifact
    }
    Assert-Inputs
    Run-Configuration $false (Join-Path $backupRoot 'configuration')
    $after=Read-Idle
    if ((Stable-Targets $before.devices $before.browsers) -ne (Stable-Targets $after.devices $after.browsers)) { throw 'Existing target configuration changed unexpectedly' }
    if ($before.channels.default_channel -ne $after.channels.default_channel) { throw 'Global default notification channel changed' }
    $after | ConvertTo-Json -Depth 60 | Set-Content -LiteralPath (Join-Path $backupRoot 'api-after.json') -Encoding utf8
    Write-Output "Applied and checked. Backups and logs: $backupRoot"
} catch {
    $_ | Out-String | Set-Content -LiteralPath (Join-Path $backupRoot 'failure.txt')
    if ($coldBackup) {
        # Never discard a failed candidate's artifacts/history or inactive plugin payloads.
        if (@(Get-NetTCPConnection -LocalPort $uri.Port -State Listen -ErrorAction SilentlyContinue).Count) {
            Get-OwnedListener
            $null=Read-Idle
            Controller 'stop' (Join-Path $backupRoot 'stop-rollback.log')
        } elseif (@(Get-Process -Name 'gamer-server' -ErrorAction SilentlyContinue | Where-Object { [string]::Equals($_.Path,$binaryPath,[StringComparison]::OrdinalIgnoreCase) }).Count) {
            throw "Rollback paused: candidate process is alive but readiness is unavailable. Preserve $backupRoot and inspect it manually"
        }
        if (@(Get-NetTCPConnection -LocalPort $uri.Port -State Listen -ErrorAction SilentlyContinue).Count -or @(Get-Process -Name 'gamer-server' -ErrorAction SilentlyContinue | Where-Object { [string]::Equals($_.Path,$binaryPath,[StringComparison]::OrdinalIgnoreCase) }).Count) { throw "Rollback paused: service still listens or its process is alive. Preserve $backupRoot and inspect failure.txt" }
        $failed=Assert-WorkspacePath (Join-Path $backupRoot 'failed-state'); New-Item -ItemType Directory -Path $failed | Out-Null
        foreach ($tree in $trees) {
            $source=Assert-WorkspacePath (Join-Path $dataRoot $tree); $saved=Assert-WorkspacePath (Join-Path $backupRoot "data/$tree")
            if ($tree -eq 'extensions') { Copy-Overlay $saved $source; continue }
            if (Test-Path -LiteralPath $source) { Assert-Tree $source; $preserved=Assert-WorkspacePath (Join-Path $failed $tree); New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($preserved)) -Force | Out-Null; Move-Item -LiteralPath $source -Destination $preserved }
            if (Test-Path -LiteralPath $saved) { Copy-Overlay $saved $source }
        }
        Copy-Item -LiteralPath (Join-Path $backupRoot 'gamer-server.before.exe') -Destination $binaryPath -Force
        Controller 'start' (Join-Path $backupRoot 'start-rollback.log'); Wait-Ready
    } elseif ($stopped) { Controller 'start' (Join-Path $backupRoot 'restart-old.log') }
    throw "Application failed; preserve backups/logs at $backupRoot. See failure.txt."
}
exit 0
