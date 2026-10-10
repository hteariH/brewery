"""Fetch IntelliJ New UI icons (Apache 2.0) and write Brewery's icon theme.

Pinned to one intellij-community commit. Every icon key Zed's default icon
theme knows (crates/theme/src/icon_theme.rs FILE_ICONS) is mapped: to a
specific IntelliJ icon when there is one, otherwise to the plain text icon.
Run from the repo root:  python script/brewery/fetch_intellij_icons.py
"""

import json
import re
import urllib.error
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
        "https://www.apache.org/licenses/LICENSE-2.0\n"
        "A copy of the Apache License 2.0 is in LICENSE-APACHE at the root of this repository.\n",
        encoding="utf-8",
        newline="\n",
    )
    print(f"{len(keys)} Zed icon keys mapped to {len(needed)} IntelliJ icons per variant")


if __name__ == "__main__":
    main()
