<#
.SYNOPSIS
    Generate the winget manifest folder for a released version of ClutterCutter.

.DESCRIPTION
    Downloads the published ClutterCutter.msi release asset (the WiX per-machine
    installer that winget packages since 0.13.2), computes its SHA256, reads the
    ProductCode / UpgradeCode straight out of the MSI, and writes the three-file
    winget manifest set under winget/manifests/s/StruisICT/ClutterCutter/<Version>/
    using schema 1.12.0.

    The installer manifest declares `ElevationRequirement: elevationRequired`.
    Without it, `winget install` from a NON-elevated terminal dies with
    "0x8007029c : An assertion failure has occurred" (issue #93): the per-machine
    MSI is elevated out-of-band by Windows Installer, ShellExecuteEx returns no
    process handle, and winget asserts (microsoft/winget-cli#3771). With the flag
    winget launches msiexec via `runas` and the user just gets a normal UAC prompt.

    This only stages the manifest *in this repo*. It does NOT submit anything to
    microsoft/winget-pkgs — copying the folder into a winget-pkgs fork and opening
    that PR stays a deliberate, manual step (see winget/README.md).

.EXAMPLE
    pwsh ./scripts/Update-WingetManifest.ps1 -Version 0.14.0 -ReleaseDate 2026-09-11
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,

    # Defaults to v<Version> — the tag release-please creates.
    [string]$Tag = "v$Version",

    # YYYY-MM-DD; defaults to today (UTC).
    [string]$ReleaseDate = ([DateTime]::UtcNow.ToString('yyyy-MM-dd')),

    # Optional release notes body. Review/refine in the PR before submitting.
    [string]$ReleaseNotes = '',

    # Normally read from the MSI's Property table (needs the Windows Installer
    # COM object, i.e. a Windows host). Pass explicitly on non-Windows.
    [string]$ProductCode = ''
)

$ErrorActionPreference = 'Stop'

$repo        = 'StruisICT/ClutterCutter'
$asset       = 'ClutterCutter.msi'
$assetUrl    = "https://github.com/$repo/releases/download/$Tag/$asset"
$notesUrl    = "https://github.com/$repo/releases/tag/$Tag"
$upgradeCode = '{FADE883B-0102-4A40-A367-3E96BD9692F7}'   # fixed in msi/ClutterCutter.wxs

$repoRoot  = Split-Path -Parent $PSScriptRoot
$outDir    = Join-Path $repoRoot "winget/manifests/s/StruisICT/ClutterCutter/$Version"

Write-Host "Resolving release asset: $assetUrl"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "ClutterCutter-$Version.msi"
Invoke-WebRequest -Uri $assetUrl -OutFile $tmp -UseBasicParsing
$sha = (Get-FileHash -Algorithm SHA256 -Path $tmp).Hash.ToUpperInvariant()
Write-Host "SHA256: $sha"

# Read a value from the MSI Property table via the Windows Installer COM API.
function Get-MsiProperty([string]$Path, [string]$Name) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $db   = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($Path, 0))
    $view = $db.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $db, @("SELECT Value FROM Property WHERE Property = '$Name'"))
    $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
    $rec  = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
    if ($null -eq $rec) { throw "Property '$Name' not found in $Path" }
    $rec.GetType().InvokeMember('StringData', 'GetProperty', $null, $rec, @(1))
}

if (-not $ProductCode) {
    try {
        $ProductCode = Get-MsiProperty $tmp 'ProductCode'
        $msiVersion  = Get-MsiProperty $tmp 'ProductVersion'
        $msiUpgrade  = Get-MsiProperty $tmp 'UpgradeCode'
        if ($msiUpgrade -ne $upgradeCode) { throw "UpgradeCode in MSI ($msiUpgrade) differs from the expected $upgradeCode" }
        if (-not $msiVersion.StartsWith($Version)) { throw "MSI ProductVersion $msiVersion does not match requested version $Version" }
    } catch {
        throw "Could not read the ProductCode from the MSI ($($_.Exception.Message)). Pass -ProductCode '{...}' explicitly (see Orca, or msiexec /l*v)."
    }
}
Write-Host "ProductCode: $ProductCode"

if (-not $ReleaseNotes) {
    $ReleaseNotes = "See the full release notes at $notesUrl"
}
# Indent each release-notes line by two spaces for the YAML block scalar.
$notesBlock = ($ReleaseNotes -split "`r?`n" | ForEach-Object { "  $_" }) -join "`n"

New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$installer = @"
# Created with: scripts/Update-WingetManifest.ps1
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.1.12.0.schema.json

PackageIdentifier: StruisICT.ClutterCutter
PackageVersion: $Version
InstallerLocale: en-US
MinimumOSVersion: 10.0.0.0
InstallerType: wix
Scope: machine
ElevationRequirement: elevationRequired
InstallModes:
- interactive
- silent
- silentWithProgress
InstallerSwitches:
  Silent: /quiet LAUNCHAFTERINSTALL=1
  SilentWithProgress: /passive LAUNCHAFTERINSTALL=1
UpgradeBehavior: install
Commands:
- cluttercutter
ReleaseDate: $ReleaseDate
ProductCode: '$ProductCode'
AppsAndFeaturesEntries:
- ProductCode: '$ProductCode'
  UpgradeCode: '$upgradeCode'
InstallationMetadata:
  DefaultInstallLocation: '%ProgramFiles%\ClutterCutter'
Installers:
- Architecture: x64
  InstallerUrl: $assetUrl
  InstallerSha256: $sha
ManifestType: installer
ManifestVersion: 1.12.0
"@

$locale = @"
# Created with: scripts/Update-WingetManifest.ps1
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.1.12.0.schema.json

PackageIdentifier: StruisICT.ClutterCutter
PackageVersion: $Version
PackageLocale: en-US
Publisher: Struis ICT
PublisherUrl: https://struisict.com
PublisherSupportUrl: https://github.com/StruisICT/ClutterCutter/issues
PackageName: ClutterCutter
PackageUrl: https://github.com/StruisICT/ClutterCutter
License: MIT
LicenseUrl: https://github.com/StruisICT/ClutterCutter/blob/main/LICENSE
Copyright: Copyright (c) 2026 Struis ICT
ShortDescription: Fast disk-usage browser with NTFS MFT scanning.
Description: |-
  ClutterCutter is a lightweight Windows disk-usage browser. On NTFS drives it
  reads the Master File Table directly for very fast full-drive scans (roughly
  one million files in six seconds), with a parallel FindFirstFileEx walker as
  a fallback for non-NTFS drives and non-admin runs. Includes a treeview
  drill-in, a Top-largest-files view, an Oldest-files (by date modified) view,
  and a safe-to-delete temp/cache files view. Installs per-machine to Program Files with a Start
  Menu entry; no .NET runtime required.
Moniker: cluttercutter
Tags:
- disk
- disk-usage
- ntfs
- mft
- treesize
- utility
- windows
ReleaseNotes: |-
$notesBlock
ReleaseNotesUrl: $notesUrl
ManifestType: defaultLocale
ManifestVersion: 1.12.0
"@

$version = @"
# Created with: scripts/Update-WingetManifest.ps1
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.1.12.0.schema.json

PackageIdentifier: StruisICT.ClutterCutter
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.12.0
"@

# winget tooling expects UTF-8 (no BOM) with LF line endings.
$enc = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText((Join-Path $outDir 'StruisICT.ClutterCutter.installer.yaml'),    ($installer -replace "`r`n","`n") + "`n", $enc)
[System.IO.File]::WriteAllText((Join-Path $outDir 'StruisICT.ClutterCutter.locale.en-US.yaml'), ($locale    -replace "`r`n","`n") + "`n", $enc)
[System.IO.File]::WriteAllText((Join-Path $outDir 'StruisICT.ClutterCutter.yaml'),              ($version    -replace "`r`n","`n") + "`n", $enc)

Write-Host ""
Write-Host "Wrote manifest set to: $outDir"
Get-ChildItem $outDir | ForEach-Object { Write-Host "  $($_.Name)" }
Write-Host ""
Write-Host "Next: review the ReleaseNotes/Description, run 'winget validate --manifest <folder>',"
Write-Host "then (when you choose to) copy this folder into a microsoft/winget-pkgs fork and open that PR."
