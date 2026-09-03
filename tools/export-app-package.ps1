#requires -Version 5.1
<#
.SYNOPSIS
    把 server/data/<分区> 打包为 .gamerpkg（App Package 归档），用于把既有
    本地资产迁移到「零业务资源发行」的新部署。

.DESCRIPTION
    分区 → 包内容映射（server 端包格式只认 templates/scripts/keymaps/presets/resources 根）：
        tmpl/   -> templates/   模板图片
        yaml/   -> scripts/     脚本（保留子目录结构）
        keymap/ -> keymaps/     按键映射方案
        func/   -> （包格式第一版无对应目录，跳过并告警；函数库继续以本地分区兜底）
        presets -> presets/     任务预设（可选，-PresetsDir 传入；目录为空则不打包 presets）
    安装：POST /api/app-packages/install（body = 归档字节，可带 X-Expected-Sha256 头），
    安装即激活，包内 presets/*.yaml 自动灌入任务预设。

.EXAMPLE
    .\tools\export-app-package.ps1 -Partition com.miHoYo.hkrpg -OutFile .\hkrpg-1.0.0.gamerpkg

.EXAMPLE
    .\tools\export-app-package.ps1 -Partition com.tencent.nrc -Id official.nrc -Version 1.0.0 `
        -Name "NRC 支持包" -OutFile .\nrc.gamerpkg

.NOTES
    使用 .NET ZipArchive 显式以 '/' 作为条目分隔符（规避 Windows PowerShell 5.1
    Compress-Archive 写入 '\' 分隔路径导致服务端归档校验拒绝的问题）。
#>
param(
    ## 分区名 = 设备配置的应用包名（如 com.miHoYo.hkrpg），同时作为缺省包 id 与 android target
    [Parameter(Mandatory = $true)][string]$Partition,
    [Parameter(Mandatory = $true)][string]$OutFile,
    ## 包 id（缺省 = 分区名；规范建议与 android 包名区分，如 official.hkrpg）
    [string]$Id,
    [string]$Version = "1.0.0",
    ## 展示名（可空）
    [string]$Name,
    [string]$DataDir,
    ## 可选：任务预设目录（*.yaml），拷贝为包内 presets/
    [string]$PresetsDir
)
$ErrorActionPreference = 'Stop'

if (-not $DataDir) {
    $base = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
    $DataDir = Join-Path $base "..\server\data"
}

if (-not $Id) { $Id = $Partition }

# server 端 manifest 校验对齐：id/version 为 ASCII 安全名（≤128 字符），version 至少含一个数字
function Assert-SafeAscii([string]$Value, [string]$Field) {
    if (-not $Value -or $Value.Length -gt 128) { throw "$Field 必须为 1-128 个字符: '$Value'" }
    foreach ($ch in $Value.ToCharArray()) {
        if ([int]$ch -lt 0x21 -or [int]$ch -gt 0x7E) { throw "$Field 只允许 ASCII 可见字符: '$Value'" }
    }
    if ($Value -match '[\\/:*?"<>|]') { throw "$Field 含非法文件名字符: '$Value'" }
}
Assert-SafeAscii $Id 'id'
Assert-SafeAscii $Version 'version'
if ($Version -notmatch '\d') { throw "version 必须至少包含一个数字: '$Version'" }

$partitionDir = Join-Path $DataDir $Partition
if (-not (Test-Path $partitionDir -PathType Container)) { throw "分区目录不存在: $partitionDir" }
$partitionDir = [System.IO.Path]::GetFullPath($partitionDir)

Add-Type -AssemblyName System.IO.Compression | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null

function Add-ZipEntryFile([System.IO.Compression.ZipArchive]$Zip, [string]$EntryName, [string]$SourcePath) {
    # 统一 '/' 分隔（服务端 ResourcePath 拒绝 '\'）
    $entry = $Zip.CreateEntry(($EntryName -replace '\\', '/'), [System.IO.Compression.CompressionLevel]::Optimal)
    $stream = $entry.Open()
    try {
        $bytes = [System.IO.File]::ReadAllBytes($SourcePath)
        $stream.Write($bytes, 0, $bytes.Length)
    } finally {
        $stream.Dispose()
    }
}

function Escape-TomlString([string]$Value) {
    $Value.Replace('\', '\\').Replace('"', '\"')
}

$outPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutFile)
$parent = Split-Path $outPath -Parent
if (-not (Test-Path $parent)) { New-Item -ItemType Directory -Path $parent | Out-Null }

$fs = [System.IO.File]::Open($outPath, [System.IO.FileMode]::Create)
$zip = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
$added = 0
$skippedFunc = 0
try {
    # ---- manifest.toml
    $manifestLines = @(
        "id = `"$(Escape-TomlString $Id)`"",
        "version = `"$(Escape-TomlString $Version)`""
    )
    if ($Name) { $manifestLines += "name = `"$(Escape-TomlString $Name)`"" }
    $manifestLines += @('', '[android]', "packages = [`"$(Escape-TomlString $Partition)`"]")
    $tmpManifest = Join-Path ([System.IO.Path]::GetTempPath()) ("manifest-" + [guid]::NewGuid().ToString('N') + '.toml')
    # UTF-8 无 BOM（server 端 manifest 必须是 UTF-8；PS5.1 的 -Encoding UTF8 带 BOM）
    [System.IO.File]::WriteAllText($tmpManifest, ($manifestLines -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
    Add-ZipEntryFile $zip 'manifest.toml' $tmpManifest
    Remove-Item $tmpManifest -Force
    $added++

    # ---- 分区目录映射
    $map = [ordered]@{ 'tmpl' = 'templates'; 'yaml' = 'scripts'; 'keymap' = 'keymaps' }
    foreach ($src in $map.Keys) {
        # GetFullPath 归一 '..'，保证与 FullName 前缀一致可切相对路径
        $srcDir = [System.IO.Path]::GetFullPath((Join-Path $partitionDir $src))
        if (-not (Test-Path $srcDir -PathType Container)) { continue }
        $dstRoot = $map[$src]
        Get-ChildItem -Path $srcDir -File -Recurse | ForEach-Object {
            $rel = $_.FullName.Substring($srcDir.Length).TrimStart('\', '/')
            Add-ZipEntryFile $zip ("$dstRoot/" + $rel) $_.FullName
            $script:added++
        }
    }

    # ---- func/ 不在包格式第一版：告警跳过
    $funcDir = Join-Path $partitionDir 'func'
    if (Test-Path $funcDir -PathType Container) {
        $skippedFunc = (Get-ChildItem -Path $funcDir -File -Recurse).Count
    }

    # ---- presets/（可选）
    if ($PresetsDir) {
        if (-not (Test-Path $PresetsDir -PathType Container)) { throw "预设目录不存在: $PresetsDir" }
        Get-ChildItem -Path $PresetsDir -File -Filter *.yaml | ForEach-Object {
            Add-ZipEntryFile $zip ("presets/" + $_.Name) $_.FullName
            $script:added++
        }
    }
} finally {
    $zip.Dispose()
    $fs.Dispose()
}

$hash = (Get-FileHash -Algorithm SHA256 $outPath).Hash.ToLower()
Write-Host "已生成 $outPath（$added 个条目）"
Write-Host "SHA-256: $hash"
if ($skippedFunc -gt 0) {
    Write-Warning "func/ 下 $skippedFunc 个函数库文件未打包（包格式第一版不含函数库；新机继续以本地分区兜底）"
}
Write-Host "安装：POST /api/app-packages/install（body=归档字节；建议带 X-Expected-Sha256: $hash）"
