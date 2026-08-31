# REL-006：离线双钥轮换 fixture 校验。
# 只使用 test fixture 公钥和签名，不生成/读取/伪造任何生产私钥。

[CmdletBinding()]
param(
    [string]$FixtureDir = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

if (-not $FixtureDir) { $FixtureDir = Join-Path (Split-Path -Parent $PSScriptRoot) 'contracts\fixtures\key-rotation' }
$fixtureDir = (Resolve-Path -LiteralPath $FixtureDir).Path
$metaPath = Join-Path $fixtureDir 'rotation.json'

function Fail {
    param([string]$Message)
    Write-Error "[verify-key-rotation] FAIL: $Message"
    exit 1
}

if (-not (Test-Path -LiteralPath $metaPath)) { Fail "fixture metadata 不存在: $metaPath" }
try { $meta = Get-Content -LiteralPath $metaPath -Raw | ConvertFrom-Json }
catch { Fail "fixture metadata 不是合法 JSON: $($_.Exception.Message)" }
if (-not [bool]$meta.fixtureOnly) { Fail 'key rotation fixture 必须明确标记 fixtureOnly=true' }
if ([string]$meta.schemaVersion -cne '1') { Fail 'key rotation fixture schemaVersion 必须为 1' }
if ([string]$meta.current.key_id -eq [string]$meta.next.key_id) { Fail 'current 与 next key_id 不能相同' }

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$validator = Join-Path $repoRoot 'release\contracts\validate-manifest.mjs'
$keysDir = Join-Path $fixtureDir '..\keys'
if (-not (Get-Command node -ErrorAction SilentlyContinue)) { Fail '缺少 node' }

foreach ($slot in @('current', 'next')) {
    $entry = $meta.$slot
    $keyId = [string]$entry.key_id
    if ($keyId -match '(?i)^dev-') { Fail "$slot fixture 不得使用 dev key: $keyId" }
    $publicPath = [IO.Path]::GetFullPath((Join-Path $fixtureDir ([string]$entry.public_key)))
    $manifestPath = [IO.Path]::GetFullPath((Join-Path $fixtureDir ([string]$entry.manifest)))
    $sigPath = [IO.Path]::GetFullPath((Join-Path $fixtureDir ([string]$entry.signature)))
    foreach ($path in @($publicPath, $manifestPath, $sigPath)) {
        if (-not (Test-Path -LiteralPath $path)) { Fail "$slot fixture 文件不存在: $path" }
    }
    $pem = Get-Content -LiteralPath $publicPath -Raw
    if ($pem -notmatch '-----BEGIN PUBLIC KEY-----') { Fail "$slot fixture 不是 SPKI public key: $publicPath" }
    & node $validator check $manifestPath --sig $sigPath --keys-dir $keysDir `
        --expect-current-version 0.2.0 --expect-channel stable
    if ($LASTEXITCODE -ne 0) { Fail "$slot fixture manifest 验签失败: $keyId" }
    Write-Host "[verify-key-rotation] OK: $slot key_id=$keyId（双钥期可验签）"
}

$privateKeys = @(Get-ChildItem -LiteralPath $fixtureDir -Recurse -File -Filter '*.private.pem' -ErrorAction SilentlyContinue)
if ($privateKeys.Count -ne 0) { Fail 'fixture 中出现 private key 文件' }
Write-Host '[verify-key-rotation] PASS: fixture-only current/next 双公钥轮换演练'
exit 0
