# Offline Docker CLI fixture for DKR-002 behavior tests.
# The fixture only reads and writes local JSON/text files. It never contacts a
# Docker daemon, a registry, or any other network endpoint.

[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$Command = '',

    [Parameter(Position = 1, ValueFromRemainingArguments = $true)]
    [string[]]$Arguments = @()
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

function Fail {
    param([string]$Message)
    [Console]::Error.WriteLine('[mock-docker] ' + $Message)
    exit 1
}

function Get-RequiredEnvironmentValue {
    param([Parameter(Mandatory = $true)][string]$Name)
    $value = [Environment]::GetEnvironmentVariable($Name, 'Process')
    if ([string]::IsNullOrWhiteSpace($value)) {
        Fail "missing environment variable: $Name"
    }
    return $value
}

function Read-State {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        Fail "state file does not exist: $Path"
    }
    try {
        return Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    } catch {
        Fail "state file is not valid JSON: $Path"
    }
}

function Write-State {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][object]$State
    )
    $json = $State | ConvertTo-Json -Depth 30
    [IO.File]::WriteAllText($Path, $json + [Environment]::NewLine, (New-Object Text.UTF8Encoding($false)))
}

function Get-BackupReadyCount {
    param([Parameter(Mandatory = $true)][string]$Root)
    if (-not (Test-Path -LiteralPath $Root -PathType Container)) {
        return 0
    }
    return @(
        Get-ChildItem -LiteralPath $Root -Directory -Force |
            Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'BACKUP_READY') -PathType Leaf }
    ).Count
}

function Get-FormatValue {
    param([Parameter(Mandatory = $true)][string[]]$Values)
    for ($index = 0; $index -lt $Values.Count - 1; $index++) {
        if ($Values[$index] -eq '--format') {
            return $Values[$index + 1]
        }
    }
    return ''
}

if (-not $Command) {
    Fail 'no command was supplied'
}

$statePath = Get-RequiredEnvironmentValue -Name 'GAMER_MOCK_DOCKER_STATE_PATH'
$logPath = Get-RequiredEnvironmentValue -Name 'GAMER_MOCK_DOCKER_LOG_PATH'
$backupRoot = Get-RequiredEnvironmentValue -Name 'GAMER_MOCK_DOCKER_BACKUP_ROOT'
$state = Read-State -Path $statePath
$allArguments = @($Arguments)
$image = [Environment]::GetEnvironmentVariable('GAMER_IMAGE', 'Process')
$dataDir = [Environment]::GetEnvironmentVariable('GAMER_DATA_DIR', 'Process')
$configDir = [Environment]::GetEnvironmentVariable('GAMER_CONFIG_DIR', 'Process')
$logDir = [Environment]::GetEnvironmentVariable('GAMER_LOG_DIR', 'Process')
$backupReadyCount = Get-BackupReadyCount -Root $backupRoot

$event = [ordered]@{
    command = $Command
    arguments = $allArguments
    image = $image
    dataDir = $dataDir
    configDir = $configDir
    logDir = $logDir
    backupReadyCount = $backupReadyCount
}
$state.calls = @($state.calls) + [pscustomobject]$event

if ($Command -eq 'compose') {
    if ([string]::IsNullOrWhiteSpace($image)) {
        Fail 'compose call did not receive GAMER_IMAGE'
    }

    if ($allArguments -contains 'pull') {
        if ($allArguments -notcontains 'gamer') {
            Fail 'pull fixture call did not target gamer'
        }
        if ($image -ne [string]$state.newDigest) {
            Fail "pull received unexpected image: $image"
        }
        if ($backupReadyCount -ne 0) {
            Fail 'pull happened after a backup snapshot was created'
        }
        Write-State -Path $statePath -State $state
        Add-Content -LiteralPath $logPath -Value (($event | ConvertTo-Json -Depth 10 -Compress) + [Environment]::NewLine) -Encoding UTF8
        Write-Output ('pulled ' + $image)
        exit 0
    }

    if ($allArguments -contains 'up') {
        if ($allArguments -notcontains 'gamer' -or $allArguments -notcontains '--force-recreate') {
            Fail 'up fixture call did not use the release recreate contract'
        }
        if ($backupReadyCount -lt 1) {
            Fail 'up happened before a backup snapshot was ready'
        }

        if ($image -eq [string]$state.newDigest) {
            $state.runtime.image = [string]$state.newDigest
            $state.runtime.containerId = 'mock-new-container'
            $state.runtime.health = [string]$state.newHealth
        } elseif ($image -eq [string]$state.oldDigest) {
            $state.runtime.image = [string]$state.oldDigest
            $state.runtime.containerId = 'mock-old-container'
            $state.runtime.health = [string]$state.oldHealth
        } else {
            Fail "up received unexpected image: $image"
        }

        Write-State -Path $statePath -State $state
        Add-Content -LiteralPath $logPath -Value (($event | ConvertTo-Json -Depth 10 -Compress) + [Environment]::NewLine) -Encoding UTF8
        Write-Output ('recreated ' + $image)
        exit 0
    }

    if ($allArguments -contains 'ps') {
        Write-State -Path $statePath -State $state
        Add-Content -LiteralPath $logPath -Value (($event | ConvertTo-Json -Depth 10 -Compress) + [Environment]::NewLine) -Encoding UTF8
        Write-Output ([string]$state.runtime.containerId)
        exit 0
    }

    Fail ('unsupported compose arguments: ' + ($allArguments -join ' '))
}

if ($Command -eq 'inspect') {
    $format = Get-FormatValue -Values $allArguments
    if ($format -eq '{{.State.Health.Status}}') {
        Write-State -Path $statePath -State $state
        Add-Content -LiteralPath $logPath -Value (($event | ConvertTo-Json -Depth 10 -Compress) + [Environment]::NewLine) -Encoding UTF8
        Write-Output ([string]$state.runtime.health)
        exit 0
    }
    if ($format -eq '{{.Config.Image}}') {
        Write-State -Path $statePath -State $state
        Add-Content -LiteralPath $logPath -Value (($event | ConvertTo-Json -Depth 10 -Compress) + [Environment]::NewLine) -Encoding UTF8
        Write-Output ([string]$state.runtime.image)
        exit 0
    }
    Fail ('unsupported inspect format: ' + $format)
}

if ($Command -eq 'image' -and $allArguments -contains 'inspect') {
    Write-State -Path $statePath -State $state
    Add-Content -LiteralPath $logPath -Value (($event | ConvertTo-Json -Depth 10) + [Environment]::NewLine) -Encoding UTF8
    Write-Output ('["' + [string]$state.oldDigest + '"]')
    exit 0
}

Fail ('unsupported command: ' + $Command)
