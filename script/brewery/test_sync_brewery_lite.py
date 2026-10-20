"""Tests for sync_brewery_lite.py.

    python -m unittest -v script/brewery/test_sync_brewery_lite.py
"""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("sync_brewery_lite", HERE / "sync_brewery_lite.py")
sync = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sync)

# The IDE's icon theme names icons relative to its own directory (crates/brewery_theme/icons).
ICON_THEME = {
    "$schema": "https://zed.dev/schema/icon_themes/v0.3.0.json",
    "name": "Brewery Icons",
    "themes": [
        {
            "name": "Brewery Icons Dark",
            "appearance": "dark",
            "directory_icons": {"collapsed": "dark/folder.svg", "expanded": "dark/folder.svg"},
            "named_directory_icons": {"src": {"collapsed": "dark/src.svg", "expanded": "dark/src.svg"}},
            "chevron_icons": {"collapsed": "dark/right.svg", "expanded": "dark/down.svg"},
            "file_stems": {"Dockerfile": "docker"},
            "file_suffixes": {"java": "java"},
            "file_icons": {"java": {"path": "dark/java.svg"}},
        }
    ],
}


class SyncTest(unittest.TestCase):
    def make_repo(self, root: Path):
        (root / "assets/themes/brewery").mkdir(parents=True)
        (root / "assets/themes/brewery/brewery.json").write_text('{"name":"Brewery"}\n')
        icons = root / "crates/brewery_theme/icons"
        (icons / "dark").mkdir(parents=True)
        (icons / "dark/java.svg").write_text("<svg/>")
        (icons / "NOTICE").write_text("Apache-2.0 icons\n")
        (icons / "icon_theme.json").write_text(json.dumps(ICON_THEME, indent=2) + "\n")
        (root / "crates/zed").mkdir(parents=True)
        (root / "crates/zed/BREWERY_VERSION").write_text("1.2.3\n")

    def make_manifests(self, root: Path, version: str):
        ext = root / "extensions/brewery-lite"
        ext.mkdir(parents=True, exist_ok=True)
        (ext / "extension.toml").write_text(
            f'id = "brewery-lite"\nversion = "{version}"\nschema_version = 1\n'
        )
        (ext / "Cargo.toml").write_text(
            f'[package]\nname = "brewery_lite"\nversion = "{version}"\n\n'
            '[dependencies]\nzed_extension_api = "0.7.0"\n'
        )

    def test_copy_then_check_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_repo(root)
            self.assertEqual(sync.sync(root, check=False), [])
            ext = root / "extensions/brewery-lite"
            self.assertTrue((ext / "themes/brewery.json").is_file())
            self.assertTrue((ext / "icon_themes/brewery-icons.json").is_file())
            self.assertTrue((ext / "icons/dark/java.svg").is_file())
            self.assertTrue((ext / "icons/NOTICE").is_file())
            self.assertFalse((ext / "icons/icon_theme.json").exists())
            self.assertEqual(sync.sync(root, check=True), [])

    def test_check_reports_drift(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_repo(root)
            sync.sync(root, check=False)
            (root / "crates/brewery_theme/icons/dark/java.svg").write_text("<svg changed/>")
            (root / "extensions/brewery-lite/icons/dark/stale.svg").write_text("<svg/>")
            drift = sync.sync(root, check=True)
            self.assertIn("icons/dark/java.svg", drift)
            self.assertIn("icons/dark/stale.svg", drift)
            # check=True never writes.
            self.assertTrue((root / "extensions/brewery-lite/icons/dark/stale.svg").exists())
            self.assertEqual(sync.sync(root, check=False), [])
            self.assertFalse((root / "extensions/brewery-lite/icons/dark/stale.svg").exists())
            self.assertEqual(sync.sync(root, check=True), [])

    def test_icon_paths_are_relative_to_the_extension_root(self):
        # Zed resolves an extension's icon paths against the extension directory, so the
        # IDE's "dark/x.svg" (relative to crates/brewery_theme/icons) becomes "icons/dark/x.svg".
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_repo(root)
            sync.sync(root, check=False)
            out = json.loads(
                (root / "extensions/brewery-lite/icon_themes/brewery-icons.json").read_text()
            )
            self.assertEqual(out["$schema"], ICON_THEME["$schema"])
            self.assertEqual(out["name"], "Brewery Icons")
            theme = out["themes"][0]
            self.assertEqual(
                theme["directory_icons"],
                {"collapsed": "icons/dark/folder.svg", "expanded": "icons/dark/folder.svg"},
            )
            self.assertEqual(
                theme["named_directory_icons"]["src"],
                {"collapsed": "icons/dark/src.svg", "expanded": "icons/dark/src.svg"},
            )
            self.assertEqual(
                theme["chevron_icons"],
                {"collapsed": "icons/dark/right.svg", "expanded": "icons/dark/down.svg"},
            )
            self.assertEqual(theme["file_icons"], {"java": {"path": "icons/dark/java.svg"}})
            # Keys that aren't paths are untouched.
            self.assertEqual(theme["file_stems"], {"Dockerfile": "docker"})
            self.assertEqual(theme["file_suffixes"], {"java": "java"})

    def test_icon_paths_with_dot_slash_are_rewritten_once(self):
        self.assertEqual(sync.extension_icon_path("./dark/a.svg"), "icons/dark/a.svg")
        self.assertEqual(sync.extension_icon_path("dark/a.svg"), "icons/dark/a.svg")

    def test_every_rewritten_icon_path_exists(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_repo(root)
            icons = root / "crates/brewery_theme/icons/dark"
            for name in ("folder.svg", "src.svg", "right.svg", "down.svg"):
                (icons / name).write_text("<svg/>")
            sync.sync(root, check=False)
            ext = root / "extensions/brewery-lite"
            for path in sync.icon_paths(json.loads((ext / "icon_themes/brewery-icons.json").read_text())):
                self.assertTrue((ext / path).is_file(), path)

    def test_versions_follow_brewery_version(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_repo(root)
            self.make_manifests(root, "0.0.9")
            sync.sync(root, check=False)
            ext = root / "extensions/brewery-lite"
            self.assertEqual(
                sorted(sync.sync(root, check=True)), []
            )
            # Bumping BREWERY_VERSION leaves both manifests out of date.
            (root / "crates/zed/BREWERY_VERSION").write_text("1.3.0\n")
            self.assertEqual(sync.sync(root, check=True), ["Cargo.toml", "extension.toml"])
            self.assertIn('version = "1.2.3"', (ext / "Cargo.toml").read_text())
            sync.sync(root, check=False)
            cargo = (ext / "Cargo.toml").read_text()
            self.assertIn('version = "1.3.0"', cargo)
            self.assertIn('zed_extension_api = "0.7.0"', cargo)
            self.assertIn('version = "1.3.0"', (ext / "extension.toml").read_text())
            self.assertIn('schema_version = 1', (ext / "extension.toml").read_text())
            self.assertEqual(sync.sync(root, check=True), [])

    def test_main_check_exit_codes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_repo(root)
            self.assertEqual(sync.main(["--check"], repo=root), 1)
            self.assertEqual(sync.main([], repo=root), 0)
            self.assertEqual(sync.main(["--check"], repo=root), 0)


if __name__ == "__main__":
    unittest.main()
