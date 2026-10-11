# Brewery M1b — Window Chrome Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Brewery's main window is laid out like IntelliJ New UI: tool-window stripes on the left and right edges instead of panel buttons in the status bar, project/structure/commit tool windows on the left, a Run/Debug widget in the title bar, and Search Everywhere on Shift Shift.

**Architecture:** Stripe rendering needs `Dock` internals, so it lives in a new file inside Zed's `workspace` crate (`crates/workspace/src/dock/brewery_stripes.rs`, a child module of `dock`, which can see `Dock`'s private fields); Zed files get only marked hook-ins (one `mod` line, the workspace layout wrapper, status-bar registration). The Run widget and Search Everywhere are new crates (`brewery_run_widget`, `brewery_search_everywhere`) hooked into `title_bar` and `zed` with one call each. Run/Debug reuse Zed's own task and debugger actions — run configurations from the daemon arrive in M2.

**Tech Stack:** Rust, GPUI (`Render`, `ModalView`, `Picker`), Zed `workspace`/`title_bar`/`file_finder`/`project_symbols`/`command_palette` crates.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md`, section 7 ("UI in the fork": stripes, main toolbar with run widget, Search Everywhere, tool windows in 0.1). M1a (themes, icons, fonts, keymap, rebrand) is done.

## Global Constraints

- Every edit inside a file that came from Zed carries a `brewery:` marker; keep hook-ins to a few lines each. New Brewery files (even inside Zed crates) need none.
- Do not rename or delete Zed crates or files.
- LF line endings only.
- No JetBrains names, logos or product icons; UI icons come from Zed's `ui::IconName` set.
- Spec layout: Left: Project, Commit, Structure, Database (Database arrives in M4). Right: Gradle/Maven, Notifications (Gradle arrives in M2). Bottom: Run, Debug, Services, Problems, Terminal, Git (Log) — M1b covers the panels Zed already has (Terminal, Debug).
- Hook-in test set for CI: `cargo test -p brewery_cloud -p brewery_core -p brewery_theme -p release_channel -p paths -p settings -p client`, plus the crates this plan adds/changes (`-p workspace --lib brewery`, `-p brewery_run_widget`, `-p brewery_search_everywhere`).

---

### Task 1: IntelliJ tool-window placement by default

**Files:** `assets/settings/default.json` (`project_panel.dock` ~line 898, `outline_panel.dock` ~1022, `git_panel.dock` ~1095), `crates/settings/src/brewery_defaults.rs`.

**Interfaces:** Produces defaults `project_panel.dock = "left"`, `outline_panel.dock = "left"`, `git_panel.dock = "left"`; terminal and debugger stay `"bottom"`.

- [ ] **Step 1: Failing test** (append to `brewery_defaults.rs`):

```rust
#[test]
fn test_intellij_tool_window_placement() {
    let d = defaults();
    for panel in ["project_panel", "outline_panel", "git_panel"] {
        assert_eq!(d[panel]["dock"], Value::String("left".into()), "{panel}");
    }
    for panel in ["terminal", "debugger"] {
        assert_eq!(d[panel]["dock"], Value::String("bottom".into()), "{panel}");
    }
}
```
Run `cargo test -p settings test_intellij_tool_window_placement` → FAIL (`right`).

- [ ] **Step 2: Implement** — in each of the three panel blocks replace `"dock": "right",` with
```jsonc
    // brewery: IntelliJ keeps this tool window on the left
    "dock": "left",
```
Verify the three edited `"dock"` keys are the ones inside `"project_panel": {`, `"outline_panel": {`, `"git_panel": {` (not `collaboration_panel` or the agent panel).

- [ ] **Step 3:** `cargo test -p settings` → PASS (fix any Zed settings test that asserts the old dock the M0 way: keep what it proves, mark the edit).

- [ ] **Step 4: Commit** `feat: IntelliJ tool-window placement by default`

---

### Task 2: Tool-window stripes

**Files:**
- Create: `crates/workspace/src/dock/brewery_stripes.rs`
- Modify: `crates/workspace/src/dock.rs` (one `mod` + `pub use` line, top of file)
- Modify: `crates/workspace/src/workspace.rs` — status bar registration (~lines 1974-1987) and the layout in `impl Render for Workspace` (~line 9770, the `.child({ match bottom_dock_layout { ... } })` block)

**Interfaces:**
- Consumes: `Dock` (private fields `panel_entries`, `active_panel_index`, `is_open`, `position`), `PanelEntry.panel: Arc<dyn PanelHandle>` with `icon(window, cx) -> Option<IconName>`, `icon_tooltip(window, cx) -> Option<&'static str>`, `toggle_action(window, cx) -> Box<dyn Action>`, `persistent_name() -> &'static str`; `Dock::toggle_action()`; `Dock::focus_handle(cx)`.
- Produces: `pub struct ToolWindowStripe` (GPUI view) with `pub fn new(top: Entity<Dock>, bottom: Option<Entity<Dock>>, cx: &mut Context<Self>) -> Self`; a pure helper `fn stripe_buttons(dock: &Dock, window: &Window, cx: &App) -> Vec<StripeButton>` where `struct StripeButton { name: &'static str, icon: IconName, tooltip: SharedString, active: bool, action: Box<dyn Action> }`.

Behaviour (IntelliJ New UI):
- Left stripe: buttons of the left dock's panels at the top; buttons of the bottom dock's panels at the bottom of the stripe, separated by flexible space. Right stripe: right dock's panels at the top.
- Stripe is a vertical column, 40 px wide, background `panel.background`, border on the inner edge (`border` colour). Buttons are `IconButton`s, `IconSize::Medium`, toggled when that panel is the dock's active panel and the dock is open; clicking an active button closes the dock (dock's `toggle_action`), any other opens that panel (`panel.toggle_action`). Tooltip: the panel's `icon_tooltip`, with keybinding via `Tooltip::for_action`.
- The status bar no longer shows panel buttons (the stripes replace them). Keep Zed's `PanelButtons` type untouched; only stop adding its three instances to the status bar.

- [ ] **Step 1: Failing test** — in `brewery_stripes.rs` add a `#[cfg(test)] mod tests` with a `#[gpui::test]` that builds a workspace with test panels the way `crates/workspace/src/dock.rs`'s own tests do (see its `TestPanel` in `crates/workspace/src/dock.rs` / `crates/workspace/src/workspace.rs` test modules — reuse them via `crate::dock::test::TestPanel` or the existing helper; read them first), adds two panels to the left dock, activates the second, opens the dock, and asserts `stripe_buttons(left_dock)` yields both names in order with only the second `active`. Then closes the dock and asserts no button is active.

Run `cargo test -p workspace --lib brewery_stripes` → FAIL (missing).

- [ ] **Step 2: Implement** `brewery_stripes.rs` (`ToolWindowStripe` + `stripe_buttons` + `impl Render`), hooking into `dock.rs`:

```rust
// brewery: IntelliJ-style tool-window stripes (child module so it can read Dock's fields)
mod brewery_stripes;
pub use brewery_stripes::ToolWindowStripe;
```
Observe the docks (`cx.observe(&dock, |_, _, cx| cx.notify())`) so stripes repaint on panel changes.

- [ ] **Step 3: Workspace hook-ins**
  - Create the stripes next to the existing `PanelButtons` (~line 1974): `left_stripe = cx.new(|cx| ToolWindowStripe::new(left_dock.clone(), Some(bottom_dock.clone()), cx))`, `right_stripe = cx.new(|cx| ToolWindowStripe::new(right_dock.clone(), None, cx))`; store them in two new `Workspace` fields (`brewery_left_stripe`, `brewery_right_stripe`, each line marked).
  - Stop adding `left_dock_buttons`, `right_dock_buttons`, `bottom_dock_buttons` to the status bar (wrap the three `add_*_item` calls in a marked `if false`-free way: delete the three calls and keep the `PanelButtons` entities only if something else uses them; if nothing does, also stop creating them — mark the change).
  - In `render`, wrap the `match bottom_dock_layout { ... }` child: `h_flex().size_full().child(self.brewery_left_stripe.clone()).child(div().flex_1().h_full().min_w_0().child({ match ... })).child(self.brewery_right_stripe.clone())` — only the opening and closing lines change; mark both.

- [ ] **Step 4: Verify** — `cargo test -p workspace --lib brewery_stripes`, `cargo test -p workspace --lib dock` (Zed's dock tests still pass), `cargo clippy -p workspace -- -D warnings`, `cargo check -p zed`. Zed workspace tests that count status-bar items or panel buttons may need marked updates; list them.

- [ ] **Step 5: Commit** `feat: IntelliJ-style tool-window stripes`

---

### Task 3: Run/Debug widget in the title bar

**Files:**
- Create: `crates/brewery_run_widget/` (`Cargo.toml`, `src/brewery_run_widget.rs`, `LICENSE-GPL` symlink)
- Modify: root `Cargo.toml` (member + workspace dep, marked), `crates/title_bar/Cargo.toml`, `crates/title_bar/src/title_bar.rs` (last `children.push(h_flex()...)` ~line 379)

**Interfaces:**
- Consumes: `zed_actions::Spawn::ViaModal { reveal_target: None }` (check the variant's real fields in `crates/zed_actions/src/lib.rs` ~line 737), `zed_actions::Rerun::default()`; debugger actions by name `"debugger::Start"` and `"debugger::Stop"` built with `cx.build_action(name, None)` (the `debugger_ui` crate is not a dependency).
- Produces: `pub fn brewery_run_widget::render_run_widget(window: &mut Window, cx: &mut App) -> AnyElement`.

Widget: `h_flex` with a ghost button labelled "Run…" (icon `IconName::ChevronDown`, opens the task picker via `Spawn::ViaModal`), then icon buttons ▶ `IconName::PlayFilled` (Rerun last task; tooltip "Run"), Debug `IconName::Debug` (tooltip "Debug", dispatches `debugger::Start`), Stop `IconName::Stop` (tooltip "Stop", dispatches `debugger::Stop`). Each button dispatches via `window.dispatch_action(action, cx)`; tooltips via `Tooltip::for_action`.

- [ ] **Step 1: Failing test** — `#[gpui::test]` that initialises the action registry (a `TestAppContext` with `zed_actions` and a stub `debugger::Start`/`debugger::Stop` registered via `gpui::actions!(debugger, [Start, Stop])` inside the test module) and asserts the widget's action list (expose `fn widget_actions(cx: &App) -> Vec<(SharedString, Box<dyn Action>)>`) has names `["Run…", "Run", "Debug", "Stop"]` and action names `task::Spawn`, `task::Rerun`, `debugger::Start`, `debugger::Stop`. Run → FAIL.

- [ ] **Step 2: Implement** the crate (`render_run_widget` built from `widget_actions`; if `build_action` fails because the debugger is not registered, omit that button and `log::debug!` once).

- [ ] **Step 3: Hook** in `title_bar.rs`, first child of the last `children.push(h_flex()...)`:
```rust
                // brewery: IntelliJ-style Run/Debug widget
                .child(brewery_run_widget::render_run_widget(window, cx))
```
and `brewery_run_widget.workspace = true # brewery:` in `crates/title_bar/Cargo.toml`.

- [ ] **Step 4: Verify** — `cargo test -p brewery_run_widget`, `cargo clippy -p brewery_run_widget -p title_bar -- -D warnings`, `cargo check -p zed`.

- [ ] **Step 5: Commit** `feat: Run/Debug widget in the title bar`

---

### Task 4: Search Everywhere on Shift Shift

**Files:**
- Create: `crates/brewery_search_everywhere/` (`Cargo.toml`, `src/brewery_search_everywhere.rs`, `LICENSE-GPL` symlink)
- Modify: root `Cargo.toml`, `crates/zed/Cargo.toml`, `crates/zed/src/main.rs` (init after `command_palette::init(cx)`), `assets/keymaps/linux/jetbrains.json` (line ~131 `"shift shift"`), and the macOS JetBrains keymap if it binds `shift shift` too.

**Interfaces:**
- Consumes: `file_finder::FileFinder` (ModalView), `project_symbols` (its toggle action / picker), `command_palette::CommandPalette` (`CommandPalette::toggle`), `workspace::Workspace::toggle_modal`.
- Produces: action `brewery::SearchEverywhere` (`actions!(brewery, [SearchEverywhere, NextSearchTab, PreviousSearchTab])`), `pub fn init(cx: &mut App)`, pure `enum SearchTab { Files, Symbols, Actions }` with `fn next(self) -> Self` / `fn previous(self) -> Self` and `fn label(self) -> &'static str` ("Files", "Symbols", "Actions").

Behaviour: Shift Shift opens a modal with a tab row (Files | Symbols | Actions) above the active tab's picker. Tab / Shift-Tab (bound in a `SearchEverywhere` key context) switch tabs, keeping the query text. Files hosts Zed's file finder picker, Symbols Zed's project-symbols picker, Actions the command palette picker. Simplest correct implementation: the modal owns the active child entity (an `AnyView` plus its `FocusHandle` and query getter/setter), recreating it on tab switch and forwarding the child's `DismissEvent`. Read `crates/file_finder/src/file_finder.rs`, `crates/project_symbols/src/project_symbols.rs`, `crates/command_palette/src/command_palette.rs` to find constructors usable from outside (`FileFinder::new` / delegate constructors); if a constructor is private, add the smallest `pub` change with a `// brewery:` marker and say so.

- [ ] **Step 1: Failing tests** — unit test for `SearchTab::next/previous/label` cycling (Files→Symbols→Actions→Files); a `#[gpui::test]` using `VisualTestContext` that opens a workspace (copy the setup pattern from `crates/file_finder/src/file_finder_tests.rs`), dispatches `SearchEverywhere`, asserts the modal is `SearchEverywhere` on the Files tab, dispatches `NextSearchTab`, asserts the Symbols tab is active and the modal is still open. Run → FAIL.

- [ ] **Step 2: Implement** the crate and `init` (registers the workspace action that toggles the modal).

- [ ] **Step 3: Keymap** — in `assets/keymaps/linux/jetbrains.json` replace `"shift shift": "command_palette::Toggle"` with `"shift shift": "brewery::SearchEverywhere"` (marker comment above, JSONC), and add a context block:
```jsonc
  // brewery: Search Everywhere tab switching
  {
    "context": "SearchEverywhere",
    "bindings": { "tab": "brewery::NextSearchTab", "shift-tab": "brewery::PreviousSearchTab" }
  },
```
Run `script/check-keymaps` if it exists (Zed validates keymaps); also `cargo test -p settings` (keymap parsing tests).

- [ ] **Step 4: Verify** — `cargo test -p brewery_search_everywhere`, `cargo clippy -p brewery_search_everywhere --tests -- -D warnings`, `cargo check -p zed`.

- [ ] **Step 5: Commit** `feat: Search Everywhere on Shift Shift`

---

### Task 5: Remaining Zed branding in new-user files

**Files:** `assets/settings/initial_user_settings.json` and `assets/keymaps/initial.json` (headers mention Zed), anything else the first-run creates in the user's config dir (check `crates/settings/src/settings.rs` for `initial_*` assets).

- [ ] Replace "Zed" product mentions in those headers with "Brewery" (marked), keep documentation URLs (they point to Zed docs that still apply) but label them "Zed documentation". Test: `cargo test -p settings` (initial files parse). Commit `feat: Brewery headers in new-user settings files`.

---

### Task 6: CI, docs, on-screen check

- [ ] Add the new crates to the CI hook-in test list and `docs/brewery/FORK.md`.
- [ ] Build `cargo build -p cli -p zed`, controller captures the window with a clean trusted profile on the demo project and checks: stripes left/right with Project, Structure (outline), Commit (git) on the left, Terminal/Debug at the bottom of the left stripe; no panel buttons in the status bar; Run/Debug widget in the title bar; Shift Shift opens Search Everywhere with three tabs.
- [ ] Commit `docs: M1b test list`.
