"""Tests for render_manifests.py.

Run from the repo root:
    python -m unittest discover script/brewery
"""

import hashlib
import io
import re
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path

import render_manifests as rm

SHA_A = "a" * 64
SHA_B = "b" * 64


def render(**overrides):
    args = dict(
        version="0.1.0",
        tarball_sha256=SHA_A,
        installer_sha256=SHA_B,
        release_date="2026-10-14",
    )
    args.update(overrides)
    return rm.render_all(**args)


class RenderTests(unittest.TestCase):
    def test_aur_fills_version_url_and_checksum(self):
        pkgbuild = render()["aur/PKGBUILD"]
        self.assertIn("pkgname=brewery-bin", pkgbuild)
        self.assertIn("pkgver=0.1.0\n", pkgbuild)
        self.assertIn(
            'source=("https://github.com/hteariH/brewery/releases/download/v0.1.0/'
            'brewery-0.1.0-linux-x86_64.tar.gz")',
            pkgbuild,
        )
        self.assertIn(f"sha256sums=('{SHA_A}')", pkgbuild)
        self.assertIn("license=('GPL-3.0-or-later')", pkgbuild)
        self.assertIn("provides=('brewery')", pkgbuild)
        self.assertIn("conflicts=('brewery')", pkgbuild)

    def test_aur_source_override_names_the_local_file(self):
        pkgbuild = render(source_url="file:///pkg/brewery-0.1.0-linux-x86_64.tar.gz")["aur/PKGBUILD"]
        self.assertIn(
            'source=("brewery-0.1.0-linux-x86_64.tar.gz::file:///pkg/brewery-0.1.0-linux-x86_64.tar.gz")',
            pkgbuild,
        )

    def test_aur_pkgver_has_no_hyphen(self):
        pkgbuild = render(version="0.2.0-rc.1")["aur/PKGBUILD"]
        self.assertIn("pkgver=0.2.0_rc.1\n", pkgbuild)
        self.assertIn("/download/v0.2.0-rc.1/brewery-0.2.0-rc.1-linux-x86_64.tar.gz", pkgbuild)

    def test_winget_installer_fields(self):
        files = render()
        installer = files["winget/hteariH.Brewery.installer.yaml"]
        self.assertIn("PackageIdentifier: hteariH.Brewery\n", installer)
        self.assertIn("PackageVersion: 0.1.0\n", installer)
        self.assertIn("InstallerType: inno\n", installer)
        self.assertIn("Scope: user\n", installer)
        self.assertIn("UpgradeBehavior: install\n", installer)
        self.assertIn("  - brewery\n", installer)
        self.assertIn("ReleaseDate: 2026-10-14\n", installer)
        self.assertIn(
            "InstallerUrl: https://github.com/hteariH/brewery/releases/download/v0.1.0/"
            "Brewery-0.1.0-x86_64-setup.exe\n",
            installer,
        )
        self.assertIn(f"InstallerSha256: {SHA_B.upper()}\n", installer)
        self.assertIn("ProductCode: '{96B775F6-6B94-45D9-B9AB-17BEEACD3A33}_is1'", installer)

    def test_winget_locale_and_version(self):
        files = render()
        locale = files["winget/hteariH.Brewery.locale.en-US.yaml"]
        self.assertIn("License: GPL-3.0-or-later\n", locale)
        self.assertIn("Publisher: Brewery contributors\n", locale)
        self.assertIn("PackageName: Brewery\n", locale)
        self.assertIn("releases/tag/v0.1.0\n", locale)
        version = files["winget/hteariH.Brewery.yaml"]
        self.assertIn("DefaultLocale: en-US\n", version)
        self.assertIn("ManifestType: version\n", version)

    def test_winget_file_names_match_the_identifier(self):
        names = sorted(n for n in render() if n.startswith("winget/"))
        self.assertEqual(
            names,
            [
                "winget/hteariH.Brewery.installer.yaml",
                "winget/hteariH.Brewery.locale.en-US.yaml",
                "winget/hteariH.Brewery.yaml",
            ],
        )

    def test_no_placeholder_survives(self):
        for name, text in render().items():
            self.assertIsNone(re.search(r"@[A-Z_]+@", text), name)

    def test_repo_override_changes_every_url(self):
        text = "\n".join(render(repo="someone/fork").values())
        self.assertIn("github.com/someone/fork/releases/download/v0.1.0/", text)
        self.assertNotIn("hteariH/brewery/releases", text)

    def test_installer_url_override(self):
        installer = render(installer_url="https://example.test/x.exe")["winget/hteariH.Brewery.installer.yaml"]
        self.assertIn("InstallerUrl: https://example.test/x.exe\n", installer)

    def test_rejects_bad_input(self):
        for bad in ("", "1.0", "v1.0.0", "1.0.0 ", "1.0.0\nX: y", "latest"):
            with self.assertRaises(ValueError, msg=bad):
                render(version=bad)
        for bad in ("abc", "a" * 63, "g" * 64, ""):
            with self.assertRaises(ValueError, msg=bad):
                render(tarball_sha256=bad)
        with self.assertRaises(ValueError):
            render(release_date="14.10.2026")

    def test_unknown_placeholder_in_template_fails(self):
        with self.assertRaises(ValueError):
            rm.fill("x: @NOPE@\n", {"VERSION": "1"})

    def test_unused_value_is_fine_but_templates_must_be_complete(self):
        self.assertEqual(rm.fill("v=@VERSION@", {"VERSION": "1", "EXTRA": "2"}), "v=1")


class CliTests(unittest.TestCase):
    def test_cli_hashes_files_and_writes_lf_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            tarball = tmp / "t.tar.gz"
            installer = tmp / "s.exe"
            tarball.write_bytes(b"tarball")
            installer.write_bytes(b"installer")
            out = tmp / "out"
            rc = rm.main(
                [
                    "--version", "0.1.0", "--tarball", str(tarball), "--installer", str(installer),
                    "--release-date", "2026-10-14", "--out", str(out),
                ]
            )
            self.assertEqual(rc, 0)
            pkgbuild = (out / "aur" / "PKGBUILD").read_bytes()
            self.assertIn(hashlib.sha256(b"tarball").hexdigest().encode(), pkgbuild)
            self.assertNotIn(b"\r", pkgbuild)
            installer_yaml = (out / "winget" / "hteariH.Brewery.installer.yaml").read_bytes()
            self.assertIn(hashlib.sha256(b"installer").hexdigest().upper().encode(), installer_yaml)
            self.assertNotIn(b"\r", installer_yaml)

    def test_cli_only_aur_needs_no_installer(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "out"
            rc = rm.main(["--version", "0.1.0", "--tarball-sha256", SHA_A, "--only", "aur", "--out", str(out)])
            self.assertEqual(rc, 0)
            self.assertTrue((out / "aur" / "PKGBUILD").exists())
            self.assertFalse((out / "winget").exists())

    def test_cli_missing_checksum_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            with redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as ctx:
                    rm.main(["--version", "0.1.0", "--out", tmp])
            self.assertNotEqual(ctx.exception.code, 0)


class RealTemplateTests(unittest.TestCase):
    def test_templates_have_the_required_fixed_values(self):
        root = Path(__file__).resolve().parents[2] / "packaging"
        installer = (root / "winget/Brewery.installer.yaml.in").read_text()
        for needle in ("InstallerType: inno", "Scope: user", "  - brewery"):
            self.assertIn(needle, installer)
        self.assertIn("License: GPL-3.0-or-later", (root / "winget/Brewery.locale.en-US.yaml.in").read_text())
        for path in root.glob("**/*.in"):
            self.assertNotIn(b"", path.read_bytes(), path.name)


if __name__ == "__main__":
    unittest.main()
