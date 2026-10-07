#requires -Version 5.1
[CmdletBinding()]
param(
    [ValidateRange(1, 1024)]
    [double]$MaxGiB = 20,
    [switch]$Apply
)
$ErrorActionPreference = 'Stop'

function Get-BuildCacheBytes {
    param([string[]]$Paths)
    $pending = [Collections.Generic.Stack[string]]::new()
    foreach ($path in $Paths) { $pending.Push($path) }
    [long]$bytes = 0
    while ($pending.Count) {
        $item = Get-Item -LiteralPath $pending.Pop() -Force
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "缓存含链接，暂不清理：$($item.FullName)"
        }
        if ($item.PSIsContainer) {
            foreach ($child in Get-ChildItem -LiteralPath $item.FullName -Force) {
                $pending.Push($child.FullName)
            }
        } else { $bytes += $item.Length }
    }
    return $bytes
}

function ConvertTo-BuildCacheProcessPath {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { return $null }
    try {
        $value = $Path.Replace('/', '\')
        if ($value.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) {
            $value = '\\' + $value.Substring(8)
        } elseif ($value.StartsWith('\\?\')) { $value = $value.Substring(4) }
        if (-not [IO.Path]::IsPathRooted($value)) { return $null }
        return [IO.Path]::GetFullPath($value)
    } catch { return $null }
}

function Invoke-BuildCacheMaintenance {
    param([string]$RepoRoot, [double]$MaxGiB = 20, [bool]$Apply = $false)
    $root = [IO.Path]::GetFullPath($RepoRoot).TrimEnd('\', '/')
    $boundary = $root + [IO.Path]::DirectorySeparatorChar
    if ((Get-Item -LiteralPath $root -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        Write-Warning '构建缓存：仓库根目录是链接，暂不自动清理。'
        return
    }
    if (@(Get-Process -Name cargo,rustc -ErrorAction SilentlyContinue).Count) {
        Write-Host '构建缓存：编译器正在运行，暂缓清理。'
        return
    }
    $processes = @(Get-CimInstance Win32_Process -ErrorAction Stop)
    foreach ($relative in @('server/target/debug', 'updater/target/debug')) {
        $profile = [IO.Path]::GetFullPath((Join-Path $root $relative))
        if (-not $profile.StartsWith($boundary, [StringComparison]::OrdinalIgnoreCase)) {
            throw '构建缓存路径越过仓库边界'
        }
        if (-not (Test-Path -LiteralPath $profile)) { continue }
        $profileBoundary = $profile + [IO.Path]::DirectorySeparatorChar
        $busy = @($processes | Where-Object {
            $processPath = ConvertTo-BuildCacheProcessPath $_.ExecutablePath
            ($processPath -and $processPath.StartsWith($profileBoundary, [StringComparison]::OrdinalIgnoreCase)) -or
            (-not $processPath -and $_.Name -in @('gamer-server.exe', 'Gamer.exe'))
        })
        if ($busy.Count) {
            Write-Host "构建缓存：$relative 有运行中的程序，暂缓清理。"
            continue
        }
        try {
            $current = $root
            foreach ($segment in $relative.Split('/')) {
                $current = Join-Path $current $segment
                if ((Get-Item -LiteralPath $current -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
                    throw '构建缓存父目录是链接'
                }
            }
            $paths = @(@('.fingerprint', 'build', 'deps', 'incremental') | ForEach-Object {
                $path = Join-Path $profile $_
                if (Test-Path -LiteralPath $path) {
                    $resolved = (Resolve-Path -LiteralPath $path).Path
                    if (-not $resolved.StartsWith($profileBoundary, [StringComparison]::OrdinalIgnoreCase)) {
                        throw '构建缓存路径越过目标目录'
                    }
                    $resolved
                }
            })
            $bytes = Get-BuildCacheBytes -Paths $paths
            $size = [math]::Round($bytes / 1GB, 2)
            if ($bytes -le $MaxGiB * 1GB) {
                Write-Host "构建缓存：$relative 约 $size GiB，未超过 $MaxGiB GiB。"
                continue
            }
            if (-not $Apply) {
                Write-Host "构建缓存：$relative 约 $size GiB，将清理缓存目录（预览，未删除）。"
                continue
            }
            # All targets are fixed cache subdirectories, checked above; retain
            # root executables, release artifacts and every runtime data folder.
            foreach ($path in $paths) { Remove-Item -LiteralPath $path -Recurse -Force }
            Write-Host "构建缓存：已清理 $relative 约 $size GiB，下次构建重新生成。"
        } catch {
            Write-Warning "构建缓存清理暂缓：$($_.Exception.Message)"
        }
    }
}

Invoke-BuildCacheMaintenance -RepoRoot (Split-Path -Parent $PSScriptRoot) -MaxGiB $MaxGiB -Apply ([bool]$Apply)
