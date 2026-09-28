# Real portable startup smoke. Only operates on the supplied disposable installation.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$Root, [int]$Port = 18449)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path -LiteralPath $Root).Path
if ($Port -eq 8443) { throw 'Smoke must not use the development port' }
$exe = Join-Path $Root 'Gamer.exe'
$config = Join-Path $Root 'config/config.toml'
if (-not (Test-Path -LiteralPath $exe) -or -not (Test-Path -LiteralPath $config)) { throw 'Incomplete portable installation' }
$original = [IO.File]::ReadAllText($config)
$smokeConfig = $original -replace '(?m)^port\s*=.*$', "port = $Port"
[IO.File]::WriteAllText($config, $smokeConfig, (New-Object Text.UTF8Encoding($false)))
$saved = @{}
foreach ($key in @('GAMER_INSTALL_ROOT','GAMER_PORTABLE_CHILD','GAMER_LOCAL_ONLY','ADB_MDNS')) { $saved[$key] = [Environment]::GetEnvironmentVariable($key) }
$child = $null
try {
    $env:GAMER_INSTALL_ROOT = $Root
    $env:GAMER_PORTABLE_CHILD = '1'
    $env:GAMER_LOCAL_ONLY = '1'
    $env:ADB_MDNS = '0'
    $child = Start-Process -FilePath $exe -WorkingDirectory $Root -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    $ready = $false
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($child.HasExited) { throw "Gamer exited: $($child.ExitCode)" }
        try { $health = Invoke-RestMethod "http://127.0.0.1:$Port/health/ready" -TimeoutSec 3; $ready = $health.ready -eq $true } catch {}
        if ($ready) { break }
        Start-Sleep -Milliseconds 250
    }
    if (-not $ready) { throw 'Portable server did not become ready' }
    $credential = Get-Content -LiteralPath (Join-Path $Root 'state/admin-token') -Raw | ConvertFrom-Json
    $headers = @{ 'X-Admin-Token'=$credential.token }
    $info = Invoke-RestMethod "http://127.0.0.1:$Port/api/system/info" -Headers $headers
    if ($info.deployment.mode -ne 'portable' -or -not $info.capabilities.install) { throw 'Direct update capability is unavailable' }
    $entry = Invoke-WebRequest "http://127.0.0.1:$Port/" -UseBasicParsing
    if ($entry.StatusCode -ne 200 -or $entry.Content -notmatch '<html') { throw 'Frontend entry unavailable' }
    Invoke-RestMethod "http://127.0.0.1:$Port/api/shutdown" -Method Post -Headers $headers -TimeoutSec 95 | Out-Null
    if (-not $child.WaitForExit(15000)) { throw 'Gamer did not exit after shutdown' }
    Write-Host "PASS portable startup, frontend, update capability and shutdown ($($info.app.version))"
} finally {
    if ($child -and -not $child.HasExited) { Stop-Process -Id $child.Id -Force }
    foreach ($key in $saved.Keys) { [Environment]::SetEnvironmentVariable($key, $saved[$key]) }
    [IO.File]::WriteAllText($config, $original, (New-Object Text.UTF8Encoding($false)))
}
