"""Tests for mac_info_plist.py (the macOS Info.plist of the unsigned zip).

Run from the repo root:
    python -m unittest discover script/brewery
"""

import plistlib
import re
import unittest
from pathlib import Path

import mac_info_plist as mip

ROOT = Path(__file__).resolve().parents[2]


def rust_app_ids():
    """ReleaseChannel::app_id() as written in crates/release_channel."""
    src = (ROOT / "crates/release_channel/src/lib.rs").read_text(encoding="utf-8")
    body = src[src.index("pub fn app_id(&self)"):]
    body = body[:body.index("\n    }\n")]
    return dict(re.findall(r"ReleaseChannel::(\w+) => \"([^\"]+)\"", body))


class InfoPlistTests(unittest.TestCase):
    def build(self, channel="stable", version="1.2.3"):
        data = mip.build_info_plist(channel, version, ROOT / "crates/zed/resources/info")
        return plistlib.loads(data)

    def test_identifier_equals_the_rust_app_id_for_every_channel(self):
        ids = rust_app_ids()
        self.assertEqual(set(ids), {"Dev", "Nightly", "Preview", "Stable"})
        for channel in ("dev", "nightly", "preview", "stable"):
            self.assertEqual(self.build(channel)["CFBundleIdentifier"], ids[channel.capitalize()])

    def test_cargo_bundle_metadata_matches(self):
        toml = (ROOT / "crates/zed/Cargo.toml").read_text(encoding="utf-8")
        for channel in ("dev", "nightly", "preview", "stable"):
            block = toml.split(f"[package.metadata.bundle-{channel}]")[1].split("\n[")[0]
            self.assertIn(f'identifier = "{mip.CHANNELS[channel][1]}"', block)
            self.assertIn(f'name = "{mip.CHANNELS[channel][0]}"', block)

    def test_names_and_versions(self):
        p = self.build("stable", "1.2.3")
        self.assertEqual(p["CFBundleExecutable"], "brewery")
        self.assertEqual(p["CFBundleName"], "Brewery")
        self.assertEqual(p["CFBundleDisplayName"], "Brewery")
        self.assertEqual(p["CFBundleShortVersionString"], "1.2.3")
        self.assertEqual(p["CFBundleVersion"], "1.2.3")
        self.assertEqual(p["CFBundleIconFile"], "Brewery")
        self.assertEqual(self.build("dev")["CFBundleName"], "Brewery Dev")

    def test_no_zed_scheme_and_no_zed_names(self):
        p = self.build("stable")
        self.assertNotIn("CFBundleURLTypes", p)
        self.assertNotIn("Zed", plistlib.dumps(p).decode())

    def test_fragments_are_merged(self):
        p = self.build()
        self.assertIn("CFBundleDocumentTypes", p)
        self.assertIn("NSCameraUsageDescription", p)
        self.assertEqual(p["CFBundleSupportedPlatforms"], ["MacOSX"])

    def test_bad_input(self):
        with self.assertRaises(SystemExit):
            mip.build_info_plist("beta", "1.0.0", ROOT / "crates/zed/resources/info")
        with self.assertRaises(SystemExit):
            mip.build_info_plist("stable", "v1", ROOT / "crates/zed/resources/info")


if __name__ == "__main__":
    unittest.main()
