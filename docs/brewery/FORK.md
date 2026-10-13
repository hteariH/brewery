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
   and tags only, and branches reuse main's caches.

## Merging upstream (monthly)

    git checkout main
    script/brewery/merge-upstream
    cargo test -p brewery_cloud -p brewery_core -p brewery_theme -p release_channel -p paths -p settings -p theme_settings -p client -p brewery_run_widget -p brewery_search_everywhere -p file_finder -p brewery_daemon_client -p brewery_spring_config -p brewery_build -p brewery_services -p brewery_db -p brewery_git -p brewery_bundle -p cli -p auto_update -p auto_update_ui -p extension_host -p util -p extension_suggest -- --skip test_extension_store_with_test_extension --skip test_kill_terminates_grandchildren --skip test_drop_terminates_grandchildren
    cargo test -p workspace --lib
    script/brewery/list-hooks   # every hook still present?

(The `--skip` drops `test_extension_store_with_test_extension`: it builds a WASM test extension and downloads toolchains, so it times out on hosted CI runners. The two `*_terminates_grandchildren` tests in `util` wait 5 s for PowerShell to start a process, which cold Windows runners often miss. Run them locally.)

Shift Shift (Search Everywhere) is bound only in the JetBrains base keymap.

The script resolves Zed workflow changes (kept deleted) and `README.md` (ours).
Everything else it leaves to you and lists.

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

## Bundle layout

`brewery_bundle` finds the files shipped next to the app: `jre/`, `brewery-daemon/
brewery-daemon.jar`, `jdtls/` and `extensions/<id>`. The root is the exe's folder on
Windows, the parent of `libexec/` on Linux (`/opt/brewery/libexec/brewery-editor`) and
`Contents/Resources` in a macOS app. A bundled JRE is used for the daemon before
`JAVA_HOME` and `PATH`.

## Updates

Brewery checks `https://github.com/hteariH/brewery/releases/latest/download/latest.json`
(Stable and Preview builds only; Dev and Nightly never poll). The manifest:

    {"version": "0.1.1",
     "notes_url": "https://github.com/hteariH/brewery/releases/tag/v0.1.1",
     "assets": {"windows-x86_64": {"url": "...setup.exe", "sha256": "..."},
                "linux-x86_64": {...}, "macos-aarch64": {...}}}

On Windows the installer is downloaded, verified against `sha256`, run in update mode,
and `toolsuto_update_helper.exe` swaps the files on quit, including the `jre`,
`brewery-daemon`, `jdtls` and `extensions` directories. The helper waits up to 5 s for
processes started from those directories and then stops them. On Linux and macOS
Brewery only shows a notification with a link to the release notes; packages update
through the package manager. Don't set `ZED_UPDATE_EXPLANATION` in builds: it turns
the check off completely. To test updates, set `BREWERY_UPDATE_MANIFEST_URL` to an
HTTPS URL of a test manifest (for example a file in a public test release or a gist);
draft release files are not public, so they don't work. Brewery logs when the
override is active. Anyone who can set that variable can point Brewery at another
installer, so treat it like any other environment variable that changes what runs.
On Windows an installer without `sha256` in the manifest is refused.
