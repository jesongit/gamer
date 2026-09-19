$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$qaRoot = Join-Path $repo 'server/target/yaml-acceptance'
$pidFile = Join-Path $qaRoot 'server.pid'
if (!(Test-Path -LiteralPath $pidFile)) { return }
$qaPid = [int](Get-Content -LiteralPath $pidFile)
$qaProcess = Get-Process -Id $qaPid -ErrorAction SilentlyContinue
if ($qaProcess) {
    $expectedExe = Join-Path $qaRoot 'gamer-qa-server.exe'
    if ($qaProcess.Path -ne $expectedExe) { throw 'PID 已被其他进程复用，拒绝停止。' }
    Stop-Process -Id $qaPid
}
Remove-Item -LiteralPath $pidFile
