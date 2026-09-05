#requires -Version 5.1
<#
.SYNOPSIS
    构建官方插件产物：keymap / yaml v3 guest → WASM Component → 签名 .gplugin → registry.json。

.DESCRIPTION
    Phase 10 验收链路的打包端：
      1. 构建 tools/plugin-signer（签名/打包/proof 工具）。
      2. 以 wasm32-unknown-unknown 编译 server/tests/keymap-guest（测试 fixture）与
         server/guests/yaml-guest（gamer.yaml 官方产品 guest），
         经各自 componentize bin 封装为 WIT Component。
      3. 用 tools/plugin-signing/<key-id>.key（ed25519 私钥，hex）给 manifest.toml 签名，
         打包 .gplugin（zip：manifest.toml + plugin.wasm + ui + signature.sig），
         输出到 web/public/plugins/（web-dist 托管后市场 URL 可直接下载）。
      4. 为每个包产出 Registry proof（base64(JSON)，绑定 id/version/download_url/sha256），
         生成 web/public/registry.json（插件中心「市场」页签数据源）。

    签名信任链：server 内嵌 <key-id>.pem 对应公钥（src/extensions/signature.rs 内置信任锚）；
    私钥不入库（.gitignore tools/plugin-signing/*.key）。若密钥缺失，脚本自动生成新对——
    但新生成的公钥不会自动被已构建的 server 信任，必须把打印出的公钥更新到
    signature.rs 的内置锚（或放进服务端 plugin-trust 目录）后重新构建 server。

.PARAMETER KeyId
    签名 key id（默认 gamer-dev-1）。仅本地/开发市场用，勿用于生产签名。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools\build-plugins.ps1
#>
[CmdletBinding()]
param(
    [string]$KeyId = 'gamer-dev-1'
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$ServerDir = Join-Path $RepoRoot 'server'
$SignerDir = Join-Path $RepoRoot 'tools\plugin-signer'
$SigningDir = Join-Path $RepoRoot 'tools\plugin-signing'
$TargetRoot = Join-Path $ServerDir 'target\plugin-build'
$OutDir = Join-Path $RepoRoot 'web\public\plugins'
$RegistryPath = Join-Path $RepoRoot 'web\public\registry.json'
# cargo --target-dir 指向 <TargetRoot> 时，signer 产物直接落在 <TargetRoot>\release\
$SignerExe = Join-Path $TargetRoot 'release\gamer-plugin-signer.exe'

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

foreach ($tool in @('cargo')) {
    if ($null -eq (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Write-Host "[precheck] 缺少工具：$tool" -ForegroundColor Red
        exit 1
    }
}

# ---- 1. plugin-signer ----
Write-Host "===[1/5] 构建 plugin-signer ===" -ForegroundColor Cyan
Invoke-Native 'cargo' @(
    'build', '--quiet', '--release',
    '--manifest-path', "$SignerDir\Cargo.toml",
    '--target-dir', $TargetRoot
) $RepoRoot

# ---- 2. 签名密钥（缺则生成，生成即提示必须同步信任锚）----
Write-Host "===[2/5] 检查签名密钥 $KeyId ===" -ForegroundColor Cyan
$keyPath = Join-Path $SigningDir "$KeyId.key"
$pemPath = Join-Path $SigningDir "$KeyId.pem"
$generated = $false
if (-not (Test-Path $keyPath)) {
    $generated = $true
    New-Item -ItemType Directory -Force -Path $SigningDir | Out-Null
    $pem = & $SignerExe keygen --out $keyPath --key-id $KeyId --pem-out $pemPath
    if ($LASTEXITCODE -ne 0) { throw 'keygen 失败' }
    Write-Host $pem
}
if (-not (Test-Path $pemPath)) {
    Write-Host "[warn] 公钥 $KeyId.pem 缺失；请把 server 内嵌信任锚（src/extensions/signature.rs）对应公钥写出。" -ForegroundColor Yellow
}
if ($generated) {
    Write-Host @"
[warn] 已生成全新 dev keypair。该公钥只有在以下任一位置登记后才会被 server 信任：
  a) 更新 server/src/extensions/signature.rs 的内置信任锚并重新构建 server；
  b) 把公钥 PEM 放入服务端信任目录（GAMER_PLUGIN_TRUST_DIR 或 <data>/plugin-trust/<key-id>.pem）。
"@ -ForegroundColor Yellow
}

# ---- 3. guest → WASM Component ----
Write-Host "===[3/5] 构建 guest Component ===" -ForegroundColor Cyan
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
$keymapComponent = Build-Guest 'keymap-guest' (Join-Path $ServerDir 'tests\keymap-guest') 'gamer_keymap_fixture.wasm'
# gamer.yaml 产品 guest：源码在 server/guests/yaml-guest（P12.8 迁出 tests 目录），
# wasm 产物名随包更名 gamer_yaml_guest.wasm
$yamlComponent = Build-Guest 'yaml-guest' (Join-Path $ServerDir 'guests\yaml-guest') 'gamer_yaml_guest.wasm'

# ---- 4. 打包 .gplugin（manifest 源：tools/plugins/<id>/manifest.toml）----
Write-Host "===[4/5] 打包并签名 .gplugin ===" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Get-ManifestVersion {
    param([string]$ManifestPath)
    $line = (Get-Content $ManifestPath) | Where-Object { $_ -match '^version\s*=' } | Select-Object -First 1
    if ($line -notmatch 'version\s*=\s*"([^"]+)"') { throw "manifest 缺少 version: $ManifestPath" }
    return $Matches[1]
}

$packages = @(
    # 两个官方插件的面板均为 runtime = "core"（宿主组件渲染，manifest 只声明
    # component 键），包内不再携带 ui/ iframe 资产。
    @{
        Id = 'gamer.keymap'; Version = $null
        Manifest = Join-Path $RepoRoot 'tools\plugins\gamer.keymap\manifest.toml'
        Component = $keymapComponent
        Files = @()
    },
    @{
        Id = 'gamer.yaml'; Version = $null
        Manifest = Join-Path $RepoRoot 'tools\plugins\gamer.yaml\manifest.toml'
        Component = $yamlComponent
        Files = @()
    }
)

$results = @()
foreach ($package in $packages) {
    $package.Version = Get-ManifestVersion $package.Manifest
    $out = Join-Path $OutDir ("{0}-{1}.gplugin" -f $package.Id, $package.Version)
    $args = @(
        'pack',
        '--manifest', $package.Manifest,
        '--wasm', $package.Component,
        '--key', $keyPath,
        '--key-id', $KeyId,
        '--out', $out
    )
    foreach ($file in $package.Files) { $args += @('--file', $file) }
    $packOutput = & $SignerExe @args
    if ($LASTEXITCODE -ne 0) { throw "打包失败: $($package.Id)" }
    $sha256 = ($packOutput | Where-Object { $_ -match '^sha256=' }) -replace '^sha256=', ''
    $size = [int64]((($packOutput | Where-Object { $_ -match '^size=' }) -replace '^size=', ''))
    Write-Host ("  {0}@{1} -> {2} (sha256={3})" -f $package.Id, $package.Version, $out, $sha256.Substring(0, 12) + '…')

    # ---- 5. Registry proof ----
    $downloadUrl = "/plugins/$($package.Id)-$($package.Version).gplugin"
    $proof = & $SignerExe registry-proof `
        --key $keyPath --key-id $KeyId `
        --id $package.Id --version $package.Version `
        --download-url $downloadUrl --sha256 $sha256
    if ($LASTEXITCODE -ne 0) { throw "registry-proof 失败: $($package.Id)" }

    $results += [pscustomobject]@{
        Id = $package.Id; Version = $package.Version; Sha256 = $sha256; Size = $size
        DownloadUrl = $downloadUrl; Proof = ($proof -join '').Trim()
    }
}

# ---- registry.json ----
$entries = @()
foreach ($result in $results) {
    if ($result.Id -eq 'gamer.keymap') {
        $entries += [ordered]@{
            id = $result.Id; version = $result.Version
            name = 'Keymap'
            description = 'Application-specific keyboard, mouse, and gamepad mappings（WASM keymap 扩展，profile 数据通道见 keymap 存储）'
            publisher = 'gamer.dev'
            download_url = $result.DownloadUrl
            sha256 = $result.Sha256
            size = $result.Size
            signature = [ordered]@{ status = 'valid'; key_id = $KeyId; algorithm = 'ed25519'; value = $result.Proof }
            permissions = @('input.tap', 'input.swipe', 'input.key', 'touch')
            host_api = [ordered]@{ input = '^1.0'; touch = '^1.0' }
            ui = [ordered]@{ contributions = @([ordered]@{ panel_id = 'keymaps'; title = '映射'; runtime = 'core'; component = 'console.keymaps' }) }
        }
    }
    else {
        $entries += [ordered]@{
            id = $result.Id; version = $result.Version
            name = 'Gamer YAML vNext'
            description = 'Surface YAML v3 lowering and execution guest（version: 3 脚本经 YamlVnextAdapter 执行）'
            publisher = 'gamer.dev'
            download_url = $result.DownloadUrl
            sha256 = $result.Sha256
            size = $result.Size
            signature = [ordered]@{ status = 'valid'; key_id = $KeyId; algorithm = 'ed25519'; value = $result.Proof }
            permissions = @('device.read', 'device.app', 'input.tap', 'input.swipe', 'input.key', 'input.text', 'vision.match', 'vision.color', 'resource.read', 'runtime.sleep', 'log.write')
            host_api = [ordered]@{ device = '^1.0'; vision = '^1.0'; input = '^1.0'; resource = '^1.0'; runtime = '^1.0'; log = '^1.0' }
            ui = [ordered]@{ contributions = @(
                [ordered]@{ panel_id = 'automation'; title = '自动化'; runtime = 'core'; component = 'console.scripts' },
                [ordered]@{ panel_id = 'functions'; title = '函数'; runtime = 'core'; component = 'console.functions' },
                [ordered]@{ panel_id = 'templates'; title = '模板'; runtime = 'core'; component = 'console.templates' }
            ) }
        }
    }
}

$registry = [ordered]@{
    schema_version = 1
    generated_at = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    host_api = '1.0.0'
    plugins = $entries
}
$json = $registry | ConvertTo-Json -Depth 8
[System.IO.File]::WriteAllText($RegistryPath, $json + "`n")
Write-Host "===[5/5] registry.json 已生成: $RegistryPath ===" -ForegroundColor Cyan
Write-Host 'OK 官方插件产物构建完成（web/public/plugins/ 与 web/public/registry.json）。' -ForegroundColor Green
