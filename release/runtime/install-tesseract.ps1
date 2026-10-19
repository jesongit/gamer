#requires -Version 5.1
<#
Explicit installation of pinned optional Windows OCR dependencies.
Does not execute the installer or remove existing runtime/user files.
Requires 7-Zip; downloads are verified before extracting or copying files.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$RuntimeDir,
    [string]$SevenZipPath
)

$ErrorActionPreference = 'Stop'
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw 'This installer supports Windows x64 only; configure a system Tesseract installation on other platforms.'
}
if (-not [Environment]::Is64BitOperatingSystem) { throw 'Tesseract requires Windows x64.' }

$lockPath = Join-Path $PSScriptRoot 'ocr.lock.json'
$lock = Get-Content -LiteralPath $lockPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ($lock.schema_version -ne 1 -or $lock.platform -ne 'windows-x86_64') { throw 'Unsupported OCR runtime lock.' }
if (-not $SevenZipPath) {
    $sevenZip = Get-Command '7z.exe' -ErrorAction SilentlyContinue
    if (-not $sevenZip) { throw 'Install 7-Zip or pass -SevenZipPath with the absolute 7z.exe path.' }
    $SevenZipPath = $sevenZip.Source
}
$SevenZipPath = (Resolve-Path -LiteralPath $SevenZipPath).ProviderPath
if (-not (Test-Path -LiteralPath $SevenZipPath -PathType Leaf)) { throw '7-Zip executable not found.' }

$runtimeRoot = [IO.Path]::GetFullPath($RuntimeDir).TrimEnd([char[]]@('\', '/'))
$volumeRoot = [IO.Path]::GetPathRoot($runtimeRoot).TrimEnd([char[]]@('\', '/'))
if ($runtimeRoot.Equals($volumeRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Choose a dedicated OCR runtime directory, not a drive root.' }
if (Test-Path -LiteralPath $runtimeRoot) {
    $runtimeItem = Get-Item -LiteralPath $runtimeRoot
    if (-not $runtimeItem.PSIsContainer -or ($runtimeItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'RuntimeDir must be an ordinary directory, not a file or junction.'
    }
}

function Assert-Destination {
    param([string]$Path)
    $absolute = [IO.Path]::GetFullPath($Path)
    if (-not $absolute.StartsWith($runtimeRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Runtime destination escapes the requested directory: $absolute"
    }
    $cursor = $absolute
    while ($cursor.StartsWith($runtimeRoot, [StringComparison]::OrdinalIgnoreCase)) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Runtime destination contains a link/junction: $cursor" }
        }
        if ($cursor.Equals($runtimeRoot, [StringComparison]::OrdinalIgnoreCase)) { break }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
    $absolute
}

function Get-Sha256 {
    param([string]$Path)
    $stream = [IO.File]::OpenRead($Path)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '').ToLowerInvariant() }
    finally { $stream.Dispose(); $sha.Dispose() }
}

function Get-VerifiedDownload {
    param($Artifact, [string]$Destination)
    if ($Artifact.sha256 -notmatch '^[a-f0-9]{64}$') { throw 'Invalid SHA256 in OCR runtime lock.' }
    $uri = [Uri]$Artifact.url
    if ($uri.Scheme -ne 'https' -or $uri.Host -notin @('github.com', 'raw.githubusercontent.com')) { throw 'Unexpected OCR dependency URL.' }
    Write-Host "Downloading $($uri.AbsolutePath)"
    Invoke-WebRequest -UseBasicParsing -Uri $uri.AbsoluteUri -OutFile $Destination -TimeoutSec 180
    if ((Get-Sha256 $Destination) -ne $Artifact.sha256) { throw "OCR dependency SHA256 mismatch: $($uri.AbsolutePath)" }
    if ($Artifact.size_bytes -and (Get-Item -LiteralPath $Destination).Length -ne [int64]$Artifact.size_bytes) {
        throw "OCR dependency size mismatch: $($uri.AbsolutePath)"
    }
}

$stage = Join-Path (Split-Path -Parent $runtimeRoot) ('download-' + [Guid]::NewGuid().ToString('N'))
$extractRoot = Join-Path $stage 'extracted'
New-Item -ItemType Directory -Path $extractRoot -Force | Out-Null
Write-Host "Download staging directory: $stage"
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$installer = Join-Path $stage 'tesseract-setup.exe'
Get-VerifiedDownload $lock.engine $installer

# Only runtime files and copyright documents are extracted. No NSIS helper,
# training executable, installed service, registry change or installer execution.
$previousPreference = $ErrorActionPreference
try {
    $ErrorActionPreference = 'Continue'
    & $SevenZipPath x $installer "-o$extractRoot" '-y' '-bd' '-i!tesseract.exe' '-i!*.dll' '-i!doc\LICENSE' '-i!doc\AUTHORS' '-i!doc\README.md' '-x!$PLUGINSDIR\*' | Out-Null
    $extractExitCode = $LASTEXITCODE
}
finally { $ErrorActionPreference = $previousPreference }
if ($extractExitCode -ne 0) { throw "Tesseract extraction failed (exit $extractExitCode)." }

$payload = @()
foreach ($file in Get-ChildItem -LiteralPath $extractRoot -File | Sort-Object Name) {
    if ($file.Name -eq 'tesseract.exe' -or $file.Extension -eq '.dll') {
        $payload += [pscustomobject]@{ Source = $file.FullName; Relative = $file.Name }
    }
}
if (-not ($payload | Where-Object { $_.Relative -eq 'tesseract.exe' })) { throw 'Pinned installer does not contain tesseract.exe.' }
if (-not ($payload | Where-Object { $_.Relative -eq 'libtesseract-5.dll' })) { throw 'Pinned installer does not contain libtesseract-5.dll.' }
foreach ($relative in $lock.engine.retained_license_files) {
    $source = Join-Path $extractRoot $relative
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Pinned installer is missing its copyright document: $relative" }
    $payload += [pscustomobject]@{ Source = $source; Relative = $relative }
}
foreach ($model in $lock.models.files) {
    if ($model.language -notmatch '^[a-z0-9_]+$') { throw 'Invalid model language in OCR runtime lock.' }
    $source = Join-Path $stage ($model.language + '.traineddata')
    Get-VerifiedDownload $model $source
    $payload += [pscustomobject]@{ Source = $source; Relative = 'tessdata/' + $model.language + '.traineddata' }
}
$payload += [pscustomobject]@{ Source = $lockPath; Relative = 'gamer-ocr-runtime.lock.json' }

# Check the whole write set before copying. A repeated install reuses identical
# files; conflicting files require another target directory and remain untouched.
$prepared = @()
foreach ($file in $payload) {
    $destination = Assert-Destination (Join-Path $runtimeRoot $file.Relative)
    $sha256 = Get-Sha256 $file.Source
    if (Test-Path -LiteralPath $destination) {
        if (-not (Test-Path -LiteralPath $destination -PathType Leaf) -or (Get-Sha256 $destination) -ne $sha256) {
            throw "Existing file differs from the pinned runtime; choose a new RuntimeDir. Preserved: $destination"
        }
    }
    $prepared += [pscustomobject]@{ Source = $file.Source; Destination = $destination; Sha256 = $sha256 }
}

foreach ($file in $prepared) {
    $destination = Assert-Destination $file.Destination
    if (-not (Test-Path -LiteralPath $destination)) {
        New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($destination)) -Force | Out-Null
        $destination = Assert-Destination $destination
        [IO.File]::Copy($file.Source, $destination, $false)
    }
    if ((Get-Sha256 $destination) -ne $file.Sha256) { throw "Installed OCR runtime verification failed: $destination" }
}

Write-Host "Ready: $runtimeRoot (Tesseract $($lock.engine.version), chi_sim + eng)"
Write-Host 'Existing files and user language packs were preserved. Staging files were retained for inspection.'
