#!/usr/bin/env python3
"""Download, verify and lay out Brewery's offline bundle.

Reads the pins in bundle-deps.toml (exact version, URL and SHA-256 per
OS/arch), downloads each artifact once into a cache, refuses anything whose
SHA-256 does not match, and lays out

    <out>/<os>-<arch>/
        jre/                         (jlink step, M6c Task 2)
        jdtls/                       (+ brewery-jdtls launchers, Task 2)
        lombok.jar
        java-debug.jar
        java-lsp-proxy[.exe]
        extensions/java/
        brewery-daemon/brewery-daemon.jar   (with --daemon-jar)
        licenses/                    (filled in Task 2)

The bundle is assembled in a hidden staging directory next to the target and
swapped in only when every step succeeded, so a failed run never leaves a
partial bundle and never destroys the previous one.

Python 3.11+ standard library only. Usage (from the repo root):

    python script/brewery/fetch_bundle_deps.py [--os windows] [--arch x64]
        [--out target/bundle] [--cache ~/.cache/brewery-bundle]
        [--daemon-jar brewery-daemon/build/libs/brewery-daemon-all.jar]
"""

from __future__ import annotations

import argparse
import hashlib
import os
import platform
import posixpath
import re
import shutil
import stat
import sys
import tarfile
import tempfile
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import Callable, Iterable

SCHEMA = 1
SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent.parent
DEFAULT_DEPS = SCRIPT_DIR / "bundle-deps.toml"
DEFAULT_OUT = REPO_ROOT / "target" / "bundle"
DEFAULT_CACHE = Path.home() / ".cache" / "brewery-bundle"
USER_AGENT = "brewery-bundle-fetcher/1"
CHUNK = 1 << 20

OSES = ("windows", "linux", "macos")
ARCHES = ("x64", "aarch64")
FORMATS = ("file", "zip", "tar.gz")
INTO = ("bundle", "work")
SHA_RE = re.compile(r"^[0-9a-f]{64}$")


class DepsError(Exception):
    """bundle-deps.toml is malformed or has no pin for the requested target."""


class FetchError(Exception):
    """A download, checksum or unpack step failed."""


# --- pins -----------------------------------------------------------------


@dataclass(frozen=True)
class Artifact:
    """One artifact resolved for one target."""

    name: str
    version: str
    license: str
    source: str
    url: str
    sha256: str
    format: str
    into: str  # "bundle": copied into the bundle at dest; "work": unpacked for build steps
    dest: str  # relative path inside the bundle ("" for into = "work")
    root: str  # directory inside the archive that becomes dest ("" = whole archive)
    member: str  # single file inside the archive that becomes dest ("" = none)
    executable: bool


@dataclass(frozen=True)
class Deps:
    targets: tuple
    artifacts: dict  # name -> raw table
    path: Path


def target_name(os_name: str, arch: str) -> str:
    return f"{os_name}-{arch}"


def normalize_os(value: str) -> str:
    v = value.lower()
    if v in ("windows", "win32", "win", "cygwin", "msys"):
        return "windows"
    if v in ("macos", "darwin", "mac", "osx"):
        return "macos"
    if v.startswith("linux"):
        return "linux"
    raise DepsError(f"unsupported OS {value!r}")


def normalize_arch(value: str) -> str:
    v = value.lower()
    if v in ("x64", "x86_64", "amd64"):
        return "x64"
    if v in ("aarch64", "arm64"):
        return "aarch64"
    raise DepsError(f"unsupported architecture {value!r}")


def _safe_relpath(value: str, what: str) -> str:
    p = PurePosixPath(value)
    if value.startswith(("/", "\\")) or ":" in value or ".." in p.parts or "\\" in value:
        raise DepsError(f"{what} {value!r} must be a relative path inside the bundle")
    return value


def _infer_format(url: str) -> str | None:
    path = urllib.parse.urlparse(url).path.lower()
    if path.endswith(".zip"):
        return "zip"
    if path.endswith((".tar.gz", ".tgz")):
        return "tar.gz"
    if path.endswith(".jar"):
        return "file"
    return None


def _resolve(name: str, table: dict, target: str) -> Artifact:
    os_name = target.split("-", 1)[0]
    pin = table.get(target) or table.get("any")
    if not isinstance(pin, dict):
        raise DepsError(f"artifact {name} has no pin for {target}")
    for key in ("version", "license", "source"):
        if not isinstance(table.get(key), str) or not table[key]:
            raise DepsError(f"artifact {name}: missing {key}")
    url = pin.get("url")
    if not isinstance(url, str) or not url:
        raise DepsError(f"artifact {name} ({target}): missing url")
    sha = pin.get("sha256")
    if not isinstance(sha, str) or not SHA_RE.match(sha):
        raise DepsError(f"artifact {name} ({target}): sha256 must be 64 lowercase hex digits")
    fmt = pin.get("format") or table.get("format") or _infer_format(url)
    if fmt not in FORMATS:
        raise DepsError(f"artifact {name} ({target}): unknown format {fmt!r}; set format")
    into = table.get("into", "bundle")
    if into not in INTO:
        raise DepsError(f"artifact {name}: into must be one of {INTO}")
    exe = ".exe" if os_name == "windows" else ""
    dest = str(pin.get("dest", table.get("dest", ""))).replace("{exe}", exe)
    if into == "bundle":
        if not dest:
            raise DepsError(f"artifact {name}: missing dest")
        _safe_relpath(dest, f"artifact {name}: dest")
    root = str(pin.get("root", table.get("root", "")))
    member = str(pin.get("member", table.get("member", ""))).replace("{exe}", exe)
    for value, what in ((root, "root"), (member, "member")):
        if value:
            _safe_relpath(value, f"artifact {name}: {what}")
    if fmt == "file" and (root or member):
        raise DepsError(f"artifact {name}: root/member need an archive format")
    return Artifact(
        name=name,
        version=table["version"],
        license=table["license"],
        source=table["source"],
        url=url,
        sha256=sha,
        format=fmt,
        into=into,
        dest=dest,
        root=root.strip("/"),
        member=member,
        executable=bool(table.get("executable", False)),
    )


def load_deps(path: Path) -> Deps:
    """Parse and fully validate bundle-deps.toml (every artifact for every target)."""
    try:
        with open(path, "rb") as f:
            data = tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError) as err:
        raise DepsError(f"cannot read {path}: {err}") from err
    if data.get("schema") != SCHEMA:
        raise DepsError(f"{path}: unsupported schema {data.get('schema')!r} (want {SCHEMA})")
    targets = data.get("targets")
    if not isinstance(targets, list) or not all(isinstance(t, str) for t in targets):
        raise DepsError(f"{path}: targets must be a list of <os>-<arch> strings")
    for t in targets:
        os_name, _, arch = t.partition("-")
        if os_name not in OSES or arch not in ARCHES:
            raise DepsError(f"{path}: bad target {t!r}")
    artifacts = data.get("artifacts")
    if not isinstance(artifacts, dict):
        raise DepsError(f"{path}: missing [artifacts]")
    deps = Deps(targets=tuple(targets), artifacts=artifacts, path=Path(path))
    for t in targets:
        select(deps, t)
    return deps


def select(deps: Deps, target: str) -> list:
    """Resolve every artifact for one target, in file order."""
    if target not in deps.targets:
        raise DepsError(f"target {target} is not listed in {deps.path} (have {list(deps.targets)})")
    return [_resolve(name, table, target) for name, table in deps.artifacts.items()]


# --- download -------------------------------------------------------------


def _url_basename(url: str) -> str:
    base = PurePosixPath(urllib.parse.unquote(urllib.parse.urlparse(url).path)).name
    base = re.sub(r"[^A-Za-z0-9._+-]", "_", base)
    return base or "download"


def cache_path(cache: Path, sha256: str, url: str) -> Path:
    """Content-addressed cache entry: <cache>/<sha256>/<url basename>."""
    return Path(cache) / sha256 / _url_basename(url)


def _hash_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(CHUNK):
            h.update(chunk)
    return h.hexdigest()


def fetch(art: Artifact, cache: Path, log: Callable) -> Path:
    """Return the verified cached file for art, downloading it on a cache miss."""
    entry = cache_path(cache, art.sha256, art.url)
    if entry.is_file():
        if _hash_file(entry) == art.sha256:
            log(f"  cached  {art.name} {art.version}")
            return entry
        log(f"  stale   {art.name}: cache entry does not match its pin, downloading again")
        entry.unlink()
    entry.parent.mkdir(parents=True, exist_ok=True)
    log(f"  fetch   {art.name} {art.version} <- {art.url}")
    fd, part_name = tempfile.mkstemp(prefix=".dl-", suffix=".part", dir=entry.parent)
    part = Path(part_name)
    try:
        h = hashlib.sha256()
        request = urllib.request.Request(art.url, headers={"User-Agent": USER_AGENT})
        with os.fdopen(fd, "wb") as out, urllib.request.urlopen(request, timeout=120) as resp:
            while chunk := resp.read(CHUNK):
                h.update(chunk)
                out.write(chunk)
        actual = h.hexdigest()
        if actual != art.sha256:
            raise FetchError(
                f"SHA-256 mismatch for {art.name} {art.version} from {art.url}: "
                f"expected {art.sha256}, got {actual}"
            )
        os.replace(part, entry)
        return entry
    except urllib.error.HTTPError as err:
        err.close()
        raise FetchError(f"download of {art.name} from {art.url} failed: HTTP {err.code}") from None
    except (urllib.error.URLError, OSError, TimeoutError) as err:
        raise FetchError(f"download of {art.name} from {art.url} failed: {err}") from err
    finally:
        part.unlink(missing_ok=True)
        try:
            entry.parent.rmdir()  # drop the empty sha dir after a failure
        except OSError:
            pass


# --- unpack ---------------------------------------------------------------


def rmtree(path: Path) -> None:
    """shutil.rmtree that also removes read-only files (Windows, jdtls jars)."""

    def onerror(func, p, _exc):
        os.chmod(p, stat.S_IWRITE)
        func(p)

    path = Path(path)
    if path.is_dir() and not path.is_symlink():
        if sys.version_info >= (3, 12):
            shutil.rmtree(path, onexc=onerror)
        else:
            shutil.rmtree(path, onerror=onerror)
    elif path.exists() or path.is_symlink():
        path.unlink()


def _check_member(name: str, archive: Path) -> None:
    """Reject absolute paths, drive letters and anything resolving outside the archive root."""
    raw = name.replace("\\", "/")
    p = PurePosixPath(raw)
    norm = posixpath.normpath(raw)
    if p.is_absolute() or norm == ".." or norm.startswith("../") or (p.parts and ":" in p.parts[0]):
        raise FetchError(f"unsafe path {name!r} in {archive.name}")


def _extract(archive: Path, fmt: str, into: Path) -> None:
    into.mkdir(parents=True)
    try:
        if fmt == "zip":
            with zipfile.ZipFile(archive) as zf:
                for name in zf.namelist():
                    _check_member(name, archive)
                zf.extractall(into)
        elif fmt == "tar.gz":
            with tarfile.open(archive, "r:gz") as tf:
                members = tf.getmembers()
                for m in members:
                    _check_member(m.name, archive)
                    if m.issym() or m.islnk():
                        base = posixpath.dirname(m.name) if m.issym() else ""
                        _check_member(posixpath.join(base, m.linkname), archive)
                        if PurePosixPath(m.linkname).is_absolute():
                            raise FetchError(f"unsafe link {m.name!r} in {archive.name}")
                    elif not (m.isfile() or m.isdir()):
                        raise FetchError(f"unsafe member type {m.name!r} in {archive.name}")
                if hasattr(tarfile, "data_filter"):
                    tf.extractall(into, members=members, filter="data")
                else:
                    tf.extractall(into, members=members)
        else:
            raise FetchError(f"cannot unpack format {fmt}")
    except (tarfile.TarError, zipfile.BadZipFile, OSError) as err:
        raise FetchError(f"cannot unpack {archive.name}: {err}") from err


def _make_executable(path: Path) -> None:
    mode = path.stat().st_mode
    path.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def install(art: Artifact, file: Path, dest: Path, scratch: Path) -> None:
    """Place art (verified file) at dest; scratch is an empty-able temp area."""
    if dest.exists():
        raise FetchError(f"{art.name}: {dest} already exists in the bundle")
    dest.parent.mkdir(parents=True, exist_ok=True)
    if art.format == "file":
        shutil.copyfile(file, dest)
        if art.executable:
            _make_executable(dest)
        return
    unpacked = scratch / art.name
    _extract(file, art.format, unpacked)
    if art.member:
        src = unpacked / art.member
        if not src.is_file():
            raise FetchError(f"{art.name}: {art.member} not found in {file.name}")
        shutil.move(str(src), str(dest))
        if art.executable:
            _make_executable(dest)
    else:
        src = unpacked / art.root if art.root else unpacked
        if not src.is_dir():
            raise FetchError(f"{art.name}: directory {art.root} not found in {file.name}")
        shutil.move(str(src), str(dest))
    rmtree(unpacked)


# --- bundle ---------------------------------------------------------------


@dataclass
class BundleContext:
    """What build steps (jlink, launchers, licences) get to work with."""

    target: str
    os: str
    arch: str
    bundle: Path  # staging dir that becomes <out>/<target>
    work: dict = field(default_factory=dict)  # name -> unpacked dir for into = "work"
    artifacts: dict = field(default_factory=dict)  # name -> Artifact
    deps_dir: Path = SCRIPT_DIR
    log: Callable = print


def build_jre(ctx: BundleContext) -> None:
    """M6c Task 2: jlink ctx.work["jdk"] into ctx.bundle / "jre" (keep legal/).

    Must run on the target OS (jlink does not cross-link here)."""
    ctx.log(f"  todo    jre: jlink from {ctx.work.get('jdk')} not implemented yet (M6c Task 2)")


def write_jdtls_launchers(ctx: BundleContext) -> None:
    """M6c Task 2: copy script/brewery/jdtls/brewery-jdtls{,.cmd} into ctx.bundle / "jdtls"."""
    ctx.log("  todo    jdtls launchers not implemented yet (M6c Task 2)")


def collect_licenses(ctx: BundleContext) -> None:
    """M6c Task 2: write licence texts for every artifact into ctx.bundle / "licenses"."""
    ctx.log("  todo    licences not collected yet (M6c Task 2)")


DEFAULT_STEPS = (build_jre, write_jdtls_launchers, collect_licenses)


def build_bundle(
    deps: Deps,
    target: str,
    out: Path,
    cache: Path,
    *,
    daemon_jar: Path | None = None,
    steps: Iterable[Callable] = DEFAULT_STEPS,
    log: Callable = print,
) -> Path:
    """Build <out>/<target>; returns its path. Atomic: all or nothing."""
    os_name, _, arch = target.partition("-")
    resolved = select(deps, target)
    out = Path(out)
    cache = Path(cache)
    final = out / target

    log(f"bundle {target}: verifying {len(resolved)} artifacts")
    files = {art.name: fetch(art, cache, log) for art in resolved}
    if daemon_jar is not None and not Path(daemon_jar).is_file():
        raise FetchError(f"daemon jar {daemon_jar} does not exist")

    out.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=f".{target}.partial-", dir=out))
    try:
        bundle = staging / "bundle"
        scratch = staging / "scratch"
        work_root = staging / "work"
        for d in (bundle, scratch, work_root):
            d.mkdir()
        ctx = BundleContext(
            target=target,
            os=os_name,
            arch=arch,
            bundle=bundle,
            artifacts={a.name: a for a in resolved},
            deps_dir=Path(deps.path).parent,
            log=log,
        )
        for art in resolved:
            if art.into == "work":
                dest = work_root / art.name
                install(art, files[art.name], dest, scratch)
                ctx.work[art.name] = dest
            else:
                install(art, files[art.name], bundle / art.dest, scratch)
        if daemon_jar is not None:
            jar_dest = bundle / "brewery-daemon" / "brewery-daemon.jar"
            jar_dest.parent.mkdir(parents=True)
            shutil.copyfile(daemon_jar, jar_dest)
        (bundle / "licenses").mkdir(exist_ok=True)
        for step in steps:
            step(ctx)

        old = None
        if final.exists():
            old = staging / "previous"
            os.replace(final, old)
        try:
            os.replace(bundle, final)
        except OSError:
            if old is not None:
                os.replace(old, final)
            raise
    finally:
        rmtree(staging)
    log(f"bundle {target}: {final}")
    return final


# --- CLI ------------------------------------------------------------------


def main(argv: list | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--deps", type=Path, default=DEFAULT_DEPS, help="pins file")
    parser.add_argument("--os", default=sys.platform, help="windows, linux or macos (default: host)")
    parser.add_argument("--arch", default=platform.machine(), help="x64 or aarch64 (default: host)")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT, help="bundle parent dir")
    parser.add_argument("--cache", type=Path, default=DEFAULT_CACHE, help="download cache")
    parser.add_argument("--daemon-jar", type=Path, help="built brewery-daemon shadow jar")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args(argv)
    log = (lambda *_: None) if args.quiet else (lambda msg: print(msg, flush=True))
    try:
        target = target_name(normalize_os(args.os), normalize_arch(args.arch))
        deps = load_deps(args.deps)
        build_bundle(deps, target, args.out, args.cache, daemon_jar=args.daemon_jar, log=log)
    except (DepsError, FetchError) as err:
        print(f"error: {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
