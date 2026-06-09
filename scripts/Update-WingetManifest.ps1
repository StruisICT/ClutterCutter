<#
.SYNOPSIS
    Generate the winget manifest folder for a released version of ClutterCutter.

.DESCRIPTION
    Downloads the published ClutterCutter-rust.exe release asset, computes its
    SHA256, and writes the three-file winget manifest set under
    winget/manifests/s/StruisICT/ClutterCutter/<Version>/ using schema 1.12.0.

    This only stages the manifest *in this repo*. It does NOT submit anything to
    microsoft/winget-pkgs — copying the folder into a winget-pkgs fork and opening
    that PR stays a deliberate, manual step (see winget/README.md).

    The SHA256 is taken from the published asset, so signing must already have
    happened in CI (the asset is the signed exe) before you run this.

.EXAMPLE
    pwsh ./scripts/Update-WingetManifest.ps1 -Version 0.4.0
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
    [string]$ReleaseNotes = ''
)

$ErrorActionPreference = 'Stop'

$repo      = 'StruisICT/ClutterCutter'
$asset     = 'ClutterCutter-rust.exe'
$assetUrl  = "https://github.com/$repo/releases/download/$Tag/$asset"
$notesUrl  = "https://github.com/$repo/releases/tag/$Tag"

$repoRoot  = Split-Path -Parent $PSScriptRoot
$outDir    = Join-Path $repoRoot "winget/manifests/s/StruisICT/ClutterCutter/$Version"

Write-Host "Resolving release asset: $assetUrl"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "$asset"
Invoke-WebRequest -Uri $assetUrl -OutFile $tmp -UseBasicParsing
$sha = (Get-FileHash -Algorithm SHA256 -Path $tmp).Hash.ToUpperInvariant()
Write-Host "SHA256: $sha"

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
MinimumOSVersion: 10.0.0.0
InstallerType: portable
Commands:
- cluttercutter
ReleaseDate: $ReleaseDate
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
  and a safe-to-delete temp/cache files view. Single self-contained exe; no
  installer, no .NET runtime.
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
Write-Host "Next: review the ReleaseNotes/Description, then (when you choose to)"
Write-Host "copy this folder into a microsoft/winget-pkgs fork and open that PR."
