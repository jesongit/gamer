# QA-005 Windows harness.
#
# This orchestrates the existing full-package E2E and local Windows
# fault-injection cases. It intentionally does not claim clean-VM, Win10, real-AV,
# reboot, or logoff coverage when those conditions are not present.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-windows-qa005.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-windows-qa005.ps1 -Phase preflight
#   powershell -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-windows-qa005.ps1 -SkipBuild

[CmdletBinding()]
param(
    [string]$RepoRoot = '',
    [string]$WorkDir = 'D:\qa005-windows',
    [ValidateSet('all', 'preflight', 'e2e', 'cross-drive', 'long-path', 'locks')]
    [string]$Phase = 'all',
    [switch]$SkipBuild,
    # 预构建 E2E 资产来源目录（dist-m1/dist-m2/manifests/keys）；缺省沿用共享
    # 目录。缺陷修复轮用它指向带新 launcher full 包的自有资产目录。
    [string]$SourceAssets = 'D:\e2e-upgrade-tmp\m2e2e',
    # 各阶段端口基数（e2e=+1, cross-drive=+2, long-path=+3；PortA=基数-179+偏移）。
    # 并行多台架时错开即可，默认保持历史端口不变。
    [int]$PortSeed = 18640
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
}

$runId = (Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss-fff')
$RunDir = Join-Path $WorkDir ("run-$runId")
$Evidence = Join-Path $RunDir 'evidence'
$E2E = Join-Path $RepoRoot 'release\packaging\test-upgrade-launcher-e2e.ps1'
$script:Results = New-Object System.Collections.Generic.List[object]

function ConvertTo-QuotedArguments {
    param([string[]]$Items)
    $parts = foreach ($item in $Items) {
        '"' + ($item -replace '"', '\"') + '"'
    }
    return ($parts -join ' ')
}

function Ensure-Dir {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -ItemType Directory -Path $Path -Force | Out-Null
    }
}

function Write-Evidence {
    param([string]$Name, [string]$Text)
    Ensure-Dir -Path $Evidence
    Set-Content -LiteralPath (Join-Path $Evidence $Name) -Value $Text -Encoding UTF8
}

function Format-Command {
    param([string]$FilePath, [string[]]$Arguments)
    return ('"{0}" {1}' -f $FilePath, (ConvertTo-QuotedArguments -Items $Arguments))
}

function Invoke-Child {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Tag,
        [string]$WorkingDirectory = $RepoRoot,
        [int]$TimeoutSec = 900
    )
    Ensure-Dir -Path $Evidence
    $stdout = Join-Path $Evidence "$Tag.stdout.log"
    $stderr = Join-Path $Evidence "$Tag.stderr.log"
    $command = Format-Command -FilePath $FilePath -Arguments $Arguments
    Write-Host "`n=== $Tag ===" -ForegroundColor Cyan
    Write-Host $command
    $started = Get-Date
    $p = Start-Process -FilePath $FilePath -ArgumentList (ConvertTo-QuotedArguments -Items $Arguments) `
        -WorkingDirectory $WorkingDirectory -RedirectStandardOutput $stdout `
        -RedirectStandardError $stderr -NoNewWindow -PassThru
    $null = $p.Handle
    $timedOut = -not $p.WaitForExit($TimeoutSec * 1000)
    if ($timedOut) {
        try { & taskkill.exe /F /T /PID $p.Id 2>$null | Out-Null } catch { }
        $p.WaitForExit(10000) | Out-Null
    }
    $null = $p.WaitForExit()
    $exitCode = $p.ExitCode
    $elapsed = [math]::Round(((Get-Date) - $started).TotalSeconds, 1)
    $result = [pscustomobject]@{
        Tag = $Tag
        ExitCode = $exitCode
        Seconds = $elapsed
        TimedOut = $timedOut
        Command = $command
        Stdout = $stdout
        Stderr = $stderr
    }
    Write-Evidence "$Tag.command.txt" ("exit=$($result.ExitCode) timed_out=$timedOut wall=$($result.Seconds)s`n$command`nstdout=$stdout`nstderr=$stderr")
    Write-Host "exit=$($result.ExitCode) wall=$($result.Seconds)s"
    return $result
}

function Add-Result {
    param(
        [string]$Id,
        [ValidateSet('PASS', 'FAIL', 'NOT_EXECUTED')][string]$Status,
        [string]$Detail,
        [string]$EvidencePath = '',
        [string]$Command = ''
    )
    $row = [pscustomobject]@{
        Id = $Id
        Status = $Status
        Detail = $Detail
        Evidence = $EvidencePath
        Command = $Command
    }
    $script:Results.Add($row) | Out-Null
    $color = switch ($Status) { 'PASS' { 'Green' } 'FAIL' { 'Red' } default { 'Yellow' } }
    Write-Host ("[{0}] {1}: {2}" -f $Status, $Id, $Detail) -ForegroundColor $color
}

function Get-ResultStatus {
    param([bool]$Condition)
    if ($Condition) { return 'PASS' }
    return 'FAIL'
}

function Invoke-E2ECase {
    param(
        [string]$Tag,
        [string]$Scenario,
        [int]$HttpPort,
        [int]$PortA,
        [string]$InstallRootA = '',
        [string]$DataRootA = ''
    )
    $args = @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $E2E,
        '-Scenario', $Scenario, '-SkipBuild', '-RepoRoot', $RepoRoot,
        '-WorkDir', $RunDir, '-HttpPort', "$HttpPort", '-PortA', "$PortA", '-PortB', "$($PortA + 1)"
    )
    if ($InstallRootA) { $args += @('-InstallRootA', $InstallRootA) }
    if ($DataRootA) { $args += @('-DataRootA', $DataRootA) }
    $powershell = (Get-Command powershell.exe -ErrorAction Stop).Source
    $r = Invoke-Child -FilePath $powershell -Arguments $args -Tag $Tag -WorkingDirectory $RepoRoot
    if ($r.ExitCode -eq 0) {
        Add-Result -Id $Tag -Status PASS -Detail "existing E2E completed" -EvidencePath $RunDir -Command $r.Command
    } else {
        $detail = (Get-Content -LiteralPath $r.Stderr -Raw -Encoding UTF8 -ErrorAction SilentlyContinue)
        if ([string]::IsNullOrWhiteSpace($detail)) { $detail = "exit code $($r.ExitCode)" }
        Add-Result -Id $Tag -Status FAIL -Detail ($detail.Trim()) -EvidencePath $RunDir -Command $r.Command
    }
    return $r
}

function Copy-E2EAssets {
    Ensure-Dir -Path $RunDir
    $required = @(
        'dist-m1\GameBot-0.1.0-windows-x64-full.zip',
        'dist-m2\gamer-app-0.2.0-windows-x64.zip',
        'dist-m2\gamer-app-0.2.0-broken-windows-x64.zip',
        'manifests\0.1.0.json', 'manifests\0.1.0.sig',
        'manifests\0.2.0.json', 'manifests\0.2.0.json.sig',
        'manifests\0.2.0-broken.json', 'manifests\0.2.0-broken.json.sig',
        'keys\dev-ed25519-1.pem', 'keys\dev-ed25519-1.private.pem'
    )
    $missing = @($required | Where-Object { -not (Test-Path -LiteralPath (Join-Path $SourceAssets $_)) })
    if ($missing.Count -gt 0) {
        if ($SkipBuild) {
            throw "-SkipBuild requested but source E2E assets are missing: $($missing -join ', ')"
        }
        $powershell = (Get-Command powershell.exe -ErrorAction Stop).Source
        $buildArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $E2E,
            '-Scenario', 'build', '-RepoRoot', $RepoRoot, '-WorkDir', $RunDir)
        $build = Invoke-Child -FilePath $powershell -Arguments $buildArgs -Tag 'build' -WorkingDirectory $RepoRoot
        if ($build.ExitCode -ne 0) { throw "existing E2E build failed: exit $($build.ExitCode)" }
        return
    }
    foreach ($dir in @('dist-m1', 'dist-m2', 'manifests', 'keys')) {
        $src = Join-Path $SourceAssets $dir
        $dst = Join-Path $RunDir $dir
        Copy-Item -LiteralPath $src -Destination $dst -Recurse -Force
    }
    Write-Evidence 'asset-source.txt' "source=$SourceAssets`ntarget=$RunDir`nmode=copy-existing-E2E-assets"
}

function Get-Preflight {
    $os = Get-CimInstance Win32_OperatingSystem
    $computer = Get-CimInstance Win32_ComputerSystem
    $longPaths = $null
    try {
        $longPaths = Get-ItemPropertyValue -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' `
            -Name LongPathsEnabled -ErrorAction Stop
    } catch { $longPaths = 'unreadable' }
    $volumes = @{}
    foreach ($letter in @('C', 'D')) {
        $drive = Get-PSDrive -Name $letter -PSProvider FileSystem -ErrorAction SilentlyContinue
        if ($drive) {
            $volumes[$letter] = [ordered]@{
                root = $drive.Root
                free_bytes = [int64]$drive.Free
                used_bytes = [int64]$drive.Used
            }
        } else {
            $volumes[$letter] = 'missing'
        }
    }
    $vmCmd = Get-Command Get-VM -ErrorAction SilentlyContinue
    $preflight = [ordered]@{
        utc = (Get-Date).ToUniversalTime().ToString('o')
        hostname = $env:COMPUTERNAME
        os_caption = $os.Caption
        os_version = $os.Version
        os_build = $os.BuildNumber
        os_architecture = $os.OSArchitecture
        last_boot = $os.LastBootUpTime
        powershell = $PSVersionTable.PSVersion.ToString()
        cmd_ver = ((cmd.exe /c ver) -join ' ')
        system_manufacturer = $computer.Manufacturer
        system_model = $computer.Model
        clean_vm_verified = $false
        hyperv_cmd_available = ($null -ne $vmCmd)
        long_paths_enabled = $longPaths
        volumes = $volumes
        repo_root = $RepoRoot
        run_dir = $RunDir
    }
    $preflight | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $Evidence 'preflight.json') -Encoding UTF8
    return $preflight
}

function Remove-ExplicitDirectory {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $resolved = [System.IO.Path]::GetFullPath($Path)
    if ($resolved.Length -lt 8 -or $resolved -in @('C:\', 'D:\')) {
        throw "refusing to remove broad path: $resolved"
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
}

function New-LongInstallRoot {
    $cn = [string][char]0x957F + [string][char]0x8DEF + [string][char]0x5F84
    $path = 'C:\qa005-windows-long\run-' + $runId + '\GameBot ' + $cn
    $i = 1
    while ($path.Length -lt 245) {
        $path = Join-Path $path ("segment_{0}_{1}" -f $i, ('x' * 12))
        $i++
    }
    return $path
}

function Get-E2ERoot {
    param([string]$Suffix)
    $cn = [string][char]0x5347 + [string][char]0x7EA7 + [string][char]0x9A8C + [string][char]0x8BC1
    return Join-Path $RunDir ("GameBot E2E $cn`_$Suffix")
}

function Get-Journal {
    param([string]$Root)
    $path = Join-Path $Root 'state\update-journal.json'
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    try { return (Get-Content -LiteralPath $path -Raw -Encoding UTF8 | ConvertFrom-Json) } catch { return $null }
}

function Wait-Ready {
    param([int]$Port, [int]$TimeoutSec = 30)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            if ((Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$Port/health/ready" -TimeoutSec 2).StatusCode -eq 200) { return $true }
        } catch { }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

function Start-ExclusiveLock {
    param([string]$Path, [string]$Tag, [int]$Milliseconds = 3000)
    $lockLog = Join-Path $Evidence "lock-$Tag.log"
    Remove-Item -LiteralPath $lockLog -Force -ErrorAction SilentlyContinue
    $pathLiteral = $Path.Replace("'", "''")
    $logLiteral = $lockLog.Replace("'", "''")
    $code = @"
`$fs = [System.IO.File]::Open('$pathLiteral', [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
Set-Content -LiteralPath '$logLiteral' -Value 'locked' -Encoding UTF8
Start-Sleep -Milliseconds $Milliseconds
`$fs.Dispose()
Add-Content -LiteralPath '$logLiteral' -Value 'released' -Encoding UTF8
"@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code))
    $powershell = (Get-Command powershell.exe -ErrorAction Stop).Source
    $p = Start-Process -FilePath $powershell -ArgumentList @('-NoProfile', '-EncodedCommand', $encoded) -WindowStyle Hidden -PassThru
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath $lockLog)) { Start-Sleep -Milliseconds 50 }
    if (-not (Test-Path -LiteralPath $lockLog)) {
        try { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } catch { }
        throw "exclusive lock child did not signal: $Path"
    }
    Start-Sleep -Milliseconds 100
    return $p
}

function Stop-ProcessTreeByRoot {
    param([string]$Root)
    $procs = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        $_.CommandLine -and $_.CommandLine -like "*$Root*"
    })
    foreach ($proc in $procs) {
        try { & taskkill.exe /F /T /PID $proc.ProcessId 2>$null | Out-Null } catch { }
    }
    Start-Sleep -Seconds 1
}

function Start-BackgroundLauncher {
    param([string]$Root, [string[]]$Arguments, [string]$Tag)
    $stdout = Join-Path $Evidence "$Tag.stdout.log"
    $stderr = Join-Path $Evidence "$Tag.stderr.log"
    Remove-Item $stdout, $stderr -Force -ErrorAction SilentlyContinue
    $launcher = Join-Path $Root 'gamer-launcher.exe'
    $p = Start-Process -FilePath $launcher -ArgumentList (ConvertTo-QuotedArguments -Items $Arguments) `
        -WorkingDirectory $Root -RedirectStandardOutput $stdout -RedirectStandardError $stderr -NoNewWindow -PassThru
    $null = $p.Handle
    return [pscustomobject]@{ Process = $p; Stdout = $stdout; Stderr = $stderr }
}

function New-TestBlob {
    param([string]$Root, [int]$Megabytes = 256)
    $path = Join-Path $Root 'data\qa005-forcekill.bin'
    $buffer = New-Object byte[] (1MB)
    for ($i = 0; $i -lt $buffer.Length; $i++) { $buffer[$i] = 0x5A }
    $stream = New-Object System.IO.FileStream($path, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
    try {
        for ($i = 0; $i -lt $Megabytes; $i++) { $stream.Write($buffer, 0, $buffer.Length) }
    } finally { $stream.Dispose() }
    return $path
}

function Invoke-LockAndJournalCases {
    $rootA = Get-E2ERoot -Suffix 'A'
    $rootB = Get-E2ERoot -Suffix 'B'
    $launcherA = Join-Path $rootA 'gamer-launcher.exe'
    $journal = Join-Path $rootA 'state\update-journal.json'
    $current = Join-Path $rootA 'state\current.json'
    $exe = Join-Path $rootA 'versions\0.2.0\gamer-server.exe'
    $keysA = Join-Path $rootA 'keys'
    if (-not (Test-Path -LiteralPath $launcherA)) {
        Add-Result -Id 'QA-005-file-lock-and-force-kill' -Status FAIL -Detail 'standard E2E root A is missing; run phase e2e first' -EvidencePath $RunDir
        return
    }

    $journalBefore = Get-FileHash -Algorithm SHA256 -LiteralPath $journal
    $journalLock = Start-ExclusiveLock -Path $journal -Tag 'journal' -Milliseconds 3000
    try {
        $r = Invoke-Child -FilePath $launcherA -Arguments @('--install-root', $rootA, 'status') -Tag 'qa005-journal-lock-status' -WorkingDirectory $rootA
        $journalOutput = (Get-Content -LiteralPath $r.Stdout -Raw -Encoding UTF8) + (Get-Content -LiteralPath $r.Stderr -Raw -Encoding UTF8)
        $ok = ($r.ExitCode -eq 0 -and $journalOutput -match '读取失败|os error 32')
        Add-Result -Id 'QA-005-journal-lock' -Status (Get-ResultStatus -Condition $ok) `
            -Detail "status reported journal read failure while update-journal.json held exclusively: exit=$($r.ExitCode)" -EvidencePath $r.Stdout -Command $r.Command
    } finally {
        try { $journalLock.WaitForExit(10000) | Out-Null } catch { }
    }
    $journalAfter = Get-FileHash -Algorithm SHA256 -LiteralPath $journal
    Add-Result -Id 'QA-005-journal-no-half-write' -Status (Get-ResultStatus -Condition ($journalBefore.Hash -eq $journalAfter.Hash)) `
        -Detail 'journal bytes unchanged after exclusive-lock rejection' -EvidencePath $journal

    $currentBefore = Get-FileHash -Algorithm SHA256 -LiteralPath $current
    $currentLock = Start-ExclusiveLock -Path $current -Tag 'current' -Milliseconds 3000
    try {
        $r = Invoke-Child -FilePath $launcherA -Arguments @('--install-root', $rootA, 'status') -Tag 'qa005-current-lock-status' -WorkingDirectory $rootA
        $ok = ($r.ExitCode -ne 0 -and (Get-Content -LiteralPath $r.Stderr -Raw -Encoding UTF8) -match 'os error 32|current.json')
        Add-Result -Id 'QA-005-current-lock' -Status (Get-ResultStatus -Condition $ok) `
            -Detail "status while current.json held exclusively: exit=$($r.ExitCode)" -EvidencePath $r.Stderr -Command $r.Command
    } finally {
        try { $currentLock.WaitForExit(10000) | Out-Null } catch { }
    }
    $currentAfter = Get-FileHash -Algorithm SHA256 -LiteralPath $current
    Add-Result -Id 'QA-005-current-no-half-write' -Status (Get-ResultStatus -Condition ($currentBefore.Hash -eq $currentAfter.Hash)) `
        -Detail 'current.json bytes unchanged after exclusive-lock rejection' -EvidencePath $current

    $exeLock = Start-ExclusiveLock -Path $exe -Tag 'server-exe' -Milliseconds 4000
    try {
        $r = Invoke-Child -FilePath $launcherA -Arguments @('--install-root', $rootA, 'start') -Tag 'qa005-exe-lock-start' -WorkingDirectory $rootA -TimeoutSec 20
        $ready = Wait-Ready -Port ($PortSeed - 179) -TimeoutSec 3
        $ok = ($r.ExitCode -ne 0 -and -not $ready)
        Add-Result -Id 'QA-005-exe-lock' -Status (Get-ResultStatus -Condition $ok) `
            -Detail "start while versions/0.2.0/gamer-server.exe held exclusively: exit=$($r.ExitCode), ready=$ready" -EvidencePath $r.Stderr -Command $r.Command
    } finally {
        try { $exeLock.WaitForExit(10000) | Out-Null } catch { }
    }

    $blob = New-TestBlob -Root $rootB -Megabytes 256
    Write-Evidence 'force-kill-fixture.txt' "root=$rootB`nblob=$blob`nbytes=$((Get-Item -LiteralPath $blob).Length)"
    $launcherB = Join-Path $rootB 'gamer-launcher.exe'
    $manifestB = Join-Path $RunDir 'manifests\0.2.0-broken.json'
    $background = Start-BackgroundLauncher -Root $rootB -Arguments @('--install-root', $rootB, '--keys-dir', (Join-Path $rootB 'keys'), 'upgrade', '--manifest', $manifestB) -Tag 'qa005-mid-upgrade'
    $seen = New-Object System.Collections.Generic.List[string]
    $killedAt = ''
    $deadline = (Get-Date).AddSeconds(180)
    while (-not $background.Process.HasExited -and (Get-Date) -lt $deadline) {
        $j = Get-Journal -Root $rootB
        if ($j) {
            $state = "$($j.state)|$($j.last_step)"
            if (-not $seen.Contains($state)) { $seen.Add($state) | Out-Null }
            if ($j.state -eq 'snapshotting') {
                $killedAt = $state
                & taskkill.exe /F /PID $background.Process.Id 2>$null | Out-Null
                break
            }
        }
        Start-Sleep -Milliseconds 20
    }
    Start-Sleep -Seconds 1
    $jKilled = Get-Journal -Root $rootB
    $pointerKilled = $null
    try { $pointerKilled = Get-Content -LiteralPath (Join-Path $rootB 'state\current.json') -Raw -Encoding UTF8 | ConvertFrom-Json } catch { }
    Write-Evidence 'force-kill-mid-upgrade.txt' ("killed_at=$killedAt`nprocess_exited=$($background.Process.HasExited)`nseen=$($seen -join ',')`njournal=$(if($jKilled){$jKilled.state + '/' + $jKilled.last_step}else{'unreadable'})`ncurrent=$(if($pointerKilled){$pointerKilled.current}else{'unreadable'})")
    $killObserved = (-not [string]::IsNullOrWhiteSpace($killedAt) -and $null -ne $pointerKilled -and $pointerKilled.current -eq '0.1.0')
    Add-Result -Id 'QA-005-launcher-force-kill-mid-upgrade' -Status (Get-ResultStatus -Condition $killObserved) `
        -Detail "launcher force-killed during snapshotting; killed_at=$killedAt current=$(if($pointerKilled){$pointerKilled.current}else{'unreadable'})" `
        -EvidencePath (Join-Path $Evidence 'force-kill-mid-upgrade.txt')

    $recovery = Start-BackgroundLauncher -Root $rootB -Arguments @('--install-root', $rootB, 'start') -Tag 'qa005-recovery-start'
    $ready = Wait-Ready -Port ($PortSeed - 178) -TimeoutSec 60
    Start-Sleep -Seconds 1
    $jRecovered = Get-Journal -Root $rootB
    $pointerRecovered = $null
    try { $pointerRecovered = Get-Content -LiteralPath (Join-Path $rootB 'state\current.json') -Raw -Encoding UTF8 | ConvertFrom-Json } catch { }
    $recoveryOut = Get-Content -LiteralPath $recovery.Stdout -Raw -Encoding UTF8 -ErrorAction SilentlyContinue
    Write-Evidence 'force-kill-recovery.txt' ("ready=$ready`ncurrent=$(if($pointerRecovered){$pointerRecovered.current}else{'unreadable'})`njournal=$(if($jRecovered){$jRecovered.state + '/' + $jRecovered.last_step}else{'unreadable'})`nstdout=$recoveryOut")
    $recoveryOk = ($ready -and $null -ne $pointerRecovered -and $pointerRecovered.current -eq '0.1.0' -and $null -ne $jRecovered -and $jRecovered.state -eq 'idle')
    Add-Result -Id 'QA-005-journal-recovery-after-force-kill' -Status (Get-ResultStatus -Condition $recoveryOk) `
        -Detail "launcher start recovered interrupted journal: ready=$ready state=$(if($jRecovered){$jRecovered.state}else{'unreadable'})" `
        -EvidencePath (Join-Path $Evidence 'force-kill-recovery.txt')

    if ($ready -and -not $recovery.Process.HasExited) {
        & taskkill.exe /F /PID $recovery.Process.Id 2>$null | Out-Null
        Start-Sleep -Seconds 1
        $orphanReady = Wait-Ready -Port ($PortSeed - 178) -TimeoutSec 3
        Add-Result -Id 'QA-005-launcher-force-kill-orphan-server' -Status (Get-ResultStatus -Condition $orphanReady) `
            -Detail "force-killing supervising launcher left the server ready=$orphanReady" -EvidencePath (Join-Path $Evidence 'force-kill-recovery.txt')
    }
    Stop-ProcessTreeByRoot -Root $rootB
}

Ensure-Dir -Path $Evidence
$preflight = Get-Preflight
Write-Host "RepoRoot=$RepoRoot"
Write-Host "RunDir=$RunDir"
Write-Host "OS=$($preflight.os_caption) build=$($preflight.os_build) arch=$($preflight.os_architecture)"
Write-Host "LongPathsEnabled=$($preflight.long_paths_enabled)"

try {
    if ($Phase -eq 'all' -or $Phase -eq 'e2e' -or $Phase -eq 'cross-drive' -or $Phase -eq 'long-path') {
        Copy-E2EAssets
    }

    if ($Phase -eq 'all' -or $Phase -eq 'e2e') {
        Invoke-E2ECase -Tag 'qa005-standard-e2e' -Scenario 'all' -HttpPort ($PortSeed + 1) -PortA ($PortSeed - 179) | Out-Null
    }

    if ($Phase -eq 'all' -or $Phase -eq 'cross-drive') {
        $cn = [string][char]0x8DE8 + [string][char]0x76D8
        $crossRoot = 'C:\qa005-windows-cross\GameBot ' + $cn + ' QA'
        $crossData = Join-Path $RunDir 'cross-drive-data-physical'
        Remove-ExplicitDirectory -Path $crossData
        Invoke-E2ECase -Tag 'qa005-cross-drive-e2e' -Scenario 'upgrade' -HttpPort ($PortSeed + 2) -PortA ($PortSeed - 178) `
            -InstallRootA $crossRoot -DataRootA $crossData | Out-Null
        $junction = Join-Path $crossRoot 'data'
        $listing = @(& cmd.exe /d /c ('dir /al "{0}"' -f $crossRoot) 2>&1) -join "`n"
        $crossJournal = $null
        try { $crossJournal = Get-Content -LiteralPath (Join-Path $crossRoot 'state\update-journal.json') -Raw -Encoding UTF8 | ConvertFrom-Json } catch { }
        $crossJournalSummary = if ($crossJournal) {
            "journal_state=$($crossJournal.state)/$($crossJournal.last_step)`njournal_error_code=$(if($crossJournal.error){$crossJournal.error.code}else{'null'})`njournal_error_message=$(if($crossJournal.error){$crossJournal.error.message}else{'null'})"
        } else { 'journal=unreadable' }
        Write-Evidence 'cross-drive-layout.txt' "install_root=$crossRoot`nlogical_data=$junction`nphysical_data=$crossData`ninstall_drive=$($crossRoot.Substring(0,1))`ndata_drive=$($crossData.Substring(0,1))`n$crossJournalSummary`n$listing"
        if ((Test-Path -LiteralPath $crossData) -and (Test-Path -LiteralPath $junction)) {
            $crossDetail = if ($crossJournal -and $crossJournal.error) {
                "layout exists; upgrade stopped with $($crossJournal.error.code): $($crossJournal.error.message)"
            } else { 'logical root/data on C with junction target on D' }
            Add-Result -Id 'QA-005-cross-drive-layout' -Status PASS `
                -Detail $crossDetail `
                -EvidencePath (Join-Path $Evidence 'cross-drive-layout.txt')
        } else {
            Add-Result -Id 'QA-005-cross-drive-layout' -Status FAIL -Detail 'cross-drive data target or junction missing' `
                -EvidencePath (Join-Path $Evidence 'cross-drive-layout.txt')
        }
    }

    if ($Phase -eq 'all' -or $Phase -eq 'long-path') {
        $longRoot = New-LongInstallRoot
        $longExe = Join-Path $longRoot 'versions\0.1.0\gamer-server.exe'
        Write-Evidence 'long-path-plan.txt' "install_root=$longRoot`ninstall_root_length=$($longRoot.Length)`nexpected_server_path_length=$($longExe.Length)`nlong_paths_enabled=$($preflight.long_paths_enabled)"
        $longRun = Invoke-E2ECase -Tag 'qa005-long-path-e2e' -Scenario 'upgrade' -HttpPort ($PortSeed + 3) -PortA ($PortSeed - 177) `
            -InstallRootA $longRoot
        if ($longRun.ExitCode -eq 0) {
            Add-Result -Id 'QA-005-long-path' -Status PASS `
                -Detail "E2E passed with install root length $($longRoot.Length) and server path length $($longExe.Length)" `
                -EvidencePath (Join-Path $Evidence 'long-path-plan.txt') -Command $longRun.Command
        } else {
            Add-Result -Id 'QA-005-long-path' -Status FAIL `
                -Detail "E2E failed with install root length $($longRoot.Length) and server path length $($longExe.Length); see stderr" `
                -EvidencePath (Join-Path $Evidence 'qa005-long-path-e2e.stderr.log') -Command $longRun.Command
        }
    }

    if ($Phase -eq 'all' -or $Phase -eq 'locks') {
        Invoke-LockAndJournalCases
    }

    Add-Result -Id 'QA-005-win10-x64-clean-vm' -Status NOT_EXECUTED `
        -Detail 'no Windows 10 clean VM was available; current host identity is recorded in preflight.json'
    Add-Result -Id 'QA-005-win11-x64-clean-vm' -Status NOT_EXECUTED `
        -Detail 'current host is Windows 11 x64 but is not a clean VM; host E2E is not promoted to clean-VM evidence'
    Add-Result -Id 'QA-005-real-av-lock' -Status NOT_EXECUTED `
        -Detail 'FileShare.None lock simulation is exercised by this harness; no real antivirus engine was changed or asserted'
    Add-Result -Id 'QA-005-reboot-logoff' -Status NOT_EXECUTED `
        -Detail 'not issued on the shared desktop/VM host because reboot or logoff would terminate the active QA session; requires operator-run clean VM checkpoint'
} finally {
    $summaryJson = Join-Path $Evidence 'qa005-summary.json'
    $script:Results | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryJson -Encoding UTF8
    $md = @(
        '# QA-005 run summary',
        '',
        "- UTC run: $runId",
        "- Host: $($preflight.os_caption) / build $($preflight.os_build) / $($preflight.os_architecture)",
        "- Clean VM verified: $($preflight.clean_vm_verified)",
        "- Evidence directory: $Evidence",
        '',
        '| ID | Status | Detail | Evidence |',
        '|---|---|---|---|'
    )
    foreach ($row in $script:Results) {
        $detail = ($row.Detail -replace '\|', '\\|') -replace "`r?`n", ' '
        $evidenceCell = if ($row.Evidence) { $row.Evidence } else { '' }
        $md += "| $($row.Id) | $($row.Status) | $detail | $evidenceCell |"
    }
    Set-Content -LiteralPath (Join-Path $Evidence 'qa005-summary.md') -Value ($md -join "`n") -Encoding UTF8
}

$failures = @($script:Results | Where-Object Status -eq 'FAIL')
Write-Host "`n=== QA-005 result ===" -ForegroundColor White
Write-Host "summary=$Evidence\qa005-summary.md"
if ($failures.Count -gt 0) {
    Write-Host "FAILURES=$($failures.Count)" -ForegroundColor Red
    exit 1
}
Write-Host 'PASS (with explicit NOT_EXECUTED blockers recorded)' -ForegroundColor Green
exit 0
