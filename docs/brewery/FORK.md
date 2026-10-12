# Working on the Brewery fork

## Rules

1. Brewery code lives in `crates/brewery_*` and `script/brewery/`.
2. Inside files that came from Zed, keep edits to short hook-ins and mark each
   with `brewery:` (`// brewery:` in Rust/JSONC, `# brewery:` in TOML/YAML/shell).
   `script/brewery/list-hooks` prints them all.
3. Never rename or delete Zed crates. Switch features off instead
   (`brewery_cloud::ZED_CLOUD_ENABLED`, default settings, command palette filter).
4. Zed's GitHub workflows are deleted; Brewery's CI is `.github/workflows/brewery_ci.yml`.

## Merging upstream (monthly)

    git checkout main
    script/brewery/merge-upstream
    cargo test -p brewery_cloud -p brewery_core -p brewery_theme -p release_channel -p paths -p settings -p theme_settings -p client -p brewery_run_widget -p brewery_search_everywhere -p file_finder -p brewery_daemon_client -p brewery_spring_config -p brewery_build -p brewery_services -p brewery_db -p extension_host -p util -p extension_suggest -- --skip test_extension_store_with_test_extension
    cargo test -p workspace --lib
    script/brewery/list-hooks   # every hook still present?

(The `--skip` drops `test_extension_store_with_test_extension`: it builds a WASM test extension and downloads toolchains, so it times out on hosted CI runners. Run it locally.)

Shift Shift (Search Everywhere) is bound only in the JetBrains base keymap.

The script resolves Zed workflow changes (kept deleted) and `README.md` (ours).
Everything else it leaves to you and lists.

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
