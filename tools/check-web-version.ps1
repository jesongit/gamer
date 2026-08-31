# WEB-006 前置工具：扫描 web/src/**（.vue / .js）中的硬编码产品版本字面量。
#
# 背景（docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md §2/§6.1）：产品版本权威源 = server/Cargo.toml
# package.version，前端运行时必须显示服务端返回的版本（/api/system/info），不得硬编码。
# 本脚本静态检查源码中残留的版本形态字面量（如 v0.1.0 / 0.1.0 出现在版本展示语境）。
#
# 判定规则：
#   - 形如 [v]<n>.<n>.<n> 的字面量（每段 1~2 位数字，前后不得紧邻字母/数字/点）视为可疑版本：
#     1~2 位段宽天然排除 IP（192.168.x.x）、端口、尺寸（1920x1080）等常见非版本数字。
#   - 白名单（不算违规）：
#     1) *.test.js / *.spec.js / __tests__/ —— 测试夹具数据（system-api 契约 fixture 中的
#        示例版本如 0.2.0/0.3.0 属测试输入，不是 UI 展示硬编码），仅作为 INFO 列出；
#     2) 行内含 scrcpy/adb/ffmpeg 依赖与协议版本语境（如 scrcpy-server 3.3.3 绑定）——
#        依赖版本不是产品版本；
#     3) web/package.json 的 version 是包元数据，计划 §6.1 明确允许，且不在 web/src 扫描范围。
#
# 退出码：发现违规且未加 -ReportOnly 时 exit 1（供批次 3 CI 接入 WEB-006 收口门禁）；
#         -ReportOnly 只报告不失败（恒 exit 0）；无违规 exit 0。
#
# 兼容性：Windows PowerShell 5.1+ 与 pwsh 均可运行；文件必须保存为 UTF-8 with BOM；
#         输出前缀保持 ASCII，便于 CI 日志检索。

[CmdletBinding()]
param(
    # 扫描根目录；缺省为仓库内 web/src
    [string]$WebSrc,
    # 只报告不失败（恒 exit 0）；缺省发现违规 exit 1。批次 3 CI 接入前建议先带此开关观察
    [switch]$ReportOnly
)

$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($WebSrc)) {
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $WebSrc = Join-Path $repoRoot 'web/src'
}
if (-not (Test-Path -LiteralPath $WebSrc)) {
    Write-Host "[web-version-check] ERROR: scan root not found: $WebSrc"
    exit 2
}

# 可疑版本字面量：[v]<1-2位>.<1-2位>.<1-2位>，边界外不得是字母/数字/下划线/点。
$versionRegex = '(?<![A-Za-z0-9_.])v?[0-9]{1,2}\.[0-9]{1,2}\.[0-9]{1,2}(?![A-Za-z0-9_.])'
# 依赖/协议版本语境关键词（这些行里的 x.y.z 属依赖版本，不是产品版本）
$depContextRegex = '(?i)scrcpy|(?<![a-z])adb(?![a-z])|ffmpeg'
# 测试夹具文件（fixture 版本数据允许，仅 INFO）
$isTestFixture = {
    param([string]$Path)
    $name = Split-Path -Leaf $Path
    if ($name -like '*.test.js' -or $name -like '*.spec.js' -or $name -like '*.test.ts') { return $true }
    if ($Path -like '*__tests__*') { return $true }
    return $false
}

$utf8 = New-Object System.Text.UTF8Encoding($false)
$files = @(Get-ChildItem -LiteralPath $WebSrc -Recurse -File |
    Where-Object { $_.Extension -in @('.vue', '.js', '.ts') } |
    Sort-Object -Property FullName)
if ($files.Count -eq 0) {
    Write-Host "[web-version-check] ERROR: no .vue/.js/.ts files under $WebSrc"
    exit 2
}

$violations = New-Object System.Collections.Generic.List[string]
$fixtureHits = New-Object System.Collections.Generic.List[string]
$whitelisted = New-Object System.Collections.Generic.List[string]

foreach ($f in $files) {
    $rel = $f.FullName.Substring((Resolve-Path -LiteralPath $WebSrc).Path.Length + 1)
    $fixture = & $isTestFixture $f.FullName
    $lines = $null
    try {
        $lines = [System.IO.File]::ReadAllLines($f.FullName, $utf8)
    } catch {
        Write-Host "[web-version-check] WARN: cannot read $rel : $($_.Exception.Message)"
        continue
    }
    for ($i = 0; $i -lt $lines.Length; $i++) {
        $line = $lines[$i]
        $matchesFound = [regex]::Matches($line, $versionRegex)
        if ($matchesFound.Count -eq 0) { continue }
        $tokens = @($matchesFound | ForEach-Object { $_.Value })
        $text = "$rel`:$($i + 1): " + $line.Trim()
        if ($line -match $depContextRegex) {
            $whitelisted.Add("[dep-context] $text")
        } elseif ($fixture) {
            foreach ($t in $tokens) {
                $fixtureHits.Add("$rel`:$($i + 1): $t")
            }
        } else {
            foreach ($t in $tokens) {
                $violations.Add("$rel`:$($i + 1): $t")
                Write-Host "[web-version-check] VIOLATION $rel`:$($i + 1): hardcoded version literal '$t'"
                Write-Host "    $($line.Trim())"
            }
        }
    }
}

Write-Host ''
if ($fixtureHits.Count -gt 0) {
    Write-Host "[web-version-check] INFO: version literals in test fixtures (whitelisted, not violations):"
    foreach ($h in $fixtureHits) { Write-Host "    $h" }
}
if ($whitelisted.Count -gt 0) {
    Write-Host "[web-version-check] INFO: dependency/protocol version context lines (whitelisted):"
    foreach ($h in $whitelisted) { Write-Host "    $h" }
}

Write-Host ("[web-version-check] summary: {0} file(s) scanned, {1} hardcoded version violation(s), {2} test-fixture literal(s), {3} dep-context line(s)." -f `
    $files.Count, $violations.Count, $fixtureHits.Count, $whitelisted.Count)

if ($violations.Count -gt 0) {
    if ($ReportOnly) {
        Write-Host "[web-version-check] ReportOnly: found $($violations.Count) violation(s) but not failing."
        exit 0
    }
    Write-Host "[web-version-check] FAILED: hardcoded product version(s) in web/src - WEB-006 requires server-provided version only."
    exit 1
}

Write-Host "[web-version-check] OK: no hardcoded product version in web/src."
exit 0
