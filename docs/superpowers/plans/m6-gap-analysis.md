# M6 (Release 0.1): gap analysis

Date: 2026-10-13. Scope: spec sections "1. Non-goals", "3. Legal boundaries",
"10. CI and releases" and the M6 row in
`docs/superpowers/specs/2026-10-10-brewery-design.md`, plus the M2a ruling that
moved offline bundling of jdtls and the JRE to M6
(`docs/superpowers/plans/2026-10-11-m2a-java-language.md:7`). Line numbers are for
the tree at `96212256b4` (branch `m5d`).

Fixed decisions: Windows = Inno Setup + winget; Linux = `.deb` + `.rpm` (nFPM) +
AUR `brewery-bin` from a release tarball; macOS = unsigned zip, unsupported.
**No Flatpak, no AppImage.** No code signing for 0.1 (SignPath application is
separate, out of band).

## TL;DR

| Area | What exists today | Gap | Approach | Size |
|---|---|---|---|---|
| Windows exe metadata | `crates/windows_resources/src/windows_resources.rs:46-51` hard-codes product names "Zed"/"Zed Dev"; `:103-104` CompanyName/Copyright "Zed Industries"; icons already Brewery placeholders (`dc574aedf1`) | Product/Description show "Zed Dev"; RELEASE_CHANNEL is `dev` | Rename to `Brewery` / `Brewery Dev` etc., CompanyName "Brewery contributors", copyright keeps Zed attribution | XS |
| Release channel | `crates/zed/RELEASE_CHANNEL` = `dev`; display names, app ids, mutex identifier already Brewery (`release_channel/src/lib.rs:45-53, 208-238`) | Release builds must be `stable` | Release workflow writes `stable` before building (Zed's own approach, `script/determine-release-channel`) | XS |
| Version | `crates/zed/Cargo.toml:5` `version = "1.25.0"` (Zed's); `AppVersion` comes from it (`zed/src/main.rs:308`) | No Brewery version; changing the crate version breaks SSH remote-server download (asks Zed's cloud for "0.1.0") | Separate `BREWERY_VERSION` used by auto-update, About, exe resources, packages; `AppVersion` stays Zed's | S |
| Windows installer | `crates/zed/resources/windows/zed.iss` (1421 lines) + `script/bundle-windows.ps1` (413 lines) | Zed names/GUIDs/URLs everywhere; LicenseFile is Zed's Terms of Service; builds `zed.exe` (now `brewery.exe`); appx needs signing; no daemon/JRE/jdtls; registers `zed://` | Copy to `brewery.iss` / `script/brewery/bundle-windows.ps1`, rename, strip appx/AMD AGS/sentry/remote-server, add bundled dirs | M |
| winget | none | manifest + submission | `winget/manifests/...` templates rendered by the release workflow; first PR to `microsoft/winget-pkgs` by the owner | S |
| Linux tarball | `script/bundle-linux` (254 lines) produces `zed.app/{bin/zed, libexec/zed-editor, lib/, share/}` | Zed names, builds `--bin zed` (now `brewery`), app id `dev.zed.Zed`, `x-scheme-handler/zed`; no bundled dirs | `script/brewery/bundle-linux` producing `brewery-<ver>-linux-x86_64.tar.gz` with `brewery/` root | M |
| deb / rpm | none | everything | `packaging/nfpm.yaml` installs the tarball tree to `/opt/brewery`, symlink `/usr/bin/brewery`, desktop file, icons | S |
| AUR | none | `brewery-bin` PKGBUILD | `packaging/aur/PKGBUILD.in` + `.SRCINFO` rendering; push by owner (AUR SSH key) | S |
| macOS | `script/bundle-mac` (cargo-bundle fork, codesign, notarise, dmg); bundle metadata `dev.zed.Zed*` / "Zed" (`zed/Cargo.toml:294-327`) | Names/identifiers; signing must be skipped | Bundle metadata rename; zip of `Brewery.app` without signing; CI job `continue-on-error` | S |
| Auto-update | `auto_update` crate talks to Zed cloud (`auto_update.rs:677-738`); polling is hard-off (`release_channel/src/lib.rs:201-205` `poll_for_updates() -> false`); `auto_update` default `false` (`assets/settings/default.json:1740`, guarded by `settings/src/brewery_defaults.rs:18`) | No GitHub source, Windows helper only moves Zed files, Linux installer path assumes `~/.local/zed.app` | Hook `get_release_asset` for the app asset → GitHub `latest.json`; Windows full update; Linux/macOS notify-only | M |
| Offline JRE | `find_java` order: `BREWERY_DAEMON_JAVA`, `JAVA_HOME`, PATH, `~/.jdks`, system dirs (`brewery_daemon_client/src/jdk.rs:110-205`) | No bundled JRE lookup | jlink JRE in `<bundle>/jre`, checked right after `BREWERY_DAEMON_JAVA` | S |
| Offline daemon jar | `find_daemon_jar` checks `<exe_dir>/brewery-daemon/brewery-daemon.jar` (`jdk.rs:208-240`) | Linux exe lives in `libexec/`, so `<exe_dir>` is wrong there; packaging never copies the jar | One `bundle_root()` helper; bundle scripts copy `brewery-daemon-all.jar` | XS |
| Offline Java extension + jdtls | Java extension 6.8.28 auto-installed from Zed's gallery (`default.json:2365-2371`); it downloads jdtls, lombok, java-debug, java-lsp-proxy, gradle LS; `jdk_auto_download: true` (`default.json:2771-2775`) | First start needs network for all of these | Bundle extension + components; seed the extension on first run; computed default-settings overlay pointing `lsp.jdtls.settings.*` at bundled files | M/L |
| CLI | `cli` binary named `cli` (`crates/cli/Cargo.toml:15-18`); looks for `../libexec/zed-editor`, `./zed`, `../Zed.exe`, `./zed.exe` (`cli/src/main.rs:909-913, 1277-1279`); app looks for `bin/zed.exe`, `../bin/zed` (`util/src/util.rs:365-376`) | Dev fallback broken since the app binary is `brewery`; packaged names | Rename candidates to `brewery-editor` / `Brewery.exe` / `./brewery[.exe]` / `bin/brewery[.exe]`; clap name `brewery` | S |
| README / licences | README has the JetBrains disclaimer (`README.md:6-7`), status says M0; daemon builds `THIRD-PARTY-NOTICES.txt`; icons NOTICE at `crates/brewery_theme/icons/NOTICE`; `script/generate-licenses` (cargo-about) → `assets/licenses.md` | No Zed non-affiliation line, stale status/install docs; no top-level `NOTICE`; packages ship no licence set; jdtls/JRE/extension notices absent | README rewrite; `NOTICE` + `licenses/` dir assembled by bundle scripts | S |
| Release workflow | `brewery_ci.yml`: tag `v*` runs `build-windows` + `build-other` (release-fast, raw binaries only, `:241-319`) | No bundling, no checksums, no GitHub Release, no dry run | New `.github/workflows/brewery_release.yml` (tag + `workflow_dispatch` dry run) | M |

## 1. Windows

### 1.1 Exe metadata (`windows_resources`)

`crates/windows_resources/src/windows_resources.rs` is compiled into three
binaries: the app (`crates/zed/build.rs:213`), the CLI (`crates/cli/build.rs:35`)
and `auto_update_helper` (`crates/auto_update_helper/build.rs:9`, with manifest).

- `:46-51` channel → `(icon, product_name)`: `"Zed"`, `"Zed Preview"`,
  `"Zed Nightly"`, `"Zed Dev"`. This is the "Zed Dev" seen in Explorer, because
  `RELEASE_CHANNEL` defaults to `dev`.
- `:99-104` `FileDescription`, `ProductName`, `CompanyName "Zed Industries, Inc."`,
  `LegalCopyright "Copyright 2022 - 2025 Zed Industries, Inc."`.
- `:62-74` `FILEVERSION` is built from `CARGO_PKG_VERSION` of the crate being built
  (zed 1.25.0, cli 0.1.0, ...). With a Brewery version (section 6) it should
  take `BREWERY_VERSION` when set.

Approach: change the four product names to the `ReleaseChannel::display_name()`
strings (`Brewery`, `Brewery Preview`, `Brewery Nightly`, `Brewery Dev`),
`CompanyName "Brewery contributors"`, `LegalCopyright "Copyright 2026 Brewery
contributors. Portions copyright Zed Industries, Inc."` (keeps the GPL
attribution). Add an `OriginalFilename`/`InternalName` per binary only if cheap.
Mark each edit `// brewery:`. Optional: per-binary description ("Brewery CLI",
"Brewery update helper") through a `compile(manifest, description)` parameter.

Verification: unit test is impossible (`#![cfg(target_os = "windows")]`, build
helper); check in CI with
`(Get-Item target\release\brewery.exe).VersionInfo | Format-List` and assert
`ProductName -eq 'Brewery'` in the release workflow.

### 1.2 Inno Setup installer

Reuse `crates/zed/resources/windows/zed.iss` and
`script/bundle-windows.ps1`. Copy them to Brewery-owned files
(`crates/zed/resources/windows/brewery.iss`,
`script/brewery/bundle-windows.ps1`) rather than editing in place: they change
upstream and Brewery's version diverges heavily (renames on almost every line).

Things that must change:

| Where | Zed today | Brewery |
|---|---|---|
| `zed.iss:5-8` | `AppPublisher=Zed Industries`, URLs `zed.dev` | `Brewery contributors`, `https://github.com/hteariH/brewery` (+ `/releases`) |
| `zed.iss:45-46` | `LicenseFile: "script\terms\terms.rtf"` = **Zed's Terms of Service** | GPL-3 notice page (`InfoBeforeFile` with a short "GPL-3, no warranty, not affiliated" text) or drop; GPL needs no click-through |
| `zed.iss:46` | zh-cn language | keep English only (messages `en.isl` mention Zed? check strings `AddContextMenuFiles` etc. use `{#AppDisplayName}`; fine) |
| `zed.iss:68-81` | `Zed.exe`, `bin\*`, `tools\*`, `appx\*`, `amd_ags_x64.dll`, `OpenConsole.exe`, `conpty.dll` | `Brewery.exe`, `bin\brewery.exe` (CLI), `bin\brewery` (WSL sh), `tools\auto_update_helper.exe`, conpty/OpenConsole, plus `brewery-daemon\*`, `jre\*`, `jdtls\*`, `extensions\java\*`, `licenses\*` (recursesubdirs) |
| `zed.iss:71, 91`, `:1241, 1245` + `[Code]` AddAppxPackage | Win11 context menu via sparse **appx** (`explorer_command_injector`) | **Drop.** Sparse packages must be signed to register; unsigned → install error. Use the classic `HKCU\Software\Classes\*\shell` entries on all Windows versions (remove the `not IsWindows11OrLater` checks); on Win11 they show under "Show more options" |
| `zed.iss:1260` | registers `zed` URL protocol | **Remove** (would hijack an installed Zed); `zed://` handling stays internal |
| `zed.iss:94-1240` | ~1000 lines of `associatewithfiles` entries (`{#RegValueName}.ext`) | Keep (task is user-selectable) but trim to Java/JVM/web/config types if wanted; `RegValueName=Brewery` |
| `zed.iss:1402` | `{app}\updates\versions.txt` flag | keep, the update helper relies on it |
| `bundle-windows.ps1:116-135` | `cargo build --package zed --package cli --package auto_update_helper`, copies `zed.exe` | copy `brewery.exe` → `Brewery.exe`, `cli.exe` → `bin\brewery.exe` |
| `:138-155` remote server, `:157-195` debug zip + sentry, `:197-214` MakeAppx, `:225-233` AMD AGS | | drop (AMD AGS is Zed's GPU-services download; drop unless a GPU crash appears without it) |
| `:216-223`, `zed.iss:34-36` signing | `sign.ps1` (Azure Trusted Signing) | keep the `$canCodeSign` switch false; leave a hook for SignPath later |
| `:266-321` per-channel table | Zed AppIds `{2DB0DA96-...}` etc., mutex `Zed-Stable-Instance-Mutex`, `ZedIndustries.Zed` | **New GUIDs** per channel, `AppMutex = "<app_identifier()>-Instance-Mutex"` = `Brewery-Editor-Stable-Instance-Mutex` (`zed/src/zed/windows_only_instance.rs:34` + `release_channel/src/lib.rs:45-53`), `AppUserId = "Brewery.Brewery"`, `AppExeName = Brewery`, `AppSetupName = Brewery-<ver>-x86_64-setup` |
| `:395-410` | `ZedEditorUserSetup-x64-...` | output name above |

`PrivilegesRequired=lowest` (`zed.iss:39`) gives a per-user install in
`%LOCALAPPDATA%\Programs\Brewery`: keep it, it is what lets the auto-updater
write without UAC.

Verification (CI on `windows-2025`): ISCC compile succeeds; silent install
`Brewery-...-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /CURRENTUSER` then assert
files exist (`Brewery.exe`, `bin\brewery.exe`, `jre\bin\java.exe`,
`brewery-daemon\brewery-daemon.jar`), run `bin\brewery.exe --version` and
`jre\bin\java -version`, then uninstall silently and assert the directory is
gone. No GUI launch needed.

### 1.3 winget

New: `packaging/winget/` with three YAML templates (`version`, `installer`,
`defaultLocale`, schema 1.6+). `PackageIdentifier: hteariH.Brewery`,
`InstallerType: inno`, `Scope: user`, `InstallerUrl` = release asset,
`InstallerSha256` filled by the workflow, `UpgradeBehavior: install`,
`Commands: [brewery]`, `License: GPL-3.0-or-later`, `Publisher: Brewery contributors`.

Submission is a pull request to `microsoft/winget-pkgs`. That is a public action
by the owner: the workflow renders the manifests and uploads them as a release
asset; the owner runs `wingetcreate submit` (or a later workflow step with a PAT
secret, opt-in). Risks: first submissions get manual review; unsigned installers
pass but Defender/SmartScreen reputation may cause validation failures —
re-submit after SignPath.

## 2. Linux

### 2.1 Layout

Zed's tarball layout (`script/bundle-linux:186-252`):
`zed.app/bin/zed` (CLI), `zed.app/libexec/zed-editor` (app), `lib/` (bundled
`.so` from `ldd`, rpath `$ORIGIN/../lib` from `.cargo/bundle-config.toml`),
`share/icons`, `share/applications/<app_id>.desktop`, `licenses.md`.

Brewery layout (one tree used by tarball, deb, rpm and AUR):

```
/opt/brewery/
  bin/brewery                 CLI (crates/cli)
  libexec/brewery-editor      app (crates/zed bin "brewery")
  lib/                        bundled .so (as Zed)
  jre/                        jlink'ed Temurin 21
  brewery-daemon/brewery-daemon.jar
  jdtls/  extensions/java/    (section 4)
  share/applications/io.github.htearih.Brewery.desktop
  share/icons/hicolor/{512x512,1024x1024}/apps/brewery.png
  licenses/                   (section 7)
/usr/bin/brewery -> /opt/brewery/bin/brewery           (deb/rpm/AUR)
/usr/share/applications/io.github.htearih.Brewery.desktop (deb/rpm/AUR)
/usr/share/icons/hicolor/.../brewery.png
```

`/opt` keeps the rpath-based `lib/` working without distro-specific deps, and
keeps bundled Java bits out of FHS directories. The CLI canonicalises
`current_exe()` (`cli/src/main.rs:905-917`), so the `/usr/bin` symlink resolves to
`/opt/brewery/bin` and `../libexec/brewery-editor` is found.

`find_daemon_jar` uses `<exe_dir>` = `/opt/brewery/libexec` → must use a
**bundle root** (`exe_dir`, or its parent when `exe_dir` ends in `libexec`/`MacOS`).
See section 4.1.

### 2.2 Bundle script

Reuse `script/bundle-linux` as `script/brewery/bundle-linux`: keep the
clang/lld link line (`:129`), `llvm-objcopy` stripping (`:191-192`), `find_libs`
(`:203-211`), desktop file via `envsubst`. Drop flatpak (`:14, 31`), remote server
(`:137, 166-172, 254`), sentry (`:140-147`). Rename: `--bin brewery`,
`APP_NAME=Brewery`, `APP_ID=io.github.htearih.Brewery` (must equal
`ReleaseChannel::app_id()`, `release_channel/src/lib.rs:231-238`, or the window
won't match the desktop file), `APP_CLI=brewery`, `APP_ICON=brewery`.

Desktop template: copy `crates/zed/resources/zed.desktop.in` to a Brewery file;
change `Comment`, `Keywords=java;spring;ide;`, `GenericName=Java IDE`, and
**drop `x-scheme-handler/zed`** from `MimeType` (would steal Zed's links).

Output: `target/release/brewery-<ver>-linux-x86_64.tar.gz` with a single
`brewery/` root. Per spec the tarball is the AUR source only, not advertised.

### 2.3 deb and rpm (nFPM)

New `packaging/nfpm.yaml` (one file, both packagers):
`name: brewery`, `arch: amd64`, `version: ${BREWERY_VERSION}`,
`maintainer`, `license: GPL-3.0-or-later`, `homepage`, `contents` mapping the
unpacked tarball tree to `/opt/brewery` + symlink + desktop/icon copies,
`depends` minimal (`libxkbcommon0`, `libvulkan1`/`vulkan-loader`, `libasound2`,
`libwayland-client0`, `libx11-xcb1`, `git` recommended) taken from what
`find_libs` does **not** bundle. `scripts.postinstall`: `update-desktop-database`
and `gtk-update-icon-cache` if present. Copyright file:
`/usr/share/doc/brewery/copyright` (Debian expects it) generated from
`licenses/`.

Install nFPM in CI from its GitHub release at a pinned version with checksum
(MIT licence, build tool only).

Verification: `nfpm pkg --packager deb|rpm`; `dpkg-deb -c` / `rpm -qlp` content
asserts; then in containers `docker run ubuntu:24.04 apt install ./brewery.deb &&
brewery --version` and `fedora:41 dnf install ./brewery.rpm`. `lintian` warnings
non-blocking.

### 2.4 AUR `brewery-bin`

New `packaging/aur/PKGBUILD.in`: `pkgname=brewery-bin`, `provides=(brewery)`,
`conflicts=(brewery)`, `source=(https://github.com/hteariH/brewery/releases/download/v$pkgver/brewery-$pkgver-linux-x86_64.tar.gz)`,
`sha256sums` substituted by the workflow, `package()` copies to
`$pkgdir/opt/brewery` + symlink + desktop/icons + licences into
`/usr/share/licenses/brewery-bin/`. Render `.SRCINFO` in an
`archlinux:base-devel` container (`makepkg --printsrcinfo`) and test with
`makepkg -si --noconfirm` as a non-root user in the same container.

Publishing requires an AUR account with an SSH key: owner action. The workflow
uploads `PKGBUILD` + `.SRCINFO` as release assets; optional later step pushes
with an `AUR_SSH_KEY` secret. Check name collisions on aur.archlinux.org before
first push (the search page is bot-protected; check by hand).

## 3. macOS (unsupported)

`script/bundle-mac` uses Zed's `cargo-bundle` fork (`:64-65`), builds
`--bin zed` (`:95`), signs and notarises (`:133-225`), makes a dmg. Bundle
metadata in `crates/zed/Cargo.toml:294-327` still says `name = "Zed"`,
`identifier = "dev.zed.Zed"`; the identifier must equal `ReleaseChannel::app_id()`
(comment at `release_channel/src/lib.rs:230`).

Approach: rename the four `[package.metadata.bundle-*]` blocks
(`Brewery`, `io.github.htearih.Brewery[-Preview|-Nightly|-Dev]`); a short
`script/brewery/bundle-mac` that runs `cargo bundle`, copies CLI + daemon jar +
jre (macOS jlink) into `Contents/MacOS` / `Contents/Resources`, skips all signing,
and `ditto -c -k --keepParent Brewery.app brewery-<ver>-macos-aarch64.zip`. The
release notes say "unsigned, unsupported; run `xattr -dr com.apple.quarantine`".
No auto-update on macOS (notify only, section 5). CI job stays
`continue-on-error`.

## 4. Offline bundling

Spec: "Bundle script downloads jdtls, Spring Tools LS, java-debug and JDBC
drivers at pinned versions with checksums and builds the JRE with jlink"
(spec §10). Spring config LS and JDBC drivers are already inside the daemon jar
(`brewery_spring_config.rs:86, 176`; `brewery-daemon/build.gradle.kts:23-27`), so
the remaining downloads are jdtls, the Java extension's helpers and the JRE.

### 4.1 Bundle root (new, small)

Add one helper, e.g. a leaf crate `brewery_bundle` (no deps, like
`brewery_cloud`) or a module in `brewery_daemon_client`:

```rust
pub fn bundle_root() -> Option<PathBuf>;        // exe dir; parent if exe dir is libexec/ (Linux) or MacOS/ (mac → ../Resources)
pub fn bundled_jre_home() -> Option<PathBuf>;   // <root>/jre if <root>/jre/release exists
pub fn bundled_daemon_jar() -> Option<PathBuf>; // <root>/brewery-daemon/brewery-daemon.jar
pub fn bundled_jdtls_home() -> Option<PathBuf>; // <root>/jdtls
pub fn bundled_extension(id: &str) -> Option<PathBuf>; // <root>/extensions/<id>
```

Pure path logic with a `from_exe(&Path)` variant for tests (temp dirs, as
`jdk.rs:329-350` already does).

### 4.2 JRE for the daemon and jdtls

- `find_java_in` (`brewery_daemon_client/src/jdk.rs:134-205`): insert the bundled
  JRE right after `BREWERY_DAEMON_JAVA` (spec: "Bundled JRE for internal servers
  only"). `find_java_with_env` (used by `brewery_spring_config.rs:72, 326`) gets
  the same order. Update the doc comment at `:110`. Test: fake JRE with a
  `release` file under a temp bundle root wins over `JAVA_HOME`.
- Build: download Temurin **JDK** 21 (pinned version + SHA-256 from the Adoptium
  API) per platform, then
  `jlink --add-modules <list> --strip-debug --no-man-pages --no-header-files --compress=zip-6 --output jre`.
  Module list must cover: the daemon (Gradle Tooling API, JDBC, keyring:
  `java.se`, `jdk.unsupported`, `jdk.crypto.ec`, `jdk.zipfs`, `jdk.net`,
  `jdk.management`), jdtls (`--add-modules=ALL-SYSTEM` in its launch args, so it
  takes whatever exists; it needs `java.se`, `jdk.unsupported`, `jdk.jdi`,
  `jdk.attach`, `jdk.jdwp.agent` for java-debug, `java.instrument` for the Lombok
  javaagent, `jdk.localedata`, `jdk.charsets`). Start with that list plus
  `jdk.jfr`, `jdk.httpserver`, `jdk.compiler` (annotation processing inside
  jdtls is ECJ, but some plugins look for it); measure size (~60-80 MB).
  Fallback if smoke tests fail: `ALL-MODULE-PATH`.
- Project builds and runs keep using the user's JDK (spec §2); nothing changes in
  `brewery_build` / run configurations.
- Licence: Temurin = GPLv2 with Classpath Exception (+ assembly exception). jlink
  copies `legal/` into the image: keep it. Add the Adoptium source link for the
  exact tag (and ideally attach the matching source tarball to the GitHub
  Release) to satisfy GPLv2 §3 for redistributed binaries.

### 4.3 Java extension, jdtls and helpers

What the extension (`zed-extensions/java` 6.8.28, Apache-2.0) downloads at
runtime (from strings in the installed `extension.wasm` and its README):

| Component | Source | Override setting (`lsp.jdtls.settings.*`) |
|---|---|---|
| extension itself (wasm + grammars) | Zed gallery (`api.zed.dev`, `extension_host.rs:819, 983`) | none; seed on disk |
| jdtls | `download.eclipse.org/jdtls/milestones/<v>/` (+ `latest.txt`) | `jdtls_launcher` (absolute path to a launch script; skips the managed jdtls) |
| JDK for jdtls | Amazon Corretto when `jdk_auto_download` | `java_home` |
| Lombok | projectlombok.org | `lombok_jar` |
| java-debug plugin | `github.com/zed-industries/java-debug/releases/download/0.53.2/...` | `java_debug_jar` |
| java-lsp-proxy | GitHub releases of `zed-extensions/java` | `lsp_proxy_path` |
| Gradle LS (VSIX) + bridge | VSIX / GitHub | `lsp.gradle-language-server.settings.gradle_bridge_path` |
| update checks | GitHub tags API | `check_updates`: `always` (default) / `once` / `never` |

Approach:

1. **Seed the extension.** Bundle `extensions/java/` (the installed directory:
   `extension.toml`, `extension.wasm`, `grammars/*.wasm`, `languages/`,
   `debug_adapter_schemas/`; ~9 MB) built from the pinned tag with
   `extension_cli` (or downloaded from the gallery at bundle time with its
   checksum). A Brewery hook at startup, before `extension_host` loads the index,
   copies it into `paths::extensions_dir()/installed/java` **only if absent**.
   Gallery updates still work online afterwards. `// brewery:` hook in
   `zed/src/main.rs` near `extension_host::init`.
2. **Point the extension at bundled files** with a computed default-settings
   overlay: at startup Brewery merges JSON into the default settings (before
   `SettingsStore::new`, or via `set_default_settings`) with absolute paths from
   `bundle_root()`:
   `java_home = <root>/jre`, `jdk_auto_download = false` (only when the bundled
   JRE exists), `jdtls_launcher = <root>/jdtls/brewery-jdtls(.cmd)`,
   `lombok_jar`, `java_debug_jar`, `lsp_proxy_path`, `check_updates = "once"`.
   User settings still override (they are a higher layer). Without a bundle
   (dev builds) the overlay is empty and today's download behaviour stays.
3. **jdtls launcher script.** jdtls's own `bin/jdtls` is a Python script
   (Python not guaranteed), so ship a tiny Brewery launcher
   (`brewery-jdtls.cmd` / `brewery-jdtls` sh) that runs
   `<root>/jre/bin/java` with the equinox launcher and `-configuration
   <user-writable copy of config_win|config_linux>` (jdtls writes into its config
   dir; `/opt` and Program Files are read-only, so copy or use
   `-Dosgi.sharedConfiguration.area=<root>/jdtls/config_x -Dosgi.configuration.area=<cache>`,
   which the extension already does: see `-Dosgi.sharedConfiguration.area.readOnly=true`
   in its args). **Before writing this, read `src/jdtls.rs`
   (`build_jdtls_launch_args`, `get_jdtls_launcher_from_path`) at tag 6.8.28**:
   the launcher path must accept the args the extension appends (`-data`,
   `--jvm-arg=-javaagent:lombok`), and debugging must keep working through the
   proxy.
   Alternative if the launcher route breaks the debugger: pre-populate the
   extension's managed layout (`extensions/work/java/...`) on first run with
   `check_updates = "never"`; it already falls back to a local install when the
   update check fails ("falling back to local installation" string).
4. Gradle LS: bundle too, or accept an online first use (Gradle file editing
   only). Recommend bundling for a clean "fresh machine" story, but it is the
   first thing to cut if time is short.

Pinned versions and SHA-256s live in one file, e.g.
`script/brewery/bundle-deps.toml` (jdtls milestone + build, lombok, java-debug,
java-lsp-proxy, Temurin per OS/arch, extension tag). One script
(`script/brewery/fetch-bundle-deps` in Python or bash, plus a PowerShell twin or a
Python script used by both) downloads, verifies and lays out `bundle/`.

Verification: on a clean runner **without network for the app**
(Linux: `unshare -n` / docker `--network none`; Windows: none available cheaply,
rely on Linux), run a headless smoke: start the daemon with the bundled JRE and
jar (`brewery_daemon_client` e2e test already does this given
`BREWERY_DAEMON_JAR`, "Test daemon client against the built jar" step in `.github/workflows/brewery_ci.yml`), and start jdtls through the
launcher against `spring-petclinic`-like fixture, expecting an `initialize`
response. Full UI check stays on the manual release checklist (spec §9
"End to end").

Risks: extension upgrade on the gallery may change setting names (pin and test);
jdtls config dir writes; `jdk_auto_download` switched off only when the bundle
exists; total bundle size (JRE ~70 + jdtls ~50 + daemon 26 + app ~150 MB ≈
300 MB, matches spec estimate).

### 4.4 Daemon jar

`brewery-daemon` is in this repo (`brewery-daemon/`, Apache-2.0,
`./gradlew shadowJar` → `build/libs/brewery-daemon-all.jar`, 26 MB). Bundle
scripts copy it to `<root>/brewery-daemon/brewery-daemon.jar` and its
`build/notices/THIRD-PARTY-NOTICES.txt` to `licenses/`. Version: `version =
"0.1.0"` in `build.gradle.kts:10` — tie to `BREWERY_VERSION` (Gradle
`-Pversion=`).

## 5. Auto-update via GitHub Releases

### 5.1 Today

- `ReleaseChannel::poll_for_updates()` returns `false` (`release_channel/src/lib.rs:201-205`,
  test `:311-316`), so `auto_update::init` never polls (`auto_update.rs:281-302`)
  and the `Check` action returns silently (`:325-330`).
- `auto_update` default is `false` (`default.json:1740`), guarded by
  `settings/src/brewery_defaults.rs:18` and `settings_store.rs:1866-1960` tests.
- The fetch goes to Zed's cloud: `get_release_asset` builds
  `/releases/<channel>/<version>/asset?asset=zed&os&arch&metrics_id...` with
  `build_zed_cloud_url_with_query` (`auto_update.rs:677-738`) and expects
  `ReleaseAsset { version, url }` (`:193-196`).
- The same function serves `zed-remote-server` for SSH (`:599-675`). Keep that
  path on Zed's cloud (it must match Zed's version), or SSH remoting breaks.
- `ZED_CLOUD_ENABLED` (`brewery_cloud`) gates client sign-in/telemetry
  (`client.rs:888`, `telemetry.rs:703`, `zed/src/main.rs:897`) but **not**
  auto-update; auto-update is off only through `poll_for_updates`.
- Install paths: Windows runs the downloaded Inno installer with
  `/verysilent /update=true` then `tools\auto_update_helper.exe` swaps files on
  quit (`:1313-1360`); Linux untars into `~/.local/zed<suffix>.app` and
  rsyncs (`:1129-1195`); macOS mounts a dmg (`:1197+`).
- Helper job list hard-codes `Zed.exe`, `bin\Zed.exe`, `bin\zed`, conpty,
  OpenConsole (`auto_update_helper/src/updater.rs:166-213`, `:279-285`,
  `:367-368`).
- Release notes URL: `zed.dev/releases/...` or Zed's GitHub commits
  (`auto_update.rs:345-364`).

### 5.2 Minimal change

1. Release workflow publishes `latest.json` as a release asset:
   `{"version":"0.1.1","assets":{"windows-x86_64":{"url":"...setup.exe","sha256":"..."},"linux-x86_64":{...tar.gz},"macos-aarch64":{...zip}}}`.
   Fetch `https://github.com/hteariH/brewery/releases/latest/download/latest.json`
   (a redirect to a static asset: no API token, no 60/h API rate limit;
   `/releases/latest` skips pre-releases and drafts automatically).
2. `auto_update.rs` `get_release_asset`: `// brewery:` branch for
   `asset == "zed"` that fetches and parses `latest.json`, maps
   `OS`/`ARCH` → key, returns `ReleaseAsset { version, url }` (add an optional
   `sha256`). Drop telemetry query params. Verify SHA-256 after
   `download_release` (`:1076-1127`) when present (corruption guard; not a
   security boundary while the manifest comes from the same origin unsigned).
   Put parsing in a Brewery module (`brewery_update.rs` inside `auto_update` or a
   small crate) with unit tests using `FakeHttpClient` like
   `test_auto_update_downloads` (`:1402-1510`, which asserts the Zed URL path
   `/releases/stable/latest/asset` and needs updating).
3. `poll_for_updates()`: `true` for Stable (and Preview) only on Windows; keep
   `false` for Dev/Nightly. Update `test_brewery_never_polls_zed_updates`.
4. Version source: `AutoUpdater::new(version, ...)` gets `BREWERY_VERSION`
   instead of `AppVersion::global` (`:278`); `check_if_fetched_version_is_newer_non_nightly`
   (`:960`) keeps working with semver.
5. `auto_update` default `true` in `default.json:1740` (update the guard tests),
   or keep `false` and show a notification only. Recommend `true` on Windows,
   because there is no other update channel there except winget.
6. Windows: `target_path` `"Zed.exe"` → `"Brewery-setup.exe"` (`:923-930`);
   helper JOBS: `Brewery.exe`, `bin\brewery.exe`, `bin\brewery`, plus directory
   moves for `jre`, `brewery-daemon`, `jdtls`, `extensions` (`fs::rename` works on
   directories on the same volume; rollback logic is per job). The installer in
   update mode writes them under `{app}\install\` via `{code:GetInstallDir}`
   (`zed.iss:1415-1421`). `release_file_handles` (`updater.rs:279`) must list the
   new files; the daemon JVM and jdtls hold locks on `jre\*.dll` and jars, so the
   app must stop them on quit before the helper runs (the daemon client already
   kills its process tree on drop; verify jdtls dies with the app). Helper
   strings "Zed update failed" (`auto_update_helper.rs:11, 60, 126`) → Brewery.
7. Linux and macOS: **notify only.** deb/rpm/AUR are package-managed: build the
   Linux binary with `ZED_UPDATE_EXPLANATION="Update Brewery with your package
   manager or from GitHub Releases"` (`auto_update.rs:285, 311`; `cli/build.rs:5`)
   and let a Brewery notification link to the release page when `latest.json`
   is newer. Simplest version: keep polling disabled on Linux/macOS for 0.1 and
   show "Check for updates" as a link to the Releases page.
8. `release_notes_url` → `https://github.com/hteariH/brewery/releases/tag/v<ver>`.
9. `auto_update_ui` announcement for an unrelated product
   (`auto_update_ui.rs:204-226`, "delta.dev"): make sure it never fires
   (gate or remove under `// brewery:`).

Verification: unit tests for manifest parsing, OS/arch key mapping, version
comparison, and the helper JOBS list (`updater.rs:479 test_perform_update`
already exercises jobs on a temp dir). End-to-end: a manual check from `v0.1.0`
to a throwaway `v0.1.1-rc` tag is not possible with `/releases/latest` (it skips
pre-releases) — use a `BREWERY_UPDATE_MANIFEST_URL` env override pointing at a
draft asset for the test.

## 6. Version and channel

- Add `crates/zed/BREWERY_VERSION` (or `brewery_core::BREWERY_VERSION =
  env!("BREWERY_VERSION")` with a build-script default `0.1.0-dev`), used by:
  auto-update, About window (`zed/src/zed.rs:1568-1580` shows
  `CARGO_PKG_VERSION` = 1.25.0 today), `windows_resources` FILEVERSION, Inno
  `AppVersion`, nFPM, AUR, daemon Gradle version. Keep `AppVersion` (Zed's) for
  remote server, extensions and LSP client info (`lsp.rs:1155`).
- Release workflow asserts `v$(cat crates/zed/BREWERY_VERSION) == $GITHUB_REF_NAME`
  (pattern from `script/determine-release-channel:14-31`) and writes `stable`
  into `crates/zed/RELEASE_CHANNEL`.
- Channel change side effects: the database file is per channel, so a stable
  build starts without the dev build's workspace history; display name becomes
  "Brewery".

## 7. CLI and binary names

| Place | Today | Change |
|---|---|---|
| `crates/cli/src/main.rs:51` clap `name = "zed"`, help texts `:855-862` | `zed --existing` | `brewery` |
| `:909-913` Linux app lookup | `../libexec/zed-editor`, `../lib/zed/zed-editor`, `./zed` | `../libexec/brewery-editor`, `./brewery` (keep order) |
| `:1277-1279` Windows app lookup | `../Zed.exe`, `../lib/zed/zed-editor.exe`, `./zed.exe` | `../Brewery.exe`, `./brewery.exe` |
| `:928, 1205, 1378` version strings | `Zed {}` | `Brewery {}` |
| `:949` socket name `zed-{}.sock` | | `brewery-{}.sock` (avoids talking to a running Zed's CLI socket) |
| `:1030-1160` flatpak module | | leave untouched (dead for Brewery; `FLATPAK_ID` check is `dev.zed.Zed`) |
| `crates/util/src/util.rs:365-376` app → CLI | `bin/zed.exe`, `./cli.exe`; `../bin/zed`, `./cli` | `bin/brewery.exe`, `./cli.exe`; `../bin/brewery`, `./cli` |
| `crates/zed/resources/windows/zed.sh` | WSL shim calling `zed.exe` | copy as `brewery.sh`, call `Brewery.exe` |
| `crates/install_cli/src/install_cli_binary.rs:28` | `/usr/local/bin/zed` (macOS) | `/usr/local/bin/brewery` |
| `crates/zed/src/main.rs:1711` clap name | `zed` | `brewery` |

The cargo binary stays `cli` (renaming the target touches Zed's build scripts and
askpass, `askpass.rs:227-349` expects `cli.exe` in dev). Packages rename it to
`brewery`. Tests: existing `cli` tests (flatpak arg tests `:1148-1160`); add a test
that the candidate lists contain `brewery` names (pure arrays, extract to consts).

## 8. README, licences, NOTICE

README (`README.md`): already has the JetBrains disclaimer (`:6-7`) and explains
gallery traffic (`:12-17`). Missing: "Not affiliated with or endorsed by Zed
Industries. Zed is a trademark of Zed Industries, Inc." (same for JetBrains/IntelliJ
IDEA trademarks); status line (`:19`) still says M0; install section (winget,
deb/rpm, AUR, unsigned macOS zip, SmartScreen warning); licences section
(GPL-3 app, Apache-2.0 daemon, bundled components); offline note.

Licence set shipped in every package (`licenses/` in the bundle root):

| Component | Licence | File(s) |
|---|---|---|
| Brewery (fork) | GPL-3.0-or-later | `LICENSE-GPL` (repo root) |
| Rust deps, themes, icons | various | `assets/licenses.md` from `script/generate-licenses` (cargo-about 0.8.2), already used by Zed's bundles (`bundle-linux:110, 243`) |
| IntelliJ platform icons | Apache-2.0 | `crates/brewery_theme/icons/NOTICE` (+ Apache text) |
| Fonts | OFL-1.1 | `assets/fonts/{inter,jetbrains-mono}/LICENSE*` |
| brewery-daemon + deps | Apache-2.0; MariaDB Connector/J LGPL-2.1 (separate jar inside), PostgreSQL BSD-2, sqlite-jdbc Apache-2.0 | `brewery-daemon/LICENSE`, `build/notices/THIRD-PARTY-NOTICES.txt` |
| jdtls | EPL-2.0 (+ bundled Eclipse plugins EPL) | `jdtls/` keeps its `about.html`/notice files; add EPL-2.0 text and "source at github.com/eclipse-jdtls/eclipse.jdt.ls" |
| java-debug | EPL-1.0 | notice + source link |
| Lombok | MIT | licence text |
| Java extension, java-lsp-proxy | Apache-2.0 | licence text |
| JRE (Temurin 21) | GPL-2.0 with Classpath Exception | `jre/legal/` (jlink keeps it) + source link |
| conpty / OpenConsole | MIT (microsoft/terminal) | licence text |

A top-level `NOTICE` (repo root) summarises the above and the Zed fork
attribution. The Inno installer shows it (or `LICENSE-GPL`) instead of Zed's
Terms. Zed's `legal/` (terms, privacy, subprocessors) and `script/terms/` are Zed
Industries' documents and must not ship in Brewery packages.

## 9. Release workflow

New `.github/workflows/brewery_release.yml` (keep `brewery_ci.yml` for checks; it
already builds on tags, `brewery_ci.yml:4, 88-91`, but only raw `release-fast`
binaries):

```
on: push tags v*  |  workflow_dispatch (dry_run: true by default)
jobs:
  version     -> check tag == BREWERY_VERSION, set RELEASE_CHANNEL=stable
  daemon      -> gradlew test shadowJar (+ notices)
  deps        -> fetch-bundle-deps per OS (cached by bundle-deps.toml hash), jlink JRE per OS
  windows     -> cargo build --release (bundle-config) brewery + cli + auto_update_helper;
                 bundle-windows; ISCC; install/uninstall smoke; VersionInfo assert
  linux       -> cargo build --release; bundle-linux tarball; nfpm deb+rpm;
                 container install smokes; offline jdtls/daemon smoke; PKGBUILD render + makepkg
  macos       -> continue-on-error; unsigned zip
  publish     -> needs all (macos optional); SHA256SUMS; latest.json; winget YAML;
                 gh release create --draft (tag runs) — owner publishes the draft
```

- `permissions: contents: write` only on `publish`.
- Dry run (`workflow_dispatch`) does everything except `gh release create`, and
  uploads the files as workflow artifacts — this is how packaging is verified
  before the first tag.
- Build profile: `release` with `.cargo/bundle-config.toml` (as Zed) vs
  `release-fast` used today. Full `release` + `codegen-units=1` for `zed` may
  exceed hosted-runner limits (today's `build-windows` already has
  `timeout-minutes: 240`, `CARGO_PROFILE_RELEASE_FAST_DEBUG: limited`). Start
  with `release-fast` + `strip`, measure, then decide.
- Signing: none. Keep a single `sign` step stub gated on a secret for SignPath.
- Checksums: `SHA256SUMS` over all assets; `latest.json` carries per-asset
  SHA-256 for the updater.
- Release notes: from a `CHANGELOG.md` section or written by hand in the draft.
  Zed's `script/draft-release-notes`, `create-draft-release`, `upload-nightly*`,
  `trigger-release` target Zed's infrastructure: not reused.

## 10. Reuse vs new

| Zed file | Verdict |
|---|---|
| `crates/windows_resources` | reuse, edit names (hook-in) |
| `crates/zed/resources/windows/zed.iss` | copy → `brewery.iss`, heavy rename, drop appx / URL protocol / Terms |
| `crates/zed/resources/windows/messages/*.isl` | reuse `en.isl` |
| `crates/zed/resources/windows/sign.ps1` | not used (Azure Trusted Signing for Zed); later SignPath |
| `crates/zed/resources/windows/zed.sh` | copy → `brewery.sh` |
| `crates/explorer_command_injector` + AppxManifests | not used (needs signing, Zed identity/CLSIDs) |
| `script/bundle-windows.ps1` | copy → `script/brewery/bundle-windows.ps1`, keep CheckEnv/Conpty/CollectFiles/BuildInstaller skeleton |
| `script/bundle-linux` | copy → `script/brewery/bundle-linux`, keep link/strip/find_libs/desktop |
| `script/bundle-mac` | small new script; reuse `cargo bundle` metadata only |
| `script/install.sh`, `uninstall.sh`, `install-linux` | not used (curl-installer from zed.dev) |
| `script/flatpak/`, `crates/zed/resources/flatpak/`, `snap/` | not used (decision: no Flatpak/AppImage) |
| `script/generate-licenses` (+ `script/licenses/`) | reuse as is |
| `script/determine-release-channel` | reuse the idea; Brewery version check inline in the workflow |
| `crates/auto_update` | reuse; one hook for the GitHub manifest, Windows filenames, release notes URL |
| `crates/auto_update_helper` | reuse; rename files, add bundled directories |
| `crates/auto_update_ui` | reuse; gate Zed announcements |
| `crates/cli` | reuse; rename lookups and strings |
| nFPM config, AUR PKGBUILD, winget manifests, `latest.json`, bundle-deps fetcher, jlink step, jdtls launcher, extension seeding, settings overlay, release workflow | **new** |

## 11. Risks

1. **Windows auto-update with bundled JVM pieces.** Locked jars/DLLs if the
   daemon or jdtls outlive the app; helper timeout is 2 s per job
   (`updater.rs:388`). Needs a test on a real install, not only unit tests.
2. **Offline jdtls via extension settings** depends on the extension's
   undocumented launch contract (`jdtls_launcher` args, proxy, debugger). Pin the
   extension version; read its source before implementing; keep the
   "pre-populate managed layout" fallback.
3. **Version split** (Brewery 0.1.x vs Zed 1.25.0 `AppVersion`): every place that
   shows or compares a version must use the right one; miss one and the updater
   loops or About lies.
4. **CI time/disk.** Three full release builds + jlink + packaging per tag;
   `release` profile with `codegen-units=1` may not fit in 240 min on hosted
   Windows runners.
5. **Unsigned Windows installer**: SmartScreen warnings, possible Defender
   false positives, winget validation friction. Apply to SignPath now; it is
   out of band and slow.
6. **Name collisions** with Zed on the same machine: mutex and app ids are
   already distinct; still to do: URL protocol `zed://`, CLI socket name,
   desktop file `x-scheme-handler/zed`, install directory, Start menu name.
7. **Licence completeness**: shipping a JRE (GPLv2+CE) and jdtls (EPL) means
   source offers/links must be in the package; Zed's Terms of Service must not
   be shown.
8. **Public actions** (GitHub Release publish, winget PR, AUR push) are the
   owner's; the workflow prepares drafts/artifacts only.

## 12. Proposed split

Each task is one implementer's unit. Order: M6a → (M6b, M6c, M6d in parallel)
→ M6e → M6f.

### M6a — Identity and plumbing (Rust, unit-tested)

| Task | Files | Interface / change | Verification |
|---|---|---|---|
| A1 Brewery version | `crates/zed/BREWERY_VERSION`, `brewery_core` (const + build env), `zed/src/zed.rs:1568` About, `windows_resources.rs:62-74` | `brewery_core::BREWERY_VERSION: &str` | unit test parses as semver; About shows it |
| A2 Exe metadata | `windows_resources.rs:46-51, 99-104` | product names = display names | Windows CI: `VersionInfo` assert step |
| A3 CLI/app names | `cli/src/main.rs:51, 909-913, 949, 1277-1279, 928, 1205, 1378`; `util/src/util.rs:365-376`; `zed/src/main.rs:1711`; `install_cli_binary.rs:28` | candidate path consts | unit tests on consts; `cargo build -p cli -p zed` dev run finds `./brewery` |
| A4 Bundle root + JRE/jar lookup | new `brewery_bundle` leaf crate (or `brewery_daemon_client::bundle`), `jdk.rs:110-240` | `bundle_root()`, `bundled_jre_home()`, `bundled_daemon_jar()` … with `from_exe` | temp-dir unit tests: libexec parent, Windows flat, bundled JRE beats JAVA_HOME |

### M6b — Auto-update from GitHub Releases (Rust)

| Task | Files | Change | Verification |
|---|---|---|---|
| B1 Manifest client | `auto_update.rs:677-738` hook + new `brewery_update` module | `latest.json` schema, `ReleaseAsset { version, url, sha256 }`, SHA-256 check | `FakeHttpClient` tests (manifest, key mapping, missing asset, bad hash); fix `test_auto_update_downloads` |
| B2 Enable polling | `release_channel/src/lib.rs:201-205`, `default.json:1740`, `brewery_defaults.rs:18`, `settings_store.rs:1866-1960` tests, `auto_update.rs:278` version source, `:345-364` notes URL, `auto_update_ui.rs:204-226` | Stable+Windows polls; Linux/macOS explanation/notify | updated guard tests |
| B3 Windows helper | `auto_update_helper/src/updater.rs:166-285, 367`, `auto_update_helper.rs:11,60,126`, `auto_update.rs:923-930, 1299-1360` | Brewery file names + bundled dirs | `test_perform_update` with the new tree; manual update test via `BREWERY_UPDATE_MANIFEST_URL` override |

### M6c — Offline bundle (scripts + small Rust)

| Task | Files | Change | Verification |
|---|---|---|---|
| C1 Pinned deps + fetcher | `script/brewery/bundle-deps.toml`, `script/brewery/fetch_bundle_deps.py` | download + SHA-256 verify jdtls, lombok, java-debug, java-lsp-proxy, Java extension, Temurin JDK; lay out `bundle/` | script self-test in CI (checksum mismatch fails) |
| C2 jlink JRE | same script | module list, `jre/` with `legal/` | `jre/bin/java -version`; daemon e2e test with bundled JRE |
| C3 jdtls launcher | `script/brewery/jdtls/brewery-jdtls{,.cmd}` | equinox launch with bundled JRE, writable config area | headless `initialize` against a fixture project, offline (Linux `--network none`) |
| C4 Extension seed + settings overlay | `zed/src/main.rs` hook, Brewery module (in `brewery_core` or `brewery_bundle`) | copy `extensions/java` if absent; computed defaults for `lsp.jdtls.settings.*` | unit tests: overlay empty without bundle, paths with bundle; seed idempotent |

### M6d — Packages (scripts, CI-verified)

| Task | Files | Verification |
|---|---|---|
| D1 Windows installer | `crates/zed/resources/windows/brewery.iss`, `brewery.sh`, `script/brewery/bundle-windows.ps1` | ISCC compile; silent install/uninstall smoke; file asserts |
| D2 Linux tarball | `script/brewery/bundle-linux`, `crates/zed/resources/brewery.desktop.in` | tree asserts; `bin/brewery --version` |
| D3 deb/rpm | `packaging/nfpm.yaml`, postinstall scripts | container install on ubuntu:24.04 and fedora:41, `brewery --version` |
| D4 AUR | `packaging/aur/PKGBUILD.in` | `makepkg` in `archlinux:base-devel` |
| D5 winget | `packaging/winget/*.yaml.in` | `winget validate` on the Windows runner |
| D6 macOS zip | `crates/zed/Cargo.toml:294-327` metadata, `script/brewery/bundle-mac` | zip contains `Brewery.app`, CFBundleIdentifier asserted; job allowed to fail |

### M6e — Release workflow

| Task | Files | Verification |
|---|---|---|
| E1 `brewery_release.yml` | `.github/workflows/brewery_release.yml` (jobs in section 9), `SHA256SUMS`, `latest.json` generation | `workflow_dispatch` dry run green, artifacts downloadable; then a `v0.1.0-rc.1` tag producing a draft release |

### M6f — Docs, licences, public release

| Task | Files | Verification |
|---|---|---|
| F1 README + NOTICE | `README.md`, `NOTICE`, `licenses/` assembly in bundle scripts | grep test: README contains both disclaimers; packages contain `licenses/` (asserted in D1-D4 smokes) |
| F2 Release checklist | `docs/release-checklist.md` (spec §9 petclinic run on the installed build, offline first start, update from previous version) | manual, by the owner |
| F3 Publish | owner publishes the draft, submits winget, pushes AUR | owner actions |
