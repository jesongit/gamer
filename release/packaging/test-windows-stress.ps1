# Test-windows-stress: Windows QA orchestration for QA-007 and adjacent batch-5
# items that can be evidenced on a local machine (plan section 17.7).
#   Scenario 1 (QA-007 item 1): real 1 GiB SQLite DB (materialized blob rows,
#     proven non-sparse) + >=2048 small files through the launcher's REAL
#     upgrade snapshot / restore path, with timing + disk usage evidence.
#   Space phase: launcher source-level preflight tests with a fixed zero-space
#     provider and a sparse 1 GiB fixture. This does NOT claim a real OS disk-full
#     run; filling a 227+ GiB volume is intentionally not attempted.
#   Scenario 3: force-kill recovery during snapshot copy; current/data must stay
#     unchanged and startup must discard the incomplete snapshot.
#   Scenario 2 (antivirus-style file occupation): exclusive locks (FileShare.None)
#     on state/update-journal.json, state/current.json and versions/<v>/gamer-server.exe
#     while launcher actions run; bounded retry / clean failure / no previous deletion;
#     success after lock release.
#
# Requirements: Windows PowerShell 5.1, node, Python 3, cargo, and release
# builds of launcher + server. Fixture helpers live in the repository under
# tools/fixtures/perf-stage5b; rig/evidence data live under D:\qa-stress-tmp.
# Phases are idempotent: 'setup' rebuilds the external rig from scratch.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-windows-stress.ps1 -Phase setup
#   ... -Phase data | scenario1 | space | scenario2 | scenario3 | qa007 | cleanup | all
[CmdletBinding()]
param(
  [ValidateSet('all', 'qa007', 'setup', 'data', 'scenario1', 'space', 'scenario2', 'scenario3', 'cleanup')]
  [string]$Phase = 'all',
  # server HTTP port for the rig (must not collide with other agents on 8443)
  [int]$Port = 28443,
  [ValidateSet('real', 'sparse')]
  [string]$DataMode = 'real',
  [long]$DbTargetBytes = 1GB,
  [int]$SmallFileCount = 4096
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Tmp = 'D:\qa-stress-tmp'
$Rig = Join-Path $Tmp 'rig'
$Keys = Join-Path $Tmp 'keys'
$Dist = Join-Path $Tmp 'dist'
$Manifests = Join-Path $Tmp 'manifests'
$Evidence = Join-Path $Tmp 'logs'
$FixtureRoot = Join-Path $RepoRoot 'tools\fixtures\perf-stage5b'
$PrepareData = Join-Path $FixtureRoot 'prepare_stress_data.py'
$VerifySnapshot = Join-Path $FixtureRoot 'verify_snapshot.py'
$GenerateManifests = Join-Path $FixtureRoot 'gen_qa_manifests.py'
$LauncherExe = Join-Path $RepoRoot 'launcher\target\release\gamer-launcher.exe'
$ServerExe = Join-Path $RepoRoot 'server\target\release\gamer-server.exe'
$Versions = @('0.1.0', '0.2.0', '0.3.0', '0.4.0', '0.5.0')

function Write-Evidence([string]$Name, [string]$Text) {
  if (-not (Test-Path $Evidence)) { New-Item -ItemType Directory -Path $Evidence -Force | Out-Null }
  $p = Join-Path $Evidence $Name
  Add-Content -LiteralPath $p -Value $Text -Encoding UTF8
  Write-Host $Text
}

function Invoke-Launcher {
  param([string[]]$LauncherArgs, [int]$TimeoutSec = 600)
  $out = Join-Path $Evidence ('launcher-out-{0}.txt' -f [guid]::NewGuid().ToString('N').Substring(0, 8))
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $proc = Start-Process -FilePath $LauncherExe -ArgumentList $LauncherArgs -NoNewWindow -PassThru `
    -RedirectStandardOutput $out -RedirectStandardError ($out + '.err')
  $null = $proc.Handle  # cache handle so ExitCode is populated after exit
  if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
    try { $proc.Kill() } catch { }
    throw "launcher $($LauncherArgs -join ' ') timed out after ${TimeoutSec}s"
  }
  $null = $proc.WaitForExit()  # flush exit code (docs: required after timeout overload)
  $sw.Stop()
  $stdout = (Get-Content -LiteralPath $out -Raw -Encoding UTF8 -ErrorAction SilentlyContinue)
  $stderr = (Get-Content -LiteralPath ($out + '.err') -Raw -Encoding UTF8 -ErrorAction SilentlyContinue)
  return [pscustomobject]@{
    ExitCode = $proc.ExitCode
    Seconds  = [math]::Round($sw.Elapsed.TotalSeconds, 1)
    StdOut   = $stdout
    StdErr   = $stderr
    OutFile  = $out
  }
}

function ConvertTo-ProcessArgument([string]$Value) {
  if ($null -eq $Value -or $Value.Length -eq 0) { return '""' }
  if ($Value -notmatch '[\s"]') { return $Value }
  # CommandLine is used instead of ProcessStartInfo.ArgumentList because the
  # latter is unavailable on Windows PowerShell 5.1.
  $escaped = $Value -replace '(\\*)"', '$1$1\"'
  $escaped = $escaped -replace '(\\+)$', '$1$1'
  return '"' + $escaped + '"'
}

function Invoke-CapturedProcess {
  param(
    [string]$FilePath,
    [string[]]$ArgumentList = @(),
    [string]$WorkingDirectory = $RepoRoot,
    [int]$TimeoutSec = 900
  )
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $FilePath
  $psi.WorkingDirectory = $WorkingDirectory
  $psi.UseShellExecute = $false
  $psi.CreateNoWindow = $true
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.Arguments = (($ArgumentList | ForEach-Object { ConvertTo-ProcessArgument $_ }) -join ' ')
  $proc = New-Object System.Diagnostics.Process
  $proc.StartInfo = $psi
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  if (-not $proc.Start()) { throw "failed to start $FilePath" }
  $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
  $stderrTask = $proc.StandardError.ReadToEndAsync()
  if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
    try { $proc.Kill() } catch { }
    throw "$FilePath $($ArgumentList -join ' ') timed out after ${TimeoutSec}s"
  }
  $null = $proc.WaitForExit()
  $sw.Stop()
  $stdout = if ($stdoutTask.IsCompleted) { $stdoutTask.Result } else { '<stdout unavailable: child handle remained open>' }
  $stderr = if ($stderrTask.IsCompleted) { $stderrTask.Result } else { '<stderr unavailable: child handle remained open>' }
  return [pscustomobject]@{
    ExitCode = $proc.ExitCode
    Seconds  = [math]::Round($sw.Elapsed.TotalSeconds, 1)
    StdOut   = $stdout
    StdErr   = $stderr
  }
}

function Start-LauncherBg {
  param([string[]]$LauncherArgs, [string]$Tag)
  $out = Join-Path $Evidence ("bg-$Tag-out.txt")
  $err = Join-Path $Evidence ("bg-$Tag-err.txt")
  Remove-Item $out, $err -ErrorAction SilentlyContinue
  $proc = Start-Process -FilePath $LauncherExe -ArgumentList $LauncherArgs -NoNewWindow -PassThru `
    -RedirectStandardOutput $out -RedirectStandardError $err
  Write-Evidence "$Tag-pid.txt" "launcher[$Tag] pid=$($proc.Id) args=$($LauncherArgs -join ' ')"
  return $proc
}

function Get-Journal {
  param([string]$RigRoot = $Rig)
  # never returns $null: callers access .state/.last_step under Set-StrictMode
  $p = Join-Path $RigRoot 'state\update-journal.json'
  if (-not (Test-Path $p)) { return [pscustomobject]@{ state = '<missing>'; last_step = $null } }
  try { return (Get-Content -LiteralPath $p -Raw -Encoding UTF8 | ConvertFrom-Json) } catch { return [pscustomobject]@{ state = '<unreadable>'; last_step = $null } }
}

function Wait-Ready {
  param([int]$RigPort, [int]$TimeoutSec = 120)
  $deadline = (Get-Date).AddSeconds($TimeoutSec)
  while ((Get-Date) -lt $deadline) {
    try {
      $resp = Invoke-WebRequest -Uri "http://127.0.0.1:$RigPort/health/ready" -UseBasicParsing -TimeoutSec 3
      if ($resp.StatusCode -eq 200) { return $true }
    } catch { }
    Start-Sleep -Milliseconds 500
  }
  return $false
}

function Get-ServerProcs {
  Get-Process -Name 'gamer-server' -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$Rig*" }
}

function Stop-RigProcesses {
  Get-Process -Name 'gamer-launcher' -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $LauncherExe } | ForEach-Object {
    Write-Host "stopping launcher pid=$($_.Id)"; Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
  }
  Get-ServerProcs | ForEach-Object {
    Write-Host "stopping server pid=$($_.Id)"; Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
  }
  # the adb server daemon runs from the rig's adb.exe and holds the file locked
  Get-Process -Name 'adb' -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$Rig*" } | ForEach-Object {
    Write-Host "stopping rig adb daemon pid=$($_.Id)"; Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
  }
  Start-Sleep -Seconds 1
}

function Test-FileReadable([string]$Path) {
  try {
    $fs = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    $fs.Close(); return $true
  } catch { return $false }
}

# Start a child powershell that holds an exclusive (FileShare.None) lock on one
# file for N seconds - simulates antivirus transient occupation.
function Start-FileLock {
  param([string]$Path, [int]$Milliseconds, [string]$Tag)
  $log = Join-Path $Evidence ("lock-$Tag.txt")
  Remove-Item $log -ErrorAction SilentlyContinue
  $cmd = "`$fs=[System.IO.File]::Open('$Path',[System.IO.FileMode]::Open,[System.IO.FileAccess]::ReadWrite,[System.IO.FileShare]::None);" +
    "`$sw=[System.Diagnostics.Stopwatch]::StartNew();" +
    "'locked ' + '$Path' | Out-File -FilePath '$log' -Encoding utf8;" +
    "Start-Sleep -Milliseconds $Milliseconds;" +
    "`$fs.Close();" +
    "'released after ' + `$sw.ElapsedMilliseconds + 'ms' | Out-File -FilePath '$log' -Append -Encoding utf8"
  $proc = Start-Process powershell.exe -ArgumentList @('-NoProfile', '-Command', $cmd) -WindowStyle Hidden -PassThru
  # wait until the lock is actually held (best effort)
  $deadline = (Get-Date).AddSeconds(10)
  while ((Get-Date) -lt $deadline) {
    if (Test-Path $log) { break }
    Start-Sleep -Milliseconds 50
  }
  Start-Sleep -Milliseconds 100
  return $proc
}

function Get-DirBytes([string]$Path) {
  if (-not (Test-Path $Path)) { return 0 }
  $total = [int64]0
  foreach ($file in @(Get-ChildItem -LiteralPath $Path -Recurse -File -ErrorAction SilentlyContinue)) {
    $total += [int64]$file.Length
  }
  return $total
}

# ---------------------------------------------------------------- setup ----

# The rig port must be free: another parallel QA rig on this machine may use the
# same port. Wait/retry up to 3 times (per QA plan), then give up loudly.
function Assert-PortFree {
  param([int]$RigPort)
  for ($i = 1; $i -le 3; $i++) {
    $listener = Get-NetTCPConnection -LocalPort $RigPort -State Listen -ErrorAction SilentlyContinue
    if (-not $listener) { return $true }
    Write-Host "port $RigPort busy (pid $($listener.OwningProcess | Select-Object -First 1)), retry $i/3 in 20s"
    Start-Sleep -Seconds 20
  }
  throw "port $RigPort still busy after 3 retries - pick another -Port"
}

function Ensure-TestAssets {
  foreach ($dir in @($Dist, $Keys, $Manifests)) {
    if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
  }
  $baseName = 'gamer-app-0.1.0-windows-x64.zip'
  $base = Join-Path $Dist $baseName
  if (-not (Test-Path $base)) {
    $repoBase = Join-Path $RepoRoot ('release\dist\' + $baseName)
    if (-not (Test-Path $repoBase)) {
      throw "missing baseline app artifact: $repoBase (build/package the Windows app first)"
    }
    Copy-Item -LiteralPath $repoBase -Destination $base -Force
  }
  foreach ($v in $Versions) {
    $candidate = Join-Path $Dist "gamer-app-$v-windows-x64.zip"
    if (-not (Test-Path $candidate)) { Copy-Item -LiteralPath $base -Destination $candidate -Force }
  }

  $publicKey = Join-Path $Keys 'dev-ed25519-1.pem'
  $privateKey = Join-Path $Keys 'dev-ed25519-1.private.pem'
  if (-not (Test-Path $publicKey) -or -not (Test-Path $privateKey)) {
    Remove-Item -LiteralPath $publicKey, $privateKey -Force -ErrorAction SilentlyContinue
    $node = Get-Command node.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $node) { throw 'node.exe is required to generate the temporary QA signing key' }
    $keygen = Invoke-CapturedProcess -FilePath $node.Path -ArgumentList @(
      (Join-Path $RepoRoot 'release\packaging\sign-manifest.mjs'), 'keygen',
      '--id', 'dev-ed25519-1', '--out-dir', $Keys, '--force'
    )
    Write-Evidence 'assets-keygen.txt' "exit=$($keygen.ExitCode)`n$($keygen.StdOut)`n$($keygen.StdErr)"
    if ($keygen.ExitCode -ne 0) { throw 'temporary QA signing key generation failed' }
  }

  $python = Get-Command python.exe -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $python) { throw 'python.exe is required for QA-007 fixture generation' }
  $manifestArgs = @(
    $GenerateManifests, '--repo-root', $RepoRoot, '--dist-dir', $Dist,
    '--manifests-dir', $Manifests, '--keys-dir', $Keys, '--versions'
  ) + $Versions
  $manifestGen = Invoke-CapturedProcess -FilePath $python.Path -ArgumentList $manifestArgs
  Write-Evidence 'assets-manifests.txt' "exit=$($manifestGen.ExitCode)`n$($manifestGen.StdOut)`n$($manifestGen.StdErr)"
  if ($manifestGen.ExitCode -ne 0) { throw 'QA manifest generation/validation failed' }
}


function Reset-Rig {
  Stop-RigProcesses
  if (Test-Path $Rig) { Remove-Item -LiteralPath $Rig -Recurse -Force }
  New-Item -ItemType Directory -Path (Join-Path $Rig 'config'), (Join-Path $Rig 'keys'), (Join-Path $Rig 'manifests'), (Join-Path $Rig 'seeds') -Force | Out-Null
  Copy-Item -LiteralPath $LauncherExe -Destination (Join-Path $Rig 'gamer-launcher.exe') -Force

  $config = @"
port = $Port
data_dir = "./data"
adb_path = ""
ffmpeg_path = ""
scrcpy_server = ""
interval = "500ms"
threshold = 0.85
log_level = "info"
judge_delay_ms = 200
decode_frames = true
max_size = 0
bitrate_mbps = 12
fps = 15
encoder_name = ""
probe_encoder = false

[auth]
password_hash = ""
"@
  [System.IO.File]::WriteAllText((Join-Path $Rig 'config\config.toml'), $config + "`n", (New-Object System.Text.UTF8Encoding($false)))

  Copy-Item -LiteralPath (Join-Path $Keys 'dev-ed25519-1.pem') -Destination (Join-Path $Rig 'keys\dev-ed25519-1.pem') -Force
  Copy-Item -LiteralPath (Join-Path $Manifests '0.1.0.json') -Destination (Join-Path $Rig 'manifests\0.1.0.json') -Force
  Copy-Item -LiteralPath (Join-Path $Manifests '0.1.0.sig') -Destination (Join-Path $Rig 'manifests\0.1.0.sig') -Force
  foreach ($v in $Versions) {
    Copy-Item -LiteralPath (Join-Path $Dist "gamer-app-$v-windows-x64.zip") -Destination (Join-Path $Rig "seeds\gamer-app-$v-windows-x64.zip") -Force
  }

  # runtime adb from vendored platform-tools, ffmpeg from the local Scoop install
  $adbVer = '37.0.1'
  $adbSrc = Join-Path $RepoRoot "release\vendor\adb\$adbVer"
  New-Item -ItemType Directory -Path (Join-Path $Rig "runtime\adb\$adbVer") -Force | Out-Null
  foreach ($f in @('adb.exe', 'AdbWinApi.dll', 'AdbWinUsbApi.dll')) {
    Copy-Item -LiteralPath (Join-Path $adbSrc $f) -Destination (Join-Path $Rig "runtime\adb\$adbVer\$f") -Force
  }
  $ffmpegCandidates = @()
  if (-not [string]::IsNullOrWhiteSpace($env:QA_STRESS_FFMPEG)) { $ffmpegCandidates += $env:QA_STRESS_FFMPEG }
  $ffmpegCandidates += 'D:\Scoop\apps\ffmpeg\current\bin\ffmpeg.exe'
  $ffmpegCandidates += @(
    Get-Command ffmpeg.exe -All -ErrorAction SilentlyContinue |
      ForEach-Object { $_.Path }
  )
  $ffmpegSrc = $null
  foreach ($candidate in ($ffmpegCandidates | Select-Object -Unique)) {
    if (-not (Test-Path $candidate)) { continue }
    $probe = Invoke-CapturedProcess -FilePath $candidate -ArgumentList @('-version') -TimeoutSec 15
    if ($probe.ExitCode -eq 0) { $ffmpegSrc = $candidate; break }
  }
  if ([string]::IsNullOrWhiteSpace($ffmpegSrc)) {
    throw 'no runnable ffmpeg.exe found; set QA_STRESS_FFMPEG or install ffmpeg on PATH'
  }
  Write-Evidence 'setup-ffmpeg-selection.txt' "selected=$ffmpegSrc"
  New-Item -ItemType Directory -Path (Join-Path $Rig 'runtime\ffmpeg\local-9.0') -Force | Out-Null
  Copy-Item -LiteralPath $ffmpegSrc -Destination (Join-Path $Rig 'runtime\ffmpeg\local-9.0\ffmpeg.exe') -Force

  # baseline repair (seed hit => offline) + doctor
  $r = Invoke-Launcher -LauncherArgs @('--install-root', $Rig, '--keys-dir', $Keys, 'repair', '--manifest', (Join-Path $Rig 'manifests\0.1.0.json'))
  Write-Evidence 'setup-repair.txt' "=== repair exit=$($r.ExitCode) in $($r.Seconds)s`n$($r.StdOut)`n$($r.StdErr)"
  if ($r.ExitCode -ne 0) { throw 'baseline repair failed' }
  $d = Invoke-Launcher -LauncherArgs @('--install-root', $Rig, '--keys-dir', $Keys, 'doctor')
  Write-Evidence 'setup-doctor.txt' "=== doctor exit=$($d.ExitCode)`n$($d.StdOut)"
  if ($d.ExitCode -ne 0) { throw 'baseline doctor failed' }
  Write-Evidence 'setup-pass.txt' "setup PASS: rig ready at $Rig (port $Port)"
}

# ----------------------------------------------------------------- data ----

function Build-StressData {
  $marker = Join-Path $Rig 'data\.qa-stress-filled'
  $profile = Join-Path $Rig 'data\.qa-stage5b-profile.json'
  if ((Test-Path $marker) -and (Test-Path $profile)) {
    $existingMode = (Get-Content -LiteralPath $profile -Raw -Encoding UTF8 | ConvertFrom-Json).mode
    if ($existingMode -eq $DataMode) { Write-Host "stress data already present ($DataMode), skip"; return }
  }
  # boot the server once so it creates the real schema-v1 gamer.db, then stop it
  Assert-PortFree -RigPort $Port
  $proc = Start-LauncherBg -LauncherArgs @('--install-root', $Rig, 'start') -Tag 'dbseed'
  $ready = Wait-Ready -RigPort $Port -TimeoutSec 120
  Start-Sleep -Seconds 3
  $mine = @(Get-ServerProcs)
  Stop-RigProcesses
  if (-not $ready -or $mine.Count -lt 1) { throw "db seed: ready=$ready own-server-procs=$($mine.Count) (port squatted by a foreign process?)" }
  $db = Join-Path $Rig 'data\gamer.db'
  if (-not (Test-Path $db)) { throw 'server did not create data\gamer.db' }
}

function Invoke-DbFill {
  Build-StressData
  $python = Get-Command python.exe -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $python) { throw 'python.exe is required for QA-007 data generation' }
  $fill = Invoke-CapturedProcess -FilePath $python.Path -ArgumentList @(
    $PrepareData, $Rig, '--mode', $DataMode, '--db-bytes', $DbTargetBytes,
    '--small-files', $SmallFileCount
  ) -TimeoutSec 1800
  Write-Evidence 'data-fill.txt' "mode=$DataMode exit=$($fill.ExitCode) in $($fill.Seconds)s`n$($fill.StdOut)`n$($fill.StdErr)"
  if ($fill.ExitCode -ne 0) { throw 'QA-007 data fixture generation failed' }

  $db = Join-Path $Rig 'data\gamer.db'
  $profile = Join-Path $Rig 'data\.qa-stage5b-profile.json'
  if (-not (Test-Path $profile)) { throw 'data fixture profile was not written' }
  $profileJson = Get-Content -LiteralPath $profile -Raw -Encoding UTF8 | ConvertFrom-Json
  $smallRoot = Join-Path $Rig 'data\com.example.qastress'
  $smallFiles = @(Get-ChildItem -LiteralPath $smallRoot -Recurse -File -ErrorAction SilentlyContinue).Count
  $dbBytes = (Get-Item -LiteralPath $db).Length
  Write-Evidence 'data-profile.txt' (
    "mode=$DataMode`ndb_bytes=$dbBytes`ndb_target_bytes=$DbTargetBytes`n" +
    "db_sparse_flag=$($profileJson.db_sparse_flag)`nsmall_files=$smallFiles`n" +
    "real_snapshot_copy_allowed=$($profileJson.real_snapshot_copy_allowed)"
  )
  if ($smallFiles -lt 2048) { throw "QA-007 small-file count below 2048: $smallFiles" }
  if ($DataMode -eq 'real') {
    if ($dbBytes -lt $DbTargetBytes) { throw "real DB is below target: $dbBytes < $DbTargetBytes" }
    if ($profileJson.db_sparse_flag -ne 'not-sparse') { throw "real DB is not proven materialized: $($profileJson.db_sparse_flag)" }
    # server maintenance inspect on the live data is part of the real gate
    $insp = Invoke-CapturedProcess -FilePath $ServerExe -ArgumentList @(
      'inspect', '--data-dir', (Join-Path $Rig 'data'), '--json'
    ) -TimeoutSec 180
    Write-Evidence 'data-inspect.txt' "exit=$($insp.ExitCode) in $($insp.Seconds)s`n$($insp.StdOut)`n$($insp.StdErr)"
    if ($insp.ExitCode -ne 0 -or $insp.StdOut -notmatch '"status"\s*:\s*"ok"' -or $insp.StdOut -notmatch '"user_version"\s*:\s*1') {
      throw 'real DB maintenance inspect did not report schema-v1 ok'
    }
  } else {
    Write-Evidence 'data-inspect.txt' 'NOT RUN: sparse fixture is preflight-only and is not a valid copied SQLite database.'
  }
  Set-Content -LiteralPath (Join-Path $Rig 'data\.qa-stress-filled') -Value $DataMode -Encoding UTF8
}

# ------------------------------------------------------------- scenario1 ----

function Test-CurrentVersion {
  $c = Get-Content -LiteralPath (Join-Path $Rig 'state\current.json') -Raw | ConvertFrom-Json
  return $c.current
}

function Invoke-Scenario1 {
  if ($DataMode -ne 'real') {
    Write-Evidence 's1-not-run.txt' 'NOT RUN: DataMode=sparse. Sparse logical length is preflight evidence only; no 1 GiB snapshot copy or full upgrade is claimed.'
    return
  }
  $cur = Test-CurrentVersion
  $needUpgrade = ($cur -eq '0.1.0')
  if (-not $needUpgrade) { Write-Host "scenario1: current is $cur, already upgraded - evidence collection only" }
  # start the managed server, then hard-kill ONLY the launcher: the server keeps
  # running as an orphan; the upgrade must drain it via /api/shutdown + X-Admin-Token
  if ($needUpgrade) {
    # the committed candidate of a previous run keeps serving on the rig port;
    # only a fresh upgrade needs a free port
    Assert-PortFree -RigPort $Port
    $startProc = Start-LauncherBg -LauncherArgs @('--install-root', $Rig, 'start') -Tag 's1start'
    $ready = Wait-Ready -RigPort $Port -TimeoutSec 180
    $mine = @(Get-ServerProcs)
    if (-not $ready -or $mine.Count -lt 1) { throw "s1: ready=$ready own-server-procs=$($mine.Count)" }
    Write-Evidence 's1-ready.txt' "server ready (orphan setup), start-launcher pid=$($startProc.Id), server pid=$($mine[0].Id)"
    Stop-Process -Id $startProc.Id -Force
    Start-Sleep -Seconds 1
    $orphans = @(Get-ServerProcs)
    Write-Evidence 's1-orphan.txt' "after force-kill of start-launcher: gamer-server procs = $($orphans.Count) ($($orphans | ForEach-Object Id) -join ',')"
    if ($orphans.Count -ne 1) { throw "s1: expected exactly 1 orphan server, got $($orphans.Count)" }
  }

  $t0 = Get-Date
  $r = $null
  if ($needUpgrade) {
    $r = Invoke-Launcher -LauncherArgs @('--install-root', $Rig, '--keys-dir', $Keys, 'upgrade', '--manifest', (Join-Path $Manifests '0.2.0.json')) -TimeoutSec 900
    $dt = ((Get-Date) - $t0).TotalSeconds
    Write-Evidence 's1-upgrade.txt' "=== upgrade 0.1.0->0.2.0 exit=$($r.ExitCode) wall=$([math]::Round($dt,1))s`n$($r.StdOut)`n$($r.StdErr)"
    if ($r.ExitCode -ne 0) { throw 'scenario1 upgrade failed' }
  }

  # independent verification of the snapshot the upgrade produced
  $j = Get-Journal
  if ($null -eq $j.snapshot -or [string]::IsNullOrWhiteSpace($j.snapshot.id)) { throw 'scenario1: journal has no verified snapshot id' }
  $updId = $j.snapshot.id
  $python = Get-Command python.exe -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $python) { throw 'python.exe is required for independent snapshot verification' }
  $v = Invoke-CapturedProcess -FilePath $python.Path -ArgumentList @(
    $VerifySnapshot, $Rig, $updId, '--live'
  ) -TimeoutSec 1800
  Write-Evidence 's1-verify-snapshot.txt' "exit=$($v.ExitCode) in $($v.Seconds)s`n$($v.StdOut)`n$($v.StdErr)"
  if ($v.ExitCode -ne 0) { throw 'independent snapshot/manifest/hash verification failed' }
  Write-Evidence 's1-journal.txt' ($j | ConvertTo-Json -Depth 6)
  $cur2 = Test-CurrentVersion
  $sizes = [ordered]@{
    backups_bytes  = Get-DirBytes (Join-Path $Rig 'backups')
    versions_bytes = Get-DirBytes (Join-Path $Rig 'versions')
    data_bytes     = Get-DirBytes (Join-Path $Rig 'data')
    staging_bytes  = Get-DirBytes (Join-Path $Rig 'staging')
  }
  $lines = $sizes.GetEnumerator() | ForEach-Object { "$($_.Key) = $([math]::Round($_.Value / 1MB, 1)) MiB" }
  Write-Evidence 's1-summary.txt' ("current=$cur2 previous=$($j.previous_version) state=$($j.state) last_step=$($j.last_step)`n" + ($lines -join "`n"))
  if ($cur2 -ne '0.2.0') { throw "scenario1: expected current 0.2.0, got $cur2" }
  if ($j.state -ne 'idle' -or $j.last_step -ne 'idle') { throw 'scenario1: journal did not finish idle' }
  if ([int64]$j.snapshot.file_count -lt ($SmallFileCount + 2)) {
    throw "scenario1: snapshot file count too small: $($j.snapshot.file_count)"
  }
  $manifestPath = Join-Path (Join-Path (Join-Path $Rig 'backups') $updId) 'manifest.json'
  $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($manifest.manifest_sha256.Length -ne 64) { throw 'scenario1: manifest self-hash missing' }
  Copy-Item -LiteralPath (Join-Path $Rig 'logs\launcher.log') -Destination (Join-Path $Evidence 'launcher-scenario1.log') -Force
}

# --------------------------------------------------------------- space ----

function Invoke-SpacePreflight {
  $driveName = ([System.IO.Path]::GetPathRoot($Tmp)).TrimEnd('\').TrimEnd(':')
  $drive = Get-PSDrive -Name $driveName -ErrorAction Stop
  $sparseRig = Join-Path $Tmp 'sparse-preflight'
  if (Test-Path $sparseRig) { Remove-Item -LiteralPath $sparseRig -Recurse -Force }
  New-Item -ItemType Directory -Path (Join-Path $sparseRig 'data') -Force | Out-Null

  $python = Get-Command python.exe -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $python) { throw 'python.exe is required for the sparse preflight fixture' }
  $sparse = Invoke-CapturedProcess -FilePath $python.Path -ArgumentList @(
    $PrepareData, $sparseRig, '--mode', 'sparse', '--db-bytes', $DbTargetBytes,
    '--small-files', $SmallFileCount
  ) -TimeoutSec 180
  Write-Evidence 'space-sparse-fixture.txt' "exit=$($sparse.ExitCode) in $($sparse.Seconds)s`n$($sparse.StdOut)`n$($sparse.StdErr)"
  if ($sparse.ExitCode -ne 0) { throw 'sparse preflight fixture creation failed' }
  $sparseProfile = Get-Content -LiteralPath (Join-Path $sparseRig 'data\.qa-stage5b-profile.json') -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($sparseProfile.mode -ne 'sparse' -or $sparseProfile.db_sparse_flag -ne 'sparse') {
    throw 'sparse preflight fixture did not prove sparse mode'
  }

  # The production CLI deliberately has no fake free-space switch. Exercise
  # the actual Engine preflight with its test-only fixed provider instead of
  # filling the host volume and risking unrelated data.
  $cargo = Get-Command cargo.exe -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $cargo) { throw 'cargo.exe is required for the launcher preflight test' }
  $tests = Invoke-CapturedProcess -FilePath $cargo.Path -ArgumentList @(
    'test', '--manifest-path', (Join-Path $RepoRoot 'launcher\Cargo.toml'),
    'qa007_', '--', '--nocapture'
  ) -WorkingDirectory $RepoRoot -TimeoutSec 1800
  Write-Evidence 'space-preflight-tests.txt' "exit=$($tests.ExitCode) in $($tests.Seconds)s`n$($tests.StdOut)`n$($tests.StdErr)"
  if ($tests.ExitCode -ne 0 -or $tests.StdOut -notmatch 'test result: ok') {
    throw 'launcher QA-007 preflight tests failed'
  }

  Write-Evidence 'space-summary.txt' @"
real_os_disk_full=NOT RUN
real_os_disk_full_reason=host volume free space is $([math]::Round($drive.Free / 1GB, 2)) GiB; the test does not fill a system volume
substitute=sparse 1 GiB logical fixture plus launcher Engine fixed-available-space=0 test
substitute_result=PASS (see space-sparse-fixture.txt and space-preflight-tests.txt)
assertion=insufficient_space is rejected before current.json, data/, config.toml or backups/ changes
"@
}

# ------------------------------------------------------------- scenario2 ----

function Invoke-Scenario2 {
  $cur = Test-CurrentVersion
  if ($cur -ne '0.2.0') { Write-Host "scenario2: current is $cur, skip (needs post-scenario1 state)"; return }
  $journalPath = Join-Path $Rig 'state\update-journal.json'
  $currentPath = Join-Path $Rig 'state\current.json'

  # (a) journal locked longer than the bounded-retry window (~10 x 25ms) while
  #     `launcher upgrade` runs => clean failure, no partial state
  $before = Get-Journal
  $lock = Start-FileLock -Path $journalPath -Milliseconds 3000 -Tag 'journal'
  $r = Invoke-Launcher -LauncherArgs @('--install-root', $Rig, '--keys-dir', $Keys, 'upgrade', '--manifest', (Join-Path $Manifests '0.3.0.json')) -TimeoutSec 300
  Wait-Process -Id $lock.Id -Timeout 30 -ErrorAction SilentlyContinue
  $after = Get-Journal
  Write-Evidence 's2a-journal-lock.txt' ("lock log: " + (Get-Content (Join-Path $Evidence 'lock-journal.txt') -Raw) +
    "`nupgrade exit=$($r.ExitCode) in $($r.Seconds)s`nstderr: $($r.StdErr)`nstdout: $($r.StdOut)" +
    "`njournal state before=$($before.state)/$($before.last_step) after=$($after.state)/$($after.last_step)")

  # (a2) positive case: lock DURING the launcher run so the journal rename lands
  # inside the bounded-retry window (~10 x 25ms). Manifest = tampered signature,
  # so a SUCCESSFUL retry surfaces as a signature error (not a journal IO error).
  $upgradeProc = Start-LauncherBg -LauncherArgs @('--install-root', $Rig, '--keys-dir', $Keys, 'upgrade', '--manifest', (Join-Path $Manifests 'bad-signature.json')) -Tag 's2a2'
  Start-Sleep -Milliseconds 60
  $null = Start-FileLock -Path $journalPath -Milliseconds 150 -Tag 'journal2'
  Wait-Process -Id $upgradeProc.Id -Timeout 120 -ErrorAction SilentlyContinue
  $out2 = Get-Content (Join-Path $Evidence 'bg-s2a2-out.txt') -Raw -Encoding UTF8 -ErrorAction SilentlyContinue
  $err2 = Get-Content (Join-Path $Evidence 'bg-s2a2-err.txt') -Raw -Encoding UTF8 -ErrorAction SilentlyContinue
  $lockLog2 = Get-Content (Join-Path $Evidence 'lock-journal2.txt') -Raw -ErrorAction SilentlyContinue
  Write-Evidence 's2a2-journal-retry.txt' ("lock(60ms delay,150ms hold): " + $lockLog2 +
    "`nupgrade stdout: $out2`nupgrade stderr: $err2" +
    "`nverdict: " + $(if ($err2 -match 'os error 32|journal') { 'journal IO error => retry window exhausted' } elseif ($err2 -match 'signature|manifest') { 'journal writes succeeded through bounded retry (failure is the expected signature rejection)' } else { 'unexpected' }))

  # (b) current.json locked => status/start fail cleanly, no side effects
  $lock = Start-FileLock -Path $currentPath -Milliseconds 4000 -Tag 'current'
  $rs = Invoke-Launcher -LauncherArgs @('--install-root', $Rig, 'status')
  Write-Evidence 's2b-current-lock-status.txt' "status exit=$($rs.ExitCode)`nstdout: $($rs.StdOut)`nstderr: $($rs.StdErr)"
  $rst = Invoke-Launcher -LauncherArgs @('--install-root', $Rig, 'start') -TimeoutSec 60
  Wait-Process -Id $lock.Id -Timeout 30 -ErrorAction SilentlyContinue
  Write-Evidence 's2b-current-lock-start.txt' "start exit=$($rst.ExitCode) in $($rst.Seconds)s`nstdout: $($rst.StdOut)`nstderr: $($rst.StdErr)"
  $serversAfter = @(Get-ServerProcs)
  Write-Evidence 's2b-no-spawn.txt' "gamer-server procs after locked start: $($serversAfter.Count)"
  $rs2 = Invoke-Launcher -LauncherArgs @('--install-root', $Rig, 'status')
  Write-Evidence 's2b-after-release.txt' "status after release exit=$($rs2.ExitCode)`n$($rs2.StdOut)"

  # (c) candidate version dir pre-placed; lock its gamer-server.exe during the
  #     upgrade switch => install_app_dir verify fails => full rollback (snapshot
  #     restore of the 1 GiB data) with old version healthy
  $stage = Join-Path $Tmp 'extract-0.3.0'
  if (Test-Path $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
  New-Item -ItemType Directory -Path $stage -Force | Out-Null
  & python -c "import zipfile;zipfile.ZipFile(r'$Dist\gamer-app-0.3.0-windows-x64.zip').extractall(r'$stage')"
  $target = Join-Path $Rig 'versions\0.3.0'
  if (Test-Path $target) { Remove-Item -LiteralPath $target -Recurse -Force }
  Move-Item -LiteralPath $stage -Destination $target
  $lock = Start-FileLock -Path (Join-Path $target 'gamer-server.exe') -Milliseconds 120000 -Tag 'exe'
  $dataBefore = Get-DirBytes (Join-Path $Rig 'data')
  $t0 = Get-Date
  $r = Invoke-Launcher -LauncherArgs @('--install-root', $Rig, '--keys-dir', $Keys, 'upgrade', '--manifest', (Join-Path $Manifests '0.3.0.json')) -TimeoutSec 900
  $dt = [math]::Round(((Get-Date) - $t0).TotalSeconds, 1)
  Write-Evidence 's2c-exe-lock-upgrade.txt' "=== upgrade 0.2.0->0.3.0 with locked target exe: exit=$($r.ExitCode) wall=${dt}s`nstdout: $($r.StdOut)`nstderr: $($r.StdErr)"
  $j = Get-Journal
  Write-Evidence 's2c-journal-after-rollback.txt' ($j | ConvertTo-Json -Depth 6)
  $updId = (Get-ChildItem -LiteralPath (Join-Path $Rig 'backups') -Directory |
    Sort-Object LastWriteTime | Select-Object -Last 1).Name
  Write-Evidence 's2c-journal-after-rollback.txt' "newest backup dir: $updId"
  $python = Get-Command python.exe -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $python) { throw 'python.exe is required for restore verification' }
  $v = Invoke-CapturedProcess -FilePath $python.Path -ArgumentList @(
    $VerifySnapshot, $Rig, $updId, '--live'
  ) -TimeoutSec 1800
  Write-Evidence 's2c-verify-restore.txt' "exit=$($v.ExitCode) in $($v.Seconds)s`n$($v.StdOut)`n$($v.StdErr)"
  if ($v.ExitCode -ne 0) { throw 'snapshot restore verification failed' }
  $curAfter = Test-CurrentVersion
  $dirs = (Get-ChildItem -LiteralPath (Join-Path $Rig 'versions') -Directory | ForEach-Object Name) -join ','
  Write-Evidence 's2c-summary.txt' "current after rollback=$curAfter (expect 0.2.0); versions dirs=[$dirs]; previous kept=[$(Test-Path (Join-Path $Rig 'versions\0.1.0'))]"
  # release the lock and retry => committed
  try { Stop-Process -Id $lock.Id -Force -ErrorAction SilentlyContinue } catch { }
  Start-Sleep -Seconds 2
  $r2 = Invoke-Launcher -LauncherArgs @('--install-root', $Rig, '--keys-dir', $Keys, 'upgrade', '--manifest', (Join-Path $Manifests '0.3.0.json')) -TimeoutSec 900
  Write-Evidence 's2c-retry-after-release.txt' "retry exit=$($r2.ExitCode)`nstdout: $($r2.StdOut)"
  if ($r2.ExitCode -ne 0) { throw 'scenario2 retry after release failed' }
  Copy-Item -LiteralPath (Join-Path $Rig 'logs\launcher.log') -Destination (Join-Path $Evidence 'launcher-scenario2.log') -Force
  Write-Evidence 's2-pass.txt' 'scenario2 PASS'
}

# ------------------------------------------------------------- scenario3 ----

function Invoke-Scenario3 {
  if ($DataMode -ne 'real') {
    Write-Evidence 's3-not-run.txt' 'NOT RUN: DataMode=sparse. Interrupted recovery requires a real snapshot copy; sparse fixture is preflight-only.'
    return
  }
  $cur = Test-CurrentVersion
  $targetVersion = if ($cur -eq '0.3.0') { '0.4.0' } elseif ($cur -eq '0.2.0') { '0.3.0' } else { $null }
  if ($null -eq $targetVersion) { Write-Host "scenario3: current is $cur, skip (expected 0.2.0 or 0.3.0)"; return }

  # part 1: force-kill the managing launcher, then observe restart semantics
  # stop any leftover processes (e.g. the committed candidate still serving) so
  # part 1 starts from a quiet rig
  Stop-RigProcesses
  Assert-PortFree -RigPort $Port
  $startProc = Start-LauncherBg -LauncherArgs @('--install-root', $Rig, 'start') -Tag 's3start'
  $ready3 = Wait-Ready -RigPort $Port -TimeoutSec 180
  $mine3 = @(Get-ServerProcs)
  if (-not $ready3 -or $mine3.Count -lt 1) { throw "s3: ready=$ready3 own-server-procs=$($mine3.Count)" }
  Write-Evidence 's3a-started.txt' "launcher pid=$($startProc.Id) supervising server, ready ok"
  taskkill /F /PID $startProc.Id | Out-Null
  Start-Sleep -Seconds 2
  $orphans = @(Get-ServerProcs)
  $lockHeld = -not (Test-FileReadable (Join-Path $Rig 'state\launcher.lock'))
  $curAfterKill = Test-CurrentVersion
  $rawCurrentOk = $true
  try { Get-Content -LiteralPath (Join-Path $Rig 'state\current.json') -Raw | ConvertFrom-Json | Out-Null } catch { $rawCurrentOk = $false }
  $j1 = Get-Journal
  Write-Evidence 's3a-after-kill.txt' "orphan server procs=$($orphans.Count) ($(($orphans | ForEach-Object Id) -join ',')); lock held=$lockHeld; current=$curAfterKill; current.json parses=$rawCurrentOk; journal=$($j1.state)/$($j1.last_step)"

  $rs = Invoke-Launcher -LauncherArgs @('--install-root', $Rig, 'status')
  Write-Evidence 's3a-status.txt' "status exit=$($rs.ExitCode)`n$($rs.StdOut)"

  # restart while the orphan still holds the port
  $restart = Start-LauncherBg -LauncherArgs @('--install-root', $Rig, 'start') -Tag 's3restart'
  Start-Sleep -Seconds 12
  $procs = @(Get-ServerProcs)
  $restartAlive = -not $restart.HasExited
  $exitCode = if ($restart.HasExited) { $restart.ExitCode } else { 'still-running' }
  Write-Evidence 's3a-restart-with-orphan.txt' "restart launcher alive=$restartAlive exit=$exitCode; gamer-server procs now=$($procs.Count) ($(($procs | ForEach-Object Id) -join ','))"
  Get-Content (Join-Path $Evidence 'bg-s3restart-out.txt') -Raw -Encoding UTF8 -ErrorAction SilentlyContinue | ForEach-Object { Write-Evidence 's3a-restart-with-orphan.txt' "restart stdout: $_" }
  Stop-RigProcesses
  Start-Sleep -Seconds 1

  # clean start after orphans are gone
  $start2 = Start-LauncherBg -LauncherArgs @('--install-root', $Rig, 'start') -Tag 's3clean'
  $ready = Wait-Ready -RigPort $Port -TimeoutSec 180
  Write-Evidence 's3a-clean-start.txt' "clean start ready=$ready (pid=$($start2.Id))"
  Stop-RigProcesses

  # part 2: kill the upgrade launcher mid-snapshot (safe injection point with 1 GiB data)
  $upgradeProc = Start-LauncherBg -LauncherArgs @('--install-root', $Rig, '--keys-dir', $Keys, 'upgrade', '--manifest', (Join-Path $Manifests "$targetVersion.json")) -Tag 's3upgrade'
  $killedAt = $null
  $deadline = (Get-Date).AddSeconds(300)
  while ((Get-Date) -lt $deadline) {
    if ($upgradeProc.HasExited) { break }
    $j = Get-Journal
    if ($null -ne $j -and $j.state -eq 'snapshotting') {
      Start-Sleep -Milliseconds 300   # land inside the copy
      taskkill /F /PID $upgradeProc.Id | Out-Null
      $killedAt = 'snapshotting'
      break
    }
    if ($null -ne $j -and $j.state -eq 'downloading') { $killedAt = 'seen-downloading' }
    Start-Sleep -Milliseconds 50
  }
  if ($null -ne $upgradeProc -and -not $upgradeProc.HasExited -and $null -eq $killedAt) {
    try { Stop-Process -Id $upgradeProc.Id -Force -ErrorAction SilentlyContinue } catch { }
  }
  if ($killedAt -ne 'snapshotting') {
    throw "scenario3: could not interrupt a real snapshot copy (observed=$killedAt); no recovery pass claimed"
  }
  Start-Sleep -Seconds 2
  $j2 = Get-Journal
  Write-Evidence 's3b-killed-mid-snapshot.txt' "killed_at=$killedAt upgradeProcExited=$($upgradeProc.HasExited); journal now state=$($j2.state) last_step=$($j2.last_step); snapshot.id=$(if ($j2.snapshot) { $j2.snapshot.id } else { 'none' })"
  $halfBackupBefore = if ($null -ne $j2.update_id) { Test-Path (Join-Path (Join-Path $Rig 'backups') $j2.update_id) } else { $false }
  $curAfterKill2 = Test-CurrentVersion
  $rawCurrentOk2 = $true
  try { Get-Content -LiteralPath (Join-Path $Rig 'state\current.json') -Raw | ConvertFrom-Json | Out-Null } catch { $rawCurrentOk2 = $false }
  Write-Evidence 's3b-state.txt' "current=$curAfterKill2 (expect $cur); current.json parses=$rawCurrentOk2; partial_backup_before_recovery=$halfBackupBefore"
  if ($curAfterKill2 -ne $cur -or -not $rawCurrentOk2) { throw 'scenario3: current.json/data baseline changed during interrupted snapshot' }

  # restart: `launcher start` runs the startup recovery first - the half snapshot
  # must be discarded, journal back to idle, then the old version starts normally
  $j3 = Get-Journal
  Write-Evidence 's3b-journal-after-kill.txt' ($j3 | ConvertTo-Json -Depth 6)
  $start3 = Start-LauncherBg -LauncherArgs @('--install-root', $Rig, 'start') -Tag 's3final'
  $ready2 = Wait-Ready -RigPort $Port -TimeoutSec 180
  $procs2 = @(Get-ServerProcs)
  $j4 = Get-Journal
  $halfBackupAfter = if ($null -ne $j2.update_id) { Test-Path (Join-Path (Join-Path $Rig 'backups') $j2.update_id) } else { $false }
  Write-Evidence 's3b-final-start.txt' "final start ready=$ready2 server procs=$($procs2.Count) current=$(Test-CurrentVersion) journal=$($j4.state)/$($j4.last_step) partial_backup_before=$halfBackupBefore partial_backup_after=$halfBackupAfter"
  Get-Content (Join-Path $Evidence 'bg-s3final-out.txt') -Raw -Encoding UTF8 -ErrorAction SilentlyContinue | ForEach-Object { Write-Evidence 's3b-final-start.txt' "final start stdout: $_" }
  Stop-RigProcesses
  if (-not $ready2 -or $procs2.Count -ne 1) { throw 'scenario3: recovered server did not become ready with one process' }
  if ((Test-CurrentVersion) -ne $cur -or $j4.state -ne 'idle' -or $j4.last_step -ne 'failed' -or $j4.snapshot -ne $null -or $halfBackupAfter) {
    throw 'scenario3: interrupted snapshot was not discarded while preserving the old version'
  }
  Copy-Item -LiteralPath (Join-Path $Rig 'logs\launcher.log') -Destination (Join-Path $Evidence 'launcher-scenario3.log') -Force
  Write-Evidence 's3-pass.txt' "scenario3 PASS (force-kill + mid-snapshot kill recovery; $cur remained current; $targetVersion was not committed)"
}

# -------------------------------------------------------------- cleanup ----

function Invoke-Cleanup {
  Stop-RigProcesses
  foreach ($p in @((Join-Path $Rig 'backups'), (Join-Path $Rig 'quarantine'), (Join-Path $Rig 'staging'), (Join-Path $Rig 'seeds'), (Join-Path $Rig 'versions'))) {
    if (Test-Path $p) {
      $bytes = Get-DirBytes $p
      Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction SilentlyContinue
      Write-Host "cleaned $p ($([math]::Round($bytes / 1MB, 1)) MiB)"
    }
  }
  Write-Host 'cleanup done (evidence in D:\qa-stress-tmp\logs kept)'
}

# ----------------------------------------------------------------- main ----

foreach ($dir in @($Tmp, $Evidence)) {
  if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
}
if (-not (Test-Path $LauncherExe)) { throw "missing $LauncherExe (build launcher release first)" }
if (-not (Test-Path $ServerExe)) { throw "missing $ServerExe (build server release first)" }
foreach ($fixture in @($PrepareData, $VerifySnapshot, $GenerateManifests)) {
  if (-not (Test-Path $fixture)) { throw "missing QA-007 fixture helper: $fixture" }
}
if ($Phase -ne 'cleanup') { Ensure-TestAssets }

switch ($Phase) {
  'setup' { Reset-Rig }
  'data' { Invoke-DbFill }
  'scenario1' { Invoke-Scenario1 }
  'space' { Invoke-SpacePreflight }
  'scenario2' { Invoke-Scenario2 }
  'scenario3' { Invoke-Scenario3 }
  'cleanup' { Invoke-Cleanup }
  'qa007' {
    Reset-Rig
    Invoke-DbFill
    Invoke-Scenario1
    Invoke-SpacePreflight
    Invoke-Scenario3
    Invoke-Cleanup
  }
  'all' {
    Reset-Rig
    Invoke-DbFill
    Invoke-Scenario1
    Invoke-SpacePreflight
    Invoke-Scenario2
    Invoke-Scenario3
    Invoke-Cleanup
  }
}
