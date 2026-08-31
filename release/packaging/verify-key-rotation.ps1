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

# ---------- 负例（仍 fixture-only：只消费 fixture 公钥与签名，全部期望被拒） ----------
# 覆盖 §11.1：未签名 / 错误 key / 未知 key（撤销）/ manifest 改一字节 全部拒绝（fail closed）。
$workRoot = Join-Path ([IO.Path]::GetTempPath()) ('gamer-keyrot-neg-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $workRoot -Force | Out-Null
$fixtureKeysDir = [IO.Path]::GetFullPath((Join-Path $fixtureDir '..\keys'))
$current = $meta.current
$manifestPath = [IO.Path]::GetFullPath((Join-Path $fixtureDir ([string]$current.manifest)))
$sigPath = [IO.Path]::GetFullPath((Join-Path $fixtureDir ([string]$current.signature)))
try {
    function Invoke-ExpectReject {
        param([string]$Label, [string]$ExpectedCode, [string[]]$ValidatorArgs)
        $output = (& node $validator @ValidatorArgs 2>&1 | Out-String)
        if ($LASTEXITCODE -eq 0) { Fail "负例未被拒: $Label`n$output" }
        if ($output -notmatch "\[$ExpectedCode\]") { Fail "负例错误码不符: $Label 期望 [$ExpectedCode]`n$output" }
        Write-Host "[verify-key-rotation] OK（负例拒绝）: $Label → [$ExpectedCode]"
    }

    # Neg A：未签名（无 .sig）
    $unsignedDir = Join-Path $workRoot 'unsigned'
    New-Item -ItemType Directory -Path $unsignedDir | Out-Null
    Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $unsignedDir 'm.json')
    Invoke-ExpectReject -Label '未签名 manifest' -ExpectedCode 'unsigned-manifest' `
        -ValidatorArgs @('check', (Join-Path $unsignedDir 'm.json'), '--keys-dir', $fixtureKeysDir)

    # Neg B：manifest 篡改 1 字节（签名覆盖原始字节，必须 signature-invalid）
    $tamperDir = Join-Path $workRoot 'tampered'
    New-Item -ItemType Directory -Path $tamperDir | Out-Null
    $raw = [IO.File]::ReadAllBytes($manifestPath)
    for ($i = 0; $i -lt $raw.Length; $i++) {
        if ($raw[$i] -ge 0x61 -and $raw[$i] -le 0x7a) { $raw[$i] = $raw[$i] + 1 - 0x61 + 0x41; break }  # 首个小写字母翻成大写
    }
    [IO.File]::WriteAllBytes((Join-Path $tamperDir 'm.json'), $raw)
    Copy-Item -LiteralPath $sigPath -Destination (Join-Path $tamperDir 'm.sig')
    Invoke-ExpectReject -Label 'manifest 篡改 1 字节' -ExpectedCode 'signature-invalid' `
        -ValidatorArgs @('check', (Join-Path $tamperDir 'm.json'), '--keys-dir', $fixtureKeysDir)

    # Neg C：未知 key / 撤销（信任库只剩 next 公钥，current 签名必须被拒）
    $revokeDir = Join-Path $workRoot 'trust-next-only'
    New-Item -ItemType Directory -Path $revokeDir | Out-Null
    Copy-Item -LiteralPath ([IO.Path]::GetFullPath((Join-Path $fixtureDir ([string]$meta.next.public_key)))) -Destination $revokeDir
    Invoke-ExpectReject -Label '撤销 current key（信任库无其公钥）' -ExpectedCode 'unknown-key-id' `
        -ValidatorArgs @('check', $manifestPath, '--sig', $sigPath, '--keys-dir', $revokeDir)

    # Neg D：错误 key（信任库有 next 公钥但用它验 current 签名）
    $nextPub = [IO.Path]::GetFullPath((Join-Path $fixtureDir ([string]$meta.next.public_key)))
    Invoke-ExpectReject -Label '错误 key（next 公钥验 current 签名）' -ExpectedCode 'signature-invalid' `
        -ValidatorArgs @('check', $manifestPath, '--sig', $sigPath, '--key', $nextPub)
} finally {
    Remove-Item -LiteralPath $workRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host '[verify-key-rotation] PASS: fixture-only current/next 双公钥轮换演练 + 未签名/篡改/撤销/错误钥负例'
exit 0
