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
5. Validate locally: `winget validate --manifest winget/manifests/s/StruisICT/ClutterCutter/<version>`
6. Submit a new PR per the steps above.

## Notes

- `InstallerType: portable` means winget downloads the exe, drops it under
  `%LOCALAPPDATA%\Microsoft\WinGet\Packages\...`, and adds it to `PATH` under
  the alias `cluttercutter`.
- The packaged binary is `ClutterCutter-rust.exe` (the self-contained Rust
  build), renamed by winget to match the `Commands` alias.
- `License: Proprietary` is a placeholder. Add a real `LICENSE` file at the
  repo root and switch this to a SPDX identifier (e.g. `MIT`) before the first
  upstream PR — winget reviewers will likely ask.
