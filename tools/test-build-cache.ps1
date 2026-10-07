# Metadata-only safety regressions: no real cache or filesystem deletion.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$tokens = $null; $parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $repo 'tools/maintain-build-cache.ps1'), [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
foreach ($node in $ast.FindAll({ param($item)
    $item -is [Management.Automation.Language.FunctionDefinitionAst]
}, $true)) { . ([scriptblock]::Create($node.Extent.Text)) }

$testRoot = 'D:\QA 空间\gamer'
$profile = "$testRoot\server\target\debug"
$script:Nodes = @{}
$script:Deleted = @()
$script:Workers = @()
$script:Programs = @()
$script:Escaped = $false
function Reset-Fixture {
    $script:Nodes = @{}; $script:Deleted = @(); $script:Workers = @(); $script:Programs = @(); $script:Escaped = $false
    foreach ($path in @($testRoot, "$testRoot\server", "$testRoot\server\target", $profile,
        "$profile\deps", "$profile\incremental", "$testRoot\server\data")) {
        $script:Nodes[$path] = [pscustomobject]@{ FullName=$path; PSIsContainer=$true; Attributes=[IO.FileAttributes]::Directory; Length=0 }
    }
    foreach ($entry in @(@("$profile\deps\old-test.exe", 24GB), @("$profile\incremental\old.o", 1GB),
        @("$profile\gamer-server.exe", 1GB), @("$testRoot\server\data\gamer.db", 1MB))) {
        $script:Nodes[$entry[0]] = [pscustomobject]@{ FullName=$entry[0]; PSIsContainer=$false; Attributes=[IO.FileAttributes]::Archive; Length=$entry[1] }
    }
}
function Test-Path { param($LiteralPath) $script:Nodes.ContainsKey($LiteralPath) }
function Get-Item { param($LiteralPath, [switch]$Force) $script:Nodes[$LiteralPath] }
function Get-ChildItem {
    param($LiteralPath, [switch]$Force)
    $script:Nodes.Values | Where-Object { [IO.Path]::GetDirectoryName($_.FullName) -eq $LiteralPath }
}
function Resolve-Path {
    param($LiteralPath)
    [pscustomobject]@{Path=$(if ($script:Escaped) {'D:\outside\deps'} else {$LiteralPath})}
}
function Get-Process { param($Name, $ErrorAction) $script:Workers }
function Get-CimInstance { param($ClassName, $ErrorAction) $script:Programs }
function Remove-Item { param($LiteralPath, [switch]$Recurse, [switch]$Force) $script:Deleted += $LiteralPath }
function Assert { param([bool]$Condition, [string]$Message) if (-not $Condition) { throw $Message } }

Reset-Fixture
Invoke-BuildCacheMaintenance -RepoRoot $testRoot -MaxGiB 20 -Apply $false
Assert ($script:Deleted.Count -eq 0) 'Preview must not delete'
Invoke-BuildCacheMaintenance -RepoRoot $testRoot -MaxGiB 30 -Apply $true
Assert ($script:Deleted.Count -eq 0) 'Below threshold must not delete'
Invoke-BuildCacheMaintenance -RepoRoot $testRoot -MaxGiB 20 -Apply $true
Assert (($script:Deleted -join '|') -eq "$profile\deps|$profile\incremental") 'Only fixed cache folders may be removed'
Assert (-not ($script:Deleted -contains "$profile\gamer-server.exe")) 'Keep root program'
Assert (-not ($script:Deleted -contains "$testRoot\server\data")) 'Keep runtime data'

foreach ($path in @("$profile\deps\active-test.exe", "\\?\$profile\gamer-server.exe")) {
    Reset-Fixture
    $script:Programs = @([pscustomobject]@{Name='gamer-server.exe'; ExecutablePath=$path})
    Invoke-BuildCacheMaintenance -RepoRoot $testRoot -MaxGiB 20 -Apply $true
    Assert ($script:Deleted.Count -eq 0) 'A running target must defer cleanup'
}
Reset-Fixture; $script:Workers = @([pscustomobject]@{Name='cargo'})
Invoke-BuildCacheMaintenance -RepoRoot $testRoot -MaxGiB 20 -Apply $true
Assert ($script:Deleted.Count -eq 0) 'An active compiler must defer cleanup'
Reset-Fixture; $script:Programs = @([pscustomobject]@{Name='gamer-server.exe'; ExecutablePath=$null})
Invoke-BuildCacheMaintenance -RepoRoot $testRoot -MaxGiB 20 -Apply $true
Assert ($script:Deleted.Count -eq 0) 'Unknown Gamer ownership must defer cleanup'

Reset-Fixture; $script:Escaped = $true
Invoke-BuildCacheMaintenance -RepoRoot $testRoot -MaxGiB 20 -Apply $true
Assert ($script:Deleted.Count -eq 0) 'A resolved path outside the cache must be rejected'
foreach ($path in @($testRoot, "$testRoot\server\target", "$profile\deps", "$profile\deps\old-test.exe")) {
    Reset-Fixture
    $script:Nodes[$path].Attributes = [IO.FileAttributes]::ReparsePoint
    Invoke-BuildCacheMaintenance -RepoRoot $testRoot -MaxGiB 20 -Apply $true
    Assert ($script:Deleted.Count -eq 0) 'Parent and nested links must be rejected before deletion'
}
Write-Host 'PASS build cache: quota, preview, running programs, compilers, data preservation and path boundaries.'
exit 0
