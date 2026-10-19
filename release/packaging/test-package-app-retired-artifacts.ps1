# Offline fixtures only: extract the two helper functions without running any
# release build, download, public copy, installation or executable.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

function Import-TestFunction([string]$Path, [string]$Name) {
    $tokens = $null; $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
    if ($errors.Count -gt 0) { throw "AST parse failed: $Path" }
    $function = @($ast.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $Name }, $false))
    if ($function.Count -ne 1) { throw "Expected one function: $Name" }
    return [scriptblock]::Create($function[0].Extent.Text)
}
function Assert-True([bool]$Value, [string]$Message) { if (-not $Value) { throw $Message } }
function Assert-Rejected([scriptblock]$Action, [string]$Pattern) {
    $rejected = $false
    try { & $Action } catch { $rejected = $_.Exception.Message -match $Pattern; if (-not $rejected) { throw } }
    Assert-True $rejected "Expected rejection: $Pattern"
}
function Write-Fixture([string]$Path, [string]$Text) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}
function Get-FixtureHashes([string]$Root) {
    return (@(Get-ChildItem -LiteralPath $Root -File -Recurse | Sort-Object FullName | ForEach-Object {
        $_.FullName.Substring($Root.Length) + ':' + (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    }) -join "`n")
}

. (Import-TestFunction (Join-Path $PSScriptRoot 'package-app.ps1') 'Remove-RetiredAiStageArtifacts')
. (Import-TestFunction (Join-Path $PSScriptRoot 'fetch-plugins.ps1') 'Assert-PublishedPluginSet')
. (Import-TestFunction (Join-Path $PSScriptRoot 'fetch-plugins.ps1') 'Resolve-PluginAssetDestinations')
$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Import-TestFunction (Join-Path $repoRoot 'plugins/build.ps1') 'Assert-OfficialBuildPlugin')
. (Import-TestFunction (Join-Path $repoRoot 'plugins/build.ps1') 'Test-RetiredOfficialBuildMetadata')
$tempBoundary = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([char[]]'\/') + [IO.Path]::DirectorySeparatorChar
$testRoot = [IO.Path]::GetFullPath((Join-Path $tempBoundary ('gamer-retired-stage-' + [guid]::NewGuid().ToString('N'))))
if (-not $testRoot.StartsWith($tempBoundary, [StringComparison]::OrdinalIgnoreCase)) { throw 'Fixture escaped temp root' }
$junction = $null
try {
    $source = Join-Path $testRoot 'source/web-dist'
    $stage = Join-Path $testRoot 'stage'
    $keep = @('index.html', 'assets/app.js', 'plugin-ui/gamer-keymap/plugin.js', 'plugin-ui/gamer-ai-helper/plugin.js', 'plugins/gamer-keymap-0.3.21.gplugin', 'plugins/gamer-ai-1helper-1.0.gplugin')
    foreach ($name in $keep + @('plugin-ui/gamer-ai/plugin.js', 'plugin-ui/gamer-ai/nested/style.css', 'plugin-ui/gamer-ocr/plugin.js', 'plugins/gamer-ocr-0.1.1.gplugin', 'plugin-ui/gamer-yaml/plugin.js', 'plugins/gamer-ai-0.1.0.gplugin', 'downloads/gamer-ai-0.1.0.gplugin', 'plugins/gamer-yaml-0.1.0.gplugin')) {
        Write-Fixture (Join-Path $source $name) "unchanged fixture: $name"
    }
    Write-Fixture (Join-Path $source 'registry.json') '{"schema_version":2,"provenance":{"keep":"original"},"plugins":[{"id":"gamer-ai","version":"0.1.0"},{"id":"gamer-ocr","version":"0.1.1"},{"id":"gamer-yaml","version":"0.1.0","permissions":[" ai.connect "]},{"id":"gamer-keymap","version":"0.3.21","sha256":"unchanged"},{"id":"gamer-ai-1helper","version":"1.0"}]}'
    $sourceHashes = Get-FixtureHashes $source
    New-Item -ItemType Directory -Path $stage | Out-Null
    Copy-Item -LiteralPath $source -Destination $stage -Recurse
    Remove-RetiredAiStageArtifacts -StageRoot $stage
    $stagedWeb = Join-Path $stage 'web-dist'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $stagedWeb 'plugin-ui/gamer-ai'))) 'Retired UI remained'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $stagedWeb 'plugin-ui/gamer-ocr'))) 'Retired OCR UI remained'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $stagedWeb 'plugins/gamer-ocr-0.1.1.gplugin'))) 'Retired OCR archive remained'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $stagedWeb 'plugin-ui/gamer-yaml'))) 'Retired permission UI remained'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $stagedWeb 'plugins/gamer-ai-0.1.0.gplugin')) -and -not (Test-Path -LiteralPath (Join-Path $stagedWeb 'downloads/gamer-ai-0.1.0.gplugin')) -and -not (Test-Path -LiteralPath (Join-Path $stagedWeb 'plugins/gamer-yaml-0.1.0.gplugin'))) 'Retired archive remained'
    foreach ($name in $keep) {
        Assert-True ((Get-FileHash -LiteralPath (Join-Path $source $name)).Hash -eq (Get-FileHash -LiteralPath (Join-Path $stagedWeb $name)).Hash) "Other bytes changed: $name"
    }
    $registry = [IO.File]::ReadAllText((Join-Path $stagedWeb 'registry.json')) | ConvertFrom-Json
    Assert-True (@($registry.plugins).Count -eq 2 -and $registry.plugins[0].id -eq 'gamer-keymap' -and $registry.plugins[0].sha256 -eq 'unchanged' -and $registry.plugins[1].id -eq 'gamer-ai-1helper' -and $registry.provenance.keep -eq 'original') 'Registry removed more than retired distributions'
    Assert-True ((Get-FixtureHashes $source) -eq $sourceHashes) 'Source snapshot changed'
    $after = Get-FixtureHashes $stage
    Remove-RetiredAiStageArtifacts -StageRoot $stage
    Assert-True ((Get-FixtureHashes $stage) -eq $after) 'Filtering is not idempotent'

    $multiVersion = Join-Path $testRoot 'multi-version'
    Write-Fixture (Join-Path $multiVersion 'web-dist/registry.json') '{"plugins":[{"id":"gamer-yaml","version":"0.1.0","permissions":["ai.connect"]},{"id":"gamer-yaml","version":"1.0.0","permissions":["resource.read"]}]}'
    Write-Fixture (Join-Path $multiVersion 'web-dist/plugin-ui/gamer-yaml/plugin.js') 'clean current shared UI'
    Write-Fixture (Join-Path $multiVersion 'web-dist/plugins/gamer-yaml-0.1.0.gplugin') 'retired version'
    Write-Fixture (Join-Path $multiVersion 'web-dist/plugins/gamer-yaml-1.0.0.gplugin') 'current version'
    Remove-RetiredAiStageArtifacts -StageRoot $multiVersion
    Assert-True ((Test-Path -LiteralPath (Join-Path $multiVersion 'web-dist/plugin-ui/gamer-yaml/plugin.js')) -and (Test-Path -LiteralPath (Join-Path $multiVersion 'web-dist/plugins/gamer-yaml-1.0.0.gplugin')) -and -not (Test-Path -LiteralPath (Join-Path $multiVersion 'web-dist/plugins/gamer-yaml-0.1.0.gplugin'))) 'Retired version blocked the clean version shared UI'

    $withoutRegistry = Join-Path $testRoot 'without-registry'
    Write-Fixture (Join-Path $withoutRegistry 'web-dist/plugin-ui/gamer-ai/plugin.js') 'retired'
    Write-Fixture (Join-Path $withoutRegistry 'web-dist/plugins/gamer-ai-1.0.gplugin') 'retired'
    Assert-Rejected { Remove-RetiredAiStageArtifacts -StageRoot $withoutRegistry } '缺少 registry'
    Assert-True (Test-Path -LiteralPath (Join-Path $withoutRegistry 'web-dist/plugins/gamer-ai-1.0.gplugin')) 'Missing registry caused filename-based deletion'

    $unknownStage = Join-Path $testRoot 'unknown-stage'
    Write-Fixture (Join-Path $unknownStage 'web-dist/registry.json') '{"plugins":[]}'
    Write-Fixture (Join-Path $unknownStage 'web-dist/plugins/gamer-ai-1helper-1.0.gplugin') 'cannot guess ownership'
    Assert-Rejected { Remove-RetiredAiStageArtifacts -StageRoot $unknownStage } '权威归属'
    Assert-True (Test-Path -LiteralPath (Join-Path $unknownStage 'web-dist/plugins/gamer-ai-1helper-1.0.gplugin')) 'Unknown archive was deleted'

    # A reparse point fails before any deletion, even with a matching filename.
    $linkedStage = Join-Path $testRoot 'linked-stage'
    $outside = Join-Path $testRoot 'outside'
    Write-Fixture (Join-Path $outside 'keep.txt') 'outside must survive'
    Write-Fixture (Join-Path $linkedStage 'web-dist/plugins/gamer-ai-1.0.gplugin') 'must remain after rejection'
    $junction = Join-Path $linkedStage 'web-dist/plugin-ui/gamer-ai'
    New-Item -ItemType Directory -Path (Split-Path -Parent $junction) -Force | Out-Null
    $linkType = if ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT) { 'Junction' } else { 'SymbolicLink' }
    New-Item -ItemType $linkType -Path $junction -Target $outside | Out-Null
    Assert-Rejected { Remove-RetiredAiStageArtifacts -StageRoot $linkedStage } '重解析点'
    Assert-True ((Test-Path -LiteralPath (Join-Path $outside 'keep.txt')) -and (Test-Path -LiteralPath (Join-Path $linkedStage 'web-dist/plugins/gamer-ai-1.0.gplugin'))) 'Reparse rejection changed files'
    # Directory.Delete without recursive removes only this junction. Windows
    # PowerShell 5.1 Remove-Item can throw for a directory junction.
    [IO.Directory]::Delete($junction)
    $junction = $null

    $required = @('gamer-keymap', 'gamer-video', 'gamer-yaml')
    $modern = @($required + @('gamer-live', 'gamer-notify', 'gamer-package-publisher') | ForEach-Object { [pscustomobject]@{id = $_} })
    Assert-PublishedPluginSet -Plugins $modern -RequiredIds $required
    foreach ($retiredId in @('gamer-ai', 'gamer-ocr')) {
        $historical = $modern + @([pscustomobject]@{id = $retiredId})
        Assert-Rejected { Assert-PublishedPluginSet -Plugins $historical -RequiredIds $required } 'release/plugins\.lock\.json'
        Assert-PublishedPluginSet -Plugins $historical -RequiredIds $required -TestFixturesOnly
    }
    $retiredPermission = @($modern | Where-Object id -NE 'gamer-yaml') + @([pscustomobject]@{id = 'gamer-yaml'; permissions = @(' ai.connect ')})
    Assert-Rejected { Assert-PublishedPluginSet -Plugins $retiredPermission -RequiredIds $required } 'ai.connect'
    Assert-PublishedPluginSet -Plugins $retiredPermission -RequiredIds $required -TestFixturesOnly
    Assert-Rejected { Assert-PublishedPluginSet -Plugins ($modern + @([pscustomobject]@{id = 'unknown'})) -RequiredIds $required -TestFixturesOnly } '未知'
    Assert-Rejected { Assert-PublishedPluginSet -Plugins ($modern + @([pscustomobject]@{id = 'gamer-yaml'})) -RequiredIds $required } '重复'
    $fakeRepo = Join-Path $testRoot 'repo'
    $ordinary = Resolve-PluginAssetDestinations -RepoRoot $fakeRepo
    Assert-True ($ordinary.Assets -eq (Join-Path $fakeRepo 'web/public') -and $ordinary.Dist -eq (Join-Path $fakeRepo 'release/dist')) 'Formal destinations changed'
    $fixtures = Resolve-PluginAssetDestinations -RepoRoot $fakeRepo -TestFixturesOnly
    Assert-True ($fixtures.Assets -eq (Join-Path $fakeRepo 'server/target/plugin-test-fixtures') -and $fixtures.Dist.StartsWith($fixtures.Assets) -and $fixtures.Cache.StartsWith($fixtures.Assets)) 'Fixture destinations leak into publication'
    Assert-Rejected { Resolve-PluginAssetDestinations -RepoRoot $fakeRepo -FixturesDir (Join-Path $testRoot 'fixtures') } '仅允许'
    foreach ($protected in @('web/public', 'server/web-dist', 'server/data', 'release/dist', 'plugins')) {
        Assert-Rejected { Resolve-PluginAssetDestinations -RepoRoot $fakeRepo -TestFixturesOnly -FixturesDir (Join-Path $fakeRepo $protected) } '不能写入'
    }
    Assert-Rejected { Resolve-PluginAssetDestinations -RepoRoot $fakeRepo -TestFixturesOnly -DistDir (Join-Path $fakeRepo 'release/dist') } '留在 fixture'
    Assert-Rejected { Assert-OfficialBuildPlugin 'gamer-ai' } '退休'
    Assert-Rejected { Assert-OfficialBuildPlugin 'gamer-ocr' } '退休'
    Assert-OfficialBuildPlugin 'gamer-ai-1helper'
    Assert-True (Test-RetiredOfficialBuildMetadata ([pscustomobject]@{id='gamer-yaml'; permissions=@(' ai.connect ')})) 'Single-plugin registry merge keeps retired permissions'
    Assert-True (-not (Test-RetiredOfficialBuildMetadata ([pscustomobject]@{id='gamer-ai-1helper'}))) 'Unrelated plugin ID was retired by prefix'
    $releaseWorkflow = [IO.File]::ReadAllText((Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) '.github/workflows/release.yml'))
    Assert-True ($releaseWorkflow -notmatch 'fetch-plugins\.ps1[^\r\n]*-TestFixturesOnly') 'Release workflow uses historical fixture exception'
    Write-Host 'PASS: private stage AI filtering, byte preservation, source isolation, idempotence, missing registry, reparse rejection, published-set release/fixture gates'
} finally {
    if ($junction -and (Test-Path -LiteralPath $junction)) { [IO.Directory]::Delete($junction) }
    # This is the exact, prechecked temp fixture root created above.
    if ([IO.Path]::GetFullPath($testRoot).StartsWith($tempBoundary, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $testRoot)) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
