<#
.SYNOPSIS
  Exports the files of release/public-files.txt into a NEW directory for the public repository.
  Nothing outside the list is copied; git init / commit / push are not done here.

.DESCRIPTION
  1. Every list entry must be a relative path to an existing regular file, with no reparse point on the way, no
     duplicate, and no component that is .git, *.pre-*, target*, stage-*, node_modules, dist or gen. Never-public
     trees (data, train, collect, plans, upstream, engine, bench, repro, scripts, ime-art and install at the root, AI*.md,
     ime/scripts/, release/out/, release/sandbox/) are refused even if listed.
  2. The destination must not exist (default: twpotato-public next to the repository directory); it is created and the files are copied.
  3. The exported tree is scanned; any hit fails the export (the destination is then deleted):
     - data-type extensions and model/asset names (.tsv .csv .jsonl .npz .pack .pt .bin .twg1 .twg2 .fst .lst .zip ...,
       any file named model.bin), except the audited test fixtures listed in $Audited below;
     - credential patterns (private keys, GitHub / AWS / Slack / OpenAI-style tokens) and the maintainer's e-mail address (git config user.email);
     - references to the private plan documents (plans/ followed by an upper-case name or a .md file);
     - user-profile paths (drive letter, then Users, then a name; except the Windows Sandbox account WDAGUtilityAccount);
     - any file larger than 1 MB.
  4. Prints the file list with sizes, the per-directory counts and the total size.

.EXAMPLE
  & .\release\export-public.ps1 -ScrubPlanRefs
  & .\release\export-public.ps1 -Destination ..\twpotato-public-test
  & .\release\export-public.ps1 -SourceRoot ..\twpotato-build\snapshot -List ..\twpotato-build\snapshot\release\public-files.txt
#>
[CmdletBinding()]
param(
    [string] $Destination = (Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'twpotato-public'),
    [string] $List = (Join-Path $PSScriptRoot 'public-files.txt'),
    # Tree the listed files are copied from (default: this repository), e.g. a frozen build snapshot with the same layout.
    [string] $SourceRoot,
    # Rewrite references to the private plan documents inside the exported copy only (plans/<NAME>.md becomes <NAME>); the
    # source tree is not touched. The scan below still runs on the result.
    [switch] $ScrubPlanRefs
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$src = if ($SourceRoot) { (Resolve-Path -LiteralPath $SourceRoot).Path } else { $repo }
# The source root and every ancestor must be real directories (a junction could redirect the export to another tree).
for ($d = Get-Item -LiteralPath $src -Force; $d; $d = $d.Parent) {
    if (-not ($d -is [IO.DirectoryInfo]) -or ($d.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "source root or an ancestor is not a plain directory: $($d.FullName)" }
}

# Audited exceptions to the data-type scan: synthetic test fixtures only, each bound to the SHA-256 of the audited content.
$Audited = @{
    'ime/crates/twime-model/tests/fixtures/g1-tiny-model.bin'    = 'f53ef596bf517d5cc5e44a0db16e95a7bac8048181482345ef672258d8353bbd' # random weights, vocab 12 (train/n2_train.py fixture_g1, seed 11)
    'ime/crates/twime-model/tests/fixtures/g1f-tiny-model.bin'   = '03a5561434b6802923b0693fd48b638688ed537e6a2136ed3ecb9799119efb48' # random weights (same generator)
    'ime/crates/twime-model/tests/fixtures/g1f1-tiny-model.bin'  = 'c21e83db6320862b2e7278bc40071e26b9f1266583f18dffdaad328e7c27fbe8'
    'ime/crates/twime-model/tests/fixtures/g1fc-tiny-model.bin'  = '1ddfa05430f5ac73cb41d3a045873fbe5ffa7e6746fb1a7948daf15ed686cb97'
    'ime/crates/twime-model/tests/fixtures/n1-tiny-model.bin'    = '03166e888f81b05bf380e457f4b46502a3e14d3253cba73c2ed631fbfd562906' # random weights, vocab 12 (fixture, seed 7)
    'ime/crates/twime-core/tests/fixtures/lexux-session-pre.bin' = 'ff224b01dbba491cdb45fd22fa4050f50c913b1ce3a07d8cc941989a469f4174' # wire frames of scripted test sessions, re-made and compared byte for byte by session/lexux_frozen.rs
    'ime/crates/twime-tools/data/homophone-cases.tsv'            = 'f134bfff0289473a60a0b0f6a669a40bd4c7d745e9bd6c084391aabc55605a3f' # 18 hand-written example sentences, no corpus text
    'ime/xcmp/cases-userlex.tsv'                                 = '23490465f7d319dc8026d7ebb7b6fbd44bf58bfdc5d6d7b63c7bd27ca615432d' # 12 hand-written cases around made-up user words
    'ime/xcmp/userlex/userlex.tsv'                               = '3978ba0baa70eec2190a4e3212841a651dde7e983066051c753494334c5ee37a' # 6 made-up user words
}
$DataExt = '.tsv', '.csv', '.jsonl', '.npz', '.npy', '.pack', '.pt', '.pth', '.safetensors', '.bin', '.twg1', '.twg2',
    '.fst', '.lst', '.zip', '.gz', '.7z', '.tar', '.parquet', '.db', '.sqlite', '.pkl', '.msi', '.cab', '.pfx', '.p12', '.snk', '.pem', '.key'
$Secret = @(
    '-----BEGIN [A-Z ]*PRIVATE KEY-----', 'ghp_[A-Za-z0-9]{36}', 'github_pat_[A-Za-z0-9_]{20,}', 'gho_[A-Za-z0-9]{36}',
    'AKIA[0-9A-Z]{16}', 'xox[abprs]-[A-Za-z0-9-]{10,}', 'sk-[A-Za-z0-9_-]{20,}'
)
# The maintainer's own e-mail address (read from git, not written here) must not appear anywhere.
$personal = git -C $repo config user.email
if ($personal) { $Secret += [regex]::Escape($personal.Trim()) }
# References to the private plan documents (plans/<NAME> or plans/<file>.md); ignore patterns such as plans/reviews/*.log are not references.
$PlanRef = '(?<![\w-])plans[\\/](?:[A-Z]|(?:[\w.-]+[\\/])*[\w.-]*\.md)'
$UserPath = '(?i)[A-Z]:[\\/]+Users[\\/]+(?!WDAGUtilityAccount)[^\\/\s"''`]+'

# Never public, whatever the list says; -in and -like are case-insensitive.
function Test-NeverPublic([string] $rel) {
    $top = $rel.Split('/')[0]
    ($top -in 'data', 'train', 'collect', 'plans', 'upstream', 'engine', 'bench', 'repro', 'scripts', 'ime-art', 'install') -or
        ($top -like 'AI*.md') -or ($rel -like 'ime/scripts/*') -or ($rel -like 'release/out/*') -or ($rel -like 'release/sandbox/*')
}

# --- 1. the list ----------------------------------------------------------------------------------------------
$entries = @(Get-Content -LiteralPath $List -Encoding UTF8 | ForEach-Object { $_.Trim() } | Where-Object { $_ -and -not $_.StartsWith('#') })
if ($entries.Count -eq 0) { throw "empty list: $List" }
$seen = @{}
foreach ($e in $entries) {
    if ($e -match '\\' -or $e.StartsWith('/') -or $e -match '^[A-Za-z]:' -or $e -match '(^|/)\.\.?(/|$)') { throw "not a clean relative path: $e" }
    if ($seen.ContainsKey($e.ToLowerInvariant())) { throw "duplicate entry: $e" }
    if (Test-NeverPublic $e) { throw "never-public path: $e" }
    $seen[$e.ToLowerInvariant()] = $true
    foreach ($c in $e.Split('/')) {
        if ($c -eq '.git' -or $c -like '*.pre-*' -or $c -like 'target*' -or $c -like 'stage-*' -or $c -in 'node_modules', 'dist', 'gen') {
            throw "forbidden path component '$c' in $e"
        }
    }
    # The file and every directory on its way must be real (no junction or symbolic link).
    $p = $src
    foreach ($c in $e.Split('/')) {
        $p = Join-Path $p $c
        $item = Get-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue
        if (-not $item) { throw "missing: $e" }
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "reparse point: $e" }
        # Exact spelling: Windows would otherwise accept "plans." or "plans " or a short 8.3 name for "plans".
        if ($item.Name -cne $c) { throw "path spelling '$c' differs from the file system ('$($item.Name)'): $e" }
    }
    if ($item.PSIsContainer) { throw "not a file: $e" }
}

# --- 2. copy --------------------------------------------------------------------------------------------------
if (Test-Path -LiteralPath $Destination) { throw "destination already exists: $Destination" }
New-Item -ItemType Directory -Path $Destination | Out-Null
$Destination = (Resolve-Path -LiteralPath $Destination).Path
try {
    foreach ($e in $entries) {
        $dst = Join-Path $Destination ($e -replace '/', '\')
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
        Copy-Item -LiteralPath (Join-Path $src ($e -replace '/', '\')) -Destination $dst
    }

    if ($ScrubPlanRefs) {
        # The full reference token of every match of $PlanRef: a .md path, or an upper-case name with optional sub-paths.
        $RefToken = '(?<![\w-])plans[\\/](?:(?:[\w.-]+[\\/])*[\w.-]*\.md|[A-Z][\w.-]*(?:[\\/][\w.-]+)*)'
        $nFiles = 0; $nRefs = 0
        foreach ($f in @(Get-ChildItem -LiteralPath $Destination -Recurse -Force -File)) {
            $bytes = [IO.File]::ReadAllBytes($f.FullName)
            if ($bytes -contains 0) { continue }   # binary
            $bom = $bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF
            $text = (New-Object Text.UTF8Encoding($false, $true)).GetString($bytes, $(if ($bom) { 3 } else { 0 }), $bytes.Length - $(if ($bom) { 3 } else { 0 }))
            $n = [regex]::Matches($text, $RefToken).Count
            if ($n -eq 0) { continue }
            $new = [regex]::Replace($text, $RefToken, { param($m) ($m.Value -split '[\\/]')[-1] -replace '\.md$', '' })
            $out = (New-Object Text.UTF8Encoding($bom)).GetPreamble() + (New-Object Text.UTF8Encoding($false)).GetBytes($new)
            [IO.File]::WriteAllBytes($f.FullName, [byte[]]$out)
            $nFiles++; $nRefs += $n
        }
        "plan references rewritten in the exported copy: $nRefs in $nFiles files"
    }

    # --- 3. scan the exported tree (not the list: whatever is in the destination is checked) --------------------
    $problems = @()
    $files = @(Get-ChildItem -LiteralPath $Destination -Recurse -Force -File)
    if ($files.Count -ne $entries.Count) { $problems += "file count $($files.Count) != list $($entries.Count)" }
    foreach ($f in $files) {
        $rel = $f.FullName.Substring($Destination.Length + 1) -replace '\\', '/'
        if (Test-NeverPublic $rel) { $problems += "never-public path: $rel" }
        if ($f.Length -gt 1MB) { $problems += "larger than 1 MB: $rel ($($f.Length))" }
        $isData = ($DataExt -contains $f.Extension.ToLowerInvariant()) -or ($f.Name -ieq 'model.bin')
        if ($isData) {
            if (-not $Audited.ContainsKey($rel) -or ($Audited.Keys | Where-Object { $_ -ceq $rel }).Count -ne 1) { $problems += "data-type file: $rel" }
            elseif ((Get-FileHash -LiteralPath $f.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Audited[$rel]) { $problems += "audited fixture changed since its audit: $rel" }
        }
        # Every file is scanned as text (encoding by BOM, default UTF-8), as raw bytes (Latin-1, so ASCII strings inside
        # binary files match) and as UTF-16LE at both byte alignments (Windows resources and PE strings without a BOM).
        $bytes = [IO.File]::ReadAllBytes($f.FullName)
        $views = @([IO.File]::ReadAllText($f.FullName), [Text.Encoding]::GetEncoding(28591).GetString($bytes))
        foreach ($k in 0, 1) { if ($bytes.Length -gt $k) { $views += [Text.Encoding]::Unicode.GetString($bytes, $k, $bytes.Length - $k) } }
        foreach ($text in $views) {
            foreach ($s in $Secret) { if ($text -match $s) { $problems += "credential-like pattern '$s': $rel" } }
            if ($text -match $UserPath) { $problems += "user profile path '$($Matches[0])': $rel" }
            if ($text -cmatch $PlanRef) { $problems += "reference to a private plan file '$($Matches[0])': $rel" }
        }
        $problems = @($problems | Select-Object -Unique)
    }
    if ($problems) { throw ("export rejected:`n  " + ($problems -join "`n  ")) }
} catch {
    Remove-Item -LiteralPath $Destination -Recurse -Force -ErrorAction SilentlyContinue
    throw
}

# --- 4. report ------------------------------------------------------------------------------------------------
$total = 0
foreach ($f in ($files | Sort-Object FullName)) {
    $total += $f.Length
    '{0,10}  {1}' -f $f.Length, ($f.FullName.Substring($Destination.Length + 1) -replace '\\', '/')
}
''
'per directory:'
$files | Group-Object { Split-Path -Parent ($_.FullName.Substring($Destination.Length + 1)) } | Sort-Object Name |
    ForEach-Object { '{0,5}  {1}' -f $_.Count, $(if ($_.Name) { $_.Name -replace '\\', '/' } else { '(root)' }) }
''
"exported $($files.Count) files, $total bytes, to $Destination"
