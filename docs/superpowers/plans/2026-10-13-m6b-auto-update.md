# Brewery M6b — Auto-Update from GitHub Releases Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Brewery updates itself on Windows from GitHub Releases (download the new installer, apply on quit), and on Linux/macOS tells the user a newer version exists with a link to the release page; nothing talks to Zed's update servers for the app itself.

**Architecture:** Gap analysis `docs/superpowers/plans/m6-gap-analysis.md` section 5 and "M6b". A `latest.json` manifest is published as a release asset (M6e) and fetched from `https://github.com/hteariH/brewery/releases/latest/download/latest.json` (no token, no API rate limit, drafts and pre-releases skipped). A Brewery module parses it; Zed's `auto_update::get_release_asset` gets a `// brewery:` branch for the app asset only (`asset == "zed"`), keeping the remote-server path on Zed's cloud for SSH. Polling is turned on for Stable/Preview on Windows; Linux/macOS show a notification only. The Windows update helper moves Brewery's files and bundled directories.

**Tech Stack:** Rust; Zed `auto_update`, `auto_update_helper`, `auto_update_ui`, `release_channel`, `settings`, `http_client` crates.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (release/auto-update, M6).

## Global Constraints

- Manifest schema (`brewery_update` module; fixture `crates/auto_update/test-data/brewery-latest.json` or inline): `{"version": "0.1.1", "notes_url": "https://github.com/hteariH/brewery/releases/tag/v0.1.1", "assets": {"windows-x86_64": {"url": "...Brewery-0.1.1-x86_64-setup.exe", "sha256": "<hex>"}, "linux-x86_64": {...}, "linux-aarch64": {...}, "macos-aarch64": {...}, "macos-x86_64": {...}}}`. Unknown keys ignored; missing key for the current OS/arch → "no update available" (not an error); `sha256` optional but verified after download when present (mismatch → error, file deleted).
- Manifest URL override for testing: env `BREWERY_UPDATE_MANIFEST_URL` (read once at startup; documented in FORK.md).
- Version compared against `brewery_core::BREWERY_VERSION` (semver), not Zed's `AppVersion`; pre-release manifests never offered to a release build.
- `poll_for_updates()`: Stable and Preview on Windows → true; Dev and Nightly → false; Linux/macOS → polling allowed but the "install" step replaced by a notification "Brewery <v> is available" with buttons "Release notes" (opens notes URL) and "Dismiss"; Linux builds keep `ZED_UPDATE_EXPLANATION`-style text "Update Brewery with your package manager or from GitHub Releases". `auto_update` defaults to `true` (it only polls on Stable/Preview), update the guard tests in `crates/settings/src/brewery_defaults.rs` and the settings store tests. Telemetry/metrics query parameters never sent.
- Release notes URL → `https://github.com/hteariH/brewery/releases/tag/v<version>`; Zed's unrelated announcement in `auto_update_ui.rs` (delta.dev) never fires (gated `// brewery:`).
- Windows install: downloaded file `Brewery-setup.exe` run with `/verysilent /update=true`; the helper's job list moves `Brewery.exe`, `bin\brewery.exe`, `bin\brewery` (shell shim), conpty/OpenConsole as today, and the directories `jre`, `brewery-daemon`, `jdtls`, `extensions` (directory rename on the same volume; rollback per job as today); helper strings say Brewery. Before the helper runs, the app's child processes (daemon JVM, jdtls) must be gone: verify the daemon is killed on quit and jdtls exits with the app; if not, add a shutdown step.
- Zed-file edits marked `// brewery:`, as small as possible; SSH remote-server asset path unchanged.
- Tests: `FakeHttpClient` tests for manifest fetch/parse/key mapping/missing asset/bad hash/older/same/newer version; the updated `test_auto_update_downloads`; helper `test_perform_update` with the Brewery tree including directories.

### Task 1: Manifest client
**Files:** `crates/auto_update/src/{auto_update.rs, brewery_update.rs}`, tests, fixture.
- [ ] TDD per Global Constraints; hook `get_release_asset` for the app asset; SHA-256 check after download.
- [ ] `cargo test -p auto_update`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: updates from GitHub Releases`.

### Task 2: Enable polling and notify
**Files:** `crates/release_channel/src/lib.rs` (+ test), `assets/settings/default.json`, `crates/settings/src/brewery_defaults.rs`, settings store tests, `crates/auto_update/src/auto_update.rs` (version source, notes URL, Linux/macOS notify path), `crates/auto_update_ui/src/auto_update_ui.rs`.
- [ ] TDD: polling matrix per channel/OS; default setting; version source is BREWERY_VERSION; notes URL; announcement gated; Linux/macOS newer manifest → notification state instead of download (unit-test the decision fn).
- [ ] Commit `feat: check for Brewery updates`.

### Task 3: Windows update helper
**Files:** `crates/auto_update_helper/src/{updater.rs, auto_update_helper.rs}`, `crates/auto_update/src/auto_update.rs` (installer file name), app quit path if child processes need stopping.
- [ ] TDD: `test_perform_update` with a temp tree containing the Brewery exe names and the four directories, including rollback when a job fails; strings.
- [ ] Commit `feat: Windows update helper for Brewery`.

### Task 4: CI and docs
- [ ] CI hook-in lists: `-p auto_update -p release_channel` (+ `-p auto_update_helper` on Windows if its tests run there). FORK.md: manifest format, URL override, how updates work per OS. README: "Updates" paragraph.
- [ ] Commit `docs: updates`.
