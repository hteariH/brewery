# Brewery M6d — Packages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Installable Brewery 0.1 packages: a per-user Windows installer (Inno Setup) and winget manifests; a Linux tarball laid out as `/opt/brewery`, deb and rpm built with nFPM, and an AUR `brewery-bin` PKGBUILD; an unsigned, unsupported macOS zip. No Flatpak, no AppImage. Every package contains the app, the CLI, the offline bundle (M6c) and the licences, and is verified in CI by installing it.

**Architecture:** Gap analysis `docs/superpowers/plans/m6-gap-analysis.md` sections 1.2, 1.3, 2.x, 3 and "M6d". Brewery-owned copies of Zed's packaging files (never edit Zed's `zed.iss`, `bundle-windows.ps1`, `bundle-linux`, `bundle-mac`, `zed.desktop.in` in place). Inputs to every packaging script: a release-fast app binary, the CLI binary, (Windows) `auto_update_helper.exe`, and the M6c bundle directory for that OS/arch (`script/brewery/fetch_bundle_deps.py` output: `jre/`, `brewery-daemon/`, `jdtls/` (incl. `jdtls/tools/`), `extensions/java/`, `licenses/`). A `package` CI job matrix consumes the build and bundle artifacts and runs install smoke tests.

**Tech Stack:** Inno Setup 6 (ISCC, preinstalled on windows-2025 runners or installed via choco at a pinned version), PowerShell, bash, nFPM (pinned release + checksum), Docker (ubuntu:24.04, fedora:41, archlinux:base-devel), `winget validate`, macOS `ditto`/`cargo-bundle` (Zed's fork).

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (packaging: Windows + Linux main, macOS unofficial; deb/rpm/AUR only — user decision: no Flatpak, no AppImage).

## Global Constraints

- Names and identity: app display name `Brewery`; `AppId` GUIDs new per channel (generate once, record in the .iss); `AppMutex` = the app's instance mutex (`release_channel::app_identifier()` + `-Instance-Mutex`, check `crates/zed/src/zed/windows_only_instance.rs`); Linux app id `io.github.htearih.Brewery` (must equal `ReleaseChannel::app_id()` — verify in `crates/release_channel/src/lib.rs`, adjust the desktop file name accordingly); macOS bundle identifier = the same app id per channel; CLI command `brewery` everywhere; version from `crates/zed/BREWERY_VERSION`; output names `Brewery-<ver>-x86_64-setup.exe`, `brewery-<ver>-linux-x86_64.tar.gz`, `brewery_<ver>_amd64.deb`, `brewery-<ver>-1.x86_64.rpm`, `brewery-<ver>-macos-aarch64.zip`.
- Must NOT ship: Zed's Terms of Service (`script/terms/terms.rtf`) as licence page (use a short GPL-3 info page or none), the Windows 11 sparse appx context menu (unsigned → install fails; use classic `HKCU\Software\Classes\*\shell` entries on all Windows versions), the `zed://` URL protocol registration and `x-scheme-handler/zed` MIME entry, Zed's remote server, sentry/debug-symbol uploads, AMD AGS download, any signing step (keep a disabled hook).
- Windows install is per-user (`PrivilegesRequired=lowest`, `%LOCALAPPDATA%\Programs\Brewery`), Start-menu entry, optional desktop icon, optional "Open with Brewery" context menu, optional PATH entry for `bin\` (so `brewery` works in a terminal), file associations limited to Java/JVM/web/config types (`.java .kt .kts .gradle .xml .properties .yml .yaml .json .sql .md .toml`). Update mode (`/update=true`) writes exactly the layout the M6b helper expects (see `.superpowers/sdd/2026-10-13-m6b-auto-update/task-3-report.md`, "In update mode, the installer must write these under `{app}\install\`" table and the list of files written directly to `{app}`), and `{app}\updates\versions.txt` last.
- Linux tree (tarball root `brewery/`, installed to `/opt/brewery`): `bin/brewery` (CLI), `libexec/brewery-editor` (app), `lib/` (bundled .so as Zed's script does, rpath `$ORIGIN/../lib`), `jre/`, `brewery-daemon/`, `jdtls/`, `extensions/java/`, `share/applications/<app id>.desktop`, `share/icons/hicolor/{512x512,1024x1024}/apps/brewery.png`, `licenses/`. deb/rpm/AUR add `/usr/bin/brewery` → `/opt/brewery/bin/brewery`, the desktop file and icons under `/usr/share`, licences under `/usr/share/doc/brewery/` (deb copyright) / `/usr/share/licenses/brewery(-bin)/`. Minimal runtime `depends` (what `find_libs` doesn't bundle: libxkbcommon, vulkan loader, alsa, wayland-client, x11-xcb; `git` recommended). Desktop file: `Name=Brewery`, `GenericName=Java IDE`, `Keywords=java;spring;ide;`, no zed scheme.
- Licences in every package: Brewery GPL-3.0-or-later (fork) + Zed's licences as shipped by Zed's `licenses.md` generation (`script/generate-licenses` if usable, else the existing approach), daemon Apache-2.0 + its THIRD-PARTY-NOTICES, the M6c bundle `licenses/` (JRE GPLv2+CE with source link, jdtls EPL-2.0, Lombok MIT, java-debug EPL-1.0, proxy/extension Apache-2.0), IntelliJ icons Apache-2.0 NOTICE (brewery_theme).
- Every script is runnable locally and in CI with explicit inputs (`--app`, `--cli`, `--helper`, `--bundle`, `--version`, `--out`); no network access except pinned tool downloads with checksums.
- CI: a `package` job per format, triggered on tags and on dispatch (`all_platforms` or a new `packages` input), consuming `build-*` and `bundle-*` artifacts; uploads `package-*` artifacts. Smoke tests: Windows silent install → file asserts → `bin\brewery.exe --version` → `jre\bin\java -version` → silent uninstall → directory gone; deb in `ubuntu:24.04` (`apt install ./x.deb`, `brewery --version`), rpm in `fedora:41` (`dnf install ./x.rpm`), AUR `makepkg -si` as non-root in `archlinux:base-devel` with a local `file://` source; macOS zip contents + `CFBundleIdentifier` (job continue-on-error); `winget validate` on the rendered manifests.
- Publishing (winget PR, AUR push, GitHub Release) is NOT done here: M6e/M6f produce the files; the owner publishes.

### Task 1: Windows installer
**Files:** `crates/zed/resources/windows/brewery.iss` (from zed.iss), `script/brewery/bundle-windows.ps1` (from bundle-windows.ps1), `script/brewery/windows-info.txt` (short GPL-3 / not-affiliated notice), CI job steps.
- [ ] Build an installer locally from the CI Windows build artifact + a local Windows bundle; silent install to a temp per-user dir, asserts, `--version`, uninstall; update-mode run (`/update=true`) produces the `install\` layout the helper expects (assert the file list).
- [ ] Commit `build: Windows installer`.

### Task 2: Linux tarball, deb, rpm
**Files:** `script/brewery/bundle-linux`, `crates/zed/resources/brewery.desktop.in`, `packaging/nfpm.yaml`, `packaging/linux/postinstall.sh`, `packaging/linux/postremove.sh`, CI steps.
- [ ] Tarball tree asserts + `bin/brewery --version` (on WSL Ubuntu locally if a Linux build is available, else in CI); nFPM deb/rpm; container installs in ubuntu:24.04 and fedora:41 with `brewery --version` and the desktop file present.
- [ ] Commit `build: Linux tarball, deb and rpm`.

### Task 3: AUR and winget manifests
**Files:** `packaging/aur/PKGBUILD.in`, `packaging/winget/{Brewery.yaml.in, Brewery.installer.yaml.in, Brewery.locale.en-US.yaml.in}` (`PackageIdentifier: hteariH.Brewery`, `InstallerType: inno`, `Scope: user`, `Commands: [brewery]`, `License: GPL-3.0-or-later`), `script/brewery/render_manifests.py` (fills version, URLs, sha256; unit-tested).
- [ ] Python tests for rendering; CI: `makepkg -si` in archlinux:base-devel against the CI tarball (file:// source), `.SRCINFO` rendered; `winget validate` on Windows.
- [ ] Commit `build: AUR and winget manifests`.

### Task 4: macOS zip
**Files:** `crates/zed/Cargo.toml` bundle metadata blocks (marked `# brewery:`; identifiers per channel = app id), `script/brewery/bundle-mac`, CI step (continue-on-error).
- [ ] Zip contains `Brewery.app` with `Contents/MacOS/brewery` (fix the M6a-noted inner binary name lookups if the CLI expects `Contents/MacOS/zed`), CLI, `Contents/Resources/{jre,brewery-daemon,jdtls,extensions}`; `CFBundleIdentifier` asserted; README note "unsigned, unsupported; `xattr -dr com.apple.quarantine Brewery.app`".
- [ ] Commit `build: unsigned macOS zip`.

### Task 5: CI package jobs and docs
- [ ] Wire the package jobs (Global Constraints), FORK.md "Packaging" section (how to build each package locally), README "Install" section (Windows installer / winget, deb, rpm, AUR, macOS unsupported).
- [ ] Commit `ci: build and smoke-test packages`.
