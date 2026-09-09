#requires -Version 5.1
<#
.SYNOPSIS
    构建官方插件产物：guest → WASM Component → 无签名 .gplugin → registry v2 → 完整性自检。

.DESCRIPTION
    Phase 1 免签名链路（plan §4.2，keygen / 私钥 / signature.sig / Registry proof
    已整体移出默认链路，CI 不存私钥）：
      1. 构建 tools/plugin-signer（打包/校验工具）。
      2. 枚举 <ManifestsRoot>/<id>/manifest.toml（manifest v2：[execution]
         kind = wasm | builtin），经 signer inspect 解析元数据——id/version/
         name/description/publisher/permissions/host_api/ui 全部以 manifest 为
         唯一权威源，本脚本不再维护第二份。
      3. wasm 包构建对应 guest Component（keymap 源 server/guests/keymap-guest、
         yaml 源 server/guests/yaml-guest）；builtin 包（gamer.video）无 guest、
         只打 manifest，不携带任何占位 WASM。
      4. signer pack 出 .gplugin（zip：manifest.toml + plugin.wasm + 附加文件，
         无 signature.sig）——先落在 staging 临时目录。
      5. 产物自检：signer verify 重走 zip 中央目录/entry magic 校验 + 重新计算
         sha256/大小 + id/version/kind 与 manifest 比对。任何失败 exit 非零且
         不触碰既有产物（web/public/registry.json 与 plugins/*.gplugin 原样保留）。
      6. 全部通过后：registry v2（schema_version=2，条目无 signature 字段）先写
         临时文件再原子替换；产物拷入 <OutputDir>；清理不在本轮构建清单内的旧
         .gplugin（-KeepStaleArtifacts 跳过）。
      7. 可选 -ChecksumsFile：生成 sha256sums.txt（GNU sha256sum -c 兼容，头注释
         含源提交），供 GitHub Release 上传（从指定提交构建 → 完整性清单 → 产物
         可上传）。本地构建产物可直接导入，仓库内 web/public/ 继续作为开发 seed。

.PARAMETER OutputDir
    .gplugin 产物目录（默认 <repo>\web\public\plugins）。干跑可用临时目录。

.PARAMETER RegistryFile
    registry.json 输出路径（默认 <repo>\web\public\registry.json）。

.PARAMETER ManifestsRoot
    插件 manifest 根目录（默认 <repo>\tools\plugins）；每个子目录的
    manifest.toml 即一个待构建插件。

.PARAMETER ChecksumsFile
    可选。生成 sha256sums.txt 完整性清单（含 registry.json 与全部 .gplugin）。

.PARAMETER Publisher
    manifest 未声明 publisher 字段时的默认发布者（默认 gamer.dev）。

.PARAMETER KeepStaleArtifacts
    保留 OutputDir 中不在本轮构建清单内的旧 .gplugin（默认成功后清理）。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools\build-plugins.ps1
.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools\build-plugins.ps1 -OutputDir $env:TEMP\plugins -RegistryFile $env:TEMP\registry.json -ChecksumsFile $env:TEMP\sha256sums.txt
#>
[CmdletBinding()]
param(
    [string]$OutputDir,
    [string]$RegistryFile,
    [string]$ManifestsRoot,
    [string]$ChecksumsFile,
    [string]$Publisher = 'gamer.dev',
    [switch]$KeepStaleArtifacts
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$ServerDir = Join-Path $RepoRoot 'server'
$SignerDir = Join-Path $RepoRoot 'tools\plugin-signer'
if (-not $ManifestsRoot) { $ManifestsRoot = Join-Path $RepoRoot 'tools\plugins' }
if (-not $OutputDir) { $OutputDir = Join-Path $RepoRoot 'web\public\plugins' }
if (-not $RegistryFile) { $RegistryFile = Join-Path $RepoRoot 'web\public\registry.json' }
$TargetRoot = Join-Path $ServerDir 'target\plugin-build'
# cargo --target-dir 指向 <TargetRoot> 时，signer 产物直接落在 <TargetRoot>\release\
$SignerExe = Join-Path $TargetRoot 'release\gamer-plugin-signer.exe'

# guest 构建配方（源码位置与 wasm 产物名；版本/元数据一律来自 manifest.toml）。
$GuestRecipes = @{
    'gamer.keymap' = @{ Dir = Join-Path $ServerDir 'guests\keymap-guest'; Lib = 'gamer_keymap_guest.wasm' }
    'gamer.yaml'   = @{ Dir = Join-Path $ServerDir 'guests\yaml-guest';  Lib = 'gamer_yaml_guest.wasm' }
}

# PS 5.1 坑：EAP=Stop 下原生命令 stderr 输出会被包装成 ErrorRecord 中断脚本
# （cargo 编译进度走 stderr），门禁期间临时降级，失败与否只看 $LASTEXITCODE。
$prevEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
function Invoke-Native {
    param([string]$Exe, [string[]]$ArgList, [string]$WorkDir)
    Push-Location $WorkDir
    try {
        & $Exe @ArgList
        if ($LASTEXITCODE -ne 0) { throw "命令失败(exit=$LASTEXITCODE): $Exe $($ArgList -join ' ')" }
    }
    finally { Pop-Location }
}
$ErrorActionPreference = $prevEap

function Get-FileSha256 {
    param([string]$Path)
    (Get-FileHash -Path $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Read-JsonFile {
    param([string]$Path)
    # PS 5.1：UTF-8（无 BOM）文件必须显式 -Encoding UTF8，否则按 ANSI 读出乱码。
    Get-Content -Path $Path -Raw -Encoding UTF8 | ConvertFrom-Json
}

function Write-TextFileNoBom {
    param([string]$Path, [string]$Text)
    $parent = Split-Path -Parent $Path
    if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
    [System.IO.File]::WriteAllText($Path, $Text, [System.Text.UTF8Encoding]::new($false))
}

function Get-RelativeReleasePath {
    param([string]$BaseDir, [string]$TargetPath)
    # Keep the checksum manifest portable across PowerShell 5.1 and newer runtimes.
    $base = [System.IO.Path]::GetFullPath($BaseDir)
    if (-not $base.EndsWith([System.IO.Path]::DirectorySeparatorChar)) {
        $base += [System.IO.Path]::DirectorySeparatorChar
    }
    $target = [System.IO.Path]::GetFullPath($TargetPath)
    $relative = [System.Uri]::new($base).MakeRelativeUri([System.Uri]::new($target)).ToString()
    [System.Uri]::UnescapeDataString($relative).Replace('\', '/')
}

foreach ($tool in @('cargo')) {
    if ($null -eq (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Write-Host "[precheck] 缺少工具：$tool" -ForegroundColor Red
        exit 1
    }
}

# ---- 1. plugin-signer ----
Write-Host "===[1/6] 构建 plugin-signer ===" -ForegroundColor Cyan
Invoke-Native 'cargo' @(
    'build', '--quiet', '--release',
    '--manifest-path', "$SignerDir\Cargo.toml",
    '--target-dir', $TargetRoot
) $RepoRoot

# ---- 2. 枚举 manifest 并解析元数据 ----
Write-Host "===[2/6] 读取插件 manifest（$ManifestsRoot）===" -ForegroundColor Cyan
$manifestFiles = @(Get-ChildItem -Path $ManifestsRoot -Directory -ErrorAction SilentlyContinue |
    ForEach-Object { Join-Path $_.FullName 'manifest.toml' } |
    Where-Object { Test-Path $_ } |
    Sort-Object)
if ($manifestFiles.Count -eq 0) {
    throw "构建清单为空：$ManifestsRoot 下没有含 manifest.toml 的插件目录"
}

$stagingRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("gamer-plugins-" + [guid]::NewGuid().ToString('N'))
$packages = @()
foreach ($manifestPath in $manifestFiles) {
    $metaPath = Join-Path $stagingRoot ("meta-" + [System.IO.Path]::GetFileNameWithoutExtension((Split-Path -Parent $manifestPath)) + ".json")
    Invoke-Native $SignerExe @('inspect', '--manifest', $manifestPath, '--meta-out', $metaPath) $RepoRoot | Out-Null
    $meta = Read-JsonFile $metaPath
    $id = $meta.id
    $kind = $meta.execution.kind
    if (-not $id -or -not $meta.version -or -not $kind) { throw "manifest 元数据不完整: $manifestPath" }
    $package = [pscustomobject]@{
        Id = $id
        Version = $meta.version
        Kind = $kind
        Manifest = $manifestPath
        Meta = $meta
        Component = $null
        Artifact = $null
        Name = $null
        DownloadUrl = $null
        Sha256 = ''
        Size = [int64]0
    }
    if ($kind -eq 'wasm') {
        if (-not $GuestRecipes.ContainsKey($id)) {
            throw "wasm 插件 $id 缺少 guest 构建配方（tools\build-plugins.ps1 \$GuestRecipes）"
        }
        $package.Component = 'pending'
    }
    elseif ($kind -ne 'builtin') {
        throw "插件 $id 的 execution.kind 非法: $kind"
    }
    $packages += $package
    Write-Host ("  {0}@{1} kind={2}" -f $id, $package.Version, $kind)
}

# ---- 3. guest → WASM Component（仅 wasm 包；builtin 无 guest 不打占位 wasm）----
Write-Host "===[3/6] 构建 guest Component ===" -ForegroundColor Cyan
function Build-Guest {
    param([string]$Name, [string]$GuestDir, [string]$LibArtifact)
    $target = Join-Path $TargetRoot $Name
    Invoke-Native 'cargo' @(
        'build', '--quiet', '--release', '--lib', '--target', 'wasm32-unknown-unknown',
        '--manifest-path', "$GuestDir\Cargo.toml", '--target-dir', $target
    ) $RepoRoot
    $module = Join-Path $target "wasm32-unknown-unknown\release\$LibArtifact"
    $component = Join-Path $target 'plugin.component.wasm'
    Invoke-Native 'cargo' @(
        'run', '--quiet', '--release', '--bin', 'componentize',
        '--manifest-path', "$GuestDir\Cargo.toml", '--target-dir', $target, '--',
        $module, $component
    ) $RepoRoot
    return $component
}
foreach ($package in $packages | Where-Object { $_.Kind -eq 'wasm' }) {
    $recipe = $GuestRecipes[$package.Id]
    $package.Component = Build-Guest ($package.Id -replace '\.', '-') $recipe.Dir $recipe.Lib
    Write-Host ("  guest ok: {0} -> {1}" -f $package.Id, $package.Component)
}

# ---- 4. pack（无签名）到 staging ----
Write-Host "===[4/6] 打包 .gplugin（无签名，落 staging）===" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $stagingRoot | Out-Null
foreach ($package in $packages) {
    $name = "{0}-{1}.gplugin" -f $package.Id, $package.Version
    $out = Join-Path $stagingRoot $name
    $packArgs = @('pack', '--manifest', $package.Manifest, '--out', $out)
    if ($package.Component) { $packArgs += @('--wasm', $package.Component) }
    $packOutput = Invoke-Native $SignerExe $packArgs $RepoRoot
    $package.Artifact = $out
    $package.Name = $name
    $package.DownloadUrl = "/plugins/$name"
    $package.Sha256 = (($packOutput | Where-Object { $_ -match '^sha256=' }) -replace '^sha256=', '')
    $package.Size = [int64](($packOutput | Where-Object { $_ -match '^size=' }) -replace '^size=', '')
    if (-not $package.Sha256 -or -not $package.Size) { throw "pack 未输出 sha256/size: $($package.Id)" }
    Write-Host ("  {0} -> sha256={1}…" -f $name, $package.Sha256.Substring(0, 12))
}

# ---- 5. 产物自检（全部通过才允许落 web/public）----
Write-Host "===[5/6] 产物自检（zip 重走 + sha256/size + manifest 比对）===" -ForegroundColor Cyan
foreach ($package in $packages) {
    $verifyOutput = Invoke-Native $SignerExe @('verify', '--archive', $package.Artifact) $RepoRoot
    $vid = (($verifyOutput | Where-Object { $_ -match '^id=' }) -replace '^id=', '')
    $vver = (($verifyOutput | Where-Object { $_ -match '^version=' }) -replace '^version=', '')
    $vkind = (($verifyOutput | Where-Object { $_ -match '^kind=' }) -replace '^kind=', '')
    if ($vid -ne $package.Id -or $vver -ne $package.Version -or $vkind -ne $package.Kind) {
        throw "自检失败: $($package.Name) 的 id/version/kind（$vid@$vver/$vkind）与 manifest（$($package.Id)@$($package.Version)/$($package.Kind)）不一致"
    }
    $actualSha = Get-FileSha256 $package.Artifact
    $actualSize = (Get-Item $package.Artifact).Length
    if ($actualSha -ne $package.Sha256 -or $actualSize -ne $package.Size) {
        throw "自检失败: $($package.Name) sha256/size 与 pack 输出不一致"
    }
}
Write-Host "  staging 自检全部通过" -ForegroundColor Green

# ---- registry v2 条目（元数据全部来自 manifest 解析结果）----
function New-RegistryEntry {
    param($package)
    $hostApi = [ordered]@{}
    foreach ($prop in $package.Meta.host_api.PSObject.Properties) { $hostApi[$prop.Name] = $prop.Value }
    $contributions = @($package.Meta.ui.contributions)
    $publisher = if ($package.Meta.publisher) { $package.Meta.publisher } else { $Publisher }
    [ordered]@{
        id = $package.Id
        version = $package.Version
        name = $package.Meta.name
        description = $package.Meta.description
        publisher = $publisher
        download_url = $package.DownloadUrl
        sha256 = $package.Sha256
        size = $package.Size
        permissions = @($package.Meta.permissions)
        host_api = $hostApi
        ui = [ordered]@{ contributions = $contributions }
        execution = [ordered]@{ kind = $package.Kind }
    }
}

$entries = @($packages | ForEach-Object { New-RegistryEntry $_ })
$registry = [ordered]@{
    schema_version = 2
    generated_at = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    host_api = '1.0.0'
    plugins = $entries
}
$registryJson = ($registry | ConvertTo-Json -Depth 8) + "`n"

# ---- 提交阶段：临时文件 → 拷贝产物 → 原子替换 registry ----
$registryTmp = "$RegistryFile.tmp"
try {
    Write-TextFileNoBom -Path $registryTmp -Text $registryJson

    Write-Host "===[6/6] 发布产物 ===" -ForegroundColor Cyan
    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
    $registryParent = Split-Path -Parent $RegistryFile
    if ($registryParent) { New-Item -ItemType Directory -Force -Path $registryParent | Out-Null }
    foreach ($package in $packages) {
        $finalPath = Join-Path $OutputDir $package.Name
        Copy-Item -Path $package.Artifact -Destination $finalPath -Force
        # 最终落点复核：sha256 必须与 staging 一致（防止拷贝损坏）
        if ((Get-FileSha256 $finalPath) -ne $package.Sha256) {
            throw "发布失败: $finalPath 拷贝后 sha256 不一致"
        }
    }
    Move-Item -Path $registryTmp -Destination $RegistryFile -Force
    Write-Host "  registry v2 已生成: $RegistryFile（$($entries.Count) 个条目）"

    if (-not $KeepStaleArtifacts) {
        $produced = @($packages | ForEach-Object { (Join-Path $OutputDir $_.Name) })
        $stale = @(Get-ChildItem -Path $OutputDir -Filter '*.gplugin' -File |
            Where-Object { $produced -notcontains $_.FullName })
        foreach ($file in $stale) {
            Remove-Item -Path $file.FullName -Force
            Write-Host "  已清理旧产物: $($file.Name)"
        }
    }

    if ($ChecksumsFile) {
        $commit = $null
        try {
            $commit = (& git rev-parse HEAD 2>$null)
            if ($LASTEXITCODE -ne 0) { $commit = $null }
        } catch { $commit = $null }
        $lines = @(
            '# gamer 官方插件发行产物完整性清单（sha256sum -c 兼容）',
            '# 路径相对清单所在发行根目录（通常为 registry.json 与 plugins/ 的父目录）',
            ("# generated_at: " + (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))
        )
        if ($commit) { $lines += "# source_commit: $commit" }
        $checksumRoot = Split-Path -Parent ([System.IO.Path]::GetFullPath($ChecksumsFile))
        foreach ($p in $packages) {
            $artifactPath = Join-Path $OutputDir $p.Name
            $relativeArtifactPath = Get-RelativeReleasePath $checksumRoot $artifactPath
            $lines += ("{0}  {1}" -f $p.Sha256, $relativeArtifactPath)
        }
        $relativeRegistryPath = Get-RelativeReleasePath $checksumRoot $RegistryFile
        $lines += ("{0}  {1}" -f (Get-FileSha256 $RegistryFile), $relativeRegistryPath)
        Write-TextFileNoBom -Path $ChecksumsFile -Text (($lines -join "`n") + "`n")
        Write-Host "  完整性清单已生成: $ChecksumsFile"
    }
}
finally {
    if (Test-Path $stagingRoot) { Remove-Item -Path $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue }
    # 移动成功后 tmp 已不存在；仍存在说明失败中途退出，清掉避免污染 web/public
    if (Test-Path $registryTmp) { Remove-Item -Path $registryTmp -Force -ErrorAction SilentlyContinue }
}

Write-Host 'OK 官方插件产物构建完成（无签名；registry schema_version=2）。' -ForegroundColor Green
