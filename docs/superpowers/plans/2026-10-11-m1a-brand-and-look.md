# Brewery M1a — Brand and Look Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Brewery stops looking like Zed: IntelliJ-New-UI-style dark and light themes, JetBrains Mono / Inter fonts, IntelliJ file icons, the JetBrains keymap by default, and "Brewery" instead of "Zed" in the welcome screens, About window and app menus.

**Architecture:** Look-and-feel assets live in Brewery-owned places (`assets/themes/brewery/`, `assets/fonts/{jetbrains-mono,inter}/`, new crate `brewery_theme`); Zed files only get marked hook-ins (defaults in `assets/settings/default.json`, string swaps, one menu post-processing call). Colour icons must be real files on disk because GPUI renders `icons/…` asset paths as monochrome masks (`crates/ui/src/components/icon.rs` `Icon::from_path`), so `brewery_theme` embeds the IntelliJ SVGs and writes them to the data dir at startup, then registers them as an icon theme.

**Tech Stack:** Rust (GPUI, rust-embed), JSON themes, Python 3 (theme generator, icon fetcher), `gh` CLI for pinned downloads.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (sections 3 and 7). M1 is split: this plan is M1a; M1b (tool-window stripes, main toolbar with Run/Debug, Search Everywhere) follows.

## Global Constraints

- No JetBrains names, logos or product icons anywhere. Platform icons only from `JetBrains/intellij-community` (Apache 2.0), pinned commit `d0936c36705bdaf87deca0f157a40c651345600c`, path `platform/icons/src/expui/`, with attribution in `NOTICE`.
- Fonts: JetBrains Mono (OFL) for code, Inter (OFL) for UI; licence files shipped next to the fonts.
- Every edit inside a file that came from Zed carries a `brewery:` marker (`// brewery:` Rust/JSONC, `# brewery:` TOML/YAML/shell). New Brewery-only files need none.
- Do not rename or delete Zed crates or Zed asset files; add new files instead, except `assets/images/zed_logo.svg`, which is replaced in place (it is the only logo vector GPUI knows).
- LF line endings only (`core.autocrlf=false`); check `tr -cd '\r' < FILE | wc -c` = 0 for every edited text file.
- Public text in English. Product name constant: `paths::APP_NAME` (= "Brewery").
- Theme names: `Brewery Dark`, `Brewery Light`. Icon theme names: `Brewery Icons Dark`, `Brewery Icons Light`.
- Hook-in test set for CI stays: `cargo test -p brewery_cloud -p brewery_core -p release_channel -p paths -p settings -p client`; this plan adds `-p brewery_theme`.

## File map

| Path | Change | Responsibility |
|---|---|---|
| `assets/fonts/jetbrains-mono/*.ttf`, `OFL.txt` | create | code font |
| `assets/fonts/inter/*.ttf`, `LICENSE.txt` | create | UI font |
| `script/brewery/gen_themes.py` | create | generates the Brewery theme family from One + IntelliJ palettes |
| `assets/themes/brewery/brewery.json` | create (generated) | `Brewery Dark`, `Brewery Light` |
| `script/brewery/fetch_intellij_icons.py` | create | downloads pinned IntelliJ SVGs, writes icon theme JSON |
| `crates/brewery_theme/` | create | embeds icons, installs them to the data dir, registers the icon theme; tests for theme + icon theme |
| `assets/settings/default.json` | modify | defaults: theme, icon theme, fonts, base keymap |
| `crates/settings/src/brewery_defaults.rs` | modify | guard tests for the new defaults |
| `crates/zed/src/main.rs`, `crates/zed/Cargo.toml` | modify | call `brewery_theme::init`, menu post-processing |
| `crates/zed/src/zed.rs` | modify | menu post-processing in `reload_menus`, About title |
| `crates/brewery_core/src/brewery_core.rs`, `Cargo.toml` | modify | `rebrand_menus` |
| `crates/onboarding/src/onboarding.rs`, `basics_page.rs`, `Cargo.toml` | modify | Brewery strings, hide telemetry section |
| `crates/workspace/src/welcome.rs` | modify | Brewery strings |
| `assets/images/zed_logo.svg` | replace | Brewery logo vector |
| `README.md`, `docs/brewery/FORK.md`, `.github/workflows/brewery_ci.yml` | modify | dev-build note, test list |

---

### Task 1: Fonts — JetBrains Mono and Inter by default

**Files:**
- Create: `assets/fonts/jetbrains-mono/JetBrainsMono-{Regular,Italic,Bold,BoldItalic}.ttf`, `assets/fonts/jetbrains-mono/OFL.txt`
- Create: `assets/fonts/inter/Inter-{Regular,Italic,Medium,SemiBold,Bold}.ttf`, `assets/fonts/inter/LICENSE.txt`
- Modify: `assets/settings/default.json` (`buffer_font_family` ~line 31, `ui_font_family` ~line 59)
- Modify: `crates/settings/src/brewery_defaults.rs`

**Interfaces:**
- Produces: fonts registered by Zed's existing `Assets::load_fonts` (it loads every `.ttf` under `fonts/`); defaults `buffer_font_family = "JetBrains Mono"`, `ui_font_family = "Inter"`.

- [ ] **Step 1: Write the failing test** — append to `crates/settings/src/brewery_defaults.rs`:

```rust
#[test]
fn test_brewery_fonts_are_default() {
    let d = defaults();
    assert_eq!(d["buffer_font_family"], Value::String("JetBrains Mono".into()));
    assert_eq!(d["ui_font_family"], Value::String("Inter".into()));
}
```

- [ ] **Step 2: Run it — expect FAIL** (`left: String(".ZedMono")`)

Run: `cargo test -p settings test_brewery_fonts_are_default`

- [ ] **Step 3: Download pinned font releases into a scratch dir and copy the files**

```bash
S="$(mktemp -d)"
gh release download v2.304 --repo JetBrains/JetBrainsMono --pattern "JetBrainsMono-2.304.zip" --dir "$S"
gh release download v4.1 --repo rsms/inter --pattern "Inter-4.1.zip" --dir "$S"
(cd "$S" && unzip -q JetBrainsMono-2.304.zip -d jbm && unzip -q Inter-4.1.zip -d inter)
mkdir -p assets/fonts/jetbrains-mono assets/fonts/inter
for f in Regular Italic Bold BoldItalic; do cp "$S/jbm/fonts/ttf/JetBrainsMono-$f.ttf" assets/fonts/jetbrains-mono/; done
cp "$S/jbm/OFL.txt" assets/fonts/jetbrains-mono/OFL.txt
for f in Regular Italic Medium SemiBold Bold; do cp "$S/inter/extras/ttf/Inter-$f.ttf" assets/fonts/inter/; done
cp "$S/inter/LICENSE.txt" assets/fonts/inter/LICENSE.txt
ls -la assets/fonts/jetbrains-mono assets/fonts/inter
```

If a path inside a zip differs, list the zip (`unzip -l`) and use the matching static TTF files (not variable fonts); record the actual paths in the report.

- [ ] **Step 4: Set the defaults** in `assets/settings/default.json`:

```jsonc
  // brewery: IntelliJ-style code font
  "buffer_font_family": "JetBrains Mono",
```
```jsonc
  // brewery: IntelliJ New UI font
  "ui_font_family": "Inter",
```

- [ ] **Step 5: Run tests — expect PASS**

Run: `cargo test -p settings`

- [ ] **Step 6: Commit**

```bash
git add assets/fonts/jetbrains-mono assets/fonts/inter assets/settings/default.json crates/settings/src/brewery_defaults.rs
git commit -m "feat: JetBrains Mono and Inter as default fonts"
```

---

### Task 2: Brewery Dark and Brewery Light themes

**Files:**
- Create: `script/brewery/gen_themes.py`
- Create (generated): `assets/themes/brewery/brewery.json`
- Create: `crates/brewery_theme/` crate skeleton with the theme test (the icon parts come in Task 3)
- Modify: root `Cargo.toml` (member + workspace dep, `# brewery:`), `assets/settings/default.json` (`theme` block ~line 9), `crates/settings/src/brewery_defaults.rs`

**Interfaces:**
- Produces: theme family file `assets/themes/brewery/brewery.json` loaded by Zed's `load_bundled_themes` (lists `themes/`); crate `brewery_theme` (lib path `src/brewery_theme.rs`, `LICENSE-GPL` as git symlink to Zed's blob) that Task 3 extends with `pub fn init(cx: &mut gpui::App)`.

- [ ] **Step 1: Write the generator** — `script/brewery/gen_themes.py`:

```python
"""Generate Brewery's IntelliJ-New-UI-style theme family.

Starts from Zed's One theme (so every key Zed expects is present) and
overrides colours with IntelliJ New UI palettes. Run from the repo root:
    python script/brewery/gen_themes.py
"""

import json
from pathlib import Path

SOURCE = Path("assets/themes/one/one.json")
TARGET = Path("assets/themes/brewery/brewery.json")


def c(hex_rgb: str, alpha: str = "ff") -> str:
    return f"#{hex_rgb.lstrip('#').lower()}{alpha}"


DARK = {
    "base": "One Dark",
    "name": "Brewery Dark",
    "appearance": "dark",
    "ui": {
        "background": c("2b2d30"),
        "surface.background": c("2b2d30"),
        "elevated_surface.background": c("2b2d30"),
        "panel.background": c("2b2d30"),
        "status_bar.background": c("2b2d30"),
        "title_bar.background": c("2b2d30"),
        "title_bar.inactive_background": c("2b2d30"),
        "toolbar.background": c("1e1f22"),
        "tab_bar.background": c("1e1f22"),
        "tab.inactive_background": c("1e1f22"),
        "tab.active_background": c("1e1f22"),
        "editor.background": c("1e1f22"),
        "editor.gutter.background": c("1e1f22"),
        "editor.subheader.background": c("2b2d30"),
        "editor.foreground": c("bcbec4"),
        "editor.active_line.background": c("26282e"),
        "editor.highlighted_line.background": c("26282e"),
        "editor.line_number": c("4b5059"),
        "editor.active_line_number": c("a1a3ab"),
        "editor.hover_line_number": c("a1a3ab"),
        "editor.invisible": c("6f737a", "66"),
        "editor.wrap_guide": c("393b40"),
        "editor.active_wrap_guide": c("4b5059"),
        "editor.document_highlight.read_background": c("373b39"),
        "editor.document_highlight.write_background": c("40332b"),
        "terminal.background": c("1e1f22"),
        "terminal.foreground": c("bcbec4"),
        "border": c("1e1f22"),
        "border.variant": c("393b40"),
        "border.focused": c("3574f0"),
        "border.selected": c("3574f0"),
        "border.disabled": c("393b40"),
        "element.background": c("393b40"),
        "element.hover": c("393b40"),
        "element.active": c("43454a"),
        "element.selected": c("2e436e"),
        "element.disabled": c("2b2d30"),
        "ghost_element.hover": c("393b40"),
        "ghost_element.active": c("43454a"),
        "ghost_element.selected": c("2e436e"),
        "drop_target.background": c("3574f0", "33"),
        "text": c("dfe1e5"),
        "text.muted": c("9da0a8"),
        "text.placeholder": c("6f737a"),
        "text.disabled": c("6f737a"),
        "text.accent": c("548af7"),
        "icon": c("ced0d6"),
        "icon.muted": c("9da0a8"),
        "icon.disabled": c("6f737a"),
        "icon.placeholder": c("6f737a"),
        "icon.accent": c("548af7"),
        "link_text.hover": c("548af7"),
        "panel.focused_border": c("3574f0"),
        "pane.focused_border": c("3574f0"),
        "scrollbar.thumb.background": c("4e5157", "80"),
        "scrollbar.thumb.hover_background": c("5a5d63", "cc"),
        "scrollbar.track.background": c("1e1f22", "00"),
        "search.match_background": c("32593d"),
        "search.active_match_background": c("5c8a5f"),
        "version_control.added": c("549159"),
        "version_control.modified": c("375fad"),
        "version_control.deleted": c("868a91"),
        "error": c("f75464"),
        "warning": c("f2c55c"),
        "success": c("5fb865"),
        "info": c("3574f0"),
        "hint": c("6f737a"),
        "created": c("549159"),
        "modified": c("375fad"),
        "deleted": c("868a91"),
    },
    "syntax": {
        "keyword": "cf8e6d", "boolean": "cf8e6d", "string": "6aab73", "string.escape": "cf8e6d",
        "string.regex": "6aab73", "string.special": "6aab73", "string.special.symbol": "c77dbb",
        "number": "2aacb8", "comment": "7a7e85", "comment.doc": "5f826b", "function": "56a8f5",
        "constructor": "56a8f5", "type": "bcbec4", "constant": "c77dbb", "property": "c77dbb",
        "enum": "c77dbb", "variant": "c77dbb", "attribute": "b3ae60", "preproc": "b3ae60",
        "tag": "d5b778", "variable": "bcbec4", "variable.parameter": "bcbec4",
        "variable.special": "cf8e6d", "operator": "bcbec4", "punctuation": "bcbec4",
        "punctuation.bracket": "bcbec4", "punctuation.delimiter": "bcbec4",
        "namespace": "bcbec4", "label": "bcbec4", "title": "56a8f5", "link_uri": "56a8f5",
        "link_text": "6aab73", "embedded": "bcbec4", "primary": "bcbec4",
    },
    "player": c("3574f0"),
    "selection": c("214283"),
}

LIGHT = {
    "base": "One Light",
    "name": "Brewery Light",
    "appearance": "light",
    "ui": {
        "background": c("f7f8fa"),
        "surface.background": c("f7f8fa"),
        "elevated_surface.background": c("ffffff"),
        "panel.background": c("f7f8fa"),
        "status_bar.background": c("f7f8fa"),
        "title_bar.background": c("f7f8fa"),
        "title_bar.inactive_background": c("f7f8fa"),
        "toolbar.background": c("ffffff"),
        "tab_bar.background": c("ffffff"),
        "tab.inactive_background": c("ffffff"),
        "tab.active_background": c("ffffff"),
        "editor.background": c("ffffff"),
        "editor.gutter.background": c("ffffff"),
        "editor.subheader.background": c("f7f8fa"),
        "editor.foreground": c("080808"),
        "editor.active_line.background": c("f5f8fe"),
        "editor.highlighted_line.background": c("f5f8fe"),
        "editor.line_number": c("aeb3c2"),
        "editor.active_line_number": c("767a8a"),
        "editor.hover_line_number": c("767a8a"),
        "editor.invisible": c("aeb3c2", "99"),
        "editor.wrap_guide": c("ebecf0"),
        "editor.active_wrap_guide": c("dfe1e5"),
        "editor.document_highlight.read_background": c("edebfc"),
        "editor.document_highlight.write_background": c("fceeea"),
        "terminal.background": c("ffffff"),
        "terminal.foreground": c("080808"),
        "border": c("ebecf0"),
        "border.variant": c("ebecf0"),
        "border.focused": c("3574f0"),
        "border.selected": c("3574f0"),
        "border.disabled": c("ebecf0"),
        "element.background": c("ebecf0"),
        "element.hover": c("dfe1e5"),
        "element.active": c("d3d5db"),
        "element.selected": c("d4e2ff"),
        "element.disabled": c("f7f8fa"),
        "ghost_element.hover": c("dfe1e5"),
        "ghost_element.active": c("d3d5db"),
        "ghost_element.selected": c("d4e2ff"),
        "drop_target.background": c("3574f0", "22"),
        "text": c("000000"),
        "text.muted": c("6c707e"),
        "text.placeholder": c("818594"),
        "text.disabled": c("a8adbd"),
        "text.accent": c("3574f0"),
        "icon": c("6c707e"),
        "icon.muted": c("818594"),
        "icon.disabled": c("a8adbd"),
        "icon.placeholder": c("a8adbd"),
        "icon.accent": c("3574f0"),
        "link_text.hover": c("3574f0"),
        "panel.focused_border": c("3574f0"),
        "pane.focused_border": c("3574f0"),
        "scrollbar.thumb.background": c("a8adbd", "80"),
        "scrollbar.thumb.hover_background": c("818594", "cc"),
        "scrollbar.track.background": c("ffffff", "00"),
        "search.match_background": c("f2e6c4"),
        "search.active_match_background": c("e8c66e"),
        "version_control.added": c("5fb865"),
        "version_control.modified": c("3574f0"),
        "version_control.deleted": c("9da0a8"),
        "error": c("e55230"),
        "warning": c("c27d04"),
        "success": c("208a3c"),
        "info": c("3574f0"),
        "hint": c("818594"),
        "created": c("5fb865"),
        "modified": c("3574f0"),
        "deleted": c("9da0a8"),
    },
    "syntax": {
        "keyword": "0033b3", "boolean": "0033b3", "string": "067d17", "string.escape": "0037a6",
        "string.regex": "067d17", "string.special": "067d17", "string.special.symbol": "871094",
        "number": "1750eb", "comment": "8c8c8c", "comment.doc": "8c8c8c", "function": "00627a",
        "constructor": "00627a", "type": "000000", "constant": "871094", "property": "871094",
        "enum": "871094", "variant": "871094", "attribute": "9e880d", "preproc": "9e880d",
        "tag": "0033b3", "variable": "080808", "variable.parameter": "080808",
        "variable.special": "0033b3", "operator": "080808", "punctuation": "080808",
        "punctuation.bracket": "080808", "punctuation.delimiter": "080808",
        "namespace": "080808", "label": "080808", "title": "00627a", "link_uri": "00627a",
        "link_text": "067d17", "embedded": "080808", "primary": "080808",
    },
    "player": c("3574f0"),
    "selection": c("a6d2ff"),
}


def build(palette: dict, one: dict) -> dict:
    base = next(t for t in one["themes"] if t["name"] == palette["base"])
    style = json.loads(json.dumps(base["style"]))
    for key, value in palette["ui"].items():
        if key not in style:
            raise SystemExit(f"{palette['name']}: unknown style key {key!r}")
        style[key] = value
    for key, rgb in palette["syntax"].items():
        if key not in style["syntax"]:
            raise SystemExit(f"{palette['name']}: unknown syntax key {key!r}")
        style["syntax"][key]["color"] = c(rgb)
    style["players"][0] = {
        "cursor": palette["player"],
        "background": palette["player"],
        "selection": palette["selection"],
    }
    return {"name": palette["name"], "appearance": palette["appearance"], "style": style}


def main() -> None:
    one = json.loads(SOURCE.read_text(encoding="utf-8"))
    family = {
        "$schema": one["$schema"],
        "name": "Brewery",
        "author": "Brewery contributors",
        "themes": [build(DARK, one), build(LIGHT, one)],
    }
    TARGET.parent.mkdir(parents=True, exist_ok=True)
    TARGET.write_text(json.dumps(family, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {TARGET}")


if __name__ == "__main__":
    main()
```

Run: `python script/brewery/gen_themes.py` → `wrote assets/themes/brewery/brewery.json`.

- [ ] **Step 2: Create the `brewery_theme` crate with the failing theme test**

`crates/brewery_theme/Cargo.toml`:

```toml
[package]
name = "brewery_theme"
version = "0.1.0"
edition.workspace = true
publish.workspace = true
license = "GPL-3.0-or-later"

[lints]
workspace = true

[lib]
path = "src/brewery_theme.rs"
doctest = false

[dependencies]
anyhow.workspace = true
gpui.workspace = true
log.workspace = true
paths.workspace = true
rust-embed.workspace = true
serde_json.workspace = true
theme.workspace = true
theme_settings.workspace = true

[dev-dependencies]
gpui = { workspace = true, features = ["test-support"] }
```

`crates/brewery_theme/src/brewery_theme.rs`:

```rust
//! Brewery's look: the IntelliJ-style theme family and icon theme.

pub const DARK_THEME: &str = "Brewery Dark";
pub const LIGHT_THEME: &str = "Brewery Light";

#[cfg(test)]
mod tests {
    use super::*;

    const THEME_JSON: &str = include_str!("../../../assets/themes/brewery/brewery.json");

    #[test]
    fn test_brewery_theme_family_parses() {
        let family: theme_settings::ThemeFamilyContent =
            serde_json::from_str(THEME_JSON).expect("brewery.json is a valid theme family");
        let names: Vec<_> = family.themes.iter().map(|theme| theme.name.clone()).collect();
        assert_eq!(names, vec![DARK_THEME.to_string(), LIGHT_THEME.to_string()]);
        let refined = theme_settings::refine_theme_family(family);
        assert_eq!(refined.themes.len(), 2);
    }

    #[test]
    fn test_brewery_dark_uses_intellij_palette() {
        let family: serde_json::Value = serde_json::from_str(THEME_JSON).expect("valid JSON");
        let dark = &family["themes"][0]["style"];
        assert_eq!(dark["editor.background"], "#1e1f22ff");
        assert_eq!(dark["panel.background"], "#2b2d30ff");
        assert_eq!(dark["border.focused"], "#3574f0ff");
        assert_eq!(dark["syntax"]["keyword"]["color"], "#cf8e6dff");
        assert_eq!(dark["syntax"]["string"]["color"], "#6aab73ff");
        let light = &family["themes"][1]["style"];
        assert_eq!(light["editor.background"], "#ffffffff");
        assert_eq!(light["syntax"]["keyword"]["color"], "#0033b3ff");
    }
}
```

LICENSE: `blob=$(git ls-files -s crates/zed/LICENSE-GPL | awk '{print $2}'); git update-index --add --cacheinfo 120000,$blob,crates/brewery_theme/LICENSE-GPL; git checkout -- crates/brewery_theme/LICENSE-GPL`.

Root `Cargo.toml`: add `"crates/brewery_theme", # brewery:` to `members` after `"crates/brewery_core",`, and `brewery_theme = { path = "crates/brewery_theme" } # brewery:` after the `brewery_core = ...` workspace dependency. If `theme_settings` or `rust-embed` lacks a `[workspace.dependencies]` entry, stop and report (they exist at the time of writing).

In `crates/settings/src/brewery_defaults.rs` append:

```rust
#[test]
fn test_brewery_theme_is_default() {
    let d = defaults();
    assert_eq!(d["theme"]["mode"], Value::String("system".into()));
    assert_eq!(d["theme"]["dark"], Value::String("Brewery Dark".into()));
    assert_eq!(d["theme"]["light"], Value::String("Brewery Light".into()));
}
```

- [ ] **Step 3: Run tests — `brewery_theme` passes (file generated in Step 1), the settings one fails**

Run: `cargo test -p brewery_theme -p settings`
Expected: `brewery_theme` 2 passed; `settings` FAIL on `test_brewery_theme_is_default` (`One Dark`).

- [ ] **Step 4: Set the default theme** in `assets/settings/default.json`:

```jsonc
  // brewery: IntelliJ-style themes
  "theme": {
    "mode": "system",
    "light": "Brewery Light",
    "dark": "Brewery Dark",
  },
```

- [ ] **Step 5: Run tests — PASS**

Run: `cargo test -p brewery_theme -p settings`

- [ ] **Step 6: Commit**

```bash
git add script/brewery/gen_themes.py assets/themes/brewery crates/brewery_theme Cargo.toml Cargo.lock assets/settings/default.json crates/settings/src/brewery_defaults.rs
git commit -m "feat: Brewery Dark and Brewery Light themes by default"
```

---

### Task 3: IntelliJ file icons as the default icon theme

**Files:**
- Create: `script/brewery/fetch_intellij_icons.py`
- Create (generated): `crates/brewery_theme/icons/{dark,light}/*.svg`, `crates/brewery_theme/icons/icon_theme.json`, `crates/brewery_theme/icons/NOTICE`
- Modify: `crates/brewery_theme/src/brewery_theme.rs` (add `init`, embedding, tests)
- Modify: `crates/zed/Cargo.toml`, `crates/zed/src/main.rs` (call `brewery_theme::init` right after `theme_settings::init(...)`, ~line 670)
- Modify: `assets/settings/default.json` (`icon_theme` ~line 14), `crates/settings/src/brewery_defaults.rs`

**Interfaces:**
- Consumes: crate `brewery_theme` (Task 2); `theme::ThemeRegistry::global(cx).load_icon_theme(path: &Path, icons_root_dir: &Path, contents: Vec<u8>) -> anyhow::Result<()>`; `theme_settings::reload_icon_theme(cx)`; `theme::deserialize_icon_theme(&[u8])`; `theme::default_icon_theme()` (its `file_icons` keys are the icon keys Zed maps file types to).
- Produces: `pub fn brewery_theme::init(cx: &mut gpui::App)`; icon theme names `Brewery Icons Dark` / `Brewery Icons Light`; icons installed under `paths::data_dir().join("brewery").join("icon_theme")`.

Why files on disk: `Icon::from_path` treats `icons/...` paths as embedded monochrome SVG masks and any other path as a full-colour external image (`crates/ui/src/components/icon.rs:165-181`). IntelliJ icons are colour, so they must be external files.

- [ ] **Step 1: Write the fetcher** — `script/brewery/fetch_intellij_icons.py`:

```python
"""Fetch IntelliJ New UI icons (Apache 2.0) and write Brewery's icon theme.

Pinned to one intellij-community commit. Every icon key Zed's default icon
theme knows (crates/theme/src/icon_theme.rs FILE_ICONS) is mapped: to a
specific IntelliJ icon when there is one, otherwise to the plain text icon.
Run from the repo root:  python script/brewery/fetch_intellij_icons.py
"""

import json
import re
import urllib.request
from pathlib import Path

COMMIT = "d0936c36705bdaf87deca0f157a40c651345600c"
BASE = f"https://raw.githubusercontent.com/JetBrains/intellij-community/{COMMIT}/platform/icons/src/expui"
OUT = Path("crates/brewery_theme/icons")
ZED_ICON_THEME = Path("crates/theme/src/icon_theme.rs")

# Zed icon key -> IntelliJ expui icon (relative to expui/, without .svg)
SPECIFIC = {
    "java": "fileTypes/java", "kotlin": "fileTypes/text", "groovy": "fileTypes/groovy",
    "gradle": "fileTypes/gradle", "template": "fileTypes/xml", "json": "fileTypes/json",
    "yaml": "fileTypes/yaml", "toml": "fileTypes/toml", "toml_alt": "fileTypes/toml",
    "markdown": "fileTypes/markdown", "html": "fileTypes/html", "css": "fileTypes/css",
    "javascript": "fileTypes/javaScript", "typescript": "fileTypes/typeScript",
    "vue": "fileTypes/vue", "c": "fileTypes/c", "cpp": "fileTypes/cpp", "csharp": "fileTypes/Csharp",
    "swift": "fileTypes/swiftLang", "terraform": "fileTypes/terraform", "docker": "fileTypes/docker",
    "image": "fileTypes/image", "archive": "fileTypes/archive", "font": "fileTypes/font",
    "terminal": "fileTypes/shell", "settings": "fileTypes/config", "lock": "fileTypes/config",
    "graphql": "fileTypes/graphql", "perl": "fileTypes/perl", "diff": "fileTypes/patch",
    "editorconfig": "fileTypes/editorConfig", "git": "fileTypes/gitignore",
    "sql": "fileTypes/sql", "storage": "fileTypes/sql", "csv": "fileTypes/csv", "jupyter": "fileTypes/jupyter",
    "properties": "fileTypes/properties", "default": "fileTypes/text",
}
FOLDER = "nodes/folder"
FALLBACK = "fileTypes/text"


def zed_icon_keys() -> list[str]:
    source = ZED_ICON_THEME.read_text(encoding="utf-8")
    block = source.split("const FILE_ICONS: &[(&str, &str)] = &[", 1)[1].split("];", 1)[0]
    return sorted(set(re.findall(r'\("([a-z0-9_]+)",\s*"icons/', block)))


def fetch(name: str, variant: str) -> bytes:
    suffix = "_dark" if variant == "dark" else ""
    for candidate in (f"{name}{suffix}.svg", f"{name}.svg"):
        try:
            with urllib.request.urlopen(f"{BASE}/{candidate}", timeout=30) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
    raise SystemExit(f"missing icon {name} ({variant})")


def main() -> None:
    keys = zed_icon_keys()
    mapping = {key: SPECIFIC.get(key, FALLBACK) for key in keys}
    mapping["default"] = SPECIFIC["default"]
    needed = sorted(set(mapping.values()) | {FOLDER})
    for variant in ("dark", "light"):
        target = OUT / variant
        target.mkdir(parents=True, exist_ok=True)
        for name in needed:
            (target / (name.replace("/", "_") + ".svg")).write_bytes(fetch(name, variant))

    def theme(variant: str, appearance: str, title: str) -> dict:
        folder = f"{variant}/{FOLDER.replace('/', '_')}.svg"
        return {
            "name": title,
            "appearance": appearance,
            "directory_icons": {"collapsed": folder, "expanded": folder},
            "file_icons": {
                key: {"path": f"{variant}/{name.replace('/', '_')}.svg"}
                for key, name in sorted(mapping.items())
            },
        }

    family = {
        "$schema": "https://zed.dev/schema/icon_themes/v0.3.0.json",
        "name": "Brewery Icons",
        "author": "Brewery contributors; icons from JetBrains/intellij-community (Apache 2.0)",
        "themes": [
            theme("dark", "dark", "Brewery Icons Dark"),
            theme("light", "light", "Brewery Icons Light"),
        ],
    }
    (OUT / "icon_theme.json").write_text(json.dumps(family, indent=2) + "\n", encoding="utf-8", newline="\n")
    (OUT / "NOTICE").write_text(
        "Icons in this directory come from JetBrains/intellij-community\n"
        f"(commit {COMMIT}, platform/icons/src/expui),\n"
        "licensed under the Apache License, Version 2.0:\n"
        "https://www.apache.org/licenses/LICENSE-2.0\n",
        encoding="utf-8",
        newline="\n",
    )
    print(f"{len(keys)} Zed icon keys mapped to {len(needed)} IntelliJ icons per variant")


if __name__ == "__main__":
    main()
```

Run: `python script/brewery/fetch_intellij_icons.py`. Expected: a line like `N Zed icon keys mapped to M IntelliJ icons per variant`, files under `crates/brewery_theme/icons/`. If a SPECIFIC entry 404s in both variants, change that entry to `FALLBACK` and record it in the report.

- [ ] **Step 2: Write the failing tests** — extend `crates/brewery_theme/src/brewery_theme.rs` tests module:

```rust
    #[test]
    fn test_icon_theme_covers_every_zed_icon_key() {
        let family = theme::deserialize_icon_theme(ICON_THEME_JSON.as_bytes())
            .expect("icon_theme.json is valid");
        let zed_keys: Vec<String> = theme::default_icon_theme().file_icons.keys().cloned().collect();
        for icon_theme in &family.themes {
            for key in &zed_keys {
                assert!(
                    icon_theme.file_icons.contains_key(key),
                    "{} misses icon key {key}",
                    icon_theme.name
                );
            }
        }
    }

    #[test]
    fn test_every_icon_path_is_embedded() {
        let family = theme::deserialize_icon_theme(ICON_THEME_JSON.as_bytes())
            .expect("icon_theme.json is valid");
        for icon_theme in &family.themes {
            let mut paths: Vec<String> = icon_theme
                .file_icons
                .values()
                .map(|icon| icon.path.to_string())
                .collect();
            paths.extend(icon_theme.directory_icons.collapsed.iter().map(|p| p.to_string()));
            for path in paths {
                assert!(BreweryIcons::get(&path).is_some(), "{path} is not embedded");
            }
        }
    }

    #[test]
    fn test_icon_theme_names() {
        let family = theme::deserialize_icon_theme(ICON_THEME_JSON.as_bytes()).expect("valid");
        let names: Vec<_> = family.themes.iter().map(|t| t.name.clone()).collect();
        assert_eq!(names, vec![DARK_ICON_THEME.to_string(), LIGHT_ICON_THEME.to_string()]);
    }
```

Run: `cargo test -p brewery_theme` → FAIL to compile (`ICON_THEME_JSON`, `BreweryIcons`, `DARK_ICON_THEME` missing).

- [ ] **Step 3: Implement** — add to `crates/brewery_theme/src/brewery_theme.rs` above the tests:

```rust
use std::path::Path;

use anyhow::Context as _;
use gpui::App;
use rust_embed::RustEmbed;

pub const DARK_ICON_THEME: &str = "Brewery Icons Dark";
pub const LIGHT_ICON_THEME: &str = "Brewery Icons Light";

const ICON_THEME_JSON: &str = include_str!("../icons/icon_theme.json");

#[derive(RustEmbed)]
#[folder = "icons/"]
#[include = "dark/*.svg"]
#[include = "light/*.svg"]
struct BreweryIcons;

/// Installs the IntelliJ icons to disk and registers the Brewery icon themes.
/// Colour icons must be files: GPUI renders embedded `icons/...` SVGs as
/// single-colour masks.
pub fn init(cx: &mut App) {
    let root = paths::data_dir().join("brewery").join("icon_theme");
    if let Err(error) = install_icons(&root) {
        log::error!("failed to install Brewery icons: {error:#}");
        return;
    }
    let registry = theme::ThemeRegistry::global(cx);
    if let Err(error) = registry.load_icon_theme(
        &root.join("icon_theme.json"),
        &root,
        ICON_THEME_JSON.as_bytes().to_vec(),
    ) {
        log::error!("failed to load Brewery icon theme: {error:#}");
        return;
    }
    theme_settings::reload_icon_theme(cx);
}

fn install_icons(root: &Path) -> anyhow::Result<()> {
    for path in BreweryIcons::iter() {
        let file = BreweryIcons::get(&path).context("embedded icon vanished")?;
        let target = root.join(path.as_ref());
        if std::fs::read(&target).ok().as_deref() == Some(file.data.as_ref()) {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&target, &file.data).with_context(|| format!("writing {}", target.display()))?;
    }
    std::fs::write(root.join("icon_theme.json"), ICON_THEME_JSON).context("writing icon_theme.json")?;
    Ok(())
}
```

`crates/zed/Cargo.toml` `[dependencies]` (alphabetical): `brewery_theme.workspace = true # brewery:`.

`crates/zed/src/main.rs`, directly after `theme_settings::init(theme::LoadThemes::All(Box::new(Assets)), cx);`:

```rust
        // brewery: IntelliJ file icons (colour icons must live on disk)
        brewery_theme::init(cx);
```

`assets/settings/default.json`:

```jsonc
  // brewery: IntelliJ file icons
  "icon_theme": {
    "mode": "system",
    "light": "Brewery Icons Light",
    "dark": "Brewery Icons Dark",
  },
```

`crates/settings/src/brewery_defaults.rs` append:

```rust
#[test]
fn test_brewery_icon_theme_is_default() {
    let d = defaults();
    assert_eq!(d["icon_theme"]["dark"], Value::String("Brewery Icons Dark".into()));
    assert_eq!(d["icon_theme"]["light"], Value::String("Brewery Icons Light".into()));
}
```

If the settings schema rejects an object for `icon_theme` (it should not: `IconThemeSelection::Dynamic` exists in `crates/theme_settings/src/settings.rs`), stop and report.

- [ ] **Step 4: Run tests — PASS**

Run: `cargo test -p brewery_theme -p settings` and `cargo check -p zed`.

- [ ] **Step 5: Commit**

```bash
git add script/brewery/fetch_intellij_icons.py crates/brewery_theme Cargo.lock crates/zed/Cargo.toml crates/zed/src/main.rs assets/settings/default.json crates/settings/src/brewery_defaults.rs
git commit -m "feat: IntelliJ New UI file icons as the default icon theme"
```

---

### Task 4: JetBrains keymap by default

**Files:**
- Modify: `assets/settings/default.json` (`base_keymap` ~line 27), `crates/settings/src/brewery_defaults.rs`

**Interfaces:**
- Produces: default `base_keymap = "JetBrains"` (value name as accepted by `crates/settings/src/base_keymap_setting.rs`; check the exact serialized spelling there — the enum variant is `JetBrains`).

- [ ] **Step 1: Failing test**

```rust
#[test]
fn test_jetbrains_keymap_is_default() {
    assert_eq!(defaults()["base_keymap"], Value::String("JetBrains".into()));
}
```

Run: `cargo test -p settings test_jetbrains_keymap_is_default` → FAIL (`Zed`).

- [ ] **Step 2: Implement**

```jsonc
  // brewery: IntelliJ shortcuts by default
  "base_keymap": "JetBrains",
```

- [ ] **Step 3: Run all settings tests — PASS.** If Zed's own settings tests assert the old `Zed` default, update them the way M0 did for `auto_update` (keep what the test proves, mark each edit) and list them in the report.

Run: `cargo test -p settings`

- [ ] **Step 4: Commit**

```bash
git add assets/settings/default.json crates/settings/src/brewery_defaults.rs
git commit -m "feat: JetBrains keymap by default"
```

---

### Task 5: Brewery in the welcome screens, About window and logo

**Files:**
- Modify: `crates/onboarding/src/onboarding.rs` (~lines 350-360), `crates/onboarding/src/basics_page.rs` (telemetry section ~line 728, trust text ~line 447), `crates/onboarding/Cargo.toml`
- Modify: `crates/workspace/src/welcome.rs` (~lines 450-454 and ~483)
- Modify: `crates/zed/src/zed.rs` (About window title ~line 1720)
- Replace: `assets/images/zed_logo.svg`

**Interfaces:**
- Consumes: `paths::APP_NAME` ("Brewery"); `brewery_cloud::ZED_CLOUD_ENABLED`.
- Produces: no Zed name on onboarding, welcome and About; onboarding telemetry switches hidden (telemetry can never be sent, so the switches would mislead).

- [ ] **Step 1: Strings** — each edit with a `// brewery:` line above it:

`crates/onboarding/src/onboarding.rs`:
```rust
                                                        // brewery: product name
                                                        Headline::new(format!("Welcome to {}", paths::APP_NAME))
```
```rust
                                                        // brewery: product tagline
                                                        Label::new("A Java IDE on a native, GPU-rendered editor")
```

`crates/workspace/src/welcome.rs` (both labels):
```rust
        // brewery: product name
        let welcome_label = if self.fallback_to_recent_projects {
            format!("Welcome back to {}", paths::APP_NAME)
        } else {
            format!("Welcome to {}", paths::APP_NAME)
        };
```
and the tagline `Label::new("The editor for what's next")` → `Label::new("A Java IDE on a native, GPU-rendered editor")` with marker. Adapt the type where `welcome_label` is used (it was `&str`; pass `welcome_label.clone()` or `SharedString::from(welcome_label)` as the element requires).

`crates/onboarding/src/basics_page.rs` trust text: `"Automatically mark all new projects as trusted to unlock all Zed's features"` → `format!("Automatically mark all new projects as trusted to unlock all of {}'s features", paths::APP_NAME)` (adapt to the `.into()` it feeds) with marker.

`crates/zed/src/zed.rs` About title: `title: Some("About Zed".into())` → `title: Some(format!("About {}", paths::APP_NAME).into())` with marker.

Add `paths.workspace = true # brewery:` to `crates/onboarding/Cargo.toml` and `crates/workspace/Cargo.toml` `[dependencies]` only if missing; `brewery_cloud.workspace = true # brewery:` to `crates/onboarding/Cargo.toml`.

- [ ] **Step 2: Hide the onboarding telemetry section** — in `render_basics_page` replace the last two children:

```rust
        // brewery: telemetry can never be sent, so its switches would mislead
        .when(brewery_cloud::ZED_CLOUD_ENABLED, |this| {
            this.child(Divider::horizontal().color(ui::DividerColor::BorderVariant))
                .child(render_telemetry_section(&mut tab_index, cx))
        })
```

If the closure cannot borrow `tab_index` mutably there, compute the telemetry element before the builder chain only when enabled; keep the change local and explain in the report. If `render_telemetry_section` becomes unused under clippy (`dead_code`), keep it and add `#[allow(dead_code)] // brewery: used when ZED_CLOUD_ENABLED` on it.

- [ ] **Step 3: Logo** — replace `assets/images/zed_logo.svg` with:

```xml
<svg width="96" height="96" viewBox="0 0 96 96" fill="none" xmlns="http://www.w3.org/2000/svg">
<rect x="6" y="6" width="84" height="84" rx="18" stroke="black" stroke-width="6"/>
<path d="M33 24H51C58.732 24 65 30.268 65 38C65 45.732 58.732 52 51 52H33V24Z" stroke="black" stroke-width="6" stroke-linejoin="round"/>
<path d="M33 52H55C62.732 52 69 58.268 69 66C69 73.732 62.732 80 55 80H33V52Z" stroke="black" stroke-width="6" stroke-linejoin="round"/>
</svg>
```

The `ui` crate test `assert_eq!(VectorName::ZedLogo.path().as_ref(), "images/zed_logo.svg")` keeps passing (path unchanged).

- [ ] **Step 4: Verify**

Run: `cargo check -p zed` and `cargo clippy -p onboarding -p workspace -- -D warnings` and `cargo test -p ui --lib image`.

- [ ] **Step 5: Commit**

```bash
git add crates/onboarding crates/workspace/src/welcome.rs crates/workspace/Cargo.toml crates/zed/src/zed.rs assets/images/zed_logo.svg Cargo.lock
git commit -m "feat: Brewery name, tagline and logo on welcome and About"
```

---

### Task 6: Brewery app menus

**Files:**
- Modify: `crates/brewery_core/src/brewery_core.rs`, `crates/brewery_core/Cargo.toml`
- Modify: `crates/zed/src/main.rs` (~line 856 `let menus = app_menus(cx);`), `crates/zed/src/zed.rs` (`fn reload_menus` ~line 2324)

**Interfaces:**
- Consumes: `gpui::{Menu, MenuItem}` (`MenuItem::Action { name, action, os_action, checked, disabled }`, `MenuItem::Submenu(Menu)`, `Menu { name, items, disabled }`); `zed_actions::OpenBrowser { url: String }`.
- Produces: `pub fn brewery_core::rebrand_menus(menus: Vec<gpui::Menu>) -> Vec<gpui::Menu>`; `pub const brewery_core::REPOSITORY_URL: &str = "https://github.com/hteariH/brewery"`.

Rules (applied recursively through submenus):
- Menu named `Zed` → `Brewery`. Item names: `About Zed` → `About Brewery`, `Hide Zed` → `Hide Brewery`, `Quit Zed` → `Quit Brewery`.
- Remove items named: `Check for Updates`, `View Release Notes Locally`, `Email Us...`, `Zed Twitter`, `Join the Team`, `View Telemetry`.
- Replace items: `Zed Repository` → `Brewery Repository` opening `REPOSITORY_URL`; `File Bug Report...` → opens `{REPOSITORY_URL}/issues/new`; `Request Feature...` → opens `{REPOSITORY_URL}/issues/new`; `Documentation` → `Zed Documentation` (same action).
- Collapse separators: no leading/trailing separator in a menu, no two in a row.

- [ ] **Step 1: Failing tests** — in `brewery_core`'s tests module:

```rust
    use gpui::{Menu, MenuItem, NoAction};

    fn action(name: &str) -> MenuItem {
        MenuItem::action(name.to_string(), NoAction)
    }

    fn names(menu: &Menu) -> Vec<String> {
        menu.items
            .iter()
            .map(|item| match item {
                MenuItem::Action { name, .. } => name.to_string(),
                MenuItem::Separator => "---".to_string(),
                MenuItem::Submenu(menu) => format!("> {}", menu.name),
                MenuItem::SystemMenu(_) => "system".to_string(),
            })
            .collect()
    }

    #[test]
    fn test_rebrand_menus() {
        let menus = vec![
            Menu::new("Zed").items([
                action("About Zed"),
                action("Check for Updates"),
                MenuItem::separator(),
                action("Quit Zed"),
            ]),
            Menu::new("Help").items([
                action("View Release Notes Locally"),
                action("View Telemetry"),
                action("Show Welcome"),
                MenuItem::separator(),
                action("File Bug Report..."),
                action("Email Us..."),
                MenuItem::separator(),
                action("Documentation"),
                action("Zed Repository"),
                action("Zed Twitter"),
                action("Join the Team"),
            ]),
        ];
        let menus = rebrand_menus(menus);
        assert_eq!(menus[0].name.as_ref(), "Brewery");
        assert_eq!(names(&menus[0]), vec!["About Brewery", "---", "Quit Brewery"]);
        assert_eq!(
            names(&menus[1]),
            vec!["Show Welcome", "---", "File Bug Report...", "---", "Zed Documentation", "Brewery Repository"]
        );
        let MenuItem::Action { action, .. } = &menus[1].items[5] else { panic!("not an action") };
        let open = action
            .as_any()
            .downcast_ref::<zed_actions::OpenBrowser>()
            .expect("repository item opens a browser");
        assert_eq!(open.url, REPOSITORY_URL);
    }
```

Check `Menu::new`/`.items(...)`/`MenuItem::action` signatures in `crates/gpui/src/platform/app_menu.rs` and `Action::as_any` in gpui; adapt the test helpers to the real API (not the asserted names).

Run: `cargo test -p brewery_core` → FAIL to compile (`rebrand_menus` missing).

- [ ] **Step 2: Implement** — add `zed_actions.workspace = true` to `crates/brewery_core/Cargo.toml` `[dependencies]` (Brewery file, no marker), then in `brewery_core.rs`:

```rust
use gpui::{Menu, MenuItem};

pub const REPOSITORY_URL: &str = "https://github.com/hteariH/brewery";

const REMOVED_MENU_ITEMS: &[&str] = &[
    "Check for Updates",
    "View Release Notes Locally",
    "Email Us...",
    "Zed Twitter",
    "Join the Team",
    "View Telemetry",
];

/// Turns Zed's app menus into Brewery's: Brewery names, no Zed-only
/// destinations, bug reports to Brewery's repository.
pub fn rebrand_menus(menus: Vec<Menu>) -> Vec<Menu> {
    menus.into_iter().map(rebrand_menu).collect()
}

fn rebrand_menu(mut menu: Menu) -> Menu {
    if menu.name.as_ref() == "Zed" {
        menu.name = "Brewery".into();
    }
    let items = std::mem::take(&mut menu.items);
    let mut rebranded: Vec<MenuItem> = Vec::with_capacity(items.len());
    for item in items {
        let item = match item {
            MenuItem::Submenu(submenu) => MenuItem::Submenu(rebrand_menu(submenu)),
            MenuItem::Action { ref name, .. } if REMOVED_MENU_ITEMS.contains(&name.as_ref()) => continue,
            MenuItem::Action { name, action, os_action, checked, disabled } => {
                let (name, action) = match name.as_ref() {
                    "About Zed" => ("About Brewery".into(), action),
                    "Hide Zed" => ("Hide Brewery".into(), action),
                    "Quit Zed" => ("Quit Brewery".into(), action),
                    "Documentation" => ("Zed Documentation".into(), action),
                    "Zed Repository" => (
                        "Brewery Repository".into(),
                        open_browser(REPOSITORY_URL.to_string()),
                    ),
                    "File Bug Report..." | "Request Feature..." => (
                        name.clone(),
                        open_browser(format!("{REPOSITORY_URL}/issues/new")),
                    ),
                    _ => (name, action),
                };
                MenuItem::Action { name, action, os_action, checked, disabled }
            }
            other => other,
        };
        let is_separator = matches!(item, MenuItem::Separator);
        if is_separator && rebranded.last().is_none_or(|last| matches!(last, MenuItem::Separator)) {
            continue;
        }
        rebranded.push(item);
    }
    while matches!(rebranded.last(), Some(MenuItem::Separator)) {
        rebranded.pop();
    }
    menu.items = rebranded;
    menu
}

fn open_browser(url: String) -> Box<dyn gpui::Action> {
    Box::new(zed_actions::OpenBrowser { url })
}
```

Adapt field/variant names to gpui's real definitions if they differ (e.g. extra fields on `MenuItem::Action`), keeping the behaviour.

Hook-ins:

`crates/zed/src/main.rs`:
```rust
        // brewery: Brewery names, no Zed-only menu items
        let menus = brewery_core::rebrand_menus(app_menus(cx));
```
`crates/zed/src/zed.rs` `reload_menus`:
```rust
    // brewery: Brewery names, no Zed-only menu items
    let menus = brewery_core::rebrand_menus(app_menus(cx));
```

- [ ] **Step 3: Run tests — PASS**

Run: `cargo test -p brewery_core` and `cargo check -p zed` and `cargo clippy -p brewery_core --tests -- -D warnings`.

- [ ] **Step 4: Commit**

```bash
git add crates/brewery_core crates/zed/src/main.rs crates/zed/src/zed.rs Cargo.lock
git commit -m "feat: Brewery app menus without Zed-only destinations"
```

---

### Task 7: CI, docs and on-screen check

**Files:**
- Modify: `.github/workflows/brewery_ci.yml` (test list), `docs/brewery/FORK.md` (test list, regeneration commands), `README.md` (dev build note)

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Test list** — in `.github/workflows/brewery_ci.yml` and `docs/brewery/FORK.md` the hook-in test command becomes:
  `cargo test --locked -p brewery_cloud -p brewery_core -p brewery_theme -p release_channel -p paths -p settings -p client` (FORK.md without `--locked`).

- [ ] **Step 2: FORK.md** — add a section:

```markdown
## Regenerating look assets

    python script/brewery/gen_themes.py            # after Zed changes assets/themes/one/one.json
    python script/brewery/fetch_intellij_icons.py  # after Zed adds icon keys
```

- [ ] **Step 3: README** — under "Building", after `cargo run -p zed`, add:

```markdown
Development builds also need the CLI helper next to the app binary, or the
app prints `could not find zed-cli` on start (harmless, but git over SSH
cannot ask for passwords):

    cargo build -p cli -p zed
```

- [ ] **Step 4: Build and look** — `cargo build -p cli -p zed`, start `target/debug/brewery.exe` with a Java project (e.g. `C:\Users\aberk\claude\feedback-bot` is not Java; use any folder with `.java`, `.gradle`, `.yml` and `.xml` files, or create one under the scratchpad), capture the window (controller), and check: dark IntelliJ-like colours, JetBrains Mono in the editor, Inter in the UI, IntelliJ file icons in the project panel, "Welcome to Brewery", no telemetry switches on onboarding, `Help` menu without Zed-only items, Brewery logo.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/brewery_ci.yml docs/brewery/FORK.md README.md
git commit -m "docs: M1a test list, regeneration commands, dev build note"
```
