# Brewery M2a — Java Language Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox syntax.

**Goal:** Java works out of the box: syntax highlighting, outline, brackets, `.gradle.kts`/`.properties` highlighting, and the Eclipse JDT Language Server (completion, diagnostics, navigation, refactorings) plus java-debug — without the user installing anything.

**Architecture:** Brewery auto-installs Zed's open-source Java extension (`zed-extensions/java`, Apache 2.0). It ships the tree-sitter grammars and queries, launches jdtls, provides the java-debug adapter and Lombok support, and can download a JDK when none is present. Brewery only changes defaults. Ruling (spec deviation, recorded): the spec's "bundled jdtls and JRE" becomes "auto-installed extension that downloads pinned jdtls" for M2; fully offline bundling (vendored extension + jdtls + jlink JRE) moves to M6 packaging, where the installer can carry them.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (sections 5, 11 M2).

## Global Constraints
- Edits in Zed files carry `brewery:` markers; LF only.
- Extension gallery contact is already documented in README.

### Task 1: Java extension and JDK download by default

**Files:** `assets/settings/default.json` (`auto_install_extensions` ~line 2365, `lsp` ~line 2764), `crates/settings/src/brewery_defaults.rs`.

- [ ] **Step 1: Failing test** in `brewery_defaults.rs`:
```rust
#[test]
fn test_java_support_is_preinstalled() {
    let d = defaults();
    assert_eq!(d["auto_install_extensions"]["java"], Value::Bool(true));
    assert_eq!(d["lsp"]["jdtls"]["settings"]["jdk_auto_download"], Value::Bool(true));
}
```
- [ ] **Step 2:** add to `auto_install_extensions`:
```jsonc
    // brewery: Java support out of the box (zed-extensions/java, Apache 2.0)
    "java": true,
```
and inside `"lsp": {` before the commented examples:
```jsonc
    // brewery: let the Java extension download a JDK 21+ for jdtls when none is installed
    "jdtls": {
      "settings": {
        "jdk_auto_download": true,
      },
    },
```
- [ ] **Step 3:** `cargo test -p settings` → PASS.
- [ ] **Step 4:** Commit `feat: Java extension and JDK download by default`.

### Task 2: On-screen check (controller)
- [ ] Clean trusted profile, open the demo Spring project; wait for the extension to install and jdtls to start; check Java highlighting, outline (Structure), hover/diagnostics on a broken line, and that `build.gradle.kts` and `application.yml` highlight.

### Task 3: Retry extension downloads (found on screen: jdtls download reset mid-transfer)

The Java extension downloads jdtls (~51 MB) through the extension host's `download_file`; one connection reset ("An existing connection was forcibly closed by the remote host", os error 10054) leaves Java without a language server, and Zed does not retry.

**Files:** `crates/extension_host/src/wasm_host/wit/since_v0_8_0.rs` (`download_file`, ~line 1076); if other `since_v0_*` versions share a helper, prefer changing the helper once.

- [ ] Wrap the request + write/extract part of `download_file` in a retry loop: up to 3 attempts, backoff 1 s then 3 s (`self.host.executor` timer or `smol::Timer`), retrying only on transport errors and 5xx statuses (not 4xx, not capability errors). Before each retry remove any partial destination (`fs.remove_file`/`remove_dir` with `ignore_if_not_exists`). Log each retry at `warn` with the URL and attempt number. Marker `// brewery: retry flaky downloads (jdtls is ~50 MB)`.
- [ ] Test: in the extension_host test module (find how existing wasm host tests build a host with a `FakeHttpClient`), a fake client that fails the first request with an IO error and succeeds the second must lead to a successful download; a 404 must fail without retry. If building the wasm host in a test is impractical, extract the retry policy into a small pure function (`fn should_retry(attempt: usize, error: &DownloadError) -> Option<Duration>`) next to `download_file` and unit-test that, and report the gap.
- [ ] `cargo test -p extension_host`, `cargo clippy -p extension_host -- -D warnings`. Commit `fix: retry extension file downloads`.

### Task 4: Visible border between the editor and docked panels (user report)

In Brewery Dark the `border` colour equals the editor and terminal background (`#1e1f22`), so the line between the editor and the bottom terminal panel disappears.

**Files:** `script/brewery/gen_themes.py`, regenerated `assets/themes/brewery/brewery.json`, `crates/brewery_theme/src/brewery_theme.rs` (test).

- [ ] Test first (brewery_theme): for both themes, `border` must differ from `editor.background`, `terminal.background` and `panel.background` (compare the hex strings).
- [ ] Dark: `border` → `#393b40ff` (IntelliJ separator grey); keep `border.variant` `#393b40ff`. Light: `border` stays `#ebecf0ff` (already distinct from `#ffffff`/`#f7f8fa`) — the test proves it.
- [ ] Regenerate with `python script/brewery/gen_themes.py`; `cargo test -p brewery_theme`. Commit `fix: visible panel borders in Brewery Dark`.
- [ ] Controller checks the editor/terminal separator on screen.
