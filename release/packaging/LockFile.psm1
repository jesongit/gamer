# GameBot 打包共享工具：dependencies.lock.toml 解析、下载校验、哈希。
# 兼容 Windows PowerShell 5.1 与 pwsh；供 fetch-adb.ps1 / fetch-ffmpeg.ps1 等脚本复用。

$script:TLS12 = 0

function Initialize-DownloadEnvironment {
    # PS 5.1 默认不含 TLS1.2，GitHub/dl.google.com 均需要；静默下载以提高大文件速度。
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch { }
    $ProgressPreference = 'SilentlyContinue'
}

function Import-LockComponents {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        throw "依赖锁文件不存在: $Path（应位于 release/dependencies.lock.toml）"
    }

    $components = New-Object System.Collections.Generic.List[object]
    $current = $null       # 当前 [[component]]
    $currentFile = $null   # 当前 [[component.files]]

    foreach ($raw in [System.IO.File]::ReadAllLines($Path)) {
        $line = $raw.Trim()
        if ($line.Length -eq 0 -or $line.StartsWith('#')) { continue }

        if ($line -eq '[[component]]') {
            $current = @{ files = (New-Object System.Collections.Generic.List[object]) }
            $components.Add($current)
            $currentFile = $null
            continue
        }
        if ($line -eq '[[component.files]]') {
            if ($null -eq $current) { throw "锁文件结构错误: [[component.files]] 出现在 [[component]] 之前" }
            $currentFile = @{}
            $current.files.Add($currentFile)
            continue
        }
        if ($line.StartsWith('[')) {
            # 未知表头：后续键值不归属到当前组件（避免误解析未来扩展段）
            $current = $null
            $currentFile = $null
            continue
        }

        $eq = $line.IndexOf('=')
        if ($eq -lt 1) { continue }
        $key = $line.Substring(0, $eq).Trim()
        $val = $line.Substring($eq + 1).Trim()

        if ($val.StartsWith('"')) {
            $end = $val.IndexOf('"', 1)
            if ($end -lt 1) { throw "锁文件值引号未闭合: $line" }
            $val = $val.Substring(1, $end - 1)
        } elseif ($val -match '^-?[0-9]+$') {
            $val = [long]$val
        } else {
            $ci = $val.IndexOf('#')
            if ($ci -ge 0) { $val = $val.Substring(0, $ci).Trim() }
        }

        if ($null -ne $currentFile) {
            $currentFile[$key] = $val
        } elseif ($null -ne $current -and $key -ne 'files') {
            $current[$key] = $val
        }
    }

    if ($components.Count -eq 0) { throw "锁文件未解析到任何 [[component]] 条目: $Path" }
    return $components
}

function Get-LockComponent {
    param(
        [Parameter(Mandatory = $true)]$Components,
        [Parameter(Mandatory = $true)][string]$Id
    )
    foreach ($c in $Components) {
        if ($c['id'] -eq $Id) { return $c }
    }
    throw "锁文件缺少组件条目: id = '$Id'"
}

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Resolve-ProxyCandidate {
    param([string]$Proxy)
    if (-not [string]::IsNullOrWhiteSpace($Proxy)) { return $Proxy }
    if (-not [string]::IsNullOrWhiteSpace($env:HTTPS_PROXY)) { return $env:HTTPS_PROXY }
    if (-not [string]::IsNullOrWhiteSpace($env:HTTP_PROXY)) { return $env:HTTP_PROXY }
    return ''
}

function Invoke-VerifiedDownload {
    # 下载到 <DestFile>.tmp → 校验 sha256/size → 原子改名。任何失败都删除 .tmp，不留半包。
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)][string]$DestFile,
        [string]$ExpectedSha256 = '',
        [long]$ExpectedSize = 0,
        [string]$Proxy = ''
    )

    $tmp = "$DestFile.download-tmp"
    if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force }

    try {
        $useProxy = Resolve-ProxyCandidate -Proxy $Proxy
        try {
            if ($useProxy) {
                Invoke-WebRequest -Uri $Url -OutFile $tmp -UseBasicParsing -Proxy $useProxy -TimeoutSec 900
            } else {
                Invoke-WebRequest -Uri $Url -OutFile $tmp -UseBasicParsing -TimeoutSec 900
            }
        } catch {
            # 直连失败且尚未用过代理时，用环境代理重试一次（常见于需要本地代理访问 GitHub 的环境）
            if ($useProxy -or [string]::IsNullOrWhiteSpace((Resolve-ProxyCandidate -Proxy ''))) { throw }
            Write-Host "  直连失败（$($_.Exception.Message)），改用代理重试: $((Resolve-ProxyCandidate -Proxy ''))"
            Invoke-WebRequest -Uri $Url -OutFile $tmp -UseBasicParsing -Proxy (Resolve-ProxyCandidate -Proxy '') -TimeoutSec 900
        }

        if (-not (Test-Path -LiteralPath $tmp)) { throw "下载未产生文件: $Url" }

        $size = (Get-Item -LiteralPath $tmp).Length
        if ($ExpectedSize -gt 0 -and $size -ne $ExpectedSize) {
            throw "下载大小不符: $Url（期望 $ExpectedSize 字节，实际 $size 字节）"
        }

        $sha = Get-Sha256 -Path $tmp
        if (-not [string]::IsNullOrWhiteSpace($ExpectedSha256) -and $sha -ne $ExpectedSha256.ToLowerInvariant()) {
            throw "下载 sha256 不符: $Url`n  期望: $ExpectedSha256`n  实际: $sha"
        }

        if (Test-Path -LiteralPath $DestFile) { Remove-Item -LiteralPath $DestFile -Force }
        Move-Item -LiteralPath $tmp -Destination $DestFile
        Write-Host "  下载完成并通过校验: $(Split-Path -Leaf $DestFile)（$size 字节, sha256=$($sha.Substring(0,16))...）"
    } catch {
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue }
        throw
    }
}

function Test-LockFilesInDir {
    # 校验目录内文件与锁 files[] 一致（逐文件 sha256+size）。
    # 返回 $true 全部一致；差异细节写 host，并由调用方决定 fail。$StrictExtras=$true 时多余文件也报错。
    param(
        [Parameter(Mandatory = $true)]$Files,
        [Parameter(Mandatory = $true)][string]$Dir,
        [switch]$StrictExtras
    )

    $ok = $true
    $seen = @{}
    foreach ($f in $Files) {
        $name = [string]$f['path']
        $seen[$name.ToLowerInvariant()] = $true
        $fileOk = $true
        $p = Join-Path $Dir $name
        if (-not (Test-Path -LiteralPath $p)) {
            Write-Host "  [缺失] $name" -ForegroundColor Red
            $ok = $false
            continue
        }
        $size = (Get-Item -LiteralPath $p).Length
        if ($f['size'] -is [long] -and $size -ne [long]$f['size']) {
            Write-Host "  [大小不符] $name（期望 $($f['size'])，实际 $size）" -ForegroundColor Red
            $fileOk = $false
        }
        $sha = Get-Sha256 -Path $p
        if ($sha -ne ([string]$f['sha256']).ToLowerInvariant()) {
            Write-Host "  [sha256 不符] $name（期望 $($f['sha256'])，实际 $sha）" -ForegroundColor Red
            $fileOk = $false
        }
        if ($fileOk) {
            Write-Host "  [OK] $name（$size 字节, sha256=$($sha.Substring(0,16))...）"
        } else {
            $ok = $false
        }
    }

    foreach ($existing in (Get-ChildItem -LiteralPath $Dir -File)) {
        if (-not $seen.ContainsKey($existing.Name.ToLowerInvariant())) {
            $msg = "  [额外文件] $($existing.Name)（不在锁 files[] 中）"
            if ($StrictExtras) { Write-Host $msg -ForegroundColor Red; $ok = $false }
            else { Write-Host $msg -ForegroundColor Yellow }
        }
    }
    return $ok
}

Export-ModuleMember -Function Initialize-DownloadEnvironment, Import-LockComponents, Get-LockComponent, Get-Sha256, Invoke-VerifiedDownload, Test-LockFilesInDir
