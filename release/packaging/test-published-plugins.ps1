# 真实发行包验收：默认离线；-RemotePlugins 强制启动器从插件 Release 下载。
[CmdletBinding()]
param([Parameter(Mandatory)][string]$ZipPath, [Parameter(Mandatory)][string]$InstallRoot, [switch]$RemotePlugins)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$root = [IO.Path]::GetFullPath($InstallRoot)
if (Test-Path -LiteralPath $root) { throw '验收目录必须是新目录，拒绝覆盖已有安装' }
Expand-Archive -LiteralPath $ZipPath -DestinationPath $root
$exe = Join-Path $root 'gamer-launcher.exe'
$manifestFile = Get-ChildItem -LiteralPath (Join-Path $root 'manifests') -Filter '*.json' | Select-Object -First 1
$manifest = Get-Content -LiteralPath $manifestFile.FullName -Raw | ConvertFrom-Json
$component = $manifest.platforms.'windows-x86_64'.components | Where-Object id -EQ 'official-plugins'
if ($component.artifact.url -notlike 'https://github.com/jesongit/gamer-plugins/releases/download/*') { throw '启动器插件来源未指向插件仓 Release' }
if ($RemotePlugins) {
    Move-Item -LiteralPath (Join-Path $root "seeds/$($component.artifact.name)") -Destination (Join-Path $root 'excluded-plugin-seed.zip')
}
function Invoke-Launcher([string[]]$Arguments) {
    $output = & $exe --install-root $root @Arguments 2>&1 | Out-String
    $code = $LASTEXITCODE
    Add-Content -LiteralPath (Join-Path $root 'acceptance.log') -Value $output
    if ($code -ne 0) { throw "启动器失败(exit=$code): $Arguments`n$output" }
    Write-Host "PASS: launcher $Arguments"
}
Invoke-Launcher @('repair', '--probe')
Invoke-Launcher @('doctor', '--deep', '--probe')
if ($RemotePlugins) {
    $downloaded = Join-Path $root "cache/artifacts/$($component.artifact.name)"
    if ((Get-FileHash -LiteralPath $downloaded -Algorithm SHA256).Hash.ToLowerInvariant() -ne $component.artifact.sha256) { throw '远程插件下载未进入校验缓存' }
    Write-Host 'PASS: plugin bundle downloaded from published GitHub Release'
}
$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
$listener.Start()
$port = $listener.LocalEndpoint.Port
$listener.Stop()
$config = Join-Path $root 'config/config.toml'
[IO.File]::WriteAllText($config, ([IO.File]::ReadAllText($config) -replace '(?m)^port = 8443', "port = $port"), [Text.UTF8Encoding]::new($false))
$proc = Start-Process -FilePath $exe -ArgumentList @('--install-root', $root, 'start') -WorkingDirectory $root -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $root 'start.log') -RedirectStandardError (Join-Path $root 'start.err.log')
$oldTestRoot = $env:GAMER_TEST_INSTALL_ROOT
try {
    $ready = $false
    for ($i = 0; $i -lt 60; $i++) {
        try { if ((Invoke-RestMethod "http://127.0.0.1:$port/health/ready" -TimeoutSec 1).ready) { $ready = $true; break } } catch { }
        Start-Sleep -Milliseconds 500
    }
    if (-not $ready) { throw '真实服务未就绪' }
    $token = (Get-Content -LiteralPath (Join-Path $root 'state/admin-token') -Raw | ConvertFrom-Json).token
    $headers = @{'X-Admin-Token'=$token}
    $env:GAMER_TEST_INSTALL_ROOT = $root
    & cargo test --locked --manifest-path (Join-Path $repo 'launcher/Cargo.toml') --test published_plugins installs_published_plugins_using_launcher_selection_flow -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { throw '启动器真实插件选择安装验收失败' }
    $extensions = Invoke-RestMethod "http://127.0.0.1:$port/api/extensions" -Headers $headers
    if (@($extensions.extensions).Count -ne 3) { throw '已安装插件数量不符' }
    foreach ($p in $extensions.extensions) {
        if ($p.state -ne 'running' -or $p.active_version -ne '0.1.0-beta.1') { throw "插件未正常运行: $($p.id) $($p.state) $($p.active_version)" }
        $ui = Invoke-WebRequest "http://127.0.0.1:$port/api/extensions/$($p.id)/ui/plugin.js" -Headers $headers
        if ($ui.StatusCode -ne 200 -or $ui.RawContentLength -eq 0) { throw "插件 UI 不可用: $($p.id)" }
    }
    $info = Invoke-RestMethod "http://127.0.0.1:$port/api/system/info" -Headers $headers
    if ($info.app.version -ne $manifest.release.version) { throw '本体版本与发行清单不一致' }
    Write-Host "PASS: $($info.app.version), all three plugins running with accessible UI"
} finally {
    $env:GAMER_TEST_INSTALL_ROOT = $oldTestRoot
    $tokenPath = Join-Path $root 'state/admin-token'
    if (Test-Path -LiteralPath $tokenPath) {
        $token = (Get-Content -LiteralPath $tokenPath -Raw | ConvertFrom-Json).token
        $null = Invoke-RestMethod -Method Post "http://127.0.0.1:$port/api/shutdown" -Headers @{'X-Admin-Token'=$token} -TimeoutSec 95
    }
    $proc.WaitForExit(10000) | Out-Null
    if (-not $proc.HasExited) { throw '验收启动器未随服务退出，请检查隔离安装日志' }
}
Write-Host "PASS: published plugin acceptance at $root"
