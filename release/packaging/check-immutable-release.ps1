# Release tag 与正式资产不可覆盖。
[CmdletBinding()]
param(
    [ValidateSet('Snapshot', 'GitHub')]
    [string]$Mode = 'Snapshot',
    [string]$Tag = '',
    [string]$CommitSha = '',
    [string]$Repository = '',
    [string]$SnapshotPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

function Fail {
    param([string]$Message)
    Write-Error "[immutable-release] FAIL: $Message"
    exit 1
}

function Require-Command {
    param([string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        Fail "缺少命令: $Name"
    }
}

function Assert-Tag {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { Fail 'tag 不能为空' }
    # 同时严格拒绝 SemVer 禁止的数字前导零，避免两个产品版本映射到同一 OCI 语义。
    $identifier = '(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)'
    $semver = '(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-' + $identifier + '(?:\.' + $identifier + ')*)?'
    if ($Value -notmatch ('^v' + $semver + '$')) {
        Fail "tag 不合法: $Value（须为可映射到 OCI tag 的严格 v<semver>，不含 +build metadata）"
    }
}

function Assert-CommitSha {
    param([string]$Value, [string]$Label = 'commit SHA')
    if ([string]::IsNullOrWhiteSpace($Value) -or $Value -notmatch '^[0-9a-fA-F]{40}$') {
        Fail "$Label 不是 40 位 commit SHA: $Value"
    }
}

function Test-SnapshotState {
    param(
        [Parameter(Mandatory = $true)][object]$State,
        [Parameter(Mandatory = $true)][string]$ExpectedTag,
        [Parameter(Mandatory = $true)][string]$ExpectedCommit
    )

    if ([int]$State.schemaVersion -ne 1) { Fail "snapshot schemaVersion 不是 1" }
    if ([string]$State.tag -cne $ExpectedTag) {
        Fail "snapshot tag=$($State.tag) 与触发 tag=$ExpectedTag 不一致"
    }
    $stateCommit = [string]$State.tagCommit
    if ($stateCommit -notmatch '^[0-9a-fA-F]{40}$' -and $stateCommit -notmatch '^[0-9a-fA-F]{64}$') {
        Fail "snapshot tagCommit 不是 commit SHA: $stateCommit"
    }
    if ($stateCommit.ToLowerInvariant() -ne $ExpectedCommit.ToLowerInvariant()) {
        Fail "tag $ExpectedTag 指向 $stateCommit，不等于触发 commit $ExpectedCommit"
    }

    $releaseExists = [bool]$State.release.exists
    if ($releaseExists) {
        if (-not [bool]$State.release.isDraft) {
            Fail "Release $ExpectedTag 已是正式发布，immutable tag 拒绝重跑/覆盖"
        }
        Write-Host "[immutable-release] existing Release 是 draft：允许进入同 hash 资产门禁"
    } else {
        Write-Host "[immutable-release] existing Release: none"
    }

    Write-Host "[immutable-release] PASS: $ExpectedTag -> $ExpectedCommit"
}

function Get-GitTagCommit {
    param([string]$Value)
    # refspec 用单引号拼接：PowerShell 双引号不处理 {}，`"^{{}}"` 会把字面双花括号
    # 传给 git（-f 格式串的 `{{`→`{` 转义只在 -f 运算符内生效，混用极易踩坑）
    $rows = @(& git ls-remote origin ('refs/tags/' + $Value + '^{}') 2>&1)
    $code = $LASTEXITCODE
    if ($code -ne 0) { Fail "git ls-remote 查询 tag 失败: $($rows -join ' ')" }
    $line = @($rows | Where-Object { $_ -match '^[0-9a-fA-F]{40}\s+refs/tags/.+\^\{\}$' } | Select-Object -First 1)
    if (-not $line) {
        $rows = @(& git ls-remote origin ("refs/tags/$Value") 2>&1)
        $code = $LASTEXITCODE
        if ($code -ne 0) { Fail "git ls-remote 查询 lightweight tag 失败: $($rows -join ' ')" }
        $line = @($rows | Where-Object { $_ -match '^[0-9a-fA-F]{40}\s+refs/tags/' } | Select-Object -First 1)
    }
    if (-not $line) { Fail "远端不存在 tag: $Value" }
    return ([string]$line[0] -split '\s+')[0]
}

function Get-GhReleaseJson {
    param([string]$Repo, [string]$Value)
    $endpoint = "repos/$Repo/releases/tags/$Value"
    $raw = (& gh api $endpoint --include 2>&1 | Out-String)
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        if ($raw -match '(?im)^HTTP/[^\s]+\s+404(?:\s|$)') {
            return $null
        }
        Fail "无法查询 GitHub Release（fail closed）: $($raw.Trim())"
    }
    $jsonStart = $raw.IndexOf('{')
    if ($jsonStart -lt 0) { Fail "GitHub Release 响应缺少 JSON: $($raw.Trim())" }
    try { return ($raw.Substring($jsonStart) | ConvertFrom-Json) }
    catch { Fail "GitHub Release 响应不是合法 JSON: $($_.Exception.Message)" }
}

Assert-Tag -Value $Tag

if ($Mode -eq 'Snapshot') {
    if ([string]::IsNullOrWhiteSpace($SnapshotPath)) { Fail 'Snapshot 模式需要 -SnapshotPath' }
    if (-not (Test-Path -LiteralPath $SnapshotPath)) { Fail "snapshot 不存在: $SnapshotPath" }
    if ([string]::IsNullOrWhiteSpace($CommitSha)) { Fail 'Snapshot 模式需要 -CommitSha' }
    Assert-CommitSha -Value $CommitSha -Label 'snapshot expected commit'
    try { $state = Get-Content -LiteralPath $SnapshotPath -Raw | ConvertFrom-Json }
    catch { Fail "snapshot 不是合法 JSON: $($_.Exception.Message)" }
    Test-SnapshotState -State $state -ExpectedTag $Tag -ExpectedCommit $CommitSha
    exit 0
}

if ($Mode -eq 'GitHub') {
    if ([string]::IsNullOrWhiteSpace($CommitSha)) { Fail 'GitHub 模式需要 -CommitSha' }
    Assert-CommitSha -Value $CommitSha -Label 'GitHub expected commit'
    if ([string]::IsNullOrWhiteSpace($Repository)) { Fail 'GitHub 模式需要 -Repository' }
    Require-Command -Name 'git'
    Require-Command -Name 'gh'

    # 双引号里 `{{` 是字面量：旧写法 "^{{commit}}" 把双花括号传给 git rev-parse
    # 必 fatal（exit 128）。单引号不插值，与 $Tag 拼接才能得到 `^{commit}` peel 语法
    $local = @(& git rev-parse --verify ("$Tag" + '^{commit}') 2>&1)
    if ($LASTEXITCODE -ne 0) { Fail "本地 checkout 无法解析 tag $Tag：$($local -join ' ')" }
    $localCommit = ([string]$local[0]).Trim()
    if ($localCommit.ToLowerInvariant() -ne $CommitSha.ToLowerInvariant()) {
        Fail "本地 tag $Tag -> $localCommit，不等于触发 commit $CommitSha"
    }
    $remoteCommit = Get-GitTagCommit -Value $Tag
    if ($remoteCommit.ToLowerInvariant() -ne $CommitSha.ToLowerInvariant()) {
        Fail "远端 tag $Tag -> $remoteCommit，不等于触发 commit $CommitSha"
    }
    $release = Get-GhReleaseJson -Repo $Repository -Value $Tag
    if ($null -ne $release -and -not [bool]$release.draft) {
        Fail "GitHub Release $Tag 已发布（draft=false），immutable tag 拒绝继续"
    }
    if ($null -ne $release) {
        if ([string]$release.tag_name -cne $Tag) {
            Fail "GitHub Release tag_name=$($release.tag_name) 与触发 tag=$Tag 不一致"
        }
        Write-Host "[immutable-release] GitHub Release $Tag 已存在且仍为 draft"
    } else {
        Write-Host "[immutable-release] GitHub Release $Tag 不存在"
    }
    Write-Host "[immutable-release] PASS: GitHub tag/release preflight"
    exit 0
}
