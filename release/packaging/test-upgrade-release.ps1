# Offline behavior test for the DKR-002 host-side release image upgrader.
# It injects mock-docker.ps1 through -DockerCommand; no Docker daemon, GHCR,
# network, production data, or release evidence is used.

[CmdletBinding()]
param(
    [string]$RepoRoot = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

if (-not $RepoRoot) {
    $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
}
$RepoRoot = [IO.Path]::GetFullPath($RepoRoot)
Set-Location -LiteralPath $RepoRoot

function Fail {
    param([string]$Message)
    throw '[upgrade-release-test] FAIL: ' + $Message
}

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) {
        Fail $Message
    }
}

function Assert-Equal {
    param([object]$Actual, [object]$Expected, [string]$Message)
    if ([string]$Actual -ne [string]$Expected) {
        Fail "$Message (actual=$Actual expected=$Expected)"
    }
}

function Write-Utf8NoBom {
    param([string]$Path, [string]$Text)
    [IO.File]::WriteAllText($Path, $Text + [Environment]::NewLine, (New-Object Text.UTF8Encoding($false)))
}

function Write-Json {
    param([string]$Path, [object]$Value)
    $json = $Value | ConvertTo-Json -Depth 30
    [IO.File]::WriteAllText($Path, $json + [Environment]::NewLine, (New-Object Text.UTF8Encoding($false)))
}

function Read-Json {
    param([string]$Path)
    return Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
}

function Read-EventLog {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return @()
    }
    return @(Get-Content -LiteralPath $Path | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json })
}

function Invoke-Child {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [int]$ExpectedExit = 0
    )
    $pwsh = Get-Command pwsh -ErrorAction SilentlyContinue
    if (-not $pwsh) {
        $pwsh = Get-Command powershell -ErrorAction SilentlyContinue
    }
    if (-not $pwsh) {
        Fail 'behavior test requires pwsh or powershell'
    }
    $savedErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $output = (& $pwsh.Source -NoLogo -NoProfile -File $Path @Arguments 2>&1 | Out-String)
    } finally {
        $ErrorActionPreference = $savedErrorActionPreference
    }
    $code = $LASTEXITCODE
    if ($code -ne $ExpectedExit) {
        Fail "child exit code=$code expected=$ExpectedExit`n$output"
    }
    return [pscustomobject]@{ Output = $output; ExitCode = $code }
}

function New-TestCase {
    param(
        [Parameter(Mandatory = $true)][string]$CaseRoot,
        [Parameter(Mandatory = $true)][string]$NewDigest,
        [Parameter(Mandatory = $true)][string]$OldDigest,
        [Parameter(Mandatory = $true)][string]$NewHealth,
        [Parameter(Mandatory = $true)][string]$ComposePath
    )

    $dataDir = Join-Path $CaseRoot 'data'
    $configDir = Join-Path $CaseRoot 'config'
    $logDir = Join-Path $CaseRoot 'logs'
    $backupRoot = Join-Path $CaseRoot 'backups'
    $mockStatePath = Join-Path $CaseRoot 'mock-state.json'
    $mockLogPath = Join-Path $CaseRoot 'mock-calls.jsonl'
    $upgradeStatePath = Join-Path $CaseRoot 'release-image-state.json'
    foreach ($directory in @($dataDir, $configDir, $logDir, $backupRoot)) {
        [IO.Directory]::CreateDirectory($directory) | Out-Null
    }

    [IO.Directory]::CreateDirectory((Join-Path $dataDir 'nested')) | Out-Null
    Write-Utf8NoBom -Path (Join-Path $dataDir 'data.txt') -Text 'data-before-upgrade'
    Write-Utf8NoBom -Path (Join-Path $dataDir 'nested/device.db') -Text 'stable-db-content'
    Write-Utf8NoBom -Path (Join-Path $configDir 'config.toml') -Text "mode = 'offline-test'"
    Write-Utf8NoBom -Path (Join-Path $logDir 'gamer.log') -Text 'old-log-content'

    Write-Json -Path $upgradeStatePath -Value ([ordered]@{
        schemaVersion = 1
        currentImage = $OldDigest
        currentDigest = $OldDigest
        previousImage = ''
        backupPath = ''
        composeFile = $ComposePath
        directories = [ordered]@{ data = $dataDir; config = $configDir; log = $logDir }
        updatedAt = 'fixture'
    })
    Write-Json -Path $mockStatePath -Value ([ordered]@{
        scenario = $NewHealth
        newDigest = $NewDigest
        oldDigest = $OldDigest
        newHealth = $NewHealth
        oldHealth = 'healthy'
        calls = @()
        runtime = [ordered]@{
            image = $OldDigest
            containerId = 'mock-existing-container'
            health = 'healthy'
        }
    })
    Write-Utf8NoBom -Path $mockLogPath -Text ''

    return [pscustomobject]@{
        Root = $CaseRoot
        DataDir = $dataDir
        ConfigDir = $configDir
        LogDir = $logDir
        BackupRoot = $backupRoot
        MockStatePath = $mockStatePath
        MockLogPath = $mockLogPath
        UpgradeStatePath = $upgradeStatePath
    }
}

function Invoke-UpgradeCase {
    param(
        [Parameter(Mandatory = $true)][object]$Case,
        [Parameter(Mandatory = $true)][string]$NewDigest,
        [Parameter(Mandatory = $true)][string]$OldDigest,
        [Parameter(Mandatory = $true)][string]$MockPath,
        [Parameter(Mandatory = $true)][string]$ComposePath,
        [int]$ExpectedExit = 0
    )
    $env:GAMER_MOCK_DOCKER_STATE_PATH = $Case.MockStatePath
    $env:GAMER_MOCK_DOCKER_LOG_PATH = $Case.MockLogPath
    $env:GAMER_MOCK_DOCKER_BACKUP_ROOT = $Case.BackupRoot
    return Invoke-Child -Path (Join-Path $PSScriptRoot 'upgrade-release.ps1') -ExpectedExit $ExpectedExit -Arguments @(
        '-NewDigest', $NewDigest,
        '-CurrentDigest', $OldDigest,
        '-ComposeFile', $ComposePath,
        '-DataDir', $Case.DataDir,
        '-ConfigDir', $Case.ConfigDir,
        '-LogDir', $Case.LogDir,
        '-BackupRoot', $Case.BackupRoot,
        '-StatePath', $Case.UpgradeStatePath,
        '-DockerCommand', $MockPath,
        '-ReadyTimeoutSec', '1',
        '-PollSeconds', '0'
    )
}

function Assert-CommonCallContract {
    param(
        [object[]]$Events,
        [object]$Case,
        [string]$NewDigest,
        [string]$OldDigest,
        [int]$ExpectedCount
    )
    Assert-Equal $Events.Count $ExpectedCount 'unexpected mock Docker call count'
    foreach ($event in @($Events | Where-Object { $_.command -eq 'compose' })) {
        Assert-Equal $event.dataDir $Case.DataDir 'compose data bind did not stay stable'
        Assert-Equal $event.configDir $Case.ConfigDir 'compose config bind did not stay stable'
        Assert-Equal $event.logDir $Case.LogDir 'compose log bind did not stay stable'
    }
    Assert-True ($Events[0].command -eq 'compose' -and $Events[0].arguments -contains 'pull') 'first Docker operation was not compose pull'
    Assert-Equal $Events[0].image $NewDigest 'pull did not use the new digest'
    Assert-Equal $Events[0].backupReadyCount 0 'pull was not observed before backup creation'
    Assert-True ($Events[1].command -eq 'compose' -and $Events[1].arguments -contains 'up') 'second Docker operation was not compose up'
    Assert-Equal $Events[1].image $NewDigest 'new compose up did not use the new digest'
    Assert-True ($Events[1].backupReadyCount -ge 1) 'new compose up did not observe a ready backup'
    Assert-True ($Events[2].command -eq 'compose' -and $Events[2].arguments -contains 'ps') 'readiness did not query compose ps'
    Assert-True ($Events[3].command -eq 'inspect') 'readiness did not inspect container health'
    Assert-True (($Events[3].arguments -join ' ') -match [regex]::Escape('{{.State.Health.Status}}')) 'readiness inspected the wrong field'
    if ($ExpectedCount -eq 7) {
        Assert-True ($Events[4].command -eq 'compose' -and $Events[4].arguments -contains 'up') 'rollback did not recreate the service'
        Assert-Equal $Events[4].image $OldDigest 'rollback did not use the old digest'
        Assert-True ($Events[5].command -eq 'compose' -and $Events[5].arguments -contains 'ps') 'rollback readiness did not query compose ps'
        Assert-True ($Events[6].command -eq 'inspect') 'rollback readiness did not inspect container health'
    }
}

function Assert-BackupSnapshot {
    param([object]$Case)
    $state = Read-Json -Path $Case.UpgradeStatePath
    $backupPath = ''
    if ($state.backupPath) {
        $backupPath = [string]$state.backupPath
    } else {
        $backupCandidates = @(Get-ChildItem -LiteralPath $Case.BackupRoot -Directory -Force)
        Assert-Equal $backupCandidates.Count 1 'rollback did not leave exactly one backup snapshot'
        $backupPath = [string]$backupCandidates[0].FullName
    }
    if (-not (Test-Path -LiteralPath $backupPath -PathType Container)) {
        Fail "backup path was not recorded or created: $backupPath"
    }
    Assert-True (Test-Path -LiteralPath (Join-Path $backupPath 'BACKUP_READY') -PathType Leaf) 'BACKUP_READY marker is missing'
    Assert-True (Test-Path -LiteralPath (Join-Path $backupPath 'MANIFEST.sha256') -PathType Leaf) 'MANIFEST.sha256 is missing'
    $manifest = Read-Json -Path (Join-Path $backupPath 'backup.json')
    foreach ($relative in @('data/data.txt', 'data/nested/device.db', 'config/config.toml', 'log/gamer.log')) {
        $parts = $relative.Split('/', 2)
        $area = $parts[0]
        $path = $parts[1]
        $entry = @($manifest.entries | Where-Object { $_.area -eq $area -and $_.path -eq $path })
        Assert-Equal $entry.Count 1 "backup manifest is missing $relative"
        $sourceRoot = switch ($area) {
            'data' { $Case.DataDir }
            'config' { $Case.ConfigDir }
            'log' { $Case.LogDir }
        }
        $source = Join-Path $sourceRoot ($path.Replace('/', [IO.Path]::DirectorySeparatorChar))
        $snapshot = Join-Path (Join-Path $backupPath $area) ($path.Replace('/', [IO.Path]::DirectorySeparatorChar))
        Assert-True (Test-Path -LiteralPath $snapshot -PathType Leaf) "backup file is missing $relative"
        Assert-Equal (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash (Get-FileHash -LiteralPath $snapshot -Algorithm SHA256).Hash "backup hash mismatch for $relative"
        Assert-Equal (Get-Content -LiteralPath $source -Raw) (Get-Content -LiteralPath $snapshot -Raw) "backup content mismatch for $relative"
    }
    return $state
}

$newDigest = 'ghcr.io/example/gamebot@sha256:' + ('1' * 64)
$oldDigest = 'ghcr.io/example/gamebot@sha256:' + ('2' * 64)
$composePath = Join-Path $RepoRoot 'docker-compose.release.yml'
$mockPath = Join-Path $PSScriptRoot 'mock-docker.ps1'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('gamer-dkr002-' + [guid]::NewGuid().ToString('N'))
$environmentNames = @('GAMER_MOCK_DOCKER_STATE_PATH', 'GAMER_MOCK_DOCKER_LOG_PATH', 'GAMER_MOCK_DOCKER_BACKUP_ROOT')
$savedEnvironment = @{}
foreach ($name in $environmentNames) {
    $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

[IO.Directory]::CreateDirectory($testRoot) | Out-Null
try {
    $successCase = New-TestCase -CaseRoot (Join-Path $testRoot 'success') -NewDigest $newDigest -OldDigest $oldDigest -NewHealth 'healthy' -ComposePath $composePath
    $successResult = Invoke-UpgradeCase -Case $successCase -NewDigest $newDigest -OldDigest $oldDigest -MockPath $mockPath -ComposePath $composePath
    Assert-True ($successResult.Output -match 'PASS') 'healthy upgrade did not report PASS'
    $successEvents = Read-EventLog -Path $successCase.MockLogPath
    Assert-CommonCallContract -Events $successEvents -Case $successCase -NewDigest $newDigest -OldDigest $oldDigest -ExpectedCount 4
    $successState = Assert-BackupSnapshot -Case $successCase
    Assert-Equal $successState.currentImage $newDigest 'successful upgrade did not commit the new digest'
    Assert-Equal $successState.previousImage $oldDigest 'successful upgrade did not retain the previous digest'
    $successRuntime = Read-Json -Path $successCase.MockStatePath
    Assert-Equal $successRuntime.runtime.image $newDigest 'mock runtime did not end on the new digest'
    Assert-Equal $successRuntime.runtime.health 'healthy' 'new digest was not ready in the success case'

    $rollbackCase = New-TestCase -CaseRoot (Join-Path $testRoot 'rollback') -NewDigest $newDigest -OldDigest $oldDigest -NewHealth 'unhealthy' -ComposePath $composePath
    $rollbackResult = Invoke-UpgradeCase -Case $rollbackCase -NewDigest $newDigest -OldDigest $oldDigest -MockPath $mockPath -ComposePath $composePath -ExpectedExit 1
    Assert-True ($rollbackResult.Output.Contains($oldDigest)) 'rollback output did not identify the old digest'
    $rollbackEvents = Read-EventLog -Path $rollbackCase.MockLogPath
    Assert-CommonCallContract -Events $rollbackEvents -Case $rollbackCase -NewDigest $newDigest -OldDigest $oldDigest -ExpectedCount 7
    $rollbackState = Assert-BackupSnapshot -Case $rollbackCase
    Assert-Equal $rollbackState.currentImage $oldDigest 'rollback changed the committed state away from the old digest'
    $rollbackRuntime = Read-Json -Path $rollbackCase.MockStatePath
    Assert-Equal $rollbackRuntime.runtime.image $oldDigest 'mock runtime did not return to the old digest'
    Assert-Equal $rollbackRuntime.runtime.health 'healthy' 'old digest was not ready after rollback'

    Write-Host '[upgrade-release-test] PASS: healthy switch and unhealthy rollback are offline and daemon-free'
    exit 0
} catch {
    Write-Error $_.Exception.Message
    exit 1
} finally {
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
    }
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
