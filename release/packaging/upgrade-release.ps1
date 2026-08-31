# DKR-002：宿主机 release 镜像升级器。
#
# 顺序固定为 pull 新 digest → 备份 data/config/log → 按新 digest 重建 →
# 等待 Compose healthcheck（/health/ready）→ 提交状态。新容器启动失败或
# readiness 不健康时，按原 digest 重建并再次等待 ready；回滚失败时保留快照和状态文件。
#
# 示例（只使用不可变镜像引用）：
#   pwsh -File release/packaging/upgrade-release.ps1 -NewDigest ghcr.io/<owner>/gamebot@sha256:<64-hex> -CurrentDigest ghcr.io/<owner>/gamebot@sha256:<old-64-hex>
#
# -DockerCommand 可指向离线 fixture；脚本本身不会执行 docker pull 以外的网络访问，
# 也不会访问 GHCR，真实环境的网络行为由 Docker CLI 负责。

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [Alias('NewImage', 'Image')]
    [string]$NewDigest,

    [Alias('OldDigest')]
    [string]$CurrentDigest = '',

    [string]$ComposeFile = '',
    [string]$OverrideFile = '',
    [string]$ProjectName = '',
    [string]$DataDir = '',
    [string]$ConfigDir = '',
    [string]$LogDir = '',
    [string]$BackupRoot = '',
    [string]$StatePath = '',
    [string]$DockerCommand = 'docker',
    [ValidateRange(1, 3600)]
    [int]$ReadyTimeoutSec = 120,
    [ValidateRange(0, 60)]
    [int]$PollSeconds = 2
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

function Fail {
    param([string]$Message)
    throw "[upgrade-release] $Message"
}

function Assert-DigestReference {
    param(
        [Parameter(Mandatory = $true)][string]$Value,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if ($Value -notmatch '^(?<repository>[^@\s]+)@sha256:[0-9a-fA-F]{64}$') {
        Fail "$Label 必须是完整不可变引用 repository@sha256:<64 hex>：$Value"
    }
}

function Get-AbsolutePath {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$BasePath
    )
    if ([IO.Path]::IsPathRooted($Path)) {
        return [IO.Path]::GetFullPath($Path)
    }
    return [IO.Path]::GetFullPath((Join-Path $BasePath $Path))
}

function Assert-Directory {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        Fail "$Label 不存在或不是目录：$Path"
    }
}

function Get-PropertyText {
    param(
        [Parameter(Mandatory = $true)][object]$Object,
        [Parameter(Mandatory = $true)][string]$Name
    )
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property -or $null -eq $property.Value) {
        return ''
    }
    return [string]$property.Value
}

function Read-State {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $null
    }
    try {
        return Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    } catch {
        Fail "状态文件不是合法 JSON：$Path；$($_.Exception.Message)"
    }
}

function Write-JsonAtomic {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][object]$Value
    )
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
        [IO.Directory]::CreateDirectory($parent) | Out-Null
    }
    $temp = '{0}.{1}.tmp' -f $Path, ([guid]::NewGuid().ToString('N'))
    try {
        $json = $Value | ConvertTo-Json -Depth 20
        [IO.File]::WriteAllText($temp, $json + [Environment]::NewLine, (New-Object Text.UTF8Encoding($false)))
        Move-Item -LiteralPath $temp -Destination $Path -Force | Out-Null
    } finally {
        if (Test-Path -LiteralPath $temp) {
            Remove-Item -LiteralPath $temp -Force
        }
    }
}

function Invoke-External {
    param(
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $lines = @(& $DockerCommand @Arguments 2>&1 | ForEach-Object { [string]$_ })
    $exitCode = $LASTEXITCODE
    $output = ($lines -join [Environment]::NewLine).Trim()
    if ($exitCode -ne 0) {
        $detail = if ($output) { ": $output" } else { '' }
        Fail "$Label 失败（exit=$exitCode）$detail"
    }
    return [pscustomobject]@{
        ExitCode = $exitCode
        Output = $lines
    }
}

function Get-ComposePrefix {
    $args = @('compose', '-f', $script:ComposeFileFull)
    if ($script:OverrideFileFull) {
        $args += @('-f', $script:OverrideFileFull)
    }
    if ($script:ProjectName) {
        $args += @('--project-name', $script:ProjectName)
    }
    return $args
}

function Invoke-Compose {
    param(
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$ImageReference
    )

    $names = @('GAMER_IMAGE', 'GAMER_DATA_DIR', 'GAMER_CONFIG_DIR', 'GAMER_LOG_DIR')
    $saved = @{}
    foreach ($name in $names) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    }
    try {
        $env:GAMER_IMAGE = $ImageReference
        $env:GAMER_DATA_DIR = $script:DataDirFull
        $env:GAMER_CONFIG_DIR = $script:ConfigDirFull
        $env:GAMER_LOG_DIR = $script:LogDirFull
        return Invoke-External -Arguments ((Get-ComposePrefix) + $Arguments) -Label ('docker compose ' + ($Arguments -join ' '))
    } finally {
        foreach ($name in $names) {
            [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
        }
    }
}

function Get-OutputLine {
    param([Parameter(Mandatory = $true)][object]$Result)
    $line = @($Result.Output | ForEach-Object { [string]$_ } | Where-Object { $_.Trim() } | Select-Object -Last 1)
    if ($line.Count -eq 0) {
        return ''
    }
    return $line[0].Trim()
}

function Resolve-CurrentDigest {
    if ($script:CurrentDigestInput) {
        Assert-DigestReference -Value $script:CurrentDigestInput -Label 'CurrentDigest'
        return $script:CurrentDigestInput
    }

    $state = Read-State -Path $script:StatePathFull
    if ($null -ne $state) {
        $stateImage = Get-PropertyText -Object $state -Name 'currentImage'
        if (-not $stateImage) {
            $stateImage = Get-PropertyText -Object $state -Name 'currentDigest'
        }
        if ($stateImage) {
            Assert-DigestReference -Value $stateImage -Label '状态文件 currentImage'
            return $stateImage
        }
    }

    $psResult = Invoke-Compose -Arguments @('ps', '-q', 'gamer') -ImageReference $script:NewDigest
    $containerId = Get-OutputLine -Result $psResult
    if (-not $containerId) {
        Fail '没有 CurrentDigest、有效状态文件，也找不到运行中的 gamer 容器'
    }
    $imageResult = Invoke-External -Arguments @('inspect', '--format', '{{.Config.Image}}', $containerId) -Label '读取当前容器镜像'
    $imageReference = Get-OutputLine -Result $imageResult
    if ($imageReference -match '@sha256:[0-9a-fA-F]{64}$') {
        Assert-DigestReference -Value $imageReference -Label '运行中镜像'
        return $imageReference
    }
    if (-not $imageReference) {
        Fail '运行中容器没有 Config.Image'
    }

    $repo = $imageReference
    $lastSlash = $repo.LastIndexOf('/')
    $lastColon = $repo.LastIndexOf(':')
    if ($lastColon -gt $lastSlash) {
        $repo = $repo.Substring(0, $lastColon)
    }
    $digestResult = Invoke-External -Arguments @('image', 'inspect', '--format', '{{json .RepoDigests}}', $imageReference) -Label '解析当前镜像 digest'
    $repoDigests = Get-OutputLine -Result $digestResult
    try {
        $parsed = $repoDigests | ConvertFrom-Json
    } catch {
        Fail "docker image inspect 没有返回合法 RepoDigests：$repoDigests"
    }
    foreach ($candidate in @($parsed)) {
        if ([string]$candidate -match ('^{0}@sha256:[0-9a-fA-F]{{64}}$' -f [regex]::Escape($repo))) {
            Assert-DigestReference -Value ([string]$candidate) -Label '解析出的当前镜像'
            return [string]$candidate
        }
    }
    Fail "当前镜像没有 $repo 对应的不可变 RepoDigest"
}

function Get-RelativeFilePath {
    param(
        [Parameter(Mandatory = $true)][string]$BasePath,
        [Parameter(Mandatory = $true)][string]$FilePath
    )
    $prefix = $BasePath.TrimEnd([char[]]@('\', '/')) + [IO.Path]::DirectorySeparatorChar
    if (-not $FilePath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        Fail "文件不在预期目录内：$FilePath"
    }
    return $FilePath.Substring($prefix.Length).Replace('\', '/')
}

function New-BackupSnapshot {
    param(
        [Parameter(Mandatory = $true)][string]$RootPath,
        [Parameter(Mandatory = $true)][string]$UpdateId
    )
    $backupPath = Join-Path $RootPath $UpdateId
    if (Test-Path -LiteralPath $backupPath) {
        Fail "备份目录已存在：$backupPath"
    }
    [IO.Directory]::CreateDirectory($backupPath) | Out-Null

    $sources = @(
        [pscustomobject]@{ Name = 'data'; Path = $script:DataDirFull },
        [pscustomobject]@{ Name = 'config'; Path = $script:ConfigDirFull },
        [pscustomobject]@{ Name = 'log'; Path = $script:LogDirFull }
    )
    $entries = @()
    $sumLines = @()
    foreach ($source in $sources) {
        $target = Join-Path $backupPath $source.Name
        [IO.Directory]::CreateDirectory($target) | Out-Null
        $files = @(Get-ChildItem -LiteralPath $source.Path -File -Recurse -Force)
        foreach ($file in $files) {
            $relative = Get-RelativeFilePath -BasePath $source.Path -FilePath ([IO.Path]::GetFullPath($file.FullName))
            $destination = Join-Path $target ($relative.Replace('/', [IO.Path]::DirectorySeparatorChar))
            $destinationParent = Split-Path -Parent $destination
            if (-not (Test-Path -LiteralPath $destinationParent -PathType Container)) {
                [IO.Directory]::CreateDirectory($destinationParent) | Out-Null
            }
            Copy-Item -LiteralPath $file.FullName -Destination $destination -Force | Out-Null
            $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            $entries += [ordered]@{
                area = $source.Name
                path = $relative
                sha256 = $hash
                length = [int64]$file.Length
            }
            $sumLines += '{0}  {1}/{2}' -f $hash, $source.Name, $relative
        }
    }

    $manifest = [ordered]@{
        schemaVersion = 1
        updateId = $UpdateId
        createdAt = (Get-Date).ToUniversalTime().ToString('o')
        directories = [ordered]@{
            data = $script:DataDirFull
            config = $script:ConfigDirFull
            log = $script:LogDirFull
        }
        entries = $entries
    }
    Write-JsonAtomic -Path (Join-Path $backupPath 'backup.json') -Value $manifest
    [IO.File]::WriteAllLines(
        (Join-Path $backupPath 'MANIFEST.sha256'),
        $sumLines,
        (New-Object Text.UTF8Encoding($false))
    )
    [IO.File]::WriteAllText(
        (Join-Path $backupPath 'BACKUP_READY'),
        ('updateId={0}{1}' -f $UpdateId, [Environment]::NewLine),
        (New-Object Text.UTF8Encoding($false))
    )
    return $backupPath
}

function Wait-Ready {
    param(
        [Parameter(Mandatory = $true)][string]$ImageReference,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $deadline = (Get-Date).AddSeconds($script:ReadyTimeoutSec)
    $lastStatus = ''
    do {
        $psResult = Invoke-Compose -Arguments @('ps', '-q', 'gamer') -ImageReference $ImageReference
        $containerId = Get-OutputLine -Result $psResult
        if ($containerId) {
            $healthResult = Invoke-External -Arguments @('inspect', '--format', '{{.State.Health.Status}}', $containerId) -Label ('读取' + $Label + ' health')
            $status = Get-OutputLine -Result $healthResult
            $lastStatus = $status
            if ($status -eq 'healthy') {
                Write-Host "[upgrade-release] $Label ready: healthy"
                return
            }
            if ($status -in @('unhealthy', 'exited', 'dead')) {
                Fail "$Label readiness 失败：health=$status"
            }
        }
        if ((Get-Date) -ge $deadline) {
            Fail "$Label readiness 超时（最后 health=$lastStatus）"
        }
        if ($script:PollSeconds -gt 0) {
            Start-Sleep -Seconds $script:PollSeconds
        }
    } while ($true)
}

function Restore-OldDigest {
    param(
        [Parameter(Mandatory = $true)][string]$OldImage,
        [Parameter(Mandatory = $true)][string]$Cause
    )
    Write-Warning "[upgrade-release] 新镜像未 ready，开始恢复旧 digest：$OldImage；原因：$Cause"
    try {
        Invoke-Compose -Arguments @('up', '-d', '--no-build', '--force-recreate', 'gamer') -ImageReference $OldImage | Out-Null
        Wait-Ready -ImageReference $OldImage -Label '旧镜像回滚'
        Write-Warning '[upgrade-release] 旧 digest 已恢复并 ready'
        return $true
    } catch {
        Write-Error "[upgrade-release] 旧 digest 回滚失败：$($_.Exception.Message)"
        return $false
    }
}

try {
    Assert-DigestReference -Value $NewDigest -Label 'NewDigest'
    $script:NewDigest = $NewDigest
    $script:CurrentDigestInput = $CurrentDigest
    $script:ReadyTimeoutSec = $ReadyTimeoutSec
    $script:PollSeconds = $PollSeconds

    if (-not (Get-Command $DockerCommand -ErrorAction SilentlyContinue) -and -not (Test-Path -LiteralPath $DockerCommand -PathType Leaf)) {
        Fail "找不到 Docker CLI/fixture：$DockerCommand"
    }

    $defaultRoot = Split-Path -Parent $PSScriptRoot
    if (-not $ComposeFile) { $ComposeFile = Join-Path $defaultRoot '..\docker-compose.release.yml' }
    $script:ComposeFileFull = [IO.Path]::GetFullPath($ComposeFile)
    if (-not (Test-Path -LiteralPath $script:ComposeFileFull -PathType Leaf)) {
        Fail "release compose 不存在：$script:ComposeFileFull"
    }
    $script:OverrideFileFull = ''
    if ($OverrideFile) {
        $script:OverrideFileFull = [IO.Path]::GetFullPath($OverrideFile)
        if (-not (Test-Path -LiteralPath $script:OverrideFileFull -PathType Leaf)) {
            Fail "override compose 不存在：$script:OverrideFileFull"
        }
    }
    $deployRoot = Split-Path -Parent $script:ComposeFileFull
    if (-not $DataDir) { $DataDir = Join-Path $deployRoot 'data' }
    if (-not $ConfigDir) { $ConfigDir = Join-Path $deployRoot 'config' }
    if (-not $LogDir) { $LogDir = Join-Path $deployRoot 'logs' }
    if (-not $BackupRoot) { $BackupRoot = Join-Path $deployRoot 'backups' }
    if (-not $StatePath) { $StatePath = Join-Path $deployRoot 'release-image-state.json' }
    $script:DataDirFull = Get-AbsolutePath -Path $DataDir -BasePath $deployRoot
    $script:ConfigDirFull = Get-AbsolutePath -Path $ConfigDir -BasePath $deployRoot
    $script:LogDirFull = Get-AbsolutePath -Path $LogDir -BasePath $deployRoot
    $script:BackupRootFull = Get-AbsolutePath -Path $BackupRoot -BasePath $deployRoot
    $script:StatePathFull = Get-AbsolutePath -Path $StatePath -BasePath $deployRoot

    Assert-Directory -Path $script:DataDirFull -Label 'data 目录'
    Assert-Directory -Path $script:ConfigDirFull -Label 'config 目录'
    Assert-Directory -Path $script:LogDirFull -Label 'log 目录'

    $oldDigest = Resolve-CurrentDigest
    if ($oldDigest.ToLowerInvariant() -eq $NewDigest.ToLowerInvariant()) {
        Write-Host '[upgrade-release] 当前 digest 已是目标 digest，无需升级'
        exit 0
    }

    Invoke-Compose -Arguments @('pull', 'gamer') -ImageReference $NewDigest | Out-Null
    Write-Host "[upgrade-release] pull OK: $NewDigest"

    $updateId = 'docker-' + (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssfffZ') + '-' + ([guid]::NewGuid().ToString('N').Substring(0, 8))
    $backupPath = New-BackupSnapshot -RootPath $script:BackupRootFull -UpdateId $updateId
    Write-Host "[upgrade-release] backup OK: $backupPath"

    try {
        Invoke-Compose -Arguments @('up', '-d', '--no-build', '--force-recreate', 'gamer') -ImageReference $NewDigest | Out-Null
        Wait-Ready -ImageReference $NewDigest -Label '新镜像'
    } catch {
        $upgradeError = $_.Exception.Message
        if (-not (Restore-OldDigest -OldImage $oldDigest -Cause $upgradeError)) {
            Fail "升级失败且旧 digest 回滚失败；快照保留在 $backupPath；原始错误：$upgradeError"
        }
        Fail "升级失败，已恢复旧 digest；快照保留在 $backupPath；原始错误：$upgradeError"
    }

    $state = [ordered]@{
        schemaVersion = 1
        currentImage = $NewDigest
        currentDigest = $NewDigest
        previousImage = $oldDigest
        backupPath = $backupPath
        composeFile = $script:ComposeFileFull
        directories = [ordered]@{
            data = $script:DataDirFull
            config = $script:ConfigDirFull
            log = $script:LogDirFull
        }
        updatedAt = (Get-Date).ToUniversalTime().ToString('o')
    }
    Write-JsonAtomic -Path $script:StatePathFull -Value $state
    Write-Host "[upgrade-release] PASS: $NewDigest ready；data/config/log 绑定目录保持不变"
    exit 0
} catch {
    Write-Error $_.Exception.Message
    exit 1
}
