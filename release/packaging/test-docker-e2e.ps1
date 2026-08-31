# DKR-004 Docker daemon acceptance test.
#
# The default mode builds the current Dockerfile into a base image, derives
# three short-lived fixture images, pushes them to a temporary local registry,
# and then uses
# their actual registry digests with docker-compose.release.yml:
#   healthy old -> docker stop/SIGTERM -> healthy new
#   healthy new -> non-starting candidate -> automatic rollback to new
#
# This is deliberately different from the offline mock test: the healthy
# cases run the real gamer-server entrypoint in a real Linux container. The
# only synthetic failure is an image whose entrypoint never starts the server,
# which exercises the real Compose health/readiness failure and rollback path.
# A real Android device is not required for the process-level drain check, but
# session/viewer/adb cleanup is reported as NOT RUN unless a device is present.
#
# Data-survival is asserted on two layers: static marker files in the three
# bind-mounted host directories AND a live API anchor (login via
# GAMER_ADMIN_PASSWORD + POST /api/devices before the upgrade; the record must
# still be listed after the healthy upgrade and after the failed-candidate
# rollback). SQLite integrity_check=ok / user_version=1 is asserted on the
# host-bound gamer.db right after the SIGTERM stop.
#
# Idempotency / cleanup:
#   -CleanUp removes leftovers of previous crashed runs (containers, networks
#   and images carrying the script-unique "gamer-dkr004-" name prefix, plus
#   gamer-dkr004-* artifact directories under -ArtifactsRoot) and exits.
#   Normal runs also clean everything they created in their finally block.

[CmdletBinding()]
param(
    [string]$RepoRoot = '',
    [string]$DockerCommand = 'docker',
    # Empty means build the current repository Dockerfile. Supplying a base
    # image is useful for a prebuilt release image, but it must be intentional
    # because an old local tag would not prove the current SIGTERM code.
    [string]$BaseImage = '',
    [string]$OldDigest = '',
    [string]$NewDigest = '',
    [string]$BadDigest = '',
    [switch]$RequireRealE2E,
    [switch]$KeepArtifacts,
    [ValidateRange(30, 600)]
    [int]$ReadyTimeoutSec = 180,
    [ValidateRange(1, 30)]
    [int]$PollSeconds = 2,
    # 固定 artifacts 根目录（例如 D:\qa-agentB-docker\dkr004）；空 = 系统临时目录。
    [string]$ArtifactsRoot = '',
    # 宿主侧 127.0.0.1 映射端口。0 = 自动挑选空闲端口；默认 19543（QA 端口段约束）。
    [ValidateRange(0, 65535)]
    [int]$HostPort = 19543,
    # 只清理本脚本前次运行遗留的资源（gamer-dkr004-* 前缀的容器/网络/镜像与
    # ArtifactsRoot 下的 gamer-dkr004-* 目录），然后退出。不触碰其他部署。
    [switch]$Cleanup
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
$PSNativeCommandUseErrorActionPreference = $false

if (-not $RepoRoot) {
    $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
}
$RepoRoot = [IO.Path]::GetFullPath($RepoRoot)
Set-Location -LiteralPath $RepoRoot

$script:RealE2ERan = $false
$script:RealE2ESkippedReason = ''
$script:ExitCode = 0
$script:ArtifactsDir = ''
$script:RegistryName = ''
$script:RegistryPort = 0
$script:FixtureRepository = ''
$script:FixtureImageTags = @()
$script:ComposeFile = Join-Path $RepoRoot 'docker-compose.release.yml'
$script:OverrideFile = ''
$script:ProjectName = ''
$script:DataDir = ''
$script:ConfigDir = ''
$script:LogDir = ''
$script:BackupRoot = ''
$script:StatePath = ''
$script:TcpPort = 0
$script:AdminPassword = ''
$script:ApiSession = $null
$script:AnchorDeviceName = ''
$script:AnchorDeviceId = ''

function Fail {
    param([Parameter(Mandatory = $true)][string]$Message)
    throw "[docker-e2e] FAIL: $Message"
}

function Assert-True {
    param(
        [Parameter(Mandatory = $true)][bool]$Condition,
        [Parameter(Mandatory = $true)][string]$Message
    )
    if (-not $Condition) { Fail $Message }
}

function Assert-Equal {
    param(
        [Parameter(Mandatory = $true)][object]$Actual,
        [Parameter(Mandatory = $true)][object]$Expected,
        [Parameter(Mandatory = $true)][string]$Message
    )
    if ([string]$Actual -ne [string]$Expected) {
        Fail "$Message (actual=$Actual expected=$Expected)"
    }
}

function Write-Utf8NoBom {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Text
    )
    [IO.File]::WriteAllText(
        $Path,
        $Text + [Environment]::NewLine,
        (New-Object Text.UTF8Encoding($false))
    )
}

function Invoke-Docker {
    param(
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Label,
        [switch]$AllowFailure
    )
    # powershell.exe 5.1 陷阱：$ErrorActionPreference='Stop' 时，原生命令经 2>&1
    # 重定向的**第一行 stderr**（如 docker buildx 的进度输出）会变成终止性
    # NativeCommandError，直接炸掉调用方。因此原生调用期间必须降回 Continue。
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $lines = @(& $script:DockerCommand @Arguments 2>&1 | ForEach-Object { [string]$_ })
    } finally {
        $ErrorActionPreference = $savedEap
    }
    $exitCode = $LASTEXITCODE
    $output = ($lines -join [Environment]::NewLine).Trim()
    if ($exitCode -ne 0 -and -not $AllowFailure) {
        $detail = if ($output) { ": $output" } else { '' }
        Fail "$Label 失败（exit=$exitCode）$detail"
    }
    return [pscustomobject]@{
        ExitCode = $exitCode
        Output = $lines
        Text = $output
    }
}

function Get-LastOutputLine {
    param([Parameter(Mandatory = $true)][object]$Result)
    $lines = @($Result.Output | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) })
    if ($lines.Count -eq 0) { return '' }
    return ([string]$lines[$lines.Count - 1]).Trim()
}

function Invoke-Compose {
    param(
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$ImageReference,
        [switch]$AllowFailure
    )
    $environmentNames = @(
        'GAMER_IMAGE',
        'GAMER_DATA_DIR',
        'GAMER_CONFIG_DIR',
        'GAMER_LOG_DIR'
    )
    $saved = @{}
    foreach ($name in $environmentNames) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    }
    try {
        $env:GAMER_IMAGE = $ImageReference
        $env:GAMER_DATA_DIR = $script:DataDir
        $env:GAMER_CONFIG_DIR = $script:ConfigDir
        $env:GAMER_LOG_DIR = $script:LogDir
        $prefix = @(
            'compose',
            '-f', $script:ComposeFile,
            '-f', $script:OverrideFile,
            '--project-name', $script:ProjectName
        )
        return Invoke-Docker -Arguments ($prefix + $Arguments) -Label ('docker compose ' + ($Arguments -join ' ')) -AllowFailure:$AllowFailure
    } finally {
        foreach ($name in $environmentNames) {
            [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
        }
    }
}

function Invoke-ChildPowerShell {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [int]$ExpectedExit = 0
    )
    $pwsh = Get-Command pwsh -ErrorAction SilentlyContinue
    if (-not $pwsh) { $pwsh = Get-Command powershell -ErrorAction SilentlyContinue }
    if (-not $pwsh) { Fail 'Docker 专项测试需要 pwsh 或 powershell' }
    # 同 Invoke-Docker：PS 5.1 下子 PowerShell 的 error/warning 流经 2>&1 捕获时，
    # 需要把 ErrorActionPreference 降回 Continue，否则首条 stderr 即终止。
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $lines = @(& $pwsh.Source -NoLogo -NoProfile -File $Path @Arguments 2>&1 | ForEach-Object { [string]$_ })
    } finally {
        $ErrorActionPreference = $savedEap
    }
    $exitCode = $LASTEXITCODE
    $output = $lines -join [Environment]::NewLine
    if ($exitCode -ne $ExpectedExit) {
        Fail "子脚本退出码=$exitCode，期望=$ExpectedExit：$Path`n$output"
    }
    return [pscustomobject]@{ ExitCode = $exitCode; Output = $output }
}

function Assert-DigestReference {
    param(
        [Parameter(Mandatory = $true)][string]$Value,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if ($Value -notmatch '^[^@\s]+@sha256:[0-9a-fA-F]{64}$') {
        Fail "$Label 不是不可变 digest 引用：$Value"
    }
}

function Normalize-PathText {
    param([Parameter(Mandatory = $true)][string]$Value)
    return $Value.Trim().TrimEnd('\', '/').Replace('/', '\').ToLowerInvariant()
}

function Get-FreeTcpPort {
    $listener = New-Object -TypeName System.Net.Sockets.TcpListener -ArgumentList ([Net.IPAddress]::Loopback), 0
    try {
        $listener.Start()
        return [int]$listener.LocalEndpoint.Port
    } finally {
        $listener.Stop()
    }
}

function Test-StaticComposeContracts {
    $main = Get-Content -LiteralPath (Join-Path $RepoRoot 'docker-compose.yml') -Raw
    $local = Get-Content -LiteralPath (Join-Path $RepoRoot 'docker-compose.local.yml') -Raw
    $release = Get-Content -LiteralPath $script:ComposeFile -Raw
    $override = Get-Content -LiteralPath (Join-Path $RepoRoot 'docker-compose.release.override.example.yml') -Raw
    $usb = Get-Content -LiteralPath (Join-Path $RepoRoot 'docker-compose.usb.yml') -Raw

    Assert-True ($main -match '(?m)^\s*build:\s*$') '开发 compose 必须保留 build'
    Assert-True ($main -match '(?m)^\s*stop_grace_period:\s*30s\s*$') '开发 compose 缺少 30s stop grace'
    Assert-True ($local -match '/app/data') '本机 overlay 缺少 data bind mount'
    Assert-True ($release -notmatch '(?m)^\s*build:\s*$') 'release compose 不得包含 build'
    Assert-True ($release -match '(?m)^\s*image:\s+\$\{GAMER_IMAGE:\?') 'release compose 必须强制使用 GAMER_IMAGE'
    Assert-True ($release -match '/health/ready') 'release compose 缺少 readiness healthcheck'
    Assert-True ($release -match '(?m)^\s*-\s*\$\{GAMER_DATA_DIR:') 'release compose 缺少 data bind mount'
    Assert-True ($release -match '(?m)^\s*-\s*\$\{GAMER_CONFIG_DIR:') 'release compose 缺少 config bind mount'
    Assert-True ($release -match '(?m)^\s*-\s*\$\{GAMER_LOG_DIR:') 'release compose 缺少 log bind mount'
    Assert-True ($release -match '(?m)^\s*stop_grace_period:\s*30s\s*$') 'release compose 缺少 30s stop grace'
    Assert-True ($override -match 'GAMER_ADMIN_PASSWORD') 'release override example 缺少显式密码门禁'
    Assert-True ($usb -match '/dev/bus/usb') 'USB override 缺少 device 映射'
    Write-Host '[docker-e2e] PASS: compose static contracts'
}

function Test-ComposeConfig {
    param([Parameter(Mandatory = $true)][string]$ImageReference)
    $savedPassword = [Environment]::GetEnvironmentVariable('GAMER_ADMIN_PASSWORD', 'Process')
    try {
        $env:GAMER_ADMIN_PASSWORD = 'dkr004-static-config-only'
        $mainArgs = @('compose', '-f', (Join-Path $RepoRoot 'docker-compose.yml'), 'config', '--quiet')
        Invoke-Docker -Arguments $mainArgs -Label '开发 compose config' | Out-Null

        $localArgs = @(
            'compose', '-f', (Join-Path $RepoRoot 'docker-compose.yml'),
            '-f', (Join-Path $RepoRoot 'docker-compose.local.yml'), 'config', '--quiet'
        )
        Invoke-Docker -Arguments $localArgs -Label '本机 compose config' | Out-Null

        $usbArgs = @(
            'compose', '-f', (Join-Path $RepoRoot 'docker-compose.yml'),
            '-f', (Join-Path $RepoRoot 'docker-compose.usb.yml'), 'config', '--quiet'
        )
        Invoke-Docker -Arguments $usbArgs -Label 'USB compose config' | Out-Null

        $releaseArgs = @('compose', '-f', $script:ComposeFile, 'config', '--quiet')
        $savedImage = [Environment]::GetEnvironmentVariable('GAMER_IMAGE', 'Process')
        $savedData = [Environment]::GetEnvironmentVariable('GAMER_DATA_DIR', 'Process')
        $savedConfig = [Environment]::GetEnvironmentVariable('GAMER_CONFIG_DIR', 'Process')
        $savedLog = [Environment]::GetEnvironmentVariable('GAMER_LOG_DIR', 'Process')
        try {
            $env:GAMER_IMAGE = $ImageReference
            $env:GAMER_DATA_DIR = $script:DataDir
            $env:GAMER_CONFIG_DIR = $script:ConfigDir
            $env:GAMER_LOG_DIR = $script:LogDir
            Invoke-Docker -Arguments $releaseArgs -Label 'release compose config' | Out-Null
            $overrideArgs = @(
                'compose', '-f', $script:ComposeFile,
                '-f', (Join-Path $RepoRoot 'docker-compose.release.override.example.yml'),
                'config', '--quiet'
            )
            Invoke-Docker -Arguments $overrideArgs -Label 'release override compose config' | Out-Null
        } finally {
            [Environment]::SetEnvironmentVariable('GAMER_IMAGE', $savedImage, 'Process')
            [Environment]::SetEnvironmentVariable('GAMER_DATA_DIR', $savedData, 'Process')
            [Environment]::SetEnvironmentVariable('GAMER_CONFIG_DIR', $savedConfig, 'Process')
            [Environment]::SetEnvironmentVariable('GAMER_LOG_DIR', $savedLog, 'Process')
        }
        Write-Host '[docker-e2e] PASS: docker compose config for dev/local/USB/release/override'
    } finally {
        [Environment]::SetEnvironmentVariable('GAMER_ADMIN_PASSWORD', $savedPassword, 'Process')
    }
}

function New-TestWorkspace {
    if ($ArtifactsRoot) {
        $script:ArtifactsDir = [IO.Path]::GetFullPath($ArtifactsRoot)
    } else {
        $script:ArtifactsDir = Join-Path ([IO.Path]::GetTempPath()) ('gamer-dkr004-' + [guid]::NewGuid().ToString('N'))
    }
    $script:ProjectName = 'gamer-dkr004-' + ([guid]::NewGuid().ToString('N').Substring(0, 12))
    # 一次性随机管理密码（仅注入本次测试容器；不写死、不回显）。
    $script:AdminPassword = [guid]::NewGuid().ToString('N') + [guid]::NewGuid().ToString('N')
    $script:AnchorDeviceName = 'dkr004-anchor-' + ([guid]::NewGuid().ToString('N').Substring(0, 8))
    $script:DataDir = Join-Path $script:ArtifactsDir 'data'
    $script:ConfigDir = Join-Path $script:ArtifactsDir 'config'
    $script:LogDir = Join-Path $script:ArtifactsDir 'logs'
    $script:BackupRoot = Join-Path $script:ArtifactsDir 'backups'
    $script:StatePath = Join-Path $script:ArtifactsDir 'release-image-state.json'
    foreach ($path in @($script:DataDir, $script:ConfigDir, $script:LogDir, $script:BackupRoot)) {
        [IO.Directory]::CreateDirectory($path) | Out-Null
    }
    Write-Utf8NoBom -Path (Join-Path $script:DataDir 'dkr004-data-marker.txt') -Text 'data-survives-digest-switch'
    Write-Utf8NoBom -Path (Join-Path $script:ConfigDir 'dkr004-config-marker.toml') -Text 'config_survives = true'
    Write-Utf8NoBom -Path (Join-Path $script:LogDir 'dkr004-log-marker.txt') -Text 'log-bind-survives-digest-switch'
    $configText = @(
        'port = 8443',
        'data_dir = "/app/data"',
        'adb_path = "adb"',
        'ffmpeg_path = "ffmpeg"',
        'scrcpy_server = "/app/assets/scrcpy-server.jar"',
        'interval = "500ms"',
        'threshold = 0.85',
        'log_level = "info"',
        'judge_delay_ms = 0',
        'decode_frames = true',
        'max_size = 0',
        'bitrate_mbps = 12',
        'fps = 15',
        'idle_power_secs = 0',
        'log_retain_days = 0',
        'compute_max_concurrency = 0',
        'rtc_external_ip = ""',
        'rtc_udp_port = 0',
        'rtc_external_port = 0',
        '',
        '[auth]',
        'session_abs_secs = 43200',
        'session_idle_secs = 7200',
        'login_max_fails = 10',
        'login_window_secs = 300',
        'password_hash = ""'
    ) -join [Environment]::NewLine
    Write-Utf8NoBom -Path (Join-Path $script:ConfigDir 'config.toml') -Text $configText

    if ($HostPort -gt 0) {
        $script:TcpPort = $HostPort
    } else {
        $script:TcpPort = Get-FreeTcpPort
    }
    $script:OverrideFile = Join-Path $script:ArtifactsDir 'compose.override.yml'
    # 注意：PS 5.1 里 `@('a: ' + $var, ...)` 换行分隔的数组字面量会把 `'a: ' + $var`
    # 拆成两个元素（实测），必须先拼进变量再放进数组。
    $pwLine = '      GAMER_ADMIN_PASSWORD: ' + $script:AdminPassword
    $overrideText = @(
        'services:',
        '  gamer:',
        '    # release compose has a stable production container_name; replace it for',
        '    # this isolated project so the acceptance test never touches that service.',
        "    container_name: $($script:ProjectName)-gamer",
        '    ports: !override',
        "      - `"127.0.0.1:$($script:TcpPort):8443`""
        '    environment:',
        $pwLine
    ) -join [Environment]::NewLine
    Write-Utf8NoBom -Path $script:OverrideFile -Text $overrideText
}

function New-LocalRegistryFixtures {
    $registryImage = 'registry:2'
    $fixtureBaseImage = $BaseImage
    if (-not $fixtureBaseImage) {
        $fixtureBaseImage = 'gamer-dkr004-base:' + ([guid]::NewGuid().ToString('N').Substring(0, 12))
        Invoke-Docker -Arguments @(
            'build', '--pull=false', '-f', (Join-Path $RepoRoot 'Dockerfile'),
            '-t', $fixtureBaseImage, $RepoRoot
        ) -Label '构建当前 Dockerfile 基础镜像' | Out-Null
        $script:FixtureImageTags += $fixtureBaseImage
    }
    $baseInspect = Invoke-Docker -Arguments @('image', 'inspect', $fixtureBaseImage) -Label "检查本地基础镜像 $fixtureBaseImage" -AllowFailure
    if ($baseInspect.ExitCode -ne 0) {
        throw "本地基础镜像不存在：$fixtureBaseImage"
    }

    $fixtureRoot = Join-Path $script:ArtifactsDir 'fixture-images'
    $registryData = Join-Path $fixtureRoot 'registry-data'
    [IO.Directory]::CreateDirectory($fixtureRoot) | Out-Null
    [IO.Directory]::CreateDirectory($registryData) | Out-Null
    $script:RegistryPort = Get-FreeTcpPort
    $script:RegistryName = 'gamer-dkr004-reg-' + ([guid]::NewGuid().ToString('N').Substring(0, 12))
    $registryRun = Invoke-Docker -Arguments @(
        'run', '-d', '--restart', 'unless-stopped', '--name', $script:RegistryName,
        '-v', "$registryData`:/var/lib/registry",
        # Publish a loopback registry port. Docker Desktop's Linux daemon and
        # the Compose pull path both reach localhost through this mapping;
        # localhost is accepted as an insecure test registry by Docker.
        '-p', "$($script:RegistryPort):5000", $registryImage
    ) -Label '启动临时 registry' -AllowFailure
    if ($registryRun.ExitCode -ne 0) {
        throw "无法启动临时 registry（需要可拉取 registry:2；$($registryRun.Text)）"
    }
    try {
        Wait-RegistryRunning
        $registryRepo = "localhost:$($script:RegistryPort)/gamebot-dkr004"
        $script:FixtureRepository = $registryRepo

        Write-Utf8NoBom -Path (Join-Path $fixtureRoot 'Dockerfile.old') -Text (@(
            "FROM $fixtureBaseImage",
            'LABEL org.opencontainers.image.title="GameBot DKR-004 old healthy fixture"',
            'LABEL org.opencontainers.image.revision="dkr004-old"'
        ) -join [Environment]::NewLine)
        Write-Utf8NoBom -Path (Join-Path $fixtureRoot 'Dockerfile.new') -Text (@(
            "FROM $fixtureBaseImage",
            'LABEL org.opencontainers.image.title="GameBot DKR-004 new healthy fixture"',
            'LABEL org.opencontainers.image.revision="dkr004-new"'
        ) -join [Environment]::NewLine)
        Write-Utf8NoBom -Path (Join-Path $fixtureRoot 'Dockerfile.bad') -Text (@(
            "FROM $fixtureBaseImage",
            'LABEL org.opencontainers.image.title="GameBot DKR-004 non-starting fixture"',
            'LABEL org.opencontainers.image.revision="dkr004-bad"',
            'ENTRYPOINT ["sh", "-c", "trap ''exit 0'' TERM INT; sleep 3600"]'
        ) -join [Environment]::NewLine)

        foreach ($variant in @('old', 'new')) {
            $dockerfile = Join-Path $fixtureRoot ("Dockerfile.{0}" -f $variant)
            $tag = "$registryRepo`:$variant"
            Invoke-Docker -Arguments @('build', '--pull=false', '-f', $dockerfile, '-t', $tag, $fixtureRoot) -Label "构建 $variant healthy fixture" | Out-Null
            Invoke-Docker -Arguments @('push', $tag) -Label "推送 $variant healthy fixture" | Out-Null
            $script:FixtureImageTags += $tag
        }
        $badTag = "$registryRepo`:bad"
        Invoke-Docker -Arguments @('build', '--pull=false', '-f', (Join-Path $fixtureRoot 'Dockerfile.bad'), '-t', $badTag, $fixtureRoot) -Label '构建 bad fixture' | Out-Null
        Invoke-Docker -Arguments @('push', $badTag) -Label '推送 bad fixture' | Out-Null
        $script:FixtureImageTags += $badTag

        $old = Get-RepoDigest -Tag "$registryRepo`:old" -Repository $registryRepo
        $new = Get-RepoDigest -Tag "$registryRepo`:new" -Repository $registryRepo
        $bad = Get-RepoDigest -Tag $badTag -Repository $registryRepo
        Assert-True ($old -ne $new) 'old/new healthy fixture 解析到了同一个 digest，无法证明切换'
        return [pscustomobject]@{ Old = $old; New = $new; Bad = $bad; RegistryRepo = $registryRepo }
    } catch {
        Invoke-Docker -Arguments @('rm', '-f', $script:RegistryName) -Label '清理临时 registry' -AllowFailure | Out-Null
        throw
    }
}

function Get-RepoDigest {
    param(
        [Parameter(Mandatory = $true)][string]$Tag,
        [Parameter(Mandatory = $true)][string]$Repository
    )
    $result = Invoke-Docker -Arguments @('image', 'inspect', '--format', '{{json .RepoDigests}}', $Tag) -Label "读取 $Tag digest"
    $text = Get-LastOutputLine -Result $result
    try {
        $parsedDigests = ConvertFrom-Json -InputObject $text
    } catch {
        Fail "无法解析 $Tag RepoDigests：$text"
    }
    $digests = if ($parsedDigests -is [array]) { $parsedDigests } else { @($parsedDigests) }
    foreach ($digest in $digests) {
        if ([string]$digest -match ('^{0}@sha256:[0-9a-fA-F]{{64}}$' -f [regex]::Escape($Repository))) {
            Assert-DigestReference -Value ([string]$digest) -Label "$Tag digest"
            return [string]$digest
        }
    }
    Fail "$Tag 没有 $Repository 对应的 RepoDigest：$text"
}

function Wait-RegistryRunning {
    $deadline = (Get-Date).AddSeconds(30)
    $last = ''
    do {
        $result = Invoke-Docker -Arguments @(
            'inspect', '--format', '{{.State.Status}}|{{.State.Running}}', $script:RegistryName
        ) -Label '检查临时 registry 状态' -AllowFailure
        if ($result.ExitCode -eq 0) {
            $last = Get-LastOutputLine -Result $result
            if ($last -eq 'running|true') { return }
        }
        if ((Get-Date) -ge $deadline) {
            $logs = Invoke-Docker -Arguments @('logs', $script:RegistryName) -Label '读取临时 registry 日志' -AllowFailure
            Fail "临时 registry 未保持运行（最后状态=$last）：$($logs.Text)"
        }
        Start-Sleep -Seconds 1
    } while ($true)
}

function Get-ComposeContainerId {
    param([Parameter(Mandatory = $true)][string]$ImageReference)
    $result = Invoke-Compose -Arguments @('ps', '-q', '--all', 'gamer') -ImageReference $ImageReference
    return Get-LastOutputLine -Result $result
}

function Get-ContainerState {
    param([Parameter(Mandatory = $true)][string]$ContainerId)
    $result = Invoke-Docker -Arguments @('inspect', '--format', '{{json .State}}', $ContainerId) -Label "读取容器 $ContainerId 状态"
    try { return (ConvertFrom-Json -InputObject (Get-LastOutputLine -Result $result)) } catch { Fail "容器状态不是 JSON：$($result.Text)" }
}

function Wait-Healthy {
    param(
        [Parameter(Mandatory = $true)][string]$ImageReference,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $deadline = (Get-Date).AddSeconds($ReadyTimeoutSec)
    $last = ''
    do {
        $containerId = Get-ComposeContainerId -ImageReference $ImageReference
        if ($containerId) {
            $stateResult = Invoke-Docker -Arguments @('inspect', '--format', '{{.State.Status}}|{{.State.Health.Status}}', $containerId) -Label "读取 $Label 状态" -AllowFailure
            if ($stateResult.ExitCode -eq 0) {
                $last = Get-LastOutputLine -Result $stateResult
                $parts = $last -split '\|', 2
                $status = $parts[0]
                $health = if ($parts.Count -gt 1) { $parts[1] } else { '' }
                if ($status -in @('exited', 'dead')) { Fail "$Label readiness 失败：state=$status health=$health" }
                if ($status -eq 'running' -and $health -eq 'healthy') {
                    Write-Host "[docker-e2e] PASS: $Label ready (container=$containerId)"
                    return $containerId
                }
            }
        }
        if ((Get-Date) -ge $deadline) { Fail "$Label readiness 超时（最后 state/health=$last）" }
        Start-Sleep -Seconds $PollSeconds
    } while ($true)
}

function Get-MountFingerprint {
    param([Parameter(Mandatory = $true)][string]$ContainerId)
    $result = Invoke-Docker -Arguments @('inspect', '--format', '{{json .Mounts}}', $ContainerId) -Label "读取容器 $ContainerId bind mounts"
    # 必须先赋值再归一化数组：PS 5.1 里内联 `@(ConvertFrom-Json -InputObject $json)`
    # 会把「对象数组」重新收敛成单个「属性为数组」的对象（实测：bare 赋值=3，
    # 内联 @()=1；pwsh 无此问题），导致 bind mount 断言全挂。管道形式同样中招。
    try {
        $parsed = ConvertFrom-Json -InputObject (Get-LastOutputLine -Result $result)
    } catch {
        Fail "容器 mounts 不是 JSON：$($result.Text)"
    }
    $mounts = if ($parsed -is [array]) { $parsed } else { @($parsed) }
    if (-not $mounts -or $mounts.Count -eq 0) { Fail "容器 $ContainerId mounts 解析为空" }
    $expectedDestinations = @('/app/data', '/app/config', '/app/log')
    $items = @()
    foreach ($destination in $expectedDestinations) {
        $mount = @($mounts | Where-Object { [string]$_.Destination -eq $destination })
        Assert-Equal $mount.Count 1 "缺少唯一 bind mount $destination"
        Assert-Equal $mount[0].Type 'bind' "$destination 不是 bind mount"
        $items += [pscustomobject]@{
            Destination = $destination
            Source = [string]$mount[0].Source
            Fingerprint = "$destination=$([string]$mount[0].Source)"
        }
    }
    return [pscustomobject]@{
        Items = $items
        Fingerprint = (($items | Sort-Object Destination | ForEach-Object { $_.Fingerprint }) -join '|')
    }
}

function Assert-MountsMatch {
    param(
        [Parameter(Mandatory = $true)][object]$Actual,
        [Parameter(Mandatory = $true)][object]$Expected,
        [Parameter(Mandatory = $true)][string]$Label
    )
    Assert-Equal $Actual.Fingerprint $Expected.Fingerprint "$Label bind mount 指纹发生变化"
    foreach ($item in $Actual.Items) {
        $expectedItem = @($Expected.Items | Where-Object { $_.Destination -eq $item.Destination })[0]
        Assert-Equal (Normalize-PathText $item.Source) (Normalize-PathText $expectedItem.Source) "$Label $($item.Destination) source 发生变化"
    }
}

function Assert-Markers {
    Assert-Equal (Get-Content -LiteralPath (Join-Path $script:DataDir 'dkr004-data-marker.txt') -Raw).Trim() 'data-survives-digest-switch' 'data bind marker 丢失或被覆盖'
    Assert-Equal (Get-Content -LiteralPath (Join-Path $script:ConfigDir 'dkr004-config-marker.toml') -Raw).Trim() 'config_survives = true' 'config bind marker 丢失或被覆盖'
    Assert-Equal (Get-Content -LiteralPath (Join-Path $script:LogDir 'dkr004-log-marker.txt') -Raw).Trim() 'log-bind-survives-digest-switch' 'log bind marker 丢失或被覆盖'
}

function Assert-Backups {
    param([Parameter(Mandatory = $true)][int]$MinimumCount)
    $backups = @(Get-ChildItem -LiteralPath $script:BackupRoot -Directory -Force | Where-Object {
        Test-Path -LiteralPath (Join-Path $_.FullName 'BACKUP_READY') -PathType Leaf
    })
    Assert-True ($backups.Count -ge $MinimumCount) "BACKUP_READY 快照数量不足（actual=$($backups.Count) expected>=$MinimumCount）"
    foreach ($backup in $backups) {
        Assert-True (Test-Path -LiteralPath (Join-Path $backup.FullName 'backup.json') -PathType Leaf) "快照缺少 backup.json：$($backup.FullName)"
        Assert-True (Test-Path -LiteralPath (Join-Path $backup.FullName 'MANIFEST.sha256') -PathType Leaf) "快照缺少 MANIFEST.sha256：$($backup.FullName)"
    }
}

function Assert-ImageReference {
    param(
        [Parameter(Mandatory = $true)][string]$ContainerId,
        [Parameter(Mandatory = $true)][string]$ExpectedDigest,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $result = Invoke-Docker -Arguments @('inspect', '--format', '{{.Config.Image}}', $ContainerId) -Label "读取 $Label image reference"
    $actual = Get-LastOutputLine -Result $result
    Assert-Equal $actual $ExpectedDigest "$Label 未运行预期 digest"
}

function Assert-OutputOrder {
    param([Parameter(Mandatory = $true)][string]$Output)
    $pull = $Output.IndexOf('pull OK', [StringComparison]::Ordinal)
    $backup = $Output.IndexOf('backup OK', [StringComparison]::Ordinal)
    $ready = $Output.IndexOf('ready: healthy', [StringComparison]::Ordinal)
    Assert-True ($pull -ge 0) '升级输出缺少 pull OK'
    Assert-True ($backup -gt $pull) 'backup 没有发生在 pull 之后'
    Assert-True ($ready -gt $backup) 'ready 没有发生在 backup 之后'
}

function Invoke-Upgrade {
    param(
        [Parameter(Mandatory = $true)][string]$NewImage,
        [Parameter(Mandatory = $true)][string]$CurrentImage,
        [int]$ExpectedExit = 0
    )
    if ($script:RegistryName) { Wait-RegistryRunning }
    $upgrade = Join-Path $PSScriptRoot 'upgrade-release.ps1'
    return Invoke-ChildPowerShell -Path $upgrade -ExpectedExit $ExpectedExit -Arguments @(
        '-NewDigest', $NewImage,
        '-CurrentDigest', $CurrentImage,
        '-ComposeFile', $script:ComposeFile,
        '-OverrideFile', $script:OverrideFile,
        '-ProjectName', $script:ProjectName,
        '-DataDir', $script:DataDir,
        '-ConfigDir', $script:ConfigDir,
        '-LogDir', $script:LogDir,
        '-BackupRoot', $script:BackupRoot,
        '-StatePath', $script:StatePath,
        '-DockerCommand', $script:DockerCommand,
        '-ReadyTimeoutSec', [string]$ReadyTimeoutSec,
        '-PollSeconds', [string]$PollSeconds
    )
}

function Get-ApiBase {
    return "http://127.0.0.1:$($script:TcpPort)"
}

function Invoke-Api {
    param(
        [Parameter(Mandatory = $true)][string]$Method,
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$Json = '',
        [switch]$AllowFailure
    )
    $params = @{
        Uri = (Get-ApiBase) + $Path
        Method = $Method
        UseBasicParsing = $true
        TimeoutSec = 15
    }
    if ($script:ApiSession) { $params.WebSession = $script:ApiSession }
    if ($Json) {
        $params.ContentType = 'application/json'
        $params.Body = $Json
    }
    try {
        return Invoke-WebRequest @params
    } catch {
        if ($AllowFailure) { return $null }
        Fail "API $Method $Path 失败：$($_.Exception.Message)"
    }
}

function Wait-HttpReady {
    $deadline = (Get-Date).AddSeconds($ReadyTimeoutSec)
    do {
        $resp = Invoke-Api -Method Get -Path '/health/ready' -AllowFailure
        if ($resp -and $resp.StatusCode -eq 200 -and $resp.Content -match '"ready"\s*:\s*true') {
            Write-Host "[docker-e2e] PASS: /health/ready 200 over 127.0.0.1:$($script:TcpPort)"
            return
        }
        if ((Get-Date) -ge $deadline) { Fail "/health/ready HTTP 探测超时（127.0.0.1:$($script:TcpPort)）" }
        Start-Sleep -Seconds $PollSeconds
    } while ($true)
}

function Invoke-ApiLogin {
    $params = @{
        Uri = (Get-ApiBase) + '/api/login'
        Method = 'Post'
        UseBasicParsing = $true
        TimeoutSec = 15
        ContentType = 'application/json'
        Body = (@{ username = 'admin'; password = $script:AdminPassword } | ConvertTo-Json -Compress)
        SessionVariable = 'sess'
    }
    $resp = $null
    try { $resp = Invoke-WebRequest @params } catch { Fail "POST /api/login 失败：$($_.Exception.Message)" }
    if ($resp.StatusCode -ne 200) { Fail "POST /api/login 状态码=$($resp.StatusCode)" }
    $script:ApiSession = $sess
    Write-Host '[docker-e2e] PASS: POST /api/login 200（GAMER_ADMIN_PASSWORD 注入链路）'
}

function New-DeviceAnchor {
    $resp = Invoke-Api -Method Post -Path '/api/devices' -Json (
        @{ name = $script:AnchorDeviceName; kind = '--' } | ConvertTo-Json -Compress
    )
    $body = $resp.Content | ConvertFrom-Json
    if (-not $body.id) { Fail "设备锚点创建失败：$($resp.Content)" }
    $script:AnchorDeviceId = [string]$body.id
    Write-Host "[docker-e2e] PASS: data anchor device created id=$($script:AnchorDeviceId) name=$($script:AnchorDeviceName)"
}

function Assert-DeviceAnchor {
    param([Parameter(Mandatory = $true)][string]$Label)
    # 容器重建（升级/回滚）会丢弃进程内 session 存储（401 实证），断言前必须重新登录
    Invoke-ApiLogin
    $resp = Invoke-Api -Method Get -Path '/api/devices'
    # 同 Get-MountFingerprint：PS 5.1 下 `@($json | ConvertFrom-Json)` 会折叠对象数组
    $parsedDevices = ConvertFrom-Json -InputObject $resp.Content
    $devices = if ($parsedDevices -is [array]) { $parsedDevices } else { @($parsedDevices) }
    $hit = @($devices | Where-Object { [string]$_.id -eq $script:AnchorDeviceId })
    Assert-True ($hit.Count -eq 1) "$Label 后数据锚点设备丢失（name=$($script:AnchorDeviceName) id=$($script:AnchorDeviceId)）"
    Assert-Equal $hit[0].name $script:AnchorDeviceName "$Label 后数据锚点设备名不一致"
    Write-Host "[docker-e2e] PASS: data anchor survives $Label (id=$($script:AnchorDeviceId))"
}

function Assert-SqliteIntegrity {
    param([Parameter(Mandatory = $true)][string]$Label)
    $dbPath = Join-Path $script:DataDir 'gamer.db'
    Assert-True (Test-Path -LiteralPath $dbPath -PathType Leaf) "$Label 后缺少 gamer.db：$dbPath"
    $python = Get-Command python -ErrorAction SilentlyContinue
    if (-not $python) { Fail '宿主 python 不可用，无法执行 SQLite integrity_check' }
    # PS 5.1 向原生命令传参会弄丢嵌入双引号（python -c 直接传码会语法错误），
    # 因此把校验代码写入临时 .py 文件再执行。
    $checkScript = Join-Path $script:ArtifactsDir 'sqlite-integrity-check.py'
    Write-Utf8NoBom -Path $checkScript -Text (@(
        'import sqlite3, sys',
        'conn = sqlite3.connect(sys.argv[1])',
        'print(conn.execute("PRAGMA integrity_check").fetchone()[0])',
        'print(conn.execute("PRAGMA user_version").fetchone()[0])'
    ) -join [Environment]::NewLine)
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = @(& $python.Source $checkScript $dbPath 2>&1 | ForEach-Object { [string]$_ })
    } finally {
        $ErrorActionPreference = $savedEap
    }
    if ($LASTEXITCODE -ne 0) { Fail "SQLite integrity_check 执行失败：$($out -join '; ')" }
    $out = @($out | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) })
    Assert-True ($out.Count -ge 2) "SQLite 校验输出不完整：$($out -join '; ')"
    Assert-Equal $out[$out.Count - 2].Trim() 'ok' "$Label 后 SQLite integrity_check 不是 ok"
    Assert-Equal $out[$out.Count - 1].Trim() '1' "$Label 后 SQLite user_version 不是 1"
    Write-Host "[docker-e2e] PASS: SQLite integrity_check=ok user_version=1 ($Label)"
}

function Invoke-SigtermAcceptance {
    param([Parameter(Mandatory = $true)][string]$ImageReference)
    $containerId = Wait-Healthy -ImageReference $ImageReference -Label '旧 digest'
    $beforeMounts = Get-MountFingerprint -ContainerId $containerId
    $stopResult = Invoke-Docker -Arguments @('stop', '--time', '30', $containerId) -Label 'docker stop SIGTERM'
    Assert-Equal $stopResult.ExitCode 0 'docker stop 返回非零'
    $state = Get-ContainerState -ContainerId $containerId
    Assert-Equal $state.Status 'exited' 'docker stop 后容器没有退出'
    Assert-Equal $state.ExitCode 0 'SIGTERM 后服务退出码不是 0'
    Assert-True (-not [bool]$state.OOMKilled) 'SIGTERM 验收被 OOMKilled 污染'
    $logParts = @((Invoke-Docker -Arguments @('logs', $containerId) -Label '读取 SIGTERM 容器日志').Text)
    $fileLogs = @(Get-ChildItem -LiteralPath $script:LogDir -Filter 'gamer-server.log*' -File -Force -ErrorAction SilentlyContinue | Sort-Object Name)
    foreach ($fileLog in $fileLogs) {
        $logParts += Get-Content -LiteralPath $fileLog.FullName -Raw -ErrorAction SilentlyContinue
    }
    $logs = $logParts -join [Environment]::NewLine
    foreach ($needle in @(
        'shutdown signal: SIGTERM',
        'shutdown coordinator: draining',
        'shutdown coordinator: finished',
        'server exited'
    )) {
        Assert-True ($logs.Contains($needle)) "SIGTERM 日志缺少：$needle"
    }
    Assert-SqliteIntegrity -Label 'SIGTERM 停机'
    Write-Host '[docker-e2e] PASS: docker stop -> SIGTERM -> coordinated drain -> clean exit'
    return $beforeMounts
}

function Test-RealDockerFlow {
    param(
        [Parameter(Mandatory = $true)][string]$OldImage,
        [Parameter(Mandatory = $true)][string]$NewImage,
        [Parameter(Mandatory = $true)][string]$FailureImage
    )
    Assert-DigestReference -Value $OldImage -Label 'old digest'
    Assert-DigestReference -Value $NewImage -Label 'new digest'
    Assert-DigestReference -Value $FailureImage -Label 'failure digest'

    Test-ComposeConfig -ImageReference $OldImage

    $oldMounts = $null
    $newMounts = $null
    try {
        Invoke-Compose -Arguments @('up', '-d', '--no-build') -ImageReference $OldImage | Out-Null
        $oldMounts = Invoke-SigtermAcceptance -ImageReference $OldImage
        Invoke-Compose -Arguments @('start', 'gamer') -ImageReference $OldImage | Out-Null
        $oldContainer = Wait-Healthy -ImageReference $OldImage -Label '重启后的旧 digest'
        Wait-HttpReady
        Invoke-ApiLogin
        New-DeviceAnchor
        Assert-MountsMatch -Actual (Get-MountFingerprint -ContainerId $oldContainer) -Expected $oldMounts -Label '旧 digest 重启后'
        Assert-Markers

        $success = Invoke-Upgrade -NewImage $NewImage -CurrentImage $OldImage
        Assert-OutputOrder -Output $success.Output
        $newContainer = Wait-Healthy -ImageReference $NewImage -Label '新 digest'
        Assert-ImageReference -ContainerId $newContainer -ExpectedDigest $NewImage -Label '新 digest'
        $newMounts = Get-MountFingerprint -ContainerId $newContainer
        Assert-MountsMatch -Actual $newMounts -Expected $oldMounts -Label '健康升级后'
        Assert-Markers
        Assert-DeviceAnchor -Label '健康升级'
        Assert-Backups -MinimumCount 1
        Write-Host '[docker-e2e] PASS: pull -> backup -> digest switch -> ready; bind mounts/data preserved'

        $rollback = Invoke-Upgrade -NewImage $FailureImage -CurrentImage $NewImage -ExpectedExit 1
        Assert-True ($rollback.Output.Contains('旧 digest 已恢复并 ready') -or $rollback.Output.Contains('旧镜像回滚 ready')) "失败候选没有报告旧 digest 已恢复 ready`n$($rollback.Output)"
        $rolledBack = Wait-Healthy -ImageReference $NewImage -Label '失败后回滚的旧 digest'
        Assert-ImageReference -ContainerId $rolledBack -ExpectedDigest $NewImage -Label '失败后回滚'
        Assert-MountsMatch -Actual (Get-MountFingerprint -ContainerId $rolledBack) -Expected $newMounts -Label '失败回滚后'
        Assert-Markers
        Assert-DeviceAnchor -Label '失败回滚'
        Assert-Backups -MinimumCount 2
        Write-Host '[docker-e2e] PASS: unhealthy/non-starting candidate automatically returned to old digest'

        $script:RealE2ERan = $true
        Write-Host '[docker-e2e] REAL E2E PASS: Docker daemon digest upgrade/rollback + bind mounts + SIGTERM/drain process path'
    } finally {
        if (-not $KeepArtifacts) {
            Invoke-Compose -Arguments @('down', '--remove-orphans') -ImageReference $NewImage -AllowFailure | Out-Null
        } else {
            Write-Host "[docker-e2e] keeping running test project for diagnosis: $script:ProjectName"
        }
    }
}

function Stop-TestRegistry {
    if ($script:RegistryName) {
        if ($KeepArtifacts) {
            Write-Host "[docker-e2e] keeping registry for diagnosis: $script:RegistryName"
            return
        }
        $result = Invoke-Docker -Arguments @('rm', '-f', $script:RegistryName) -Label '清理临时 registry' -AllowFailure
        if ($result.ExitCode -eq 0) {
            Write-Host "[docker-e2e] cleaned registry: $script:RegistryName"
        } else {
            Write-Host "[docker-e2e] cleanup registry already gone or failed: $script:RegistryName"
        }
    }
}

function Stop-TestProject {
    if (-not $script:ProjectName) { return }
    if ($KeepArtifacts) { return }
    Invoke-Compose -Arguments @('down', '--remove-orphans') -ImageReference ('placeholder@sha256:' + ('0' * 64)) -AllowFailure | Out-Null
    $query = Invoke-Docker -Arguments @(
        'ps', '-aq', '--filter', "label=com.docker.compose.project=$($script:ProjectName)"
    ) -Label '清理测试 project 容器' -AllowFailure
    $containers = @($query.Output | Where-Object { $_ -match '^[0-9a-f]{12,64}$' })
    foreach ($container in $containers) {
        if ($container) {
            Invoke-Docker -Arguments @('rm', '-f', $container) -Label "清理测试容器 $container" -AllowFailure | Out-Null
            Write-Host "[docker-e2e] cleaned test container: $container"
        }
    }
}

function Remove-TestFixtureImages {
    if ($KeepArtifacts) { return }
    foreach ($tag in @($script:FixtureImageTags)) {
        Invoke-Docker -Arguments @('image', 'rm', '-f', $tag) -Label "清理 fixture image $tag" -AllowFailure | Out-Null
    }
    if ($script:FixtureRepository) {
        $danglingIds = @(Invoke-Docker -Arguments @('image', 'ls', '--filter', 'dangling=true', '-q', '--no-trunc') -Label '查找临时 dangling images' -AllowFailure | ForEach-Object { $_.Output } | Where-Object { $_ })
        foreach ($id in $danglingIds) {
            $inspect = Invoke-Docker -Arguments @('image', 'inspect', $id) -Label "检查 dangling image $id" -AllowFailure
            if ($inspect.ExitCode -eq 0 -and $inspect.Text.Contains('dkr004')) {
                Invoke-Docker -Arguments @('image', 'rm', '-f', $id) -Label "清理 dangling fixture image $id" -AllowFailure | Out-Null
            }
        }
    }
}

function Invoke-CleanupLeftovers {
    # 幂等清理：只针对本脚本专有命名前缀（gamer-dkr004-*）的上次运行遗留，
    # 不触碰任何其他 project/容器/镜像（含 registry:2 公共基础镜像）。
    $containerIds = @(
        Invoke-Docker -Arguments @('ps', '-aq', '--filter', 'name=gamer-dkr004-') -Label '查找遗留测试容器' -AllowFailure
    ).ForEach({ $_.Output }) | Where-Object { $_ -match '^[0-9a-f]{12,64}$' }
    foreach ($id in $containerIds) {
        Invoke-Docker -Arguments @('rm', '-f', $id) -Label "清理遗留容器 $id" -AllowFailure | Out-Null
        Write-Host "[docker-e2e] cleaned leftover container: $id"
    }
    $networks = @(
        Invoke-Docker -Arguments @('network', 'ls', '--format', '{{.Name}}', '--filter', 'name=gamer-dkr004-') -Label '查找遗留测试网络' -AllowFailure
    ).ForEach({ $_.Output }) | Where-Object { $_ -like 'gamer-dkr004-*' }
    foreach ($n in $networks) {
        Invoke-Docker -Arguments @('network', 'rm', $n) -Label "清理遗留网络 $n" -AllowFailure | Out-Null
        Write-Host "[docker-e2e] cleaned leftover network: $n"
    }
    $imageTags = @(
        Invoke-Docker -Arguments @('images', '--format', '{{.Repository}}:{{.Tag}}') -Label '查找遗留 fixture 镜像' -AllowFailure
    ).ForEach({ $_.Output }) | Where-Object { $_ -like 'gamer-dkr004-*' -or $_ -like '*gamebot-dkr004:*' }
    foreach ($t in $imageTags) {
        Invoke-Docker -Arguments @('image', 'rm', '-f', $t) -Label "清理遗留镜像 $t" -AllowFailure | Out-Null
        Write-Host "[docker-e2e] cleaned leftover image: $t"
    }
    $danglingIds = @(
        Invoke-Docker -Arguments @('image', 'ls', '--filter', 'dangling=true', '-q', '--no-trunc') -Label '查找遗留 dangling images' -AllowFailure
    ).ForEach({ $_.Output }) | Where-Object { $_ }
    foreach ($id in $danglingIds) {
        $inspect = Invoke-Docker -Arguments @('image', 'inspect', $id) -Label "检查 dangling image $id" -AllowFailure
        if ($inspect.ExitCode -eq 0 -and $inspect.Text.Contains('dkr004')) {
            Invoke-Docker -Arguments @('image', 'rm', '-f', $id) -Label "清理 dangling fixture image $id" -AllowFailure | Out-Null
            Write-Host "[docker-e2e] cleaned leftover dangling image: $id"
        }
    }
    if ($ArtifactsRoot) {
        $root = [IO.Path]::GetFullPath($ArtifactsRoot)
        if (Test-Path -LiteralPath $root -PathType Container) {
            $stale = @(Get-ChildItem -LiteralPath $root -Directory -Filter 'gamer-dkr004-*' -Force -ErrorAction SilentlyContinue)
            foreach ($dir in $stale) {
                Remove-Item -LiteralPath $dir.FullName -Recurse -Force -ErrorAction SilentlyContinue
                Write-Host "[docker-e2e] cleaned leftover artifacts: $($dir.FullName)"
            }
        }
    }
}

try {
    $dockerCommandInfo = Get-Command $DockerCommand -ErrorAction SilentlyContinue
    if (-not $dockerCommandInfo -and -not (Test-Path -LiteralPath $DockerCommand -PathType Leaf)) {
        Fail "找不到 Docker CLI：$DockerCommand"
    }
    $script:DockerCommand = if ($dockerCommandInfo) { $dockerCommandInfo.Source } else { [IO.Path]::GetFullPath($DockerCommand) }
    if (-not (Test-Path -LiteralPath $script:ComposeFile -PathType Leaf)) { Fail "release compose 不存在：$script:ComposeFile" }

    if ($Cleanup) {
        Invoke-CleanupLeftovers
        Write-Host '[docker-e2e] PASS: cleanup of leftover gamer-dkr004-* resources finished'
        exit 0
    }

    New-TestWorkspace
    Test-StaticComposeContracts

    # Existing mock test remains the deterministic offline substitute and is
    # always run independently of the daemon path.
    $offline = Invoke-ChildPowerShell -Path (Join-Path $PSScriptRoot 'test-upgrade-release.ps1') -Arguments @('-RepoRoot', $RepoRoot)
    Assert-True ($offline.Output -match 'PASS') 'DKR-002 离线行为测试没有 PASS'
    Write-Host '[docker-e2e] PASS: offline pull/backup/switch/ready/rollback substitute'

    $daemon = Invoke-Docker -Arguments @('info', '--format', '{{.ServerVersion}}') -Label '检测 Docker daemon' -AllowFailure
    if ($daemon.ExitCode -ne 0) {
        $script:RealE2ESkippedReason = 'Docker daemon 不可用'
    } elseif (($OldDigest -and -not $NewDigest) -or ($NewDigest -and -not $OldDigest)) {
        $script:RealE2ESkippedReason = 'OldDigest/NewDigest 必须同时提供'
    } else {
        $fixture = $null
        if (-not $OldDigest -and -not $NewDigest) {
            try {
                $fixture = New-LocalRegistryFixtures
                $OldDigest = $fixture.Old
                $NewDigest = $fixture.New
                if (-not $BadDigest) { $BadDigest = $fixture.Bad }
                Write-Host "[docker-e2e] 使用临时 registry digest fixtures：$($fixture.RegistryRepo)"
            } catch {
                $script:RealE2ESkippedReason = "无法准备真实 daemon fixture/registry：$($_.Exception.Message)"
            }
        }
        if (-not $script:RealE2ESkippedReason) {
            if (-not $BadDigest) {
                $script:RealE2ESkippedReason = '未提供 BadDigest，且无法用本地 registry 构建失败候选'
            } else {
                try {
                    Test-RealDockerFlow -OldImage $OldDigest -NewImage $NewDigest -FailureImage $BadDigest
                } catch {
                    throw
                }
            }
        }
    }

    if ($script:RealE2ERan) {
        Write-Host '[docker-e2e] NOT RUN: GHCR 外部 release digest 未提供（本次使用本地 registry actual-daemon fixtures）' -ForegroundColor Yellow
        Write-Host '[docker-e2e] NOT RUN: Android session/viewer/adb cleanup 未执行（本机未提供可纳入容器的 Android 设备）' -ForegroundColor Yellow
    } else {
        Write-Host "[docker-e2e] SKIP real daemon E2E: $($script:RealE2ESkippedReason)" -ForegroundColor Yellow
        Write-Host '[docker-e2e] NOT RUN: digest switch/real container bind-mount and SIGTERM evidence; only static + offline substitute passed' -ForegroundColor Yellow
        Write-Host '[docker-e2e] NOT RUN: GHCR external pull and Android session/viewer/adb cleanup' -ForegroundColor Yellow
        if ($RequireRealE2E) { Fail 'RequireRealE2E 已指定，但真实 Docker E2E 未完成' }
    }
} catch {
    Write-Error $_.Exception.Message
    $script:ExitCode = 1
} finally {
    Stop-TestProject
    Stop-TestRegistry
    Remove-TestFixtureImages
    if ($script:ArtifactsDir -and -not $KeepArtifacts -and (Test-Path -LiteralPath $script:ArtifactsDir)) {
        Remove-Item -LiteralPath $script:ArtifactsDir -Recurse -Force -ErrorAction SilentlyContinue
    } elseif ($script:ArtifactsDir -and (Test-Path -LiteralPath $script:ArtifactsDir)) {
        Write-Host "[docker-e2e] artifacts kept: $script:ArtifactsDir"
    }
}
exit $script:ExitCode
