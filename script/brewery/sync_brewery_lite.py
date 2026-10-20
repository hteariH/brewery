"""Keeps extensions/brewery-lite's theme, icons and version identical to the IDE's.

    python script/brewery/sync_brewery_lite.py          # copy
    python script/brewery/sync_brewery_lite.py --check  # exit 1 and list drifted files

Copies assets/themes/brewery/brewery.json and crates/brewery_theme/icons/** into the
extension, rewriting the icon theme's paths to be relative to the extension root (where
Zed resolves them), and keeps the `version` of extension.toml, Cargo.toml and the
extension's own Cargo.lock entry equal to crates/zed/BREWERY_VERSION, so the extension downloads the daemon jar of its own release.
"""
import argparse
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
EXT = Path("extensions/brewery-lite")
VERSION_LINE = re.compile(r'^(version = )"[^"]*"(?=\r?$)', re.MULTILINE)
# Cargo.lock starts with its own format `version = 4`; only the extension's package entry
# follows BREWERY_VERSION, or `cargo --locked` fails after a bump.
LOCK_VERSION_LINE = re.compile(
    r'^(\[\[package\]\]\r?\nname = "brewery_lite"\r?\nversion = )"[^"]*"(?=\r?$)', re.MULTILINE
)
# File relative to the extension: the line holding the extension's version.
VERSIONED = {
    "extension.toml": VERSION_LINE,
    "Cargo.toml": VERSION_LINE,
    "Cargo.lock": LOCK_VERSION_LINE,
}


def extension_icon_path(path: str) -> str:
    """An icon path relative to crates/brewery_theme/icons, made relative to the extension root."""
    return "icons/" + path.removeprefix("./")


def _icon_slots(theme: dict):
    """(dict, key) for every icon path of one icon theme (Zed's icon theme schema v0.3.0)."""
    for group in ("directory_icons", "chevron_icons"):
        for key in ("collapsed", "expanded"):
            if key in theme.get(group, {}):
                yield theme[group], key
    for icons in theme.get("named_directory_icons", {}).values():
        for key in ("collapsed", "expanded"):
            if key in icons:
                yield icons, key
    for icon in theme.get("file_icons", {}).values():
        if "path" in icon:
            yield icon, "path"


def icon_paths(family: dict) -> list:
    return [slot[key] for theme in family.get("themes", []) for slot, key in _icon_slots(theme)]


def _extension_icon_theme(data: bytes) -> bytes:
    family = json.loads(data)
    for theme in family.get("themes", []):
        for slot, key in _icon_slots(theme):
            slot[key] = extension_icon_path(slot[key])
    return (json.dumps(family, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def _wanted(repo: Path) -> dict:
    """{path relative to the extension: wanted bytes}."""
    files = {"themes/brewery.json": (repo / "assets/themes/brewery/brewery.json").read_bytes()}
    icons = repo / "crates/brewery_theme/icons"
    for path in sorted(p for p in icons.rglob("*") if p.is_file()):
        rel = path.relative_to(icons).as_posix()
        if rel == "icon_theme.json":
            files["icon_themes/brewery-icons.json"] = _extension_icon_theme(path.read_bytes())
        else:
            files[f"icons/{rel}"] = path.read_bytes()
    ext = repo / EXT
    manifests = [name for name in VERSIONED if (ext / name).is_file()]
    if manifests:
        version = (repo / "crates/zed/BREWERY_VERSION").read_text(encoding="utf-8").strip()
        for name in manifests:
            text = (ext / name).read_bytes().decode("utf-8")
            line = VERSIONED[name]
            files[name] = line.sub(rf'\g<1>"{version}"', text, count=1).encode("utf-8")
    return files


def _managed(ext: Path) -> set:
    out = set()
    for top in ("themes", "icon_themes", "icons"):
        base = ext / top
        if base.exists():
            out |= {p.relative_to(ext).as_posix() for p in base.rglob("*") if p.is_file()}
    return out


def sync(repo: Path, check: bool) -> list:
    """Copies (or with check=True only compares) and returns the drifted relative paths."""
    ext = repo / EXT
    wanted = _wanted(repo)
    drift = sorted(
        [rel for rel, data in wanted.items() if not (ext / rel).is_file() or (ext / rel).read_bytes() != data]
        + [rel for rel in _managed(ext) if rel not in wanted]
    )
    if not check:
        for rel in drift:
            target = ext / rel
            if rel in wanted:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(wanted[rel])
            else:
                target.unlink()
        return []
    return drift


def main(argv=None, repo: Path = REPO) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="only report drifted files; exit 1 if any")
    args = parser.parse_args(argv)
    drift = sync(repo, args.check)
    for rel in drift:
        print(f"out of date: {EXT.as_posix()}/{rel}", file=sys.stderr)
    if drift:
        print("run: python script/brewery/sync_brewery_lite.py", file=sys.stderr)
    return 1 if drift else 0


if __name__ == "__main__":
    sys.exit(main())
