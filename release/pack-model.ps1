<#
.SYNOPSIS
  Model asset zip for TwPotato: allow-list, screening counts, model.lock hashes.

.DESCRIPTION
  Pack   : & .\release\pack-model.ps1 -Source <assets dir> -Name r9 -OutDir <dir> [-WriteLock]
           Checks the allow-list, prints the screening counts, writes <OutDir>\twime-model-<Name>.zip.
           Without -WriteLock every file must match release\model.lock (name, size, SHA-256).
           With -WriteLock, release\model.lock is rewritten from Source (maintainer step; the user confirms the asset table).
  Unpack : & "...\pack-model.ps1" -Zip <zip> -OutDir <new dir>
           Verifies the zip size and SHA-256 against model.lock, extracts it into a new directory and checks every file.
  Check  : & "...\pack-model.ps1" -CheckDir <dir>
           The directory must hold exactly the files of model.lock with matching size and SHA-256 (used by build-msi.ps1).

  Released content: comment lines (first non-blank character '#') are removed from the *.cfg files when packing; they are
  maintainer notes (generator and source paths) that the engine ignores. Every other file is released byte for byte.
  model.lock records the released bytes; -CheckDir and -Unpack accept only released bytes (an unstripped *.cfg fails).

  Screening only prints counts, never entries: for the text files (data.txt, common-readings.txt) the word field of each
  entry is checked for "longer than 16 characters" and "contains sentence punctuation". Non-zero counts are for the user
  to judge; they do not fail the script. Binary files are not screened.
#>
[CmdletBinding(DefaultParameterSetName = 'Check')]
param(
    [Parameter(Mandatory, ParameterSetName = 'Pack')] [string] $Source,
    [Parameter(Mandatory, ParameterSetName = 'Pack')] [ValidatePattern('^[A-Za-z0-9._-]+$')] [string] $Name,
    [Parameter(ParameterSetName = 'Pack')] [switch] $WriteLock,
    [Parameter(Mandatory, ParameterSetName = 'Unpack')] [string] $Zip,
    [Parameter(Mandatory, ParameterSetName = 'Pack')] [Parameter(Mandatory, ParameterSetName = 'Unpack')] [string] $OutDir,
    [Parameter(Mandatory, ParameterSetName = 'Check')] [string] $CheckDir,
    [string] $Lock = (Join-Path $PSScriptRoot 'model.lock')
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem

$Required = 'data.txt', 'common-readings.txt', 'model.bin', 'prior.pack'
$Optional = 'model.cfg', 'pn.fst', 'pn.lst', 'pn.cfg', 'emoji.tsv', 'gen.fst', 'gen.lst', 'wordbonus.bin', 'wordbonus.tsv', 'lm-f32.bin', 'lm-vocab.json'   # r8a+ deploy assets (user 2026-11-13: all deploy assets public)
$ZipUrlBase = 'https://github.com/meowhoo/twpotato/releases/download'

function Get-Sha256([string] $Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

# Released bytes of a *.cfg asset: the file without its comment lines (line endings of the kept lines unchanged).
function Get-ReleasedCfg($Item) {
    $text = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($Item.FullName))
    # A UTF-8 BOM is kept in front of the result and does not hide a comment on the first line.
    $bom = $text.Length -gt 0 -and $text[0] -eq [char]0xFEFF   # ordinal: culture-aware StartsWith ignores U+FEFF
    if ($bom) { $text = $text.Substring(1) }
    $kept = [regex]::Split($text, '(?<=\n)') | Where-Object { $_ -ne '' -and $_ -notmatch '^\s*#' }
    $bytes = [Text.Encoding]::UTF8.GetBytes($(if ($bom) { [string][char]0xFEFF } else { '' }) + (-join $kept))
    return ,$bytes   # one array object, also when empty (PowerShell would unroll it to $null)
}

# Size and SHA-256 of what is released for this file (see .DESCRIPTION).
function Get-ReleasedInfo($Item) {
    if ($Item.Extension -ieq '.cfg') {
        $b = Get-ReleasedCfg $Item
        $sha = [Security.Cryptography.SHA256]::Create()
        try { $h = -join ($sha.ComputeHash($b) | ForEach-Object { $_.ToString('x2') }) } finally { $sha.Dispose() }
        @{ size = [long]$b.Length; sha = $h }
    } else {
        @{ size = [long]$Item.Length; sha = Get-Sha256 $Item.FullName }
    }
}

# Allow-list (section 4.4): only plain files with allowed names; the required four must be present.
function Test-AssetSet([string] $Dir) {
    $items = @(Get-ChildItem -LiteralPath $Dir -Force)
    foreach ($a in $items) {
        if ($a.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "reparse point in assets: $($a.Name)" }
        if ($a.PSIsContainer) { throw "unexpected directory in assets: $($a.Name)" }
        if (($Required + $Optional) -cnotcontains $a.Name) { throw "asset not on the allow-list: $($a.Name)" }
    }
    foreach ($r in $Required) { if (-not ($items.Name -ccontains $r)) { throw "missing required asset $r" } }
    $items | Sort-Object Name
}

function Read-Lock {
    if (-not (Test-Path -LiteralPath $Lock -PathType Leaf)) { throw "model.lock not found: $Lock" }
    $info = @{ files = [ordered]@{} }
    foreach ($line in Get-Content -LiteralPath $Lock -Encoding ascii) {
        $f = @($line.Trim() -split '\s+')
        if ($f[0] -eq '' -or $f[0].StartsWith('#')) { continue }
        switch ($f[0]) {
            'name' { $info.name = $f[1] }
            'url'  { $info.url = $f[1] }
            'zip'  { $info.zip = $f[1]; $info.zipSize = [long]$f[2]; $info.zipSha = $f[3] }
            'file' { $info.files[$f[1]] = @{ size = [long]$f[2]; sha = $f[3] } }
            default { throw "model.lock: unknown line: $line" }
        }
    }
    if (-not $info.name -or -not $info.zip -or $info.files.Count -eq 0) { throw 'model.lock is incomplete' }
    $info
}

# Exact file set, sizes and hashes against model.lock. -Released: compare what packing would release (Pack); otherwise the
# files themselves must already be the released bytes (Check, Unpack).
function Test-AgainstLock($Info, $Items, [switch] $Released) {
    $names = @($Items | ForEach-Object Name)
    foreach ($n in $Info.files.Keys) { if ($names -cnotcontains $n) { throw "missing $n (listed in model.lock)" } }
    foreach ($a in $Items) {
        $want = $Info.files[$a.Name]
        if (-not $want) { throw "$($a.Name) is not listed in model.lock" }
        $got = if ($Released) { Get-ReleasedInfo $a } else { @{ size = [long]$a.Length; sha = Get-Sha256 $a.FullName } }
        if ($got.size -ne $want.size) { throw "$($a.Name): size $($got.size) != model.lock $($want.size)" }
        if ($got.sha -ne $want.sha) { throw "$($a.Name): SHA-256 $($got.sha) != model.lock $($want.sha)" }
    }
}

# Code points, so a character outside the BMP counts once.
function Get-CharCount([string] $s) { $n = 0; foreach ($c in $s.ToCharArray()) { if (-not [char]::IsLowSurrogate($c)) { $n++ } }; $n }

function Show-Screening([string] $Dir) {
    $punct = [char[]](0x3002, 0xFF0C, 0xFF01, 0xFF1F, 0xFF1B)   # sentence punctuation (ideographic full stop, fullwidth , ! ? ;)
    $spec = @{ 'data.txt' = @{ sep = ' '; strip = $false }; 'common-readings.txt' = @{ sep = $null; strip = $true } }
    foreach ($file in 'data.txt', 'common-readings.txt') {
        $path = Join-Path $Dir $file
        if (-not (Test-Path -LiteralPath $path)) { continue }
        $entries = 0; $long = 0; $withPunct = 0
        foreach ($raw in [IO.File]::ReadLines($path, [Text.Encoding]::UTF8)) {
            $line = $raw
            if ($spec[$file].strip) { $line = $line.Split('#')[0] }
            if ($line.StartsWith('#') -or $line.Trim() -eq '') { continue }
            # data.txt: "reading word log10"; common-readings.txt: "kind word std alt". The word is field 2 in both.
            $fields = if ($spec[$file].sep) { $line.Split(' ') } else { $line.Trim().Split([char[]]" `t", [StringSplitOptions]::RemoveEmptyEntries) }
            if ($fields.Count -lt 2) { continue }
            $entries++
            if ((Get-CharCount $fields[1]) -gt 16) { $long++ }
            if ($fields[1].IndexOfAny($punct) -ge 0) { $withPunct++ }
        }
        Write-Output ("screen {0}: entries {1}, longer than 16 characters {2}, with sentence punctuation {3}" -f $file, $entries, $long, $withPunct)
        if ($long -or $withPunct) { Write-Warning "$file has non-zero screening counts: the user decides whether this is acceptable." }
    }
    Write-Output 'screen model.bin, prior.pack, pn.fst, pn.lst: binary, not screened'
}

function New-AssetZip([string] $Path, $Items) {
    # Fixed entry order and timestamp; no directories.
    $stamp = [DateTimeOffset]::new(2026, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
    $fs = [IO.File]::Open($Path, [IO.FileMode]::CreateNew)
    try {
        $za = [IO.Compression.ZipArchive]::new($fs, [IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($a in $Items) {
                $e = $za.CreateEntry($a.Name, [IO.Compression.CompressionLevel]::Optimal)
                $e.LastWriteTime = $stamp
                $out = $e.Open()
                try {
                    if ($a.Extension -ieq '.cfg') { $b = Get-ReleasedCfg $a; $out.Write($b, 0, $b.Length) }
                    else { $in = [IO.File]::OpenRead($a.FullName); try { $in.CopyTo($out) } finally { $in.Dispose() } }
                } finally { $out.Dispose() }
            }
        } finally { $za.Dispose() }
    } finally { $fs.Dispose() }
}

switch ($PSCmdlet.ParameterSetName) {
    'Pack' {
        $items = Test-AssetSet $Source
        Show-Screening $Source
        if (-not $WriteLock) { Test-AgainstLock (Read-Lock) $items -Released; Write-Output 'assets match model.lock' }
        New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
        $zipName = "twime-model-$Name.zip"
        $zipPath = Join-Path $OutDir $zipName
        if (Test-Path -LiteralPath $zipPath) { throw "already exists: $zipPath" }
        New-AssetZip $zipPath $items
        $zipSize = (Get-Item -LiteralPath $zipPath).Length
        $zipSha = Get-Sha256 $zipPath
        if ($WriteLock) {
            $lines = @('# TwPotato model assets. Written by release/pack-model.ps1 -WriteLock.',
                '# Every row of the asset table must be confirmed by the user before the zip is uploaded.',
                "name $Name", "zip $zipName $zipSize $zipSha", "url $ZipUrlBase/model-$Name/$zipName")
            foreach ($a in $items) { $r = Get-ReleasedInfo $a; $lines += "file $($a.Name) $($r.size) $($r.sha)" }
            Set-Content -LiteralPath $Lock -Encoding ascii -Value $lines
            Write-Output "wrote $Lock"
        } else {
            $info = Read-Lock
            if ($info.zipSha -ne $zipSha) { Write-Warning "zip SHA-256 differs from model.lock (zip writer differs); the files themselves match." }
        }
        Write-Output "zip $zipPath $zipSize $zipSha"
    }
    'Unpack' {
        $info = Read-Lock
        $z = Get-Item -LiteralPath $Zip
        if ($z.Length -ne $info.zipSize) { throw "zip size $($z.Length) != model.lock $($info.zipSize)" }
        $sha = Get-Sha256 $z.FullName
        if ($sha -ne $info.zipSha) { throw "zip SHA-256 $sha != model.lock $($info.zipSha)" }
        if (Test-Path -LiteralPath $OutDir) { throw "OutDir must not exist: $OutDir" }
        $za = [IO.Compression.ZipFile]::OpenRead($z.FullName)
        try {
            foreach ($e in $za.Entries) {
                if ($e.FullName -cnotmatch '^[A-Za-z0-9._-]+$' -or -not $info.files.Contains($e.FullName)) { throw "zip entry not in model.lock: $($e.FullName)" }
            }
            New-Item -ItemType Directory -Path $OutDir | Out-Null
            foreach ($e in $za.Entries) { [IO.Compression.ZipFileExtensions]::ExtractToFile($e, (Join-Path $OutDir $e.FullName)) }
        } finally { $za.Dispose() }
        Test-AgainstLock $info (Test-AssetSet $OutDir)
        Write-Output "extracted to $OutDir; all files match model.lock"
    }
    'Check' {
        Test-AgainstLock (Read-Lock) (Test-AssetSet $CheckDir)
        Write-Output "assets in $CheckDir match model.lock"
    }
}
