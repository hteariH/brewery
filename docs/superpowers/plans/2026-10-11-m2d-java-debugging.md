# Brewery M2d — Zero-config Java Debugging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Debugging a Java `main` (and, where java-debug supports it, a test) works like IntelliJ: from the gutter ▶ menu, "Debug <label>", with no `.zed/debug.json`.

**Architecture:** The Java extension provides the `Java` debug adapter (java-debug via jdtls) but no locator, so Zed offers no debug entry for its runnables. Brewery adds a `JavaLocator` (`crates/project/src/debugger/locators/brewery_java.rs`, new Brewery file inside a Zed crate) implementing `dap::DapLocator`: for task templates tagged `java-main` and adapter `Java` it returns a `DebugScenario { adapter: "Java", label: "Debug <resolved label>", config: { "request": "launch", "mainClass": <fully-qualified class>, "cwd": "$ZED_WORKTREE_ROOT" }, build: None }`. Registered next to the other locators in `crates/project/src/debugger/dap_store.rs` (~line 126, one marked line, plus a marked `mod` line in `locators.rs`/`mod.rs`).

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (M2: debugging).

## Global Constraints
- Zed-file edits marked `brewery:`; LF; no unwrap/expect outside tests.

### Task 1: JavaLocator
- [ ] Read the existing locators (`cargo.rs`, `go.rs`, `python.rs`, `node.rs`) and `DapStore::debug_scenario_for_build_task` to learn: how task variables like `$ZED_CUSTOM_java_package_name` / `$ZED_CUSTOM_java_class_name` reach a scenario (are scenario config strings substituted with the task context when the session starts? check `debugger_ui` session start / `DebugScenario` resolution); whether `build_config.tags` is available in `create_scenario`.
- [ ] Fully-qualified class: the Java extension's runnables set `java_package_name` (optional) and `java_class_name`. If scenario strings are substituted with the task context, use `mainClass: "${ZED_CUSTOM_java_package_name:}.$ZED_CUSTOM_java_class_name"` only if the substitution syntax supports defaults and the leading `.` problem for the default package can be avoided; otherwise resolve the FQN inside the locator from `resolved_label`/template fields, or omit `mainClass` and pass `"projectName"`/nothing so java-debug auto-resolves (document the trade-off). Choose the simplest correct option and explain it.
- [ ] Tests (unit, in the new file): `create_scenario` returns None for other adapters and for templates without the `java-main` tag; returns a launch scenario with adapter `Java` and the expected config for a `java-main` template (package and default-package cases).
- [ ] Register the locator; `cargo test -p project brewery_java`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: debug Java main classes from the gutter without debug.json`.

### Task 2: On-screen check (controller, release-fast build or user)
- [ ] Gutter ▶ menu on `main` shows "Debug …"; it starts a session; a breakpoint in `main` is hit.
