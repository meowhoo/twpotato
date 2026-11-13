<#
.SYNOPSIS
  Builds the TwPotato per-machine MSI with WiX Toolset 5.0.2.
  The same script runs locally and in CI.

.EXAMPLE
  & .\release\build-msi.ps1 -Version 0.1.9 -BinDir C:\stage\bin -AssetsDir C:\stage\assets -OutDir C:\stage\out

  BinDir must contain twime_tsf.dll, twime-engined.exe, twime-settings.exe and twime-setup.exe.
  AssetsDir must hold exactly the files listed in release/model.lock, with matching size and SHA-256
  (release/pack-model.ps1 -CheckDir).

  -FailTest 1|2|3 builds a test MSI for the rollback matrix (section 3.1); never publish those.
  Every build of one Version gets the same ProductCode (derived from UpgradeCode + Version), so a fail-test build
  can be applied to an installed normal build as a repair (REINSTALL=ALL REINSTALLMODE=vomus).

  Signing hook: when the environment variable SIGN_CMD is set, it is run as
  `& $env:SIGN_CMD <file>` for every DLL/EXE before packaging and for the MSI afterwards.
  Unset: nothing is signed and the script says so.

  Prerequisites:
    .NET SDK 8 (winget install --id Microsoft.DotNet.SDK.8 --exact --source winget)
    dotnet tool install --global wix --version 5.0.2      (or pass -Wix <path to wix.exe>)
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [string] $BinDir,
    [Parameter(Mandatory)] [string] $AssetsDir,
    [Parameter(Mandatory)] [string] $OutDir,
    [string] $RepoDir = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path,
    [string] $NoticesFile = (Join-Path $PSScriptRoot '..\THIRD-PARTY-NOTICES.md'),
    [string] $IconFile = (Join-Path $PSScriptRoot '..\ime\settings-app\src-tauri\icons\icon.ico'),
    [string] $Wix = 'wix',
    # The asset table to check AssetsDir against (default release\model.lock); a test lock lets a test MSI carry assets that are
    # not in the released table (e.g. emoji.tsv) without touching the real lock. Never for a published build.
    [string] $ModelLock,
    [ValidateSet(0, 1, 2, 3)] [int] $FailTest = 0,
    # Only for the upgrade-test MSI (a higher MSI version around the same binaries); never for a published build.
    [switch] $SkipBinaryVersionCheck
)
$ErrorActionPreference = 'Stop'
$UpgradeCode = '6342DBCA-A4B4-4B68-99F5-67C0CF412F32'   # must equal Package/@UpgradeCode in wix\TwPotato.wxs
$ProductName = -join [char[]](0x5730, 0x74DC, 0x8F38, 0x5165, 0x6CD5)   # product name (kept ASCII-only in this file)

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version must be MAJOR.MINOR.PATCH: $Version" }

$wixVersion = (& $Wix --version) 2>$null
if ($LASTEXITCODE -ne 0 -or -not $wixVersion -or -not $wixVersion.StartsWith('5.0.2')) {
    throw "WiX 5.0.2 is required (found: '$wixVersion'). Install: dotnet tool install --global wix --version 5.0.2"
}

$binFiles = 'twime_tsf.dll', 'twime-engined.exe', 'twime-settings.exe', 'twime-setup.exe'
foreach ($f in $binFiles) {
    $p = Join-Path $BinDir $f
    if (-not (Test-Path -LiteralPath $p -PathType Leaf)) { throw "missing $f in $BinDir" }
    # VERSIONINFO: the same product name and version in every file (SignPath requirement).
    $vi = (Get-Item -LiteralPath $p).VersionInfo
    if ($vi.ProductName -cne $ProductName -or ($vi.ProductVersion -ne $Version -and -not $SkipBinaryVersionCheck) -or $vi.CompanyName -cne 'meowhoo') {
        throw "$f VERSIONINFO is '$($vi.ProductName)' '$($vi.ProductVersion)' '$($vi.CompanyName)'; expected '$ProductName' '$Version' 'meowhoo' (crate / tauri.conf.json version must equal -Version)"
    }
}

# Allow-list and asset-table hashes (section 4.4).
$packArgs = @{ CheckDir = $AssetsDir }
if ($ModelLock) { $packArgs.Lock = $ModelLock }
& (Join-Path $PSScriptRoot 'pack-model.ps1') @packArgs

$docs = [ordered]@{
    'LICENSE.txt'            = Join-Path $RepoDir 'LICENSE'
    'THIRD-PARTY-NOTICES.md' = $NoticesFile
    'LICENSE-McBopomofo.txt' = Join-Path $RepoDir 'third_party\mcbopomofo\LICENSE-McBopomofo.txt'
    'COPYING-libtabe.txt'    = Join-Path $RepoDir 'third_party\mcbopomofo\COPYING-libtabe.txt'
    'LICENSE-UNICODE-3.0.txt' = Join-Path $RepoDir 'emoji\LICENSE-UNICODE-3.0.txt'
}
foreach ($d in $docs.Values) { if (-not (Test-Path -LiteralPath $d -PathType Leaf)) { throw "missing $d" } }
if (-not (Test-Path -LiteralPath $IconFile -PathType Leaf)) { throw "missing icon $IconFile" }

function Invoke-Sign([string] $Path) {
    if ($env:SIGN_CMD) {
        & $env:SIGN_CMD $Path
        if ($LASTEXITCODE -ne 0) { throw "signing failed: $Path" }
    }
}

# ProductCode: name-based GUID from SHA-256("TwPotato|<UpgradeCode>|<Version>"), RFC 4122 version/variant bits set.
function Get-ProductCode([string] $Ver) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $h = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes("TwPotato|$UpgradeCode|$Ver")) } finally { $sha.Dispose() }
    $b = [byte[]]$h[0..15]
    $b[6] = ($b[6] -band 0x0F) -bor 0x50
    $b[8] = ($b[8] -band 0x3F) -bor 0x80
    $hex = -join ($b | ForEach-Object { $_.ToString('X2') })
    '{' + $hex.Substring(0, 8) + '-' + $hex.Substring(8, 4) + '-' + $hex.Substring(12, 4) + '-' + $hex.Substring(16, 4) + '-' + $hex.Substring(20, 12) + '}'
}
$productCode = Get-ProductCode $Version

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
$suffix = if ($FailTest -ne 0) { "-failtest$FailTest" } else { '' }
# Per-build staging, so building several variants into one OutDir never mixes inputs.
$stage = Join-Path $OutDir "stage-$Version$suffix"
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
$signedBin = Join-Path $stage 'bin'
$docsDir = Join-Path $stage 'docs'
New-Item -ItemType Directory -Path $signedBin, $docsDir | Out-Null
# Sign copies, never the caller's files.
foreach ($f in $binFiles) {
    Copy-Item -LiteralPath (Join-Path $BinDir $f) -Destination $signedBin
    Invoke-Sign (Join-Path $signedBin $f)
}
foreach ($k in $docs.Keys) { Copy-Item -LiteralPath $docs[$k] -Destination (Join-Path $docsDir $k) }
if (-not $env:SIGN_CMD) { Write-Warning 'SIGN_CMD is not set: the binaries and the MSI are NOT signed.' }

$msi = Join-Path $OutDir "TwPotato-$Version-x64$suffix.msi"
$wxs = Join-Path $PSScriptRoot 'wix\TwPotato.wxs'
& $Wix build $wxs -arch x64 -o $msi `
    -d "Version=$Version" -d "ProductCode=$productCode" -d "BinDir=$signedBin" `
    -d "AssetsDir=$((Resolve-Path -LiteralPath $AssetsDir).Path)" -d "DocsDir=$docsDir" `
    -d "IconFile=$((Resolve-Path -LiteralPath $IconFile).Path)" -d "FailTest=$FailTest"
if ($LASTEXITCODE -ne 0) { throw "wix build failed ($LASTEXITCODE)" }
Invoke-Sign $msi

$hash = (Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLower()
"$hash  $(Split-Path -Leaf $msi)" | Set-Content -Encoding ascii -LiteralPath "$msi.sha256"
Write-Output "built $msi"
Write-Output "size $((Get-Item -LiteralPath $msi).Length)"
Write-Output "ProductCode $productCode"
Write-Output "sha256 $hash"
