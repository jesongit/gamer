# DEP-005: 第三方组件清单（SBOM）生成。
# 解析 server/Cargo.lock 与 launcher/Cargo.lock（Cargo.lock 本身即含全部传递
# 依赖），生成 CycloneDX 1.5 JSON（name/version/purl），输出到 release/sbom/。
# 零外部依赖：手写行级解析 + 手拼 JSON；兼容 Windows PowerShell 5.1 与 pwsh。
#
# 用法:
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\gen-sbom.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\gen-sbom.ps1 -OutDir release\sbom
#
# 输出不入库（release/sbom/ 已 gitignore），随发布产物归档。

[CmdletBinding()]
param(
    # 仓库根（默认脚本位置的上一级）
    [string]$RepoRoot = '',
    # SBOM 输出目录（默认 <repo>/release/sbom）
    [string]$OutDir = '',
    # Cargo.lock 路径列表（默认 server + launcher）
    [string[]]$LockPaths = @()
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

if (-not $RepoRoot) { $RepoRoot = Split-Path -Parent $PSScriptRoot }
if (-not $OutDir)   { $OutDir   = Join-Path $RepoRoot 'release\sbom' }
if ($LockPaths.Count -eq 0) {
    $LockPaths = @(
        (Join-Path $RepoRoot 'server\Cargo.lock'),
        (Join-Path $RepoRoot 'launcher\Cargo.lock')
    )
}

function Exit-Fail {
    param([string]$Message)
    Write-Host "[gen-sbom] FAIL: $Message" -ForegroundColor Red
    exit 1
}

function JsonEscape {
    param([string]$s)
    return ($s -replace '\\', '\\\\' -replace '"', '\"')
}

# ---------- 版本（权威源 server/Cargo.toml [package].version）----------
$cargoToml = Join-Path $RepoRoot 'server\Cargo.toml'
$productVersion = $null
$section = ''
foreach ($line in (Get-Content -LiteralPath $cargoToml)) {
    if ($line -match '^\s*\[([^\]]+)\]\s*$') { $section = $Matches[1].Trim(); continue }
    if ($section -eq 'package' -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
        $productVersion = $Matches[1].Trim(); break
    }
}
if (-not $productVersion) { Exit-Fail "无法从 $cargoToml 读取 [package].version" }

# ---------- 行级解析 Cargo.lock：[[package]] 段的 name/version/source ----------
# dependencies = [ ... ] 多行数组内的条目以缩进+引号开头，不会命中 ^name = "。
function Import-CargoLockPackages {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { Exit-Fail "Cargo.lock 不存在: $Path" }
    $packages = New-Object System.Collections.Generic.List[object]
    $current = $null
    foreach ($raw in [System.IO.File]::ReadAllLines($Path)) {
        if ($raw -match '^\s*\[\[package\]\]\s*$') {
            $current = @{ name = ''; version = ''; source = '' }
            $packages.Add($current)
            continue
        }
        if ($null -eq $current) { continue }
        if ($raw -match '^\s*\[') { $current = $null; continue }  # 离开 [[package]] 段
        if ($raw -match '^(name|version|source)\s*=\s*"([^"]*)"\s*$') {
            $current[$Matches[1]] = $Matches[2]
        }
    }
    return $packages
}

# ---------- 收集并按 name@version 去重（两把锁共用依赖合并）----------
$seen = @{}
$sorted = New-Object System.Collections.Generic.List[object]
foreach ($lock in $LockPaths) {
    $pkgs = Import-CargoLockPackages -Path $lock
    foreach ($p in $pkgs) {
        if ([string]::IsNullOrEmpty($p['name']) -or [string]::IsNullOrEmpty($p['version'])) {
            Exit-Fail "$lock 中存在缺失 name/version 的 [[package]] 条目"
        }
        $key = ('{0}@{1}' -f $p['name'], $p['version']).ToLowerInvariant()
        if ($seen.ContainsKey($key)) { continue }
        $seen[$key] = $true
        # 无 source = workspace 本包（gamer-server / gamer-launcher），按应用类型登记
        [void]$sorted.Add(@{
            name    = [string]$p['name']
            version = [string]$p['version']
            isRoot  = [string]::IsNullOrEmpty($p['source'])
        })
    }
    Write-Host ("[gen-sbom] {0}: {1} 个 [[package]] 条目（含传递依赖）" -f $lock, $pkgs.Count)
}
$sorted = $sorted | Sort-Object -Property name, version

# ---------- 手拼 CycloneDX 1.5 JSON（确定性输出）----------
$libCount = 0
$appCount = 0
$itemLines = New-Object System.Collections.Generic.List[string]
foreach ($c in $sorted) {
    $type = 'library'
    if ($c['isRoot']) { $type = 'application'; $appCount++ } else { $libCount++ }
    $ref = 'pkg:cargo/{0}@{1}' -f $c['name'], $c['version']
    $itemLines.Add(('    {{ "type": "{0}", "bom-ref": "{1}", "name": "{2}", "version": "{3}", "purl": "{1}" }}' -f `
        $type, $ref, (JsonEscape $c['name']), (JsonEscape $c['version']))) | Out-Null
}

$guid = [guid]::NewGuid().ToString()
$timestamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
$sbomName = 'gamer-sbom-{0}-windows-x64.cdx.json' -f $productVersion

$json = New-Object System.Text.StringBuilder
[void]$json.AppendLine('{')
[void]$json.AppendLine('  "bomFormat": "CycloneDX",')
[void]$json.AppendLine('  "specVersion": "1.5",')
[void]$json.AppendLine(('  "serialNumber": "urn:uuid:{0}",' -f $guid))
[void]$json.AppendLine('  "version": 1,')
[void]$json.AppendLine('  "metadata": {')
[void]$json.AppendLine(('    "timestamp": "{0}",' -f $timestamp))
[void]$json.AppendLine('    "tools": [')
[void]$json.AppendLine(('      {{ "vendor": "gamebot", "name": "tools/gen-sbom.ps1", "version": "{0}" }}' -f $productVersion))
[void]$json.AppendLine('    ],')
[void]$json.AppendLine('    "component": {')
[void]$json.AppendLine(('      "type": "application", "bom-ref": "pkg:gamebot/gamebot@{0}", "name": "gamebot", "version": "{0}",' -f $productVersion))
[void]$json.AppendLine('      "description": "GameBot 游戏自动化助手（server + launcher + web 前端）"')
[void]$json.AppendLine('    }')
[void]$json.AppendLine('  },')
[void]$json.AppendLine('  "components": [')
[void]$json.AppendLine(($itemLines -join ",`n"))
[void]$json.AppendLine('  ]')
[void]$json.AppendLine('}')

if (-not (Test-Path -LiteralPath $OutDir)) { New-Item -ItemType Directory -Path $OutDir -Force | Out-Null }
$outFile = Join-Path $OutDir $sbomName
# CycloneDX JSON 不带 BOM（BOM 会破坏部分严格解析器）
[System.IO.File]::WriteAllText($outFile, $json.ToString(), (New-Object System.Text.UTF8Encoding($false)))

# 生成后用 Node 自检 JSON 合法性（失败即退出——文件由本脚本拼接，不合法说明脚本有 bug）
$node = Get-Command node -ErrorAction SilentlyContinue
if ($node) {
    $null = & node -e "JSON.parse(require('fs').readFileSync(process.argv[1],'utf8'))" $outFile 2>&1
    if ($LASTEXITCODE -ne 0) { Exit-Fail "生成的 SBOM 不是合法 JSON: $outFile" }
}

Write-Host "[gen-sbom] OK: $outFile"
Write-Host ("[gen-sbom] 组件统计: 共 {0}（library {1} + application {2}），产品版本 {3}" -f `
    ($libCount + $appCount), $libCount, $appCount, $productVersion)
exit 0
