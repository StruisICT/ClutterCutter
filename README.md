# ClutterCutter

A lightweight Windows disk-usage browser built and maintained by **Struis ICT**.

Single self-contained native `.exe`, no installer. The C# build needs only the in-box .NET Framework 4 runtime; the Rust build (packaged for winget) has no runtime dependency at all.

[![Buy Me a Coffee](https://img.shields.io/badge/Buy%20Me%20a%20Coffee-FFDD00?style=flat&logo=buy-me-a-coffee&logoColor=black)](https://buymeacoffee.com/struis112)

If ClutterCutter saves you time hunting down what's eating your disk, consider [buying me a coffee ☕](https://buymeacoffee.com/struis112) — it keeps this and other Struis ICT tools coming.

## Features

- **Scan one drive, or all at once** — click a drive to scan it, or **Scan all drives** to scan every volume in parallel; results appear per-drive as each finishes, in alphabetical order. It also auto-scans all drives on startup, so it opens straight into a populated tree.
- **MFT fast path** — on an NTFS drive as Administrator, ClutterCutter reads the Master File Table directly via `\\.\C:` and parses the records **in parallel across CPU cores**. Far faster than walking the filesystem — a full C: drive (~2 M files) in ~5 seconds.
- **FindFirstFileEx fallback** with parallel top-level fan-out and `LARGE_FETCH` for non-admin / subfolder scans.
- **Folder tree + list** — browse the tree, or double-click through folders in the list level after level. Each folder lists its subfolders then its files, sorted alphabetically; the Name column stretches to fill the width.
- **Side panel views** — an optional right-hand panel showing **Top largest files**, **Oldest files**, or **Safe-to-delete temp files** (with a one-click **Recycle all**). The panel grows with the window, has a **draggable splitter** between it and the list, and can **detach** into its own floating window.
- **Delete without waiting** — recycling a folder or file updates the view in place on a background thread; no full rescan, no freeze.
- **Dark / Light / Auto theme** that follows the Windows system theme by default. Title bar, menu bar, lists, tree, and panels are all themed via documented + undocumented uxtheme/dwmapi APIs.
- **Remembers your window size** between launches.
- **Always-visible progress** in the status bar; `Esc` stops a running scan.
- **Admin elevation prompt** at startup if not elevated, with one-click UAC relaunch.
- **Right-click / keyboard actions:** Open in Explorer, Copy path, Open Command Prompt here, Move to Recycle Bin.

## Download

Pre-built binaries are attached to each [GitHub Release](https://github.com/StruisICT/ClutterCutter/releases). Two builds of the same app are attached — download whichever you prefer and run it; both are single self-contained files:

- **`ClutterCutter.exe`** — the original C# build (needs the in-box .NET Framework 4 runtime, present on every modern Windows).
- **`ClutterCutter-rust.exe`** — the Rust port (no runtime dependency). This is the build packaged for winget.

## Building from source

ClutterCutter ships as two implementations of the same app — the original **C#** build and an ongoing **Rust port**. CI builds both on every push. Contributors should read [`AGENTS.md`](AGENTS.md) for the full dev workflow, architecture, and conventions.

### C# build

You only need a Windows machine. The .NET Framework 4 C# compiler ships with Windows; no Visual Studio or .NET SDK install required.

```powershell
& "$env:WINDIR\Microsoft.NET\Framework64\v4.0.30319\csc.exe" `
    -nologo -target:winexe -optimize+ -platform:anycpu `
    -reference:System.Windows.Forms.dll `
    -reference:System.Drawing.dll `
    -reference:System.dll `
    -reference:System.Core.dll `
    -reference:Microsoft.VisualBasic.dll `
    -win32icon:ClutterCutter.ico `
    -out:ClutterCutter.exe `
    ClutterCutter.cs
```

### Rust build

Needs the Rust stable toolchain.

```powershell
cd rust
cargo build --release   # -> rust/target/release/cluttercutter.exe
```

There's also a console harness for testing the scanners without the GUI, e.g.
`cargo run --bin cluttercutter-cli -- --top-n 20 C:\Users` (full flag list in `AGENTS.md`).

GitHub Actions reproduces both builds on every push (`.github/workflows/build.yml`).

## Releasing

Releases are managed by [release-please](https://github.com/googleapis/release-please). Conventional-commit messages on `main` keep an open "release PR" up to date with the next version + a generated changelog. Merging that PR creates the tag and the GitHub Release.

Versions follow [Semantic Versioning](https://semver.org). Conventional commits map to bumps as follows (the project is currently pre-1.0, so breaking changes bump the minor while the major is held at `0`):

| Commit | Pre-1.0 bump (now) | Post-1.0 bump |
|--------|--------------------|---------------|
| `fix:` | patch (`0.3.0` → `0.3.1`) | patch |
| `feat:` | minor (`0.3.0` → `0.4.0`) | minor |
| `feat!:` / `BREAKING CHANGE:` | minor (`0.3.0` → `0.4.0`) | major |

When the public behaviour is considered stable, graduate to `1.0.0` (set `bump-minor-pre-major: false` in `release-please-config.json`, or land a commit with a `Release-As: 1.0.0` footer); after that, breaking changes bump the major per strict SemVer. The Rust crate version in `rust/Cargo.toml` is kept in sync with the release version automatically by release-please.

Because tags created by `GITHUB_TOKEN` don't recursively trigger workflows, the **Build** workflow has a `workflow_dispatch` trigger so we can run it manually for a published tag — go to **Actions → Build → Run workflow**, enter the tag (e.g. `v0.2.0`), and the exe is built and attached to the matching Release. (For fully automated tag → exe upload, swap the release-please action's token for a PAT secret.)

## Keyboard shortcuts

| Key             | Action                                  |
|-----------------|-----------------------------------------|
| `F5`            | Refresh / re-scan                       |
| `Esc`           | Stop the running scan                   |
| `Backspace`     | Go to parent folder (when tree focused) |
| `Enter`         | Drill into the selected list row        |
| `Del`           | Move selected items to Recycle Bin      |

## Notes on file counts

When MFT mode is active, hard-linked files (which `WinSxS` uses heavily) are counted **once** by their canonical name — that's why the file/folder totals can differ from a tree-walking scanner that counts each path separately. The MFT total reflects what's actually on disk.

## Repo

Pushed and maintained at [StruisICT/ClutterCutter](https://github.com/StruisICT/ClutterCutter).

---

© Struis ICT — all rights reserved.
