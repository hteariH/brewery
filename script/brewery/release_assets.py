#!/usr/bin/env python3
"""Assembles the files of a Brewery GitHub Release from the artifacts of the CI packaging jobs.

    script/brewery/release_assets.py check-tag --tag v0.1.0 --version-file crates/zed/BREWERY_VERSION
    script/brewery/release_assets.py assemble --version 0.1.0 --artifacts <dir> --out <dir> \\
        [--changelog CHANGELOG.md] [--repo hteariH/brewery]

`assemble` looks for the packages by their exact names anywhere under --artifacts (the layout
`actions/download-artifact` produces) and writes:

    <out>/assets/            everything that is uploaded to the release: the packages,
                             SHA256SUMS, latest.json, jre-source.txt, brewery-<ver>-winget.zip,
                             brewery-<ver>-aur.zip
    <out>/winget/, <out>/aur/  the manifests, unpacked, for browsing the workflow artifact
    <out>/release-notes.md   the release body

The macOS zip is optional (macOS is unsupported); the other packages are required.
`latest.json` is checked against a Python mirror of the rules in
crates/auto_update/src/brewery_update.rs (`validate_manifest`): keep the two in step.
"""

import argparse
import datetime
import hashlib
import json
import os
import re
import shutil
import sys
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_REPO = "hteariH/brewery"
BUNDLE_DEPS = HERE / "bundle-deps.toml"

VERSION_RE = re.compile(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z][0-9A-Za-z.]*)?")
SHA256_RE = re.compile(r"[0-9a-fA-F]{64}")
# brewery_update.rs: MAX_MANIFEST_BYTES
MAX_MANIFEST_BYTES = 64 * 1024
# brewery_update.rs: platform_key accepts these OSes and architectures
PLATFORM_KEY_RE = re.compile(r"(windows|linux|macos)-(x86_64|aarch64)")

OPTIONAL = {"macos"}
# package kind -> manifest platform key, for the ones the updater is told about
UPDATER_KEYS = {"windows": "windows-x86_64", "linux": "linux-x86_64", "macos": "macos-aarch64"}


def package_names(version: str) -> dict:
    """The package file names, as the packaging scripts name them (stable channel)."""
    return {
        "windows": f"Brewery-{version}-x86_64-setup.exe",
        "linux": f"brewery-{version}-linux-x86_64.tar.gz",
        "deb": f"brewery_{version}_amd64.deb",
        "rpm": f"brewery-{version}-1.x86_64.rpm",
        "macos": f"brewery-{version}-macos-aarch64.zip",
    }


def check_tag(tag: str, version_file_text: str):
    """Returns (version, is_prerelease); the tag must be `v` plus the BREWERY_VERSION file."""
    version = version_file_text.strip()
    if not VERSION_RE.fullmatch(version):
        raise ValueError(f"BREWERY_VERSION must look like 1.2.3 or 1.2.3-rc.1, got {version!r}")
    if tag != f"v{version}":
        raise ValueError(f"tag {tag!r} does not match BREWERY_VERSION: expected 'v{version}'")
    return version, "-" in version


def sha256sums(sums: dict) -> str:
    """`sha256sum` format: `<hash>  <name>` per line, sorted by name."""
    lines = []
    for name in sorted(sums):
        digest = sums[name]
        if not SHA256_RE.fullmatch(digest or ""):
            raise ValueError(f"checksum of {name} must be 64 hex digits, got {digest!r}")
        lines.append(f"{digest.lower()}  {name}\n")
    return "".join(lines)


def parse_sha256sums(text: str) -> dict:
    out = {}
    for line in text.splitlines():
        if not line.strip():
            continue
        digest, sep, name = line.partition("  ")
        if not sep or not SHA256_RE.fullmatch(digest):
            raise ValueError(f"not a sha256sum line: {line!r}")
        out[name] = digest
    return out


def _release_url(repo: str, version: str, name: str) -> str:
    return f"https://github.com/{repo}/releases/download/v{version}/{name}"


def latest_json(version: str, sums: dict, *, repo: str = DEFAULT_REPO, published_at: str = None) -> str:
    """The updater manifest. `sums` maps package file name to SHA-256."""
    names = package_names(version)
    assets = {}
    for kind, key in UPDATER_KEYS.items():
        name = names[kind]
        if name not in sums:
            if kind in OPTIONAL:
                continue
            raise ValueError(f"latest.json needs {name}, which was not built")
        assets[key] = {"url": _release_url(repo, version, name), "sha256": sums[name].lower()}
    if published_at is None:
        published_at = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    manifest = {
        "version": version,
        "notes_url": f"https://github.com/{repo}/releases/tag/v{version}",
        "published_at": published_at,
        "assets": assets,
    }
    return json.dumps(manifest, indent=2) + "\n"


def validate_manifest(data: bytes, version: str, *, repo: str = DEFAULT_REPO) -> dict:
    """A Python mirror of `parse_manifest` + `select_release` in brewery_update.rs, plus checks of
    what a release must carry: the exact version, a Windows and a Linux asset, SHA-256s."""
    if len(data) > MAX_MANIFEST_BYTES:
        raise ValueError(f"latest.json is over {MAX_MANIFEST_BYTES} bytes")
    try:
        manifest = json.loads(data)
    except ValueError as e:
        raise ValueError(f"latest.json is not JSON: {e}") from e
    if not isinstance(manifest, dict) or not isinstance(manifest.get("version"), str):
        raise ValueError("latest.json needs a string 'version'")
    declared = manifest["version"].strip()
    declared = declared[1:] if declared.startswith("v") else declared
    if not VERSION_RE.fullmatch(declared):
        raise ValueError(f"invalid version in latest.json: {declared!r}")
    if declared != version:
        raise ValueError(f"latest.json says {declared}, the release is {version}")
    notes_url = manifest.get("notes_url")
    if notes_url is not None and not str(notes_url).startswith(f"https://github.com/{repo}/"):
        raise ValueError(f"notes_url must point into https://github.com/{repo}/, got {notes_url!r}")
    assets = manifest.get("assets")
    if not isinstance(assets, dict):
        raise ValueError("latest.json needs 'assets'")
    for key, asset in assets.items():
        if not PLATFORM_KEY_RE.fullmatch(key):
            raise ValueError(f"unknown platform key {key!r}")
        url = asset.get("url") if isinstance(asset, dict) else None
        if not isinstance(url, str) or not url.lower().startswith("https://") or len(url) <= len("https://"):
            raise ValueError(f"asset {key}: the URL must use https://, got {url!r}")
        if not SHA256_RE.fullmatch(str(asset.get("sha256") or "")):
            raise ValueError(f"asset {key}: sha256 must be 64 hex digits")
    for kind, key in UPDATER_KEYS.items():
        if kind not in OPTIONAL and key not in assets:
            raise ValueError(f"latest.json has no {key} asset")
    return manifest


INSTALL_TEMPLATE = """## Brewery {version}

Open-source IDE for Java and Spring, built on the Zed editor. It bundles its own JRE, so nothing else needs installing.

### Install

- **Windows (x86_64):** run `{windows}`. It installs per user and needs no administrator rights. Brewery updates itself from GitHub Releases.
- **Linux (x86_64):** install `{deb}` or `{rpm}`, or unpack `{linux}` anywhere. On Arch, use the `brewery-bin` PKGBUILD from `{aur}`.
- **macOS (Apple silicon, unsupported):** unzip `{macos}`.

### Unsigned builds

These builds are unsigned. Windows SmartScreen may warn about the installer ("More info", then "Run anyway"). On macOS, Gatekeeper blocks the app until you open it once with a right click, or run `xattr -dr com.apple.quarantine Brewery.app`. Check the downloads against `SHA256SUMS` below.

The bundled JRE is Eclipse Temurin (GPLv2 with the Classpath Exception); `jre-source.txt` links to its source.
"""


def _changelog_section(version: str, changelog: str):
    """The body of `## <version>` (also `## [<version>]`, with a date or link after it)."""
    header = re.compile(r"^##\s+\[?" + re.escape(version) + r"\]?(?:\s.*)?$")
    lines, inside = [], False
    for line in changelog.splitlines():
        if inside and re.match(r"^##\s", line):
            break
        if inside:
            lines.append(line)
        elif header.match(line):
            inside = True
    text = "\n".join(lines).strip()
    return text or None


def release_notes(version: str, changelog, sums_text: str) -> str:
    section = _changelog_section(version, changelog) if changelog else None
    if section:
        body = section + "\n"
    else:
        names = package_names(version)
        body = INSTALL_TEMPLATE.format(version=version, aur=f"brewery-{version}-aur.zip", **names)
    return f"{body.rstrip()}\n\n### SHA256SUMS\n\n```\n{sums_text.rstrip()}\n```\n"


def jre_source_text(bundle_deps: Path = BUNDLE_DEPS) -> str:
    import tomllib

    with open(bundle_deps, "rb") as f:
        jdk = tomllib.load(f)["artifacts"]["jdk"]
    return (
        f"The JRE bundled with Brewery is a jlink image of Eclipse Temurin JDK {jdk['version']}.\n"
        f"Licence: {jdk['license']}.\n"
        "\n"
        "Source code of the redistributed binaries (GPLv2 section 3):\n"
        f"  Source archive: {jdk['source_code']}\n"
        f"  Source tag:     {jdk['source_tag']}\n"
        f"  Release page:   {jdk['source']}\n"
    )


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _find(root: Path, name: str):
    for path in sorted(root.rglob(name)):
        if path.is_file():
            return path
    return None


def _zip_dir(directory: Path, target: Path) -> None:
    """A zip of a directory's files with fixed timestamps, so reruns give the same bytes."""
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as z:
        for path in sorted(p for p in directory.rglob("*") if p.is_file()):
            info = zipfile.ZipInfo(path.relative_to(directory).as_posix(), date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            z.writestr(info, path.read_bytes())


def assemble(version, artifacts: Path, out: Path, *, changelog=None, render, repo=DEFAULT_REPO, published_at=None):
    """`render(installer_sha256=..., release_date=...)` returns the winget manifests as
    {relative path: text} (see render_manifests.render_all)."""
    artifacts, out = Path(artifacts), Path(out)
    assets = out / "assets"
    if out.exists():
        shutil.rmtree(out)
    assets.mkdir(parents=True)

    names = package_names(version)
    pkg_sums = {}
    missing = []
    for kind, name in names.items():
        found = _find(artifacts, name)
        if found is None:
            if kind not in OPTIONAL:
                missing.append(name)
            continue
        shutil.copyfile(found, assets / name)
        pkg_sums[name] = sha256_file(assets / name)
    pkgbuild, srcinfo = _find(artifacts, "PKGBUILD"), _find(artifacts, ".SRCINFO")
    for what, found in (("PKGBUILD", pkgbuild), (".SRCINFO", srcinfo)):
        if found is None:
            missing.append(what)
    if missing:
        raise ValueError("missing from the build artifacts: " + ", ".join(missing))

    tarball_sha = pkg_sums[names["linux"]]
    if tarball_sha not in pkgbuild.read_text(encoding="utf-8"):
        raise ValueError(f"PKGBUILD does not carry the checksum of {names['linux']} ({tarball_sha})")
    (out / "aur").mkdir()
    shutil.copyfile(pkgbuild, out / "aur" / "PKGBUILD")
    shutil.copyfile(srcinfo, out / "aur" / ".SRCINFO")

    release_date = (published_at or datetime.datetime.now(datetime.timezone.utc).isoformat())[:10]
    for rel, text in render(installer_sha256=pkg_sums[names["windows"]], release_date=release_date).items():
        path = out / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.replace("\r\n", "\n").encode("utf-8"))
    _zip_dir(out / "winget", assets / f"brewery-{version}-winget.zip")
    _zip_dir(out / "aur", assets / f"brewery-{version}-aur.zip")

    (assets / "jre-source.txt").write_text(jre_source_text(), encoding="utf-8", newline="\n")
    manifest = latest_json(version, pkg_sums, repo=repo, published_at=published_at)
    validate_manifest(manifest.encode("utf-8"), version, repo=repo)
    (assets / "latest.json").write_text(manifest, encoding="utf-8", newline="\n")

    sums = {p.name: sha256_file(p) for p in sorted(assets.iterdir()) if p.name != "SHA256SUMS"}
    sums_text = sha256sums(sums)
    (assets / "SHA256SUMS").write_text(sums_text, encoding="utf-8", newline="\n")
    (out / "release-notes.md").write_text(
        release_notes(version, changelog, sums_text), encoding="utf-8", newline="\n"
    )
    return sums


def _cmd_check_tag(args) -> int:
    try:
        version, prerelease = check_tag(args.tag, Path(args.version_file).read_text(encoding="utf-8"))
    except (ValueError, OSError) as e:
        print(f"release_assets: {e}", file=sys.stderr)
        return 1
    print(f"version={version}\nprerelease={'true' if prerelease else 'false'}")
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as f:
            f.write(f"version={version}\nprerelease={'true' if prerelease else 'false'}\n")
    return 0


def _cmd_assemble(args) -> int:
    sys.path.insert(0, str(HERE))
    import render_manifests

    changelog = None
    if args.changelog and Path(args.changelog).is_file():
        changelog = Path(args.changelog).read_text(encoding="utf-8")

    def render(installer_sha256, release_date):
        return render_manifests.render_all(
            version=args.version,
            installer_sha256=installer_sha256,
            repo=args.repo,
            release_date=release_date,
            only="winget",
        )

    try:
        sums = assemble(args.version, args.artifacts, args.out, changelog=changelog, render=render, repo=args.repo)
    except (ValueError, OSError) as e:
        print(f"release_assets: {e}", file=sys.stderr)
        return 1
    for name in sorted(sums):
        print(f"{sums[name]}  {name}")
    return 0


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    c = sub.add_parser("check-tag", help="tag must equal v<BREWERY_VERSION>; writes version/prerelease outputs")
    c.add_argument("--tag", required=True)
    c.add_argument("--version-file", default="crates/zed/BREWERY_VERSION")
    c.set_defaults(func=_cmd_check_tag)
    a = sub.add_parser("assemble", help="collect the release files from the CI artifacts")
    a.add_argument("--version", required=True)
    a.add_argument("--artifacts", required=True, type=Path)
    a.add_argument("--out", required=True, type=Path)
    a.add_argument("--changelog", default="CHANGELOG.md")
    a.add_argument("--repo", default=DEFAULT_REPO)
    a.set_defaults(func=_cmd_assemble)
    args = p.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
