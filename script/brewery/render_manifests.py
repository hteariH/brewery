#!/usr/bin/env python3
"""Renders the AUR PKGBUILD and the winget manifests from packaging/*/*.in.

Placeholders are `@NAME@`. Inputs are explicit: the version, and either the release
files (their SHA-256 is computed) or the checksums themselves.

    script/brewery/render_manifests.py --version 0.1.0 \\
        --tarball target/package/brewery-0.1.0-linux-x86_64.tar.gz \\
        --installer target/installer/Brewery-0.1.0-x86_64-setup.exe --out target/manifests

Writes <out>/aur/PKGBUILD and <out>/winget/hteariH.Brewery{,.installer,.locale.en-US}.yaml
(winget wants the file names to start with the PackageIdentifier). `.SRCINFO` needs
makepkg: `cd <out>/aur && makepkg --printsrcinfo > .SRCINFO` (see smoke-aur-package).
The URLs point at the GitHub release `v<version>` of --repo unless overridden.
"""

import argparse
import datetime
import hashlib
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PACKAGING = REPO_ROOT / "packaging"
DEFAULT_REPO = "hteariH/brewery"
PACKAGE_IDENTIFIER = "hteariH.Brewery"
# AppId of the stable channel in crates/zed/resources/windows/brewery.iss; Inno's uninstall
# key is "<AppId>_is1", which winget uses to match an installed copy for upgrades.
STABLE_PRODUCT_CODE = "{96B775F6-6B94-45D9-B9AB-17BEEACD3A33}_is1"

VERSION_RE = re.compile(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z][0-9A-Za-z.]*)?")
SHA256_RE = re.compile(r"[0-9a-fA-F]{64}")
DATE_RE = re.compile(r"\d{4}-\d{2}-\d{2}")
PLACEHOLDER_RE = re.compile(r"@([A-Z_0-9]+)@")

# (template, output path relative to --out)
AUR_TEMPLATES = [("aur/PKGBUILD.in", "aur/PKGBUILD")]
WINGET_TEMPLATES = [
    ("winget/Brewery.yaml.in", f"winget/{PACKAGE_IDENTIFIER}.yaml"),
    ("winget/Brewery.installer.yaml.in", f"winget/{PACKAGE_IDENTIFIER}.installer.yaml"),
    ("winget/Brewery.locale.en-US.yaml.in", f"winget/{PACKAGE_IDENTIFIER}.locale.en-US.yaml"),
]


def fill(template: str, values: dict) -> str:
    def sub(match):
        name = match.group(1)
        if name not in values:
            raise ValueError(f"template uses @{name}@ but no value was given")
        return values[name]

    return PLACEHOLDER_RE.sub(sub, template)


def _check_sha(name: str, value) -> str:
    if not SHA256_RE.fullmatch(value or ""):
        raise ValueError(f"{name} must be 64 hex digits, got {value!r}")
    return value.lower()


def _aur_source(version: str, repo: str, source_url) -> str:
    filename = f"brewery-{version}-linux-x86_64.tar.gz"
    if source_url is None:
        return f"https://github.com/{repo}/releases/download/v{version}/{filename}"
    if "::" in source_url:
        return source_url
    return f"{filename}::{source_url}"


def render_all(
    *,
    version,
    tarball_sha256=None,
    installer_sha256=None,
    repo=DEFAULT_REPO,
    source_url=None,
    installer_url=None,
    release_date=None,
    product_code=STABLE_PRODUCT_CODE,
    only=None,
    templates_dir=PACKAGING,
) -> dict:
    """Returns {relative output path: text}."""
    if not VERSION_RE.fullmatch(version or ""):
        raise ValueError(f"version must look like 1.2.3 or 1.2.3-rc.1, got {version!r}")
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", repo):
        raise ValueError(f"repo must be owner/name, got {repo!r}")
    if release_date is None:
        release_date = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    if not DATE_RE.fullmatch(release_date):
        raise ValueError(f"release date must be YYYY-MM-DD, got {release_date!r}")

    out = {}
    if only in (None, "aur"):
        values = {
            "VERSION": version.replace("-", "_"),  # pkgver cannot contain a hyphen
            "SOURCE": _aur_source(version, repo, source_url),
            "SHA256": _check_sha("tarball sha256", tarball_sha256),
            "REPO": repo,
        }
        for src, dst in AUR_TEMPLATES:
            out[dst] = fill((Path(templates_dir) / src).read_text(encoding="utf-8"), values)
    if only in (None, "winget"):
        values = {
            "VERSION": version,
            "RELEASE_DATE": release_date,
            "INSTALLER_URL": installer_url
            or f"https://github.com/{repo}/releases/download/v{version}/Brewery-{version}-x86_64-setup.exe",
            "INSTALLER_SHA256": _check_sha("installer sha256", installer_sha256).upper(),
            "PRODUCT_CODE": product_code,
            "REPO": repo,
        }
        for src, dst in WINGET_TEMPLATES:
            out[dst] = fill((Path(templates_dir) / src).read_text(encoding="utf-8"), values)
    return out


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--version", required=True, help="Brewery version, e.g. 0.1.0 (no leading v)")
    p.add_argument("--out", required=True, type=Path, help="output directory")
    p.add_argument("--tarball", type=Path, help="brewery-<ver>-linux-x86_64.tar.gz (AUR checksum)")
    p.add_argument("--tarball-sha256")
    p.add_argument("--installer", type=Path, help="Brewery-<ver>-x86_64-setup.exe (winget checksum)")
    p.add_argument("--installer-sha256")
    p.add_argument("--repo", default=DEFAULT_REPO, help="GitHub owner/name of the release (default %(default)s)")
    p.add_argument("--source-url", help="AUR source instead of the release URL, e.g. file:///pkg/x.tar.gz")
    p.add_argument("--installer-url", help="winget InstallerUrl instead of the release URL")
    p.add_argument("--release-date", help="YYYY-MM-DD (default: today, UTC)")
    p.add_argument("--product-code", default=STABLE_PRODUCT_CODE)
    p.add_argument("--only", choices=["aur", "winget"], help="render just one of them")
    args = p.parse_args(argv)

    want_aur = args.only in (None, "aur")
    want_winget = args.only in (None, "winget")
    try:
        tarball_sha = args.tarball_sha256 or (sha256_file(args.tarball) if args.tarball else None)
        installer_sha = args.installer_sha256 or (sha256_file(args.installer) if args.installer else None)
    except OSError as e:
        print(f"render_manifests: {e}", file=sys.stderr)
        return 1
    if want_aur and not tarball_sha:
        p.error("the AUR PKGBUILD needs --tarball or --tarball-sha256")
    if want_winget and not installer_sha:
        p.error("the winget manifests need --installer or --installer-sha256")

    try:
        files = render_all(
            version=args.version,
            tarball_sha256=tarball_sha,
            installer_sha256=installer_sha,
            repo=args.repo,
            source_url=args.source_url,
            installer_url=args.installer_url,
            release_date=args.release_date,
            product_code=args.product_code,
            only=args.only,
        )
    except (ValueError, OSError) as e:
        print(f"render_manifests: {e}", file=sys.stderr)
        return 1
    for name, text in files.items():
        path = args.out / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.replace("\r\n", "\n").encode("utf-8"))  # LF, whatever the platform
        print(f"wrote {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
