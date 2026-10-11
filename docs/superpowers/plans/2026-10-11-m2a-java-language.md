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
