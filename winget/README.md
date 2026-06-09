# winget manifests

Manifests for submitting ClutterCutter to the [winget Community Repository](https://github.com/microsoft/winget-pkgs).

## Layout

```
winget/manifests/s/StruisICT/ClutterCutter/0.3.0/
  StruisICT.ClutterCutter.yaml             (version manifest)
  StruisICT.ClutterCutter.installer.yaml   (installer + SHA256)
  StruisICT.ClutterCutter.locale.en-US.yaml (publisher/description)
```

The directory layout mirrors `microsoft/winget-pkgs`, so the per-version folder
can be copy-pasted directly into a fork.

Each file starts with a `# yaml-language-server: $schema=...` header and uses
`ManifestVersion: 1.12.0` (the schema the community repo currently requires; the
old `1.6.0` is deprecated). Keep all three files on the same schema version.

## Repository rules (must conform — these are what the bot/reviewers enforce)

From the winget-pkgs [Authoring](https://github.com/microsoft/winget-pkgs/blob/master/doc/Authoring.md),
[Policies](https://github.com/microsoft/winget-pkgs/blob/master/doc/Policies.md), and
[first-contribution checklist](https://github.com/microsoft/winget-pkgs/blob/master/doc/FirstContribution.md):

- **One PR = one package version**, manifest files only. No README/doc/tooling
  changes and no second version in the same PR.
- **Multi-file manifest set required** (version + defaultLocale + installer).
  Singleton manifests are banned in the community repo.
- **Schema headers + latest schema** (`1.12.0`) on every file.
- **Stable, version-specific InstallerUrl** from the official source (our
  GitHub Release asset URL with the `vX.Y.Z` tag — never a "latest" URL).
- **Installs unattended** ("silent with progress"). A portable exe satisfies this.
- **No scripts as installers** (`.bat`/`.ps1` banned). We ship an `exe` — fine.
- **Security scans / PUA policy:** every submission is scanned (incl. Microsoft
  Defender) in a sandbox install. A flagged binary is rejected regardless of
  intent. Our exe is **unsigned**, so this is the most likely failure point —
  see the caveat under Notes.
- **CLA:** first PR requires signing the Microsoft Contributor License Agreement
  (a bot links it on the PR).

Before submitting, test locally (needs an elevated shell):

```powershell
winget settings --enable LocalManifestFiles
winget validate --manifest winget/manifests/s/StruisICT/ClutterCutter/<version>
winget install  --manifest winget/manifests/s/StruisICT/ClutterCutter/<version>
```

Or test in Windows Sandbox with the repo's `Tools\SandboxTest.ps1 <path>`
(also runs validation). Tooling like [wingetcreate](https://github.com/microsoft/winget-create)
or [komac](https://github.com/russellbanks/Komac) can generate/update + submit
the manifest for you (`wingetcreate update StruisICT.ClutterCutter ...`).

## Submitting a new version

1. Fork [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) once.
2. Copy `winget/manifests/s/StruisICT/ClutterCutter/<version>/` into the same
   path in your fork.
3. Open a PR against `microsoft/winget-pkgs:master`. The repo's bots run
   `winget validate` and a sandbox install — usually green within ~15 min if
   the manifests validate locally.
4. After merge, the package is reachable via `winget install StruisICT.ClutterCutter`.

## Updating for a new release

For each new tagged release:

1. Bump `PackageVersion` in all three YAMLs (keep them in sync).
2. Update `InstallerUrl` to point at the new release asset.
3. Recompute `InstallerSha256` (uppercase hex, no separators):
   ```bash
   curl -L <url> | sha256sum | awk '{print toupper($1)}'
   ```
4. Update `ReleaseDate`, `ReleaseNotes`, and `ReleaseNotesUrl` in the locale
   manifest.
5. Keep the `# yaml-language-server` schema header and `ManifestVersion` on the
   current schema (`1.12.0`).
6. Validate locally and test the install (see commands above).
7. Submit a new PR per the steps above (one version, manifest-only).

> Because we cut releases with [release-please](../README.md#releasing) under
> SemVer, the winget `PackageVersion` is always a sortable `MAJOR.MINOR.PATCH`,
> which keeps `winget upgrade` ordering correct. Submit the winget update only
> **after** the GitHub Release (and its `ClutterCutter-rust.exe` asset) exists,
> since the SHA256 is computed from the published asset.

## Notes

- `InstallerType: portable` means winget downloads the exe, drops it under
  `%LOCALAPPDATA%\Microsoft\WinGet\Packages\...`, and adds it to `PATH` under
  the alias `cluttercutter`.
- The packaged binary is `ClutterCutter-rust.exe` (the self-contained Rust
  build), renamed by winget to match the `Commands` alias.
- Licensed MIT (see `LICENSE` at repo root). The locale manifest declares
  `License: MIT` and `LicenseUrl` pointing at that file on `main`.
- **Unsigned-binary caveat (main risk):** `ClutterCutter-rust.exe` is not
  code-signed, and it requests admin elevation + reads the raw NTFS volume
  (`\\.\C:`) for the MFT fast path — exactly the kind of behavior heuristic
  scanners flag. If the validation sandbox's Defender scan flags it, the PR is
  rejected. Mitigations, in order of effort: (1) ensure the released exe is
  clean and, if flagged, submit it to Microsoft for analysis / dispute the
  detection; (2) consider Authenticode code-signing the release binaries to
  build reputation. Signing is **not required** by winget but materially
  reduces SmartScreen/Defender friction.
