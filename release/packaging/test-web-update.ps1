# Real webpage -> IPC -> version switch/rollback, using a fresh isolated install.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OldZip,
    [Parameter(Mandatory)][string]$ManifestPath,
    [Parameter(Mandatory)][string]$AssetDir,
    [Parameter(Mandatory)][string]$LauncherExe,
    [Parameter(Mandatory)][string]$InstallRoot
)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath($InstallRoot)
if (Test-Path -LiteralPath $root) { throw '验收必须使用新目录' }
Expand-Archive -LiteralPath $OldZip -DestinationPath $root
$exe=Join-Path $root 'gamer-launcher.exe'
$output=& $exe --install-root $root repair --probe 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { throw "旧版安装失败：$output" }
$oldVersion=(Get-Content "$root/state/current.json" -Raw | ConvertFrom-Json).current
# Test the repaired supervisor against an actual previous server installation.
Copy-Item -LiteralPath $LauncherExe -Destination $exe -Force
$manifest=Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
$targetVersion=$manifest.release.version
foreach ($asset in @($manifest.platforms.'windows-x86_64'.app.artifact)+@($manifest.platforms.'windows-x86_64'.components | ForEach-Object {$_.artifact})) {
    Copy-Item -LiteralPath (Join-Path $AssetDir $asset.name) -Destination "$root/seeds/$($asset.name)" -Force
}
$sourceFile=Join-Path $root 'qa-source.json'
$originalManifest=Get-Content -LiteralPath $ManifestPath -Raw
[IO.File]::WriteAllText($sourceFile,$originalManifest)
$listener=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
$listener.Start()
$port=$listener.LocalEndpoint.Port
$listener.Stop()
$config="$root/config/config.toml"
[IO.File]::WriteAllText($config,([IO.File]::ReadAllText($config) -replace '(?m)^port = 8443',"port = $port"),[Text.UTF8Encoding]::new($false))
$configHash=(Get-FileHash -LiteralPath $config).Hash
New-Item -ItemType Directory -Path "$root/data" -Force | Out-Null
[IO.File]::WriteAllText("$root/data/qa-preserve.txt",'existing user content')
$previousSource=$env:GAMER_LAUNCHER_RELEASE_MANIFEST
$env:GAMER_LAUNCHER_RELEASE_MANIFEST=$sourceFile
$proc=Start-Process -FilePath $exe -ArgumentList @('--install-root',$root,'start') -WorkingDirectory $root -WindowStyle Hidden -PassThru -RedirectStandardOutput "$root/start.log" -RedirectStandardError "$root/start.err.log"
$base="http://127.0.0.1:$port"
$headers=@{}
function Wait-For([string]$Description,[scriptblock]$Condition,[int]$Seconds=150) {
    $deadline=[DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        if (& $Condition) { Write-Host "PASS: $Description"; return }
        if ($proc.HasExited) { throw "启动器意外退出：$Description，exit=$($proc.ExitCode)" }
        Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "超时：$Description"
}
function Status { Invoke-RestMethod "$base/api/system/update" -Headers $headers -TimeoutSec 3 }
function Action([string]$Name) { $null=Invoke-RestMethod -Method Post "$base/api/system/update/$Name" -Headers $headers -TimeoutSec 5 }
function Check-Data {
    if ((Get-FileHash -LiteralPath $config).Hash -ne $configHash) {throw '用户配置被修改'}
    if ([IO.File]::ReadAllText("$root/data/qa-preserve.txt") -ne 'existing user content') {throw '用户数据被修改'}
}
try {
    Wait-For '旧版真实服务就绪' {try {(Invoke-RestMethod "$base/health/ready" -TimeoutSec 1).ready} catch {$false}}
    $headers=@{'X-Admin-Token'=(Get-Content "$root/state/admin-token" -Raw|ConvertFrom-Json).token}
    Action check
    Wait-For '网页发现目标版本' { $s=Status; $s.state -eq 'available' -and $s.candidate.version -eq $targetVersion }
    Action download
    Wait-For '网页下载完成' { (Status).state -eq 'staged' }
    # A release appearing after download must not change the accepted candidate.
    $manifest.release.version='0.9.99'
    [IO.File]::WriteAllText($sourceFile,($manifest|ConvertTo-Json -Depth 40))
    Action install
    Wait-For "网页完成 $oldVersion -> $targetVersion，启动器接管新服务" {
        try {
            $info=Invoke-RestMethod "$base/api/system/info" -Headers $headers -TimeoutSec 1
            $status=Status
            $info.app.version -eq $targetVersion -and $status.state -eq 'idle' -and (Get-Content "$root/state/current.json" -Raw|ConvertFrom-Json).current -eq $targetVersion
        } catch {$false}
    }
    Check-Data
    [IO.File]::WriteAllText($sourceFile,$originalManifest)
    Action check
    Wait-For '更新后 IPC 仍可用且检查版本正确' { (Status).last_error.code -eq 'update_not_available' }

    foreach ($path in @('/api/extensions/market/registry.json','/api/extensions/market/gamer-yaml/0.1.0-beta.1/archive')) {
        try { $null=Invoke-WebRequest "$base$path" -TimeoutSec 5; throw "市场接口允许未认证访问：$path" }
        catch { if (-not $_.Exception.Response -or [int]$_.Exception.Response.StatusCode -ne 401) { throw } }
    }
    Write-Host 'PASS: 插件市场列表和下载均要求认证'

    $registry=Invoke-RestMethod "$base/api/extensions/market/registry.json?refresh=true" -Headers $headers -TimeoutSec 40
    if ($registry.market_status.source -ne 'remote') {throw '插件市场未使用独立仓库发布源'}
    New-Item -ItemType Directory -Path "$root/qa-plugins" | Out-Null
    foreach ($entry in $registry.plugins) {
        $archive="$root/qa-plugins/$($entry.id)-$($entry.version).gplugin"
        $null=Invoke-WebRequest "$base$($entry.download_url)" -Headers $headers -OutFile $archive -TimeoutSec 40
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entry.sha256) {throw '市场归档 SHA256 不匹配'}
        $installHeaders=@{'X-Admin-Token'=$headers['X-Admin-Token'];'X-Gamer-Permission-Confirm'='1';'X-Gamer-Extension-Source'='official';'X-Expected-Sha256'=$entry.sha256}
        $null=Invoke-RestMethod -Method Post "$base/api/extensions/inspect" -Headers $installHeaders -InFile $archive -ContentType application/zip
        $plugin=Invoke-RestMethod -Method Post "$base/api/extensions" -Headers $installHeaders -InFile $archive -ContentType application/zip
        if ($plugin.state -ne 'running') {throw "市场插件未正常运行：$($entry.id)"}
        $null=Invoke-WebRequest "$base/api/extensions/$($entry.id)/ui/plugin.js" -Headers $headers
    }
    Write-Host 'PASS: 独立仓库发现、同源归档下载、SHA256、inspect、安装和 UI 均通过'

    # Intentionally dishonest candidate identity must roll back through the same IPC path.
    $manifest.release.version='0.9.98'
    [IO.File]::WriteAllText($sourceFile,($manifest|ConvertTo-Json -Depth 40))
    Action check
    Wait-For '故障候选可检查' { (Status).state -eq 'available' }
    Action download
    Wait-For '故障候选完成下载' { (Status).state -eq 'staged' }
    Action install
    Wait-For '网页更新失败后自动恢复原版，IPC 和服务均存活' {
        try {
            $s=Status
            $info=Invoke-RestMethod "$base/api/system/info" -Headers $headers -TimeoutSec 1
            $s.last_error.code -eq 'artifact_invalid' -and $s.last_error.message -like '*候选版本不符*' -and $info.app.version -eq $targetVersion -and (Get-Content "$root/state/current.json" -Raw|ConvertFrom-Json).current -eq $targetVersion
        } catch {$false}
    }
    Check-Data
    $plugins=Invoke-RestMethod "$base/api/extensions" -Headers $headers
    if (@($plugins.extensions | Where-Object state -EQ running).Count -ne $registry.plugins.Count) {throw '回滚丢失插件状态'}
} finally {
    $env:GAMER_LAUNCHER_RELEASE_MANIFEST=$previousSource
    if ($headers['X-Admin-Token']) {
        try {$null=Invoke-RestMethod -Method Post "$base/api/shutdown" -Headers $headers -TimeoutSec 95} catch {Write-Warning "隔离服务关闭失败：$_"}
    }
    $proc.WaitForExit(15000) | Out-Null
}
if (-not $proc.HasExited) {throw '关闭服务后启动器未退出'}
Write-Host 'PASS: 网页升级、插件市场和故障回滚真实验收完成'
