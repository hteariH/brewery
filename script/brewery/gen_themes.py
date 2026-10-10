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
