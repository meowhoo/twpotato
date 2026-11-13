<#
.SYNOPSIS
  Automated P3 checks of a built TwPotato MSI without installing it.

.EXAMPLE
  & .\release\check-msi.ps1 -Msi out\TwPotato-0.1.9-x64.msi -Version 0.1.9 -AssetsDir assets

  1. MSI tables (read-only): Property, CustomAction, InstallExecuteSequence (order and conditions of plan 2.3),
     Registry, Shortcut, AppSearch/RegLocator, LaunchCondition, Upgrade.
  2. Administrative install (msiexec /a, no elevation, nothing registered) into a new directory: the file set must be
     exactly the expected list, each file identical to its build input (build-msi.ps1 staging dir + AssetsDir).
  3. The Binary-table helper equals the staged twime-setup.exe (wix msi decompile -x).
  4. ICE validation (wix msi validate). ICE43/ICE57 are suppressed: they assume ProgramMenuFolder is per-user, but
     this package is per-machine (ALLUSERS=1), so the shortcut is in the all-users Start menu and the file key path is right.
  Exit code 0 only when every check passes.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Msi,
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [string] $AssetsDir,
    [ValidateSet(0, 1, 2, 3)] [int] $FailTest = 0,
    [string] $Wix = 'wix',
    [string] $WorkDir,
    # The asset table the installed assets are checked against (default release\model.lock); see build-msi.ps1 -ModelLock.
    [string] $ModelLock
)
$ErrorActionPreference = 'Stop'
$Msi = (Resolve-Path -LiteralPath $Msi).Path
$suffix = if ($FailTest -ne 0) { "-failtest$FailTest" } else { '' }
$stage = Join-Path (Split-Path -Parent $Msi) "stage-$Version$suffix"
if (-not (Test-Path -LiteralPath $stage)) { throw "build staging directory not found: $stage" }
if (-not $WorkDir) { $WorkDir = Join-Path (Split-Path -Parent $Msi) "check-$Version$suffix" }
if (Test-Path -LiteralPath $WorkDir) { Remove-Item -LiteralPath $WorkDir -Recurse -Force }
New-Item -ItemType Directory -Path $WorkDir | Out-Null

$script:failures = @()
function Check([bool] $ok, [string] $what) {
    if ($ok) { Write-Output "ok    $what" } else { Write-Output "FAIL  $what"; $script:failures += $what }
}

$Clsid = '{C2DD0C9B-F6E9-489D-9ADF-F97658502774}'
$ProfileGuid = '{DDDAD6E8-B6B8-4941-A61B-7BB8046D7AED}'
$Dll = "twime_tsf-$Version.dll"

# --- 1. tables -------------------------------------------------------------------------------------------------
$installer = New-Object -ComObject WindowsInstaller.Installer
$db = $installer.OpenDatabase($Msi, 0)   # read-only
function Query([string] $sql) {
    $view = $db.OpenView($sql); [void]$view.Execute()
    # Column count from the SELECT list (Record.FieldCount is not reliable through PowerShell late binding).
    $n = ([regex]::Match($sql, "SELECT (.+?) FROM").Groups[1].Value -split ",").Count; $rows = @()
    while ($null -ne ($rec = $view.Fetch())) {
        $rows += , @(for ($k = 1; $k -le $n; $k++) { $rec.StringData($k) })
    }
    [void]$view.Close(); , $rows
}
function Map([string] $sql) { $m = @{}; foreach ($r in (Query $sql)) { $m[$r[0]] = $r }; $m }

$prop = Map 'SELECT Property, Value FROM Property'
Check ($prop['ALLUSERS'][1] -eq '1') 'Property ALLUSERS=1 (per-machine)'
Check ($prop['MSIRESTARTMANAGERCONTROL'][1] -eq 'Disable') 'Property MSIRESTARTMANAGERCONTROL=Disable'
Check ($prop['UpgradeCode'][1] -eq '{6342DBCA-A4B4-4B68-99F5-67C0CF412F32}') 'Property UpgradeCode fixed'
Check ($prop['ProductVersion'][1] -eq $Version) "Property ProductVersion=$Version"
# Summary information subject / author: the SignPath MSI metadata restriction (release/signpath/APPLICATION.md).
$summary = $installer.SummaryInformation($Msi, 0)
$subject = (-join [char[]](0x5730, 0x74DC, 0x8F38, 0x5165, 0x6CD5)) + ' TwPotato'
Check ($summary.Property(3) -ceq $subject -and $summary.Property(4) -ceq 'meowhoo') "Summary subject '$subject', author 'meowhoo'"
$null = [Runtime.InteropServices.Marshal]::ReleaseComObject($summary)
Check ($prop['TIPFLAG'][1] -eq '0') 'Property TIPFLAG default 0'

# Binary-table EXE (2) + in-script (1024) + no impersonation (2048); rollback +256, commit +512, Return=ignore +64.
$deferred = 3074; $rollback = 3330; $commit = 3650
# Section 2.4a: the user-mode migration actions run as the installing user (no 2048).
$uDeferred = 1026; $uRollback = 1282; $uCommit = 1602
$tok = '--token "[MIGRATETOKEN]"'
$root = 'machine-release --root "[INSTALLDIR]."'
$rb = "machine-rollback-register --tip-present [TIPFLAG] --old-dll `"[OLDDLL]`" --dll `"[INSTALLDIR]$Dll`""
$register = 'machine-register --dll "[#TsfDll]"' + $(if ($FailTest -eq 1) { ' --test-fail-after-categories 1' } else { '' })
$expectCa = [ordered]@{
    RbRelease    = $rollback, $root
    Stop         = $deferred, 'machine-stop --root "[INSTALLDIR]."'
    RbUnregister = $rollback, $rb
    Unregister   = $deferred, 'machine-unregister'
    RbRegister   = $rollback, $rb
    Register     = $deferred, $register
    Release      = $commit, $root
    RbUserMigrate     = $uRollback, "user-migrate-restore $tok"
    UserMigrate       = $uDeferred, "user-migrate-prepare --expect-dll `"[USERMODE]`" $tok"
    UserMigrateFinish = $uCommit, "user-migrate-finish $tok"
    UserMigrateRetry  = $uCommit, 'user-migrate-finish'
}
if ($FailTest -in 2, 3) { $expectCa['FailTest'] = $deferred, 'fail-test-injected' }
$ca = Map 'SELECT Action, Type, Source, Target FROM CustomAction'
foreach ($k in $expectCa.Keys) {
    $r = $ca[$k]
    Check ($null -ne $r -and [int]$r[1] -eq $expectCa[$k][0] -and $r[2] -eq 'SetupHelper' -and $r[3] -ceq $expectCa[$k][1]) "CustomAction $k type $($expectCa[$k][0]) target '$($expectCa[$k][1])'"
}
$helperRows = @($ca.Values | Where-Object { $_[2] -eq 'SetupHelper' })
Check ($helperRows.Count -eq $expectCa.Count) "no other SetupHelper actions ($($helperRows.Count))"
Check ($null -ne $ca['SetTIPFLAG'] -and $ca['SetTIPFLAG'][2] -eq 'TIPFLAG' -and $ca['SetTIPFLAG'][3] -eq '1') 'SetProperty TIPFLAG=1'
Check ($null -ne $ca['SetMIGRATETOKEN'] -and [int]$ca['SetMIGRATETOKEN'][1] -eq 51 -and $ca['SetMIGRATETOKEN'][2] -eq 'MIGRATETOKEN' -and $ca['SetMIGRATETOKEN'][3] -ceq '[ProductCode];[Date];[Time]') 'SetProperty MIGRATETOKEN'
Check ($null -ne $ca['SetOLDDLL'] -and [int]$ca['SetOLDDLL'][1] -eq 51 -and $ca['SetOLDDLL'][2] -eq 'OLDDLL' -and $ca['SetOLDDLL'][3] -ceq '[USERMODE]') 'SetProperty OLDDLL=[USERMODE]'

$seq = Map 'SELECT Action, Condition, Sequence FROM InstallExecuteSequence'
$removeAll = 'REMOVE~="ALL" AND NOT UPGRADINGPRODUCTCODE'
$expectCond = [ordered]@{
    RbRelease = 'NOT WIX_UPGRADE_DETECTED'; Stop = ''; RbUnregister = $removeAll; Unregister = $removeAll
    RbRegister = 'NOT REMOVE~="ALL"'; Register = 'NOT REMOVE~="ALL"'; Release = ''
    RbUserMigrate = 'USERMODE AND NOT REMOVE~="ALL"'; UserMigrate = 'USERMODE AND NOT REMOVE~="ALL"'
    UserMigrateFinish = 'USERMODE AND NOT REMOVE~="ALL"'; UserMigrateRetry = 'MIGRATEPENDING="committed" AND NOT USERMODE AND NOT REMOVE~="ALL"'
}
if ($FailTest -eq 2) { $expectCond['FailTest'] = 'NOT REMOVE~="ALL"' }
if ($FailTest -eq 3) { $expectCond['FailTest'] = $removeAll }
foreach ($k in $expectCond.Keys) {
    Check ($null -ne $seq[$k] -and $seq[$k][1] -ceq $expectCond[$k]) "InstallExecuteSequence $k condition '$($expectCond[$k])'"
}
Check ($null -ne $seq['SetTIPFLAG'] -and $seq['SetTIPFLAG'][1] -ceq 'TIPPRESENT' -and [int]$seq['SetTIPFLAG'][2] -gt [int]$seq['AppSearch'][2] -and [int]$seq['SetTIPFLAG'][2] -lt [int]$seq['InstallInitialize'][2]) 'SetTIPFLAG after AppSearch, before InstallInitialize'
Check ([int]$seq['AppSearch'][2] -lt [int]$seq['LaunchConditions'][2]) 'AppSearch before LaunchConditions'
foreach ($sp in 'SetMIGRATETOKEN', 'SetOLDDLL') {
    Check ($null -ne $seq[$sp] -and [int]$seq[$sp][2] -gt [int]$seq['AppSearch'][2] -and [int]$seq[$sp][2] -lt [int]$seq['InstallInitialize'][2]) "$sp after AppSearch, before InstallInitialize"
}
Check ($seq['SetMIGRATETOKEN'][1] -ceq '' -and $seq['SetOLDDLL'][1] -ceq 'NOT OLDDLL AND USERMODE AND TIPPRESENT') 'SetMIGRATETOKEN always; SetOLDDLL only for an HKLM profile without HKLM COM'
$order = 'InstallInitialize', 'RemoveExistingProducts', 'RbRelease', 'Stop', 'RbUnregister', 'Unregister', 'RbUserMigrate', 'UserMigrate',
    'RemoveRegistryValues', 'RemoveFiles', 'InstallFiles', 'WriteRegistryValues', 'RbRegister', 'Register', 'UserMigrateFinish', 'UserMigrateRetry', 'Release', 'InstallFinalize'
if ($FailTest -eq 2) { $order = $order[0..13] + 'FailTest' + $order[14..($order.Count - 1)] }
if ($FailTest -eq 3) { $order = $order[0..7] + 'FailTest' + $order[8..($order.Count - 1)] }
$nums = foreach ($a in $order) { if ($seq[$a]) { [int]$seq[$a][2] } else { -1 } }
$sorted = $true; for ($k = 1; $k -lt $nums.Count; $k++) { if ($nums[$k - 1] -lt 0 -or $nums[$k] -le $nums[$k - 1]) { $sorted = $false } }
Check $sorted ('sequence order ' + ($order -join ' < ') + ' (' + ($nums -join ',') + ')')
Check ([int]$seq['RemoveExistingProducts'][2] -eq [int]$seq['InstallInitialize'][2] + 1) 'RemoveExistingProducts immediately after InstallInitialize'
# Nothing else of ours may sit between InstallInitialize and Unregister.
$between = @($seq.Values | Where-Object { [int]$_[2] -gt [int]$seq['InstallInitialize'][2] -and [int]$_[2] -lt [int]$seq['Unregister'][2] } | ForEach-Object { $_[0] })
Check (-not (Compare-Object $between @('RemoveExistingProducts', 'RbRelease', 'Stop', 'RbUnregister'))) "only REP, RbRelease, Stop, RbUnregister between InstallInitialize and Unregister ($($between -join ','))"

$reg = Query 'SELECT `Root`, `Key`, `Name`, `Value`, `Component_` FROM `Registry`'
$clsKey = "SOFTWARE\Classes\CLSID\$Clsid"
$want = @("2|$clsKey||TwPotato TSF text input processor", "2|$clsKey\InprocServer32||[#TsfDll]", "2|$clsKey\InprocServer32|ThreadingModel|Apartment")
$got = @($reg | ForEach-Object { "$($_[0])|$($_[1])|$($_[2])|$($_[3])" })
Check (-not (Compare-Object $want $got -CaseSensitive)) "Registry: exactly the HKLM COM rows ($($got.Count))"
$comp = Map 'SELECT Component, Directory_, Attributes, KeyPath FROM Component'
$dllComp = @($reg | ForEach-Object { $_[4] } | Sort-Object -Unique)
Check ($dllComp.Count -eq 1 -and $comp[$dllComp[0]][3] -eq 'TsfDll') 'COM rows belong to the TSF DLL component'
Check (@($comp.Values | Where-Object { ([int]$_[2] -band 256) -eq 0 }).Count -eq 0) 'every component is 64-bit'

$sc = Query 'SELECT Shortcut, Directory_, Name, Component_, Target FROM Shortcut'
Check ($sc.Count -eq 1 -and $sc[0][1] -eq 'ProgramMenuFolder' -and $sc[0][2] -like "*|$([char]0x5730)$([char]0x74DC)$([char]0x8F38)$([char]0x5165)$([char]0x6CD5) $([char]0x8A2D)$([char]0x5B9A)" -and $sc[0][4] -eq '[#SettingsExe]') 'Shortcut: one all-users Start menu entry to the settings app'

$loc = Map 'SELECT `Signature_`, `Root`, `Key`, `Name`, `Type` FROM `RegLocator`'
$app = Map 'SELECT Property, Signature_ FROM AppSearch'
function Locator([string] $p) { if ($app[$p]) { $loc[$app[$p][1]] } }
$t = Locator 'TIPPRESENT'
Check ($t -and $t[1] -eq '2' -and $t[2] -eq "SOFTWARE\Microsoft\CTF\TIP\$Clsid\LanguageProfile\0x00000404\$ProfileGuid" -and $t[3] -eq 'Description' -and [int]$t[4] -eq 18) 'AppSearch TIPPRESENT: HKLM 64-bit TIP profile'
$t = Locator 'OLDDLL'
Check ($t -and $t[1] -eq '2' -and $t[2] -eq "SOFTWARE\Classes\CLSID\$Clsid\InprocServer32" -and $t[3] -eq '' -and [int]$t[4] -eq 18) 'AppSearch OLDDLL: HKLM 64-bit COM path'
$t = Locator 'USERMODE'
Check ($t -and $t[1] -eq '1' -and $t[2] -eq "Software\Classes\CLSID\$Clsid\InprocServer32" -and [int]$t[4] -eq 18) 'AppSearch USERMODE: HKCU COM path'
$t = Locator 'MIGRATEPENDING'
Check ($t -and $t[1] -eq '1' -and $t[2] -eq 'Software\TwPotato\Migrate' -and $t[3] -eq 'state' -and [int]$t[4] -eq 18) 'AppSearch MIGRATEPENDING: HKCU migration record state'
$secure = ($prop['SecureCustomProperties'][1] -split ';')
Check (@('USERMODE', 'TIPPRESENT', 'OLDDLL', 'MIGRATEPENDING' | Where-Object { $secure -cnotcontains $_ }).Count -eq 0) "SecureCustomProperties include USERMODE, TIPPRESENT, OLDDLL, MIGRATEPENDING ($($prop['SecureCustomProperties'][1]))"

$lc = @((Query 'SELECT `Condition` FROM `LaunchCondition`') | ForEach-Object { $_[0] })
Check (-not (Compare-Object $lc @('NOT WIX_DOWNGRADE_DETECTED', 'NOT RollbackDisabled', '(Installed AND REMOVE~="ALL") OR NOT MIGRATEPENDING OR MIGRATEPENDING="committed"') -CaseSensitive)) 'LaunchCondition: no downgrade; rollback enabled; no unfinished user-mode migration (uninstall exempt)'
# Upgrade rows (MajorUpgrade): older versions of this UpgradeCode are removed completely (Remove empty = all features,
# max exclusive); a newer version is only detected (OnlyDetect = 2, min exclusive) and blocks via LaunchCondition.
$up = @((Query 'SELECT `UpgradeCode`, `VersionMin`, `VersionMax`, `Attributes`, `Remove`, `ActionProperty` FROM `Upgrade`') | ForEach-Object { $_ -join '|' })
$uc = '{6342DBCA-A4B4-4B68-99F5-67C0CF412F32}'
Check (-not (Compare-Object $up @("$uc||$Version|1||WIX_UPGRADE_DETECTED", "$uc|$Version||2||WIX_DOWNGRADE_DETECTED") -CaseSensitive)) "Upgrade: remove older versions completely, detect newer only ($($up -join '; '))"
$null = [Runtime.InteropServices.Marshal]::ReleaseComObject($db)

# --- 2. administrative install: exact file list and contents ---------------------------------------------------
$image = Join-Path $WorkDir 'image'
$p = Start-Process msiexec.exe -Wait -PassThru -ArgumentList @('/a', "`"$Msi`"", '/qn', "TARGETDIR=`"$image`"", '/l*v', "`"$(Join-Path $WorkDir 'admin.log')`"")
Check ($p.ExitCode -eq 0) "msiexec /a exit $($p.ExitCode)"
$inst = Join-Path $image 'PFiles64\TwPotato'
$expected = [ordered]@{
    $Dll                     = Join-Path $stage 'bin\twime_tsf.dll'
    'twime-engined.exe'      = Join-Path $stage 'bin\twime-engined.exe'
    'twime-settings.exe'     = Join-Path $stage 'bin\twime-settings.exe'
    'user-data.on'           = Join-Path $PSScriptRoot 'wix\user-data.on'
    'LICENSE.txt'            = Join-Path $stage 'docs\LICENSE.txt'
    'THIRD-PARTY-NOTICES.md' = Join-Path $stage 'docs\THIRD-PARTY-NOTICES.md'
    'LICENSE-McBopomofo.txt' = Join-Path $stage 'docs\LICENSE-McBopomofo.txt'
    'COPYING-libtabe.txt'    = Join-Path $stage 'docs\COPYING-libtabe.txt'
    'LICENSE-UNICODE-3.0.txt' = Join-Path $stage 'docs\LICENSE-UNICODE-3.0.txt'
}
foreach ($a in Get-ChildItem -LiteralPath $AssetsDir -File) { $expected["assets\$($a.Name)"] = $a.FullName }
$found = @(Get-ChildItem -LiteralPath $image -Recurse -File | Where-Object { $_.FullName -ne (Join-Path $image (Split-Path -Leaf $Msi)) } |
    ForEach-Object { $_.FullName.Substring($image.Length + 1) })
$wantPaths = @($expected.Keys | ForEach-Object { "PFiles64\TwPotato\$_" })
$diff = Compare-Object $wantPaths $found
Check (-not $diff) ("file list is exactly the expected $($wantPaths.Count) files" + $(if ($diff) { ': ' + (($diff | ForEach-Object { "$($_.SideIndicator)$($_.InputObject)" }) -join ', ') }))
foreach ($k in $expected.Keys) {
    $f = Join-Path $inst $k
    $same = (Test-Path -LiteralPath $f) -and ((Get-FileHash -LiteralPath $f).Hash -eq (Get-FileHash -LiteralPath $expected[$k]).Hash)
    Check $same "content $k"
}
$packArgs = @{ CheckDir = (Join-Path $inst 'assets') }
if ($ModelLock) { $packArgs.Lock = $ModelLock }
& (Join-Path $PSScriptRoot 'pack-model.ps1') @packArgs | Out-Null
Check $true 'installed assets match model.lock'

# --- 3. Binary-table helper -------------------------------------------------------------------------------------
$export = Join-Path $WorkDir 'decompiled'
& $Wix msi decompile $Msi -x $export -o (Join-Path $WorkDir 'decompiled.wxs') | Out-Null
Check ($LASTEXITCODE -eq 0) 'wix msi decompile'
$helper = Get-ChildItem -LiteralPath $export -Recurse -File | Where-Object { $_.Name -eq 'SetupHelper' -or $_.BaseName -eq 'SetupHelper' } | Select-Object -First 1
Check ($helper -and (Get-FileHash -LiteralPath $helper.FullName).Hash -eq (Get-FileHash -LiteralPath (Join-Path $stage 'bin\twime-setup.exe')).Hash) 'Binary SetupHelper equals the staged twime-setup.exe'

# --- 4. ICE validation ------------------------------------------------------------------------------------------
& $Wix msi validate $Msi -sice ICE43 -sice ICE57
Check ($LASTEXITCODE -eq 0) 'ICE validation (ICE43/ICE57 suppressed, see header)'

if ($script:failures.Count) { Write-Output "$($script:failures.Count) check(s) failed"; exit 1 }
Write-Output 'all checks passed'
