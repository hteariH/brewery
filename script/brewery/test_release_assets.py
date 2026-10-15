"""Tests for release_assets.py.

Run from the repo root:
    python -m unittest discover script/brewery
"""

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import release_assets as ra

VERSION = "0.1.0"
SHA = {name: f"{i:x}" * 64 for i, name in enumerate(ra.package_names(VERSION).values(), start=1)}
BASE = "https://github.com/hteariH/brewery/releases/download/v0.1.0"


class NamesTests(unittest.TestCase):
    def test_exact_asset_names(self):
        names = ra.package_names(VERSION)
        self.assertEqual(names["windows"], "Brewery-0.1.0-x86_64-setup.exe")
        self.assertEqual(names["linux"], "brewery-0.1.0-linux-x86_64.tar.gz")
        self.assertEqual(names["deb"], "brewery_0.1.0_amd64.deb")
        self.assertEqual(names["rpm"], "brewery-0.1.0-1.x86_64.rpm")
        self.assertEqual(names["macos"], "brewery-0.1.0-macos-aarch64.zip")
        self.assertEqual(ra.OPTIONAL, {"macos"})


class TagTests(unittest.TestCase):
    def test_tag_must_equal_v_plus_version_file(self):
        self.assertEqual(ra.check_tag("v0.1.0", "0.1.0\n"), ("0.1.0", False))

    def test_prerelease_needs_the_file_to_say_the_same(self):
        self.assertEqual(ra.check_tag("v0.1.0-rc.1", "0.1.0-rc.1"), ("0.1.0-rc.1", True))
        with self.assertRaises(ValueError):
            ra.check_tag("v0.1.0-rc.1", "0.1.0")

    def test_mismatches_and_junk_are_rejected(self):
        for tag, file in [("v0.1.1", "0.1.0"), ("0.1.0", "0.1.0"), ("v0.1", "0.1"), ("vx", "x"), ("v0.1.0", "0.1.0-rc.1")]:
            with self.subTest(tag=tag, file=file), self.assertRaises(ValueError):
                ra.check_tag(tag, file)


class ChecksumTests(unittest.TestCase):
    def test_sha256sums_format_and_order(self):
        text = ra.sha256sums({"b.zip": "b" * 64, "a.exe": "A" * 64})
        self.assertEqual(text, f"{'a' * 64}  a.exe\n{'b' * 64}  b.zip\n")

    def test_rejects_bad_hash(self):
        with self.assertRaises(ValueError):
            ra.sha256sums({"a": "xyz"})

    def test_parse_roundtrip(self):
        sums = {"a.exe": "a" * 64, "b.zip": "b" * 64}
        self.assertEqual(ra.parse_sha256sums(ra.sha256sums(sums)), sums)


class LatestJsonTests(unittest.TestCase):
    def manifest(self, **kw):
        sums = dict(SHA)
        for name in kw.pop("drop", []):
            sums.pop(ra.package_names(VERSION)[name])
        return ra.latest_json(VERSION, sums, published_at="2026-10-15T00:00:00Z", **kw)

    def test_content(self):
        data = json.loads(self.manifest())
        names = ra.package_names(VERSION)
        self.assertEqual(data["version"], "0.1.0")
        self.assertEqual(data["notes_url"], "https://github.com/hteariH/brewery/releases/tag/v0.1.0")
        self.assertEqual(set(data["assets"]), {"windows-x86_64", "linux-x86_64", "macos-aarch64"})
        self.assertEqual(
            data["assets"]["windows-x86_64"],
            {"url": f"{BASE}/{names['windows']}", "sha256": SHA[names["windows"]]},
        )
        self.assertEqual(data["assets"]["linux-x86_64"]["url"], f"{BASE}/{names['linux']}")
        self.assertEqual(data["assets"]["macos-aarch64"]["url"], f"{BASE}/{names['macos']}")

    def test_missing_macos_is_left_out(self):
        data = json.loads(self.manifest(drop=["macos"]))
        self.assertEqual(set(data["assets"]), {"windows-x86_64", "linux-x86_64"})

    def test_missing_required_asset_fails(self):
        for name in ("windows", "linux"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.manifest(drop=[name])

    def test_passes_the_updater_rules(self):
        ra.validate_manifest(self.manifest().encode(), VERSION)
        ra.validate_manifest(self.manifest(drop=["macos"]).encode(), VERSION)

    def test_validator_mirrors_the_updater_rules(self):
        good = json.loads(self.manifest())

        def check(mutate):
            data = json.loads(json.dumps(good))
            mutate(data)
            with self.assertRaises(ValueError):
                ra.validate_manifest(json.dumps(data).encode(), VERSION)

        check(lambda d: d.update(version="1.0"))
        check(lambda d: d.update(version="0.2.0"))
        check(lambda d: d.update(notes_url="https://example.test/x"))
        check(lambda d: d["assets"]["windows-x86_64"].update(url="http://x/y.exe"))
        check(lambda d: d["assets"]["windows-x86_64"].update(sha256="12"))
        check(lambda d: d["assets"].update({"beos-x86_64": d["assets"]["windows-x86_64"]}))
        check(lambda d: d["assets"].pop("windows-x86_64"))
        with self.assertRaises(ValueError):
            ra.validate_manifest(b"not json", VERSION)
        with self.assertRaises(ValueError):
            ra.validate_manifest(b" " * (ra.MAX_MANIFEST_BYTES + 1), VERSION)

    def test_v_prefix_in_version_is_allowed_by_the_updater(self):
        data = json.loads(self.manifest())
        data["version"] = "v0.1.0"
        ra.validate_manifest(json.dumps(data).encode(), VERSION)


class NotesTests(unittest.TestCase):
    SUMS = ra.sha256sums({"a.exe": "a" * 64})
    CHANGELOG = "# Changelog\n\n## 0.2.0\n\n- later\n\n## 0.1.0\n\n- first thing\n- second\n\n## 0.0.1\n\n- old\n"

    def test_changelog_section_is_used(self):
        body = ra.release_notes(VERSION, self.CHANGELOG, self.SUMS)
        self.assertIn("- first thing", body)
        self.assertIn("- second", body)
        self.assertNotIn("later", body)
        self.assertNotIn("- old", body)
        self.assertNotIn("Install", body)

    def test_template_when_no_section(self):
        body = ra.release_notes(VERSION, "# Changelog\n", self.SUMS)
        self.assertIn("Brewery 0.1.0", body)
        self.assertIn("Brewery-0.1.0-x86_64-setup.exe", body)
        self.assertIn("unsigned", body.lower())
        body_none = ra.release_notes(VERSION, None, self.SUMS)
        self.assertEqual(body, body_none)

    def test_checksums_block_is_appended(self):
        for changelog in (self.CHANGELOG, None):
            body = ra.release_notes(VERSION, changelog, self.SUMS)
            self.assertTrue(body.rstrip().endswith("```"))
            self.assertIn("SHA256SUMS", body)
            self.assertIn(f"{'a' * 64}  a.exe", body)

    def test_prefix_versions_do_not_match(self):
        body = ra.release_notes("0.1", "## 0.1.0\n\n- no\n", self.SUMS)
        self.assertNotIn("- no", body)


class JreSourceTests(unittest.TestCase):
    def test_reads_the_real_pins(self):
        text = ra.jre_source_text(Path(__file__).with_name("bundle-deps.toml"))
        self.assertIn("Eclipse Temurin", text)
        self.assertIn("OpenJDK21U-jdk-sources", text)
        self.assertIn("GPL", text)


class AssembleTests(unittest.TestCase):
    def make_artifacts(self, root: Path, *, macos=True, tamper_pkgbuild=False):
        names = ra.package_names(VERSION)
        layout = {
            "package-windows-x64": ["windows"],
            "package-linux-x64": ["linux", "deb", "rpm"],
            "package-macos-aarch64": ["macos"] if macos else [],
        }
        for artifact, keys in layout.items():
            (root / artifact).mkdir(parents=True)
            for key in keys:
                (root / artifact / names[key]).write_bytes(key.encode() * 10)
        tarball_sha = hashlib.sha256(b"linux" * 10).hexdigest()
        (root / "package-aur").mkdir()
        sha = "0" * 64 if tamper_pkgbuild else tarball_sha
        (root / "package-aur" / "PKGBUILD").write_text(f"sha256sums=('{sha}')\n")
        (root / "package-aur" / ".SRCINFO").write_text("pkgbase = brewery-bin\n")

    def run_assemble(self, root: Path, out: Path, **kw):
        return ra.assemble(
            VERSION,
            root,
            out,
            changelog=kw.get("changelog"),
            render=lambda installer_sha256, release_date: {"winget/x.yaml": f"sha={installer_sha256}\n"},
            published_at="2026-10-15T00:00:00Z",
        )

    def test_assemble_everything(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, out = Path(tmp, "in"), Path(tmp, "out")
            self.make_artifacts(root)
            self.run_assemble(root, out)
            files = {p.name for p in (out / "assets").iterdir()}
            names = ra.package_names(VERSION)
            self.assertEqual(
                files,
                set(names.values())
                | {
                    "SHA256SUMS",
                    "latest.json",
                    "jre-source.txt",
                    "brewery-0.1.0-winget.zip",
                    "brewery-0.1.0-aur.zip",
                },
            )
            sums = ra.parse_sha256sums((out / "assets" / "SHA256SUMS").read_text())
            self.assertNotIn("SHA256SUMS", sums)
            self.assertEqual(set(sums), files - {"SHA256SUMS"})
            for name, digest in sums.items():
                self.assertEqual(hashlib.sha256((out / "assets" / name).read_bytes()).hexdigest(), digest)
            ra.validate_manifest((out / "assets" / "latest.json").read_bytes(), VERSION)
            self.assertTrue((out / "winget" / "x.yaml").is_file())
            self.assertTrue((out / "aur" / ".SRCINFO").is_file())
            self.assertIn("SHA256SUMS", (out / "release-notes.md").read_text())
            win = names["windows"]
            self.assertIn(f"sha={sums[win]}", (out / "winget" / "x.yaml").read_text())

    def test_macos_is_optional(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, out = Path(tmp, "in"), Path(tmp, "out")
            self.make_artifacts(root, macos=False)
            self.run_assemble(root, out)
            data = json.loads((out / "assets" / "latest.json").read_text())
            self.assertNotIn("macos-aarch64", data["assets"])
            self.assertFalse((out / "assets" / ra.package_names(VERSION)["macos"]).exists())

    def test_missing_required_package_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, out = Path(tmp, "in"), Path(tmp, "out")
            self.make_artifacts(root)
            (root / "package-linux-x64" / ra.package_names(VERSION)["deb"]).unlink()
            with self.assertRaisesRegex(ValueError, "brewery_0.1.0_amd64.deb"):
                self.run_assemble(root, out)

    def test_pkgbuild_must_carry_the_tarball_checksum(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, out = Path(tmp, "in"), Path(tmp, "out")
            self.make_artifacts(root, tamper_pkgbuild=True)
            with self.assertRaisesRegex(ValueError, "PKGBUILD"):
                self.run_assemble(root, out)


if __name__ == "__main__":
    unittest.main()
