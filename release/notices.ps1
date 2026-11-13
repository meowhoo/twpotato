<#
.SYNOPSIS
  Generates THIRD-PARTY-NOTICES.md at the repository root, or checks it (-Check, used by CI).

.DESCRIPTION
  Content: release/notices/header.md, then the Rust crates linked into the shipped binaries (cargo-about 0.9.2,
  --locked, x86_64-pc-windows-msvc, build and dev dependencies excluded; one entry per distinct license text with the
  crates that use it; this project's own twime-* crates are left out, they are covered by LICENSE), then
  ime/settings-app/THIRD-PARTY-NOTICES.md (the settings app's accepted license exceptions and build-time tools).

.EXAMPLE
  & .\release\notices.ps1 -CargoAbout C:\tools\cargo-about.exe
  & .\release\notices.ps1 -CargoAbout C:\tools\cargo-about.exe -Check
#>
[CmdletBinding()]
param(
    [string] $CargoAbout = 'cargo-about',
    [switch] $Check
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$out = Join-Path $repo 'THIRD-PARTY-NOTICES.md'
$config = Join-Path $PSScriptRoot 'notices\about.toml'

$version = & $CargoAbout --version
if ($LASTEXITCODE -ne 0 -or $version -ne 'cargo-about 0.9.2') { throw "cargo-about 0.9.2 is required (found: '$version')" }

# The shipped binaries: TSF DLL, engine, setup helper (ime workspace) and the settings app (its own workspace).
$manifests = 'ime\crates\twime-tsf\Cargo.toml', 'ime\crates\twime-engined\Cargo.toml', 'ime\crates\twime-setup\Cargo.toml',
    'ime\settings-app\src-tauri\Cargo.toml'

$licenses = @{}   # license text -> @{ name; crates = sorted set }
foreach ($m in $manifests) {
    # cargo-about refuses to write to a PowerShell pipe (encoding); it writes UTF-8 to a temporary file instead.
    $tmp = [IO.Path]::GetTempFileName()
    try {
        & $CargoAbout generate --format json --locked -c $config -m (Join-Path $repo $m) -o $tmp
        if ($LASTEXITCODE -ne 0) { throw "cargo-about failed for $m" }
        $data = [IO.File]::ReadAllText($tmp, [Text.Encoding]::UTF8) | ConvertFrom-Json
    } finally { Remove-Item -LiteralPath $tmp -ErrorAction SilentlyContinue }
    foreach ($l in $data.licenses) {
        $text = ($l.text -replace "`r`n", "`n").Trim()
        if (-not $licenses.ContainsKey($text)) { $licenses[$text] = @{ name = $l.name; crates = New-Object 'System.Collections.Generic.SortedSet[string]' ([StringComparer]::Ordinal) } }
        foreach ($u in $l.used_by) {
            if ($u.crate.name -notlike 'twime-*') { [void]$licenses[$text].crates.Add("$($u.crate.name) $($u.crate.version)") }
        }
    }
}

$sb = New-Object Text.StringBuilder
[void]$sb.Append(((Get-Content -LiteralPath (Join-Path $PSScriptRoot 'notices\header.md') -Raw -Encoding UTF8) -replace "`r`n", "`n").TrimEnd())
[void]$sb.Append("`n`n## Rust crates in the shipped binaries`n")
$entries = $licenses.GetEnumerator() | Where-Object { $_.Value.crates.Count -gt 0 } |
    Sort-Object @{ e = { $_.Value.name } }, @{ e = { @($_.Value.crates)[0] } }
foreach ($e in $entries) {
    [void]$sb.Append("`n### $($e.Value.name)`n`nUsed by: ")
    [void]$sb.Append((@($e.Value.crates) | ForEach-Object { "``$_``" }) -join ', ')
    [void]$sb.Append("`n`n````text`n$($e.Key)`n`````n")
}
$settings = ((Get-Content -LiteralPath (Join-Path $repo 'ime\settings-app\THIRD-PARTY-NOTICES.md') -Raw -Encoding UTF8) -replace "`r`n", "`n").Trim()
# Demote the settings app's headings by one level so they nest under this file's section.
$settings = ($settings -split "`n" | ForEach-Object { if ($_ -match '^#') { "#$_" } else { $_ } }) -join "`n"
[void]$sb.Append("`n## Settings app: accepted license exceptions and build tools`n`n$settings`n")
$text = $sb.ToString()

if ($Check) {
    $current = if (Test-Path -LiteralPath $out) { [IO.File]::ReadAllText($out) -replace "`r`n", "`n" } else { '' }
    if ($current -ne $text) { throw 'THIRD-PARTY-NOTICES.md is out of date: run release/notices.ps1 and commit the result.' }
    Write-Output 'THIRD-PARTY-NOTICES.md is up to date'
} else {
    [IO.File]::WriteAllText($out, $text, (New-Object Text.UTF8Encoding $false))
    Write-Output "wrote $out ($($text.Length) characters, $(@($entries).Count) license texts)"
}
