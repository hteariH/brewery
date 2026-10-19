# Working on the Brewery fork

## Rules

1. Brewery code lives in `crates/brewery_*` and `script/brewery/`.
2. Inside files that came from Zed, keep edits to short hook-ins and mark each
   with `brewery:` (`// brewery:` in Rust/JSONC, `# brewery:` in TOML/YAML/shell).
   `script/brewery/list-hooks` prints them all.
3. Never rename or delete Zed crates. Switch features off instead
   (`brewery_cloud::ZED_CLOUD_ENABLED`, default settings, command palette filter).
4. Zed's GitHub workflows are deleted; Brewery's CI is `.github/workflows/brewery_ci.yml`.
   It runs on every push. A branch push runs only what the changed paths need: the Linux
   Rust check, the daemon tests, or nothing for docs-only changes. A newer push cancels the
   older run. Pushes to main also run the Windows check and build `brewery.exe`. On a
   branch, put `[win]` in the commit message for the same, or start the workflow manually.
   A manual run does everything; its "all_platforms" input adds the Linux and macOS
   release builds, which otherwise run only on `v*` tags. Rust caches are saved from main
   and tags only, and branches reuse main's caches. The Windows check and build get their
   compiled crates (workspace crates too) from sccache over the GitHub Actions cache, written
   from main only and read everywhere; there rust-cache keeps only the registry, and
   `/Brepro` makes proc-macro DLLs reproducible, without which sccache misses every crate
   using one. Linux keeps rust-cache's compiled dependencies: the 10 GB cache budget holds
   no more. The Windows jobs build on a Dev Drive (`script/brewery/setup-dev-drive.ps1`;
   `target` is a junction to it).

## Merging upstream (monthly)

    git checkout main
    script/brewery/merge-upstream
    cargo test -p brewery_cloud -p brewery_core -p brewery_theme -p release_channel -p paths -p settings -p theme_settings -p client -p brewery_run_widget -p brewery_search_everywhere -p file_finder -p brewery_daemon_client -p brewery_spring_config -p brewery_build -p brewery_services -p brewery_db -p brewery_git -p brewery_bundle -p cli -p auto_update -p auto_update_ui -p extension_host -p util -p extension_suggest -p diagnostics -- --skip test_extension_store_with_test_extension --skip test_kill_terminates_grandchildren --skip test_drop_terminates_grandchildren
    cargo test -p workspace --lib
    script/brewery/list-hooks   # every hook still present?

(The `--skip` drops `test_extension_store_with_test_extension`: it builds a WASM test extension and downloads toolchains, so it times out on hosted CI runners. The two `*_terminates_grandchildren` tests in `util` wait 5 s for PowerShell to start a process, which cold Windows runners often miss. Run them locally.)

Shift Shift (Search Everywhere) is bound only in the JetBrains base keymap.

The script resolves Zed workflow changes (kept deleted), Brewery's own GitHub and
community files (`README.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, the issue,
discussion and PR templates, `FUNDING.yml`: ours) and keeps Zed's removed GitHub files
removed. Everything else it leaves to you and lists. Last merge: upstream `56934ec056`
(2026-10-17).

When testing a second checkout (a worktree) with a shared `CARGO_TARGET_DIR`, debug
test binaries load assets such as `assets/settings/default.json` from the checkout
that built them first, not from the one under test. Add `--features util/debug-embed`
to embed the tested checkout's assets, or use a separate target directory.

## Log scale check

`brewery_git` has an ignored test that generates a 100k-commit repository with
`git fast-import` in a temp dir and times the Log tool window on it: `git log`, Zed's graph
loader, lane assignment, memory, and redraws scrolled to the top, middle and end, before and
after `git commit-graph write --reachable`. Run it optimised, as debug numbers are several
times slower. Turning off debug info keeps that build quick and small:

    CARGO_PROFILE_RELEASE_FAST_DEBUG=0 cargo test --profile release-fast -p brewery_git -- --ignored log_scale --nocapture

`BREWERY_LOG_SCALE_COMMITS=200000` changes the size. The last results are under
"Measurements" in `docs/superpowers/plans/2026-10-13-m5b-log-tool-window.md`.

## Daemon (`brewery-daemon/`)

Kotlin/JVM, Apache 2.0. Needs JDK 21 (`JAVA_HOME`).

    cd brewery-daemon
    ./gradlew test        # unit tests
    ./gradlew shadowJar   # build/libs/brewery-daemon-all.jar

Set `BREWERY_DAEMON_JAR` to that jar to run the `brewery_daemon_client`
integration test (it skips when unset, as in CI's Rust job).

## Regenerating look assets

    python script/brewery/gen_themes.py            # after Zed changes assets/themes/one/one.json
    python script/brewery/fetch_intellij_icons.py  # after Zed adds icon keys

## Brewery version and Zed version

Brewery has its own version in `crates/zed/BREWERY_VERSION` (read through
`brewery_core::BREWERY_VERSION`). It is shown in About, written into the Windows exe
metadata and used for packages and auto-update. Zed's own version (the `zed` crate's
`CARGO_PKG_VERSION`) stays as it is: the SSH remote server, extensions and the LSP
client info depend on it.

A pre-release such as `0.1.0-rc.1` keeps the full version wherever a string is allowed
(About, `--version`, file names, the Windows FileVersion/ProductVersion strings, Inno
`AppVersion`, winget `PackageVersion`, `latest.json`). Fields that must be numeric get
`0.1.0`: the Windows binary FILEVERSION (`0.1.0.0`), Inno `VersionInfoVersion` and the macOS
`CFBundleShortVersionString`/`CFBundleVersion` (the full version is in the `BreweryVersion`
Info.plist key, which `cli --version` reads). Package managers get a version that sorts
before the final release: `0.1.0~rc.1` in the deb and rpm (nFPM), `0.1.0rc.1` as the AUR
`pkgver`.

## Bundle layout

`brewery_bundle` finds the files shipped next to the app: `jre/`, `brewery-daemon/
brewery-daemon.jar`, `jdtls/` and `extensions/<id>`. The root is the exe's folder on
Windows, the parent of `libexec/` on Linux (`/opt/brewery/libexec/brewery-editor`) and
`Contents/Resources` in a macOS app. A bundled JRE is used for the daemon before
`JAVA_HOME` and `PATH`.

## Offline bundle

Everything Java support needs, so a packaged Brewery works on a fresh machine without
downloads. One target is laid out in `target/bundle/<os>-<arch>/`:

    jre/                          jlinked Temurin 21 for the daemon and jdtls (keeps legal/)
    jdtls/                        Eclipse JDT LS + brewery-jdtls (sh) or brewery-jdtls.cmd
      tools/                      lombok.jar  java-debug.jar  java-lsp-proxy[.exe]
                                  gradle-lsp-bridge[.exe]  gradle-ls/<version>/lib/
    extensions/java/              the Java extension, seeded into installed/ when absent
    brewery-daemon/brewery-daemon.jar
    licenses/                     one directory per component + README.txt index

Everything but `licenses/` lives in the four directories the Windows update helper
swaps (`jre`, `brewery-daemon`, `jdtls`, `extensions`), so an update replaces the
extension's helpers together with jdtls. Build it on the target OS (jlink can't
cross-link), with Python 3.11.4+ (tarfile's data filter) and a JDK 21 for the daemon.
`--daemon-jar` is required; `--no-daemon` builds a bundle without it, which is not
releasable (the updater needs `brewery-daemon/`):

    cd brewery-daemon && ./gradlew -Pversion=$(cat ../crates/zed/BREWERY_VERSION) shadowJar && cd ..
    python script/brewery/fetch_bundle_deps.py --daemon-jar brewery-daemon/build/libs/brewery-daemon-all.jar
    python script/brewery/smoke_bundle.py target/bundle/<os>-<arch>
    python -m unittest discover script/brewery

Downloads are cached in `~/.cache/brewery-bundle` (`--cache`) and checked against their
SHA-256 before anything is unpacked; a mismatch fails the build and leaves the previous
bundle in place. `smoke_bundle.py` starts the daemon on the bundled JRE and jdtls the
way the Java extension does (proxy, launcher, Lombok agent, java-debug) against a copy
of the maven-multi fixture. `--checks basic,gradle` adds a Gradle import (online: the
fixture's wrapper downloads Gradle) that starts jdtls again after "updating" a copy of
it, to catch workspaces that keep the old jdtls's Gradle init script path;
`gradle-old` reads and imports the same build on Gradle 8.4 with a JDK 17 as
`JAVA_HOME` (the hosted runners' `JAVA_HOME_17_*`, or `BREWERY_SMOKE_JDK17`). CI's `bundle`
job does the same for Windows, Linux (inside a network namespace, `--require-offline`;
the Gradle check runs in a separate online step) and macOS (allowed to fail), and uploads
`bundle-<os>-<arch>` artifacts. It runs on tags, manual runs and changes to
`script/brewery/` or `brewery-daemon/`.

Bumping a pin in `script/brewery/bundle-deps.toml`: change `version` and `url`,
download the file, `sha256sum` it, compare with the checksum the publisher lists
(Adoptium API, Eclipse `.sha256`, GitHub release asset digest; the Marketplace lists
none for the Gradle language server VSIX, so compare two downloads) and set `sha256`.
Keep jdtls, Lombok, java-debug, java-lsp-proxy, gradle-lsp-bridge and the Gradle
language server at what the pinned Java extension version downloads itself (its
`src/jdtls.rs`, `debugger.rs`, `proxy.rs`, `gradle_bridge.rs`, `gradle_ls.rs`; the
Gradle language server's `dest` names its version). If a licence changes,
update the matching file in `script/brewery/licenses/` (the upstream LICENSE at the new
tag). The JDK entry also names its GPLv2 corresponding source (`source_code`,
`source_sha256` from the Temurin release's `.sha256.txt`, `source_tag`), which ends up in
`licenses/jre/SOURCE.txt` and, as the archive itself, on every GitHub Release; bump it
together with the JDK; the EPL components (jdtls,
java-debug) name their source tag the same way (`licenses/<component>/SOURCE.txt`).

At startup `brewery_bundle::java_setup` points the extension at the bundle through the
default settings (user settings still win): `jdtls_launcher`, `lombok_jar`,
`java_debug_jar`, `lsp_proxy_path`, `jdk_auto_download = false`, `check_updates =
"once"`, and `auto_update_extensions.java = false`, so the extension stays at the
version the bundled helpers were pinned with and moves with Brewery releases. The
`lsp.jdtls` part is applied only while the installed Java extension's `extension.toml`
version equals the bundled one (checked at startup): after a manual update the
extension uses its own downloads again. On Windows the launcher and Lombok paths are given in 8.3 form when they
contain spaces, because java-lsp-proxy runs batch files through `cmd /C`, which breaks
on a quoted launcher path followed by another quoted argument. On a volume without 8.3
names (Dev Drive/ReFS, `fsutil 8dot3name`) and a spaced install path, Brewery sets
`lombok_support = false` instead, so jdtls still starts (without Lombok). Paths with
`&`, `(` or `^` are not protected either. Things to know:

- `java_home` is not set when the launcher is bundled: the launcher runs jdtls on
  `jre/` itself, and `java_home` would only hand the stripped JRE to jdtls as
  `JAVA_HOME`, the default project runtime. Projects that need a particular JDK set
  `lsp.jdtls.initialization_options.settings.java.configuration.runtimes`.
- Gradle runs on the user's JDK, as `gradlew` does, not on the bundled JRE (Gradle
  before 8.5 cannot run on Java 21). jdtls: the overlay sets
  `lsp.jdtls.initialization_options.settings.java.import.gradle.java.home` to
  `${brewery.gradle.java.home}` (initialization options: jdtls reads them before its
  first import), and the launcher sets that property to `JAVA_HOME` when the
  project's wrapper can run on it, else to "" so jdtls picks a JDK itself. The daemon
  (`GradleJvm`) leaves `org.gradle.java.home` in gradle.properties to Gradle, else
  uses `JAVA_HOME` under the same rule, and fails with what to change when the
  wrapper cannot run on the JVM it ends up with. Both use jdtls's Gradle/Java table.
- In launcher mode the extension does not pass `min_memory` and `max_memory` on, so
  Brewery follows them in the user settings and sets `BREWERY_JDTLS_MIN_MEMORY` /
  `BREWERY_JDTLS_MAX_MEMORY` in the default `lsp.jdtls.binary.env` (Zed adds it to the
  extension's command); the launcher turns them into `-Xms` (else 1G) and `-Xmx`, with
  the extension's rules (a minimum above the maximum is swapped) and invalid sizes
  dropped. Project settings are not followed; there, set the two variables in
  `lsp.jdtls.binary.env` or pass `--jvm-arg=-Xmx4G` through a wrapper script.
- jdtls writes its configuration and workspaces to the user cache
  (`%LOCALAPPDATA%\Brewery\jdtls`, `~/Library/Caches/Brewery/jdtls`,
  `~/.cache/brewery/jdtls`; `BREWERY_JDTLS_CACHE_DIR` overrides), never into the bundle.
  On Windows the launcher starts no process besides java (it used to start PowerShell
  for SHA-1 keys, 0.3-1 s per start): Brewery sets the extension's `data_directory` to
  `<cache>\workspace`, so the extension passes `-data <cache>\workspace\jdtls-<sha1 of
  the project>`, and `BREWERY_JDTLS_CONFIG_KEY` (a hash of the jdtls directory) in the
  default `lsp.jdtls.binary.env` names the configuration area. Run by hand, without
  them, the launcher uses the sanitized paths as keys. `data_directory` is left unset
  when the launcher or the cache path has no cmd-safe form (see Lombok above).
  A workspace (`workspace/jdtls-<hash>` or the extension's `-data`) is deleted and
  rebuilt on the first start after jdtls changes (a Brewery update): Gradle projects
  remember the path of jdtls's init script inside the configuration area
  (`config/<hash>/org.eclipse.osgi/<bundle id>/...`), and the update moves it, which made
  every Gradle synchronization fail with "The specified initialization script ... does
  not exist". The launcher keeps a stamp of the jdtls it last ran in
  `<workspace>/.metadata/brewery-jdtls.stamp`.
- Gradle build files work offline too: the default
  `lsp.gradle-language-server.settings.gradle_bridge_path` is the bundled
  gradle-lsp-bridge, and the bundled Gradle language server (vscode-gradle's `lib/`)
  is copied in the background at startup into the extension's work dir
  (`<extensions>/work/java/gradle-ls/<version>/`), where the extension looks before it
  downloads; the extension has no setting for that path. A copy already there is kept;
  a version dir without `lib/` (a download that failed) is replaced. Both follow the
  same installed-version check as `lsp.jdtls`. The language server itself runs on
  `JAVA_HOME` or `java` on `PATH`, not on the bundled JRE (it would also become the
  JDK Gradle builds with); with `jdk_auto_download = false` a machine without a JDK
  needs `lsp.gradle-language-server.settings.java_home` (release checklist item).
  `smoke_bundle.py` starts it the extension's way, on the bundled JRE, offline.
- Without `java.configuration.runtimes`, run/debug through java-debug may use jdtls's
  default VM, which can be the bundled jlink JRE (release checklist item).

## Packaging

All packaging scripts take already-built binaries and the offline bundle (see "Offline
bundle"); none of them runs cargo. Zed's own `bundle-*` scripts and `zed.iss` stay
untouched for upstream merges.

- Windows installer (Inno Setup 6, per-user):

      powershell -File script/brewery/bundle-windows.ps1 -App target/release-fast/brewery.exe -Cli target/release-fast/cli.exe -Helper target/release-fast/auto_update_helper.exe -Bundle target/bundle/windows-x64

  The script is `crates/zed/resources/windows/brewery.iss`. In update mode
  (`/update=true`) it writes the files the update helper swaps under `{app}\install\`.
- Linux tarball, deb and rpm (`/opt/brewery`, nFPM from `script/brewery/install-nfpm`):

      script/brewery/bundle-linux --app target/release-fast/brewery --cli target/release-fast/cli --bundle target/bundle/linux-x64 --deb --rpm

  `script/brewery/check-linux-tree` asserts the tree; `packaging/nfpm.yaml` and
  `packaging/linux/*.sh` define the packages.
- macOS zip (unsigned, unsupported): `script/brewery/bundle-mac --app ... --cli ... --bundle target/bundle/macos-aarch64`.
- AUR and winget manifests: `script/brewery/render_manifests.py --version <v> --out <dir> --tarball <tar.gz> --installer <setup.exe>` fills `packaging/aur/PKGBUILD.in` and `packaging/winget/*.yaml.in` with versions, URLs and checksums.

CI builds and installs every package on manual runs (`all_platforms`) and releases: silent
install, update mode and uninstall on Windows; deb in `ubuntu:24.04`, rpm in
`fedora:41`; `makepkg -si` in `archlinux:base-devel`; `winget validate`; macOS zip
contents (allowed to fail). Releases are drafted by CI (see "Releasing"); publishing
them, winget and the AUR is done by hand.

## Releasing

A release is a tag. Everything is built by `.github/workflows/brewery_release.yml`, which
calls `brewery_ci.yml` for the `stable` channel, so the packages are the ones CI already
installs and smoke-tests.

1. Set `crates/zed/BREWERY_VERSION` (for example `0.1.0`, or `0.1.0-rc.1`), add a
   `## <version>` section to `CHANGELOG.md` (without one the release gets a generic install
   note), merge to `main`.
2. Dry run: `gh workflow run brewery_release.yml --ref main -R hteariH/brewery`
   (`dry_run` defaults to true). It builds, packages and tests everything, and uploads the
   release files as the `release-files` artifact (`assets/`, `winget/`, `aur/` and
   `release-notes.md`, the would-be body, also in the run summary). Nothing is created. Takes 1 to 2 hours.
3. Tag and push: `git tag v0.1.0 && git push origin v0.1.0`. The tag must equal `v` plus
   `BREWERY_VERSION` and its commit must be on `main`, or the `version` job fails. Try the
   tag path with a pre-release first (`BREWERY_VERSION` `0.1.0-rc.1`, tag `v0.1.0-rc.1`),
   check the draft and delete it and the tag. A hyphenated version has not been through the
   Windows version assert and the deb/rpm smoke tests yet. If a release for the tag exists,
   `publish` fails: delete the draft (and fix a moved tag) before re-running. A pre-release tag (`v0.1.0-rc.1`) works
   when the file says `0.1.0-rc.1` too; it creates a pre-release draft.
4. The `publish` job creates a **draft** release with the packages, `SHA256SUMS`,
   `latest.json` (what the updater reads), the bundled Temurin JRE's source archive
   (`OpenJDK21U-jdk-sources_*.tar.gz`, GPLv2 section 3(a): downloaded in the `assemble`
   job (`release_assets.py fetch-jdk-source`) from the pin in `bundle-deps.toml`, `source_code` + `source_sha256`, and refused on a
   checksum mismatch), `jre-source.txt` (where that archive comes from) and zips of the winget manifests and the AUR
   `PKGBUILD` + `.SRCINFO`. The macOS zip is included when its job passed.
5. Check the draft, then publish it by hand. Publishing it is what makes
   `releases/latest/download/latest.json` serve the new version, so installed Windows
   copies start offering it. Submitting the winget manifests and pushing to the AUR are
   manual too.

The workflow never publishes the release and never touches winget or the AUR. Only the
`publish` job has `contents: write`. The channel is selected by writing it into
`crates/zed/RELEASE_CHANNEL` inside each CI job; the file in the repository stays `dev`.
`script/brewery/release_assets.py` (tests: `python -m unittest discover script/brewery`)
checks the tag, checksums the files, writes `latest.json` and checks it with a Python
mirror of the updater's rules in `crates/auto_update/src/brewery_update.rs`: change the
two together. Tag pushes are no longer built by `brewery_ci.yml` itself.

## Updates

Brewery checks `https://github.com/hteariH/brewery/releases/latest/download/latest.json`
(Stable and Preview builds only; Dev and Nightly never poll). The manifest:

    {"version": "0.1.1",
     "notes_url": "https://github.com/hteariH/brewery/releases/tag/v0.1.1",
     "assets": {"windows-x86_64": {"url": "...setup.exe", "sha256": "..."},
                "linux-x86_64": {...}, "macos-aarch64": {...}}}

On Windows the installer is downloaded, verified against `sha256`, run in update mode,
and `tools\auto_update_helper.exe` swaps the files on quit, including the `jre`,
`brewery-daemon`, `jdtls` and `extensions` directories. The helper waits up to 5 s for
processes started from those directories and then stops them. On Linux and macOS
Brewery only shows a notification with a link to the release notes; packages update
through the package manager. Preview builds poll the same (Stable) manifest but only
notify, on every OS: the Stable installer has another AppId and install directory, so
a Preview build never runs it. Give Preview its own manifest before publishing
Preview releases that should update each other. Don't set `ZED_UPDATE_EXPLANATION` in builds: it turns
the check off completely. To test updates, set `BREWERY_UPDATE_MANIFEST_URL` to an
HTTPS URL of a test manifest (for example a file in a public test release or a gist);
draft release files are not public, so they don't work. Brewery logs when the
override is active. Anyone who can set that variable can point Brewery at another
installer, so treat it like any other environment variable that changes what runs.
On Windows an installer without `sha256` in the manifest is refused.
