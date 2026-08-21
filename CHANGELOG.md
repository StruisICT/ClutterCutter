# Changelog

## [0.11.0](https://github.com/StruisICT/ClutterCutter/compare/v0.10.0...v0.11.0) (2026-08-21)


### Features

* **gui:** polish the egui UI toward Win32 parity ([#77](https://github.com/StruisICT/ClutterCutter/issues/77)) ([c811d69](https://github.com/StruisICT/ClutterCutter/commit/c811d69b1fdf908418f7cf578c982c55e75bdec5))
* MSI installer for enterprise/network deployment ([#78](https://github.com/StruisICT/ClutterCutter/issues/78)) ([cf5d325](https://github.com/StruisICT/ClutterCutter/commit/cf5d3256c35c41dc93b40e80ff233c9ce750fa43))

## [0.10.0](https://github.com/StruisICT/ClutterCutter/compare/v0.9.2...v0.10.0) (2026-08-18)


### Features

* cross-platform egui frontend + portable Linux scan core ([#75](https://github.com/StruisICT/ClutterCutter/issues/75)) ([949ed69](https://github.com/StruisICT/ClutterCutter/commit/949ed693267731260d039598522d10393fe015da))

## [0.9.2](https://github.com/StruisICT/ClutterCutter/compare/v0.9.0...v0.9.2) (2026-08-17)


### Bug Fixes

* ship the Rust build as the only ClutterCutter, drop legacy C# ([#73](https://github.com/StruisICT/ClutterCutter/issues/73)) ([108ede3](https://github.com/StruisICT/ClutterCutter/commit/108ede3046eaee416da182e9342c588dd7e0ed7f))

## [0.9.0](https://github.com/StruisICT/ClutterCutter/compare/v0.8.0...v0.9.0) (2026-08-13)


### Features

* clear button (×) in the search box, plus Esc to clear ([#67](https://github.com/StruisICT/ClutterCutter/issues/67)) ([79ea8b3](https://github.com/StruisICT/ClutterCutter/commit/79ea8b3250b8ab207c68b69123bb6b8b079914a4))
* debounce the search box so it runs once you pause typing ([#68](https://github.com/StruisICT/ClutterCutter/issues/68)) ([df27be2](https://github.com/StruisICT/ClutterCutter/commit/df27be2c72f6061f5fbbfd96dad5043d39b4cf2b))
* hover tooltip explaining protected system files in the side panel ([#61](https://github.com/StruisICT/ClutterCutter/issues/61)) ([79eff30](https://github.com/StruisICT/ClutterCutter/commit/79eff301901de0daed8c5ece52ac64f03c8f8bd6))
* search always spans all drives ([#71](https://github.com/StruisICT/ClutterCutter/issues/71)) ([c080901](https://github.com/StruisICT/ClutterCutter/commit/c08090147ca8af3a7af7954bdef5d915d0727e8d))
* space-separated AND terms in search ([#69](https://github.com/StruisICT/ClutterCutter/issues/69)) ([0de4a65](https://github.com/StruisICT/ClutterCutter/commit/0de4a6515e20b3cc4382c1b5648347a3b6d7ff5f))
* System-cleanup panel + hide protected system files from file lists ([#63](https://github.com/StruisICT/ClutterCutter/issues/63)) ([0dc1659](https://github.com/StruisICT/ClutterCutter/commit/0dc1659f19c7a280c012001796b9283e0efbc82e))
* warm-white light theme instead of pure white ([#66](https://github.com/StruisICT/ClutterCutter/issues/66)) ([77dd91b](https://github.com/StruisICT/ClutterCutter/commit/77dd91bfef65be3bf93316536a4c48231bf081a5))


### Bug Fixes

* guide to shadow-copy tools on Server instead of a dead System Protection launch ([#64](https://github.com/StruisICT/ClutterCutter/issues/64)) ([4624bea](https://github.com/StruisICT/ClutterCutter/commit/4624bead60b7eb6318604613fe5b1a1892fbd97b))
* make the search box clickable, always visible, and results readable ([#65](https://github.com/StruisICT/ClutterCutter/issues/65)) ([f3dcfb1](https://github.com/StruisICT/ClutterCutter/commit/f3dcfb1376caa36d0a74f59c00e4ba442427270f))

## [0.8.0](https://github.com/StruisICT/ClutterCutter/compare/v0.7.0...v0.8.0) (2026-08-07)


### Features

* add a Check-for-updates link in the About window ([#54](https://github.com/StruisICT/ClutterCutter/issues/54)) ([6844ce2](https://github.com/StruisICT/ClutterCutter/commit/6844ce22a2b2d9bae492cf377ee8a8b061795bde))
* add a Free column showing disk free space ([#58](https://github.com/StruisICT/ClutterCutter/issues/58)) ([f201aa4](https://github.com/StruisICT/ClutterCutter/commit/f201aa4ff15a77105c4edb49c3192161b850afa2))
* add a global search box in the top bar ([#57](https://github.com/StruisICT/ClutterCutter/issues/57)) ([b91a914](https://github.com/StruisICT/ClutterCutter/commit/b91a914aa329eda26e330869cfece9e409c92690))
* add a themed Settings page under File ([#49](https://github.com/StruisICT/ClutterCutter/issues/49)) ([21b0157](https://github.com/StruisICT/ClutterCutter/commit/21b0157ae35b9886e6c4735714d80fb06de5794c))
* inline tree expand, hover fix, drive spacing, nav order ([#41](https://github.com/StruisICT/ClutterCutter/issues/41)) ([1db70f4](https://github.com/StruisICT/ClutterCutter/commit/1db70f4c3d7adcd290b33cecfd1b5e069b09db48))
* **scan:** show real per-drive progress instead of a marquee ([#56](https://github.com/StruisICT/ClutterCutter/issues/56)) ([c7d134c](https://github.com/StruisICT/ClutterCutter/commit/c7d134c9b7cb2cd31a3032ca34cbf29e76023cd0))
* **settings:** let users hide main-list columns ([#50](https://github.com/StruisICT/ClutterCutter/issues/50)) ([4da2120](https://github.com/StruisICT/ClutterCutter/commit/4da2120f89b65847eddd1049626932a917cc553d))
* **topbar:** use a fuller crescent for the theme toggle's moon ([#48](https://github.com/StruisICT/ClutterCutter/issues/48)) ([fb58dea](https://github.com/StruisICT/ClutterCutter/commit/fb58dead284e4700919a75dc0a852f2b5b361bfe))
* UX polish + scanning/safety hardening + tested logic layer ([#43](https://github.com/StruisICT/ClutterCutter/issues/43)) ([4045dac](https://github.com/StruisICT/ClutterCutter/commit/4045dac32e381ca26db8c5838e17050acec0f1ed))


### Bug Fixes

* **about:** render the app icon crisp instead of upscaled ([#45](https://github.com/StruisICT/ClutterCutter/issues/45)) ([c628315](https://github.com/StruisICT/ClutterCutter/commit/c6283159f712959a0d7e64716d2d40732f2a7ee6))
* harden the NTFS MFT parser against corrupt/crafted volumes ([#52](https://github.com/StruisICT/ClutterCutter/issues/52)) ([931d894](https://github.com/StruisICT/ClutterCutter/commit/931d89499758666e4ab8c23a2cff4cbb85c88097))
* Home reliably returns to the all-drives overview ([#44](https://github.com/StruisICT/ClutterCutter/issues/44)) ([d6dcb2f](https://github.com/StruisICT/ClutterCutter/commit/d6dcb2f675e6cfe65c59074b85659991b93d8e3c))
* reuse the startup scan when clicking a drive in the sidebar ([#59](https://github.com/StruisICT/ClutterCutter/issues/59)) ([bd8b0b3](https://github.com/StruisICT/ClutterCutter/commit/bd8b0b30c1da9bfcfcafc4f370f7e70cb74eb1cb))
* **topbar:** anti-alias the theme-toggle pill for smooth edges ([#46](https://github.com/StruisICT/ClutterCutter/issues/46)) ([fe60328](https://github.com/StruisICT/ClutterCutter/commit/fe6032842e87d8cbb90f2772b29ccea2fc187c6e))
* **topbar:** keep the theme-pill border even on all edges ([#47](https://github.com/StruisICT/ClutterCutter/issues/47)) ([801e01a](https://github.com/StruisICT/ClutterCutter/commit/801e01adb35a1cfef6b0e0af5740cbba825cda6c))
* **ui:** settings window sizing/tooltips + tighter topbar buttons ([#51](https://github.com/StruisICT/ClutterCutter/issues/51)) ([f0bbee0](https://github.com/StruisICT/ClutterCutter/commit/f0bbee07a073e736a7a78d08e3d983fb4aef9f33))

## [0.7.0](https://github.com/StruisICT/ClutterCutter/compare/v0.6.0...v0.7.0) (2026-08-03)


### Features

* apply Struis ICT house style to the app UI ([#38](https://github.com/StruisICT/ClutterCutter/issues/38)) ([ecad4f1](https://github.com/StruisICT/ClutterCutter/commit/ecad4f1665d3850596113d39da2cce237b037cc2))

## [0.6.0](https://github.com/StruisICT/ClutterCutter/compare/v0.5.0...v0.6.0) (2026-08-02)


### Features

* faster scanning + in-place delete (no rescan) + panel fix ([#33](https://github.com/StruisICT/ClutterCutter/issues/33)) ([c2cb29c](https://github.com/StruisICT/ClutterCutter/commit/c2cb29c11190a6ff25a46983a8a6ce3311c681f2))
* labeled AAA treemap + auto-scan all drives on startup ([#31](https://github.com/StruisICT/ClutterCutter/issues/31)) ([5467316](https://github.com/StruisICT/ClutterCutter/commit/54673165e8e61645ad17f066e4a5d50f95c7bba3))
* remove treemap + folder-browsing/panel UX (drill, files, alphabetical, resizable/draggable panel, size persistence) ([#35](https://github.com/StruisICT/ClutterCutter/issues/35)) ([3f24b41](https://github.com/StruisICT/ClutterCutter/commit/3f24b413661c553296614719d7a5cb1705845908))

## [0.5.0](https://github.com/StruisICT/ClutterCutter/compare/v0.4.0...v0.5.0) (2026-07-10)


### Features

* detachable side-panel layout, scan-all-drives, temp recycle-all, WCAG AA ([#26](https://github.com/StruisICT/ClutterCutter/issues/26)) ([ce3e450](https://github.com/StruisICT/ClutterCutter/commit/ce3e45073918d78c27bc3194c8c9941217b498f7))
* treemap view in the Rust port ([#25](https://github.com/StruisICT/ClutterCutter/issues/25)) ([84f1b31](https://github.com/StruisICT/ClutterCutter/commit/84f1b31ff6b87d277b298293b42e9ad3befac32e))


### Bug Fixes

* make dark mode cover buttons, headers, menu bar, and status bar ([#27](https://github.com/StruisICT/ClutterCutter/issues/27)) ([96206e6](https://github.com/StruisICT/ClutterCutter/commit/96206e6fcb6c716a4916e777b9fade1e5bc52576))

## [0.4.0](https://github.com/StruisICT/ClutterCutter/compare/v0.3.0...v0.4.0) (2026-06-09)


### Features

* oldest-files view in the Rust port ([#12](https://github.com/StruisICT/ClutterCutter/issues/12)) ([9a5a41f](https://github.com/StruisICT/ClutterCutter/commit/9a5a41f9f26ecab281158c174ad09b0daf15b82a))
* safe-to-delete temp files view in the Rust port ([#14](https://github.com/StruisICT/ClutterCutter/issues/14)) ([fcb57ab](https://github.com/StruisICT/ClutterCutter/commit/fcb57ab9977f8702bb0a2d8d7de179e95a852065))
* top-N largest files view in the Rust port ([#10](https://github.com/StruisICT/ClutterCutter/issues/10)) ([6b0f914](https://github.com/StruisICT/ClutterCutter/commit/6b0f914a34a8ea2df53a7138304d07e01ed059ff))

## [0.3.0](https://github.com/Struis112/ClutterCutter/compare/v0.2.1...v0.3.0) (2026-05-13)


### Features

* add Rust port of ClutterCutter (low-level Win32 base) ([#6](https://github.com/Struis112/ClutterCutter/issues/6)) ([948f14f](https://github.com/Struis112/ClutterCutter/commit/948f14f41ee52d07ae7104937c223075b0305d75))


### Miscellaneous Chores

* release 0.3.0 ([#8](https://github.com/Struis112/ClutterCutter/issues/8)) ([9925c15](https://github.com/Struis112/ClutterCutter/commit/9925c15d1a232d6bfe9dfd2f016ad79d1afd3a95))

## [0.2.1](https://github.com/Struis112/ClutterCutter/compare/v0.2.0...v0.2.1) (2026-05-08)


### Features

* add Buy Me a Coffee support link ([#4](https://github.com/Struis112/ClutterCutter/issues/4)) ([81bd542](https://github.com/Struis112/ClutterCutter/commit/81bd5424037d56213b5f304c3361d5c3aca1c6cd))

## [0.2.0](https://github.com/Struis112/ClutterCutter/compare/v0.1.0...v0.2.0) (2026-05-07)


### ⚠ BREAKING CHANGES

* the binary is now ClutterCutter.exe; users with the old TreeSizeLite.exe shortcut will need to repoint it.

### Features

* initial public release of ClutterCutter ([d56ccca](https://github.com/Struis112/ClutterCutter/commit/d56cccaae334b7e8881ebde06b1f0edb11eda19e))
* rebrand to ClutterCutter ([#2](https://github.com/Struis112/ClutterCutter/issues/2)) ([6ac5ee3](https://github.com/Struis112/ClutterCutter/commit/6ac5ee3f147e303213ca1d5eb08bc9d4bb7e0454))
