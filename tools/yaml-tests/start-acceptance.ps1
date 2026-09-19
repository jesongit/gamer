param([int]$Port = 18443)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$qaRoot = Join-Path $repo 'server/target/yaml-acceptance'
if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) {
    throw "端口 $Port 已占用；请先停止旧验收实例。"
}
New-Item -ItemType Directory -Path $qaRoot -Force | Out-Null
$cfg = Get-Content (Join-Path $repo 'server/config.toml') -Raw
$cfg = $cfg -replace '(?m)^port\s*=.*$', "port = $Port"
$cfg = $cfg -replace '(?m)^password_hash\s*=.*$', 'password_hash = ""'
Set-Content (Join-Path $qaRoot 'config.toml') $cfg -Encoding utf8
$env:GB_CONFIG = Join-Path $qaRoot 'config.toml'
$env:GAMER_PROFILE = 'dev'
$env:GAMER_DATA_DIR = Join-Path $qaRoot 'data'
$env:GAMER_APP_DIR = Join-Path $repo 'server'
$env:GAMER_SCRCPY_SERVER = Join-Path $repo 'server/assets/scrcpy-server.jar'
# UI/API 验收使用隔离数据，禁用设备扫描；输入/视觉执行由真实 WASM + 测试 capability 验证。
$env:GAMER_ADB_PATH = Join-Path $qaRoot 'no-adb.exe'
$env:GAMER_ADMIN_PASSWORD = [guid]::NewGuid().ToString('N')
Set-Content (Join-Path $qaRoot 'password.txt') $env:GAMER_ADMIN_PASSWORD -NoNewline
$env:GB_LOG = 'stdout'
$process = Start-Process -FilePath (Join-Path $qaRoot 'gamer-qa-server.exe') `
    -WorkingDirectory (Join-Path $repo 'server') -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $qaRoot 'server.out.log') `
    -RedirectStandardError (Join-Path $qaRoot 'server.err.log') -PassThru
Set-Content (Join-Path $qaRoot 'server.pid') $process.Id
Write-Output "YAML acceptance: http://localhost:$Port, PID $($process.Id), data $qaRoot"
