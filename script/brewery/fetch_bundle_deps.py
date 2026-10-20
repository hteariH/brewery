#!/usr/bin/env python3
"""Download, verify and lay out Brewery's offline bundle.

Reads the pins in bundle-deps.toml (exact version, URL and SHA-256 per
OS/arch), downloads each artifact once into a cache, refuses anything whose
SHA-256 does not match, and lays out

    <out>/<os>-<arch>/
        jre/                         (jlink of the pinned JDK, on the target OS)
        jdtls/                       (+ brewery-jdtls[.cmd] launcher)
            tools/lombok.jar
            tools/java-debug.jar
            tools/java-lsp-proxy[.exe]
            tools/gradle-lsp-bridge[.exe]
            tools/gradle-ls/<version>/lib/   (the Gradle language server)
        extensions/java/
        brewery-daemon/brewery-daemon.jar   (with --daemon-jar)
        licenses/                    (every component's licence + README.txt)

The bundle is assembled in a hidden staging directory next to the target and
swapped in only when every step succeeded, so a failed run never leaves a
partial bundle and never destroys the previous one.

Python 3.11.4+ (tarfile's data filter) standard library only. Usage (from the repo root):

    python script/brewery/fetch_bundle_deps.py [--os windows] [--arch x64]
        [--out target/bundle] [--cache ~/.cache/brewery-bundle]
        (--daemon-jar brewery-daemon/build/libs/brewery-daemon-all.jar | --no-daemon)
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
import subprocess
import sys
import tarfile
import tempfile
import time
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
    # Exactly one licence source per artifact (checked by collect_licenses):
    license_file: str = ""  # text file, relative to the deps file's directory
    license_member: str = ""  # file inside the bundled jar/zip (Lombok's LICENSE)
    license_dir: str = ""  # directory inside the bundle (the JRE's legal/java.base)
    license_members: tuple = ()  # files inside the downloaded archive (licence + notices)
    source_code: str = ""  # GPL corresponding source (the JDK)
    source_tag: str = ""


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
    licence = {}
    for key in ("license_file", "license_member", "license_dir", "source_code", "source_tag"):
        value = table.get(key, "")
        if not isinstance(value, str):
            raise DepsError(f"artifact {name}: {key} must be a string")
        if value and key.startswith("license_"):
            _safe_relpath(value, f"artifact {name}: {key}")
        licence[key] = value
    members = table.get("license_members", [])
    if not isinstance(members, list) or not all(isinstance(m, str) and m for m in members):
        raise DepsError(f"artifact {name}: license_members must be a list of paths")
    for value in members:
        _safe_relpath(value, f"artifact {name}: license_members")
    if members and fmt == "file":
        raise DepsError(f"artifact {name}: license_members need an archive format")
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
        license_members=tuple(members),
        **licence,
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


# Waits before the 2nd, 3rd and 4th download attempt. Only network failures and
# HTTP 429/5xx are retried; 4xx answers and checksum mismatches fail at once.
RETRY_DELAYS = (5, 15, 45)


class _Retryable(FetchError):
    """A download failure worth another attempt."""


def fetch(art: Artifact, cache: Path, log: Callable) -> Path:
    """Return the verified cached file for art, downloading it on a cache miss.

    Network failures are retried (RETRY_DELAYS), so a single unreachable mirror
    moment doesn't fail a whole release build.
    """
    for attempt, delay in enumerate((*RETRY_DELAYS, None), start=1):
        try:
            return _fetch_once(art, cache, log)
        except _Retryable as err:
            if delay is None:
                raise FetchError(str(err)) from None
            log(f"  retry   {art.name}: {err} (attempt {attempt} of {len(RETRY_DELAYS) + 1}, waiting {delay} s)")
            time.sleep(delay)
    raise AssertionError("unreachable")


def _fetch_once(art: Artifact, cache: Path, log: Callable) -> Path:
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
        message = f"download of {art.name} from {art.url} failed: HTTP {err.code}"
        if err.code == 429 or err.code >= 500:
            raise _Retryable(message) from None
        raise FetchError(message) from None
    except (urllib.error.URLError, OSError, TimeoutError) as err:
        raise _Retryable(f"download of {art.name} from {art.url} failed: {err}") from err
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


def _resolve_member(path: str, links: dict, archive: Path, depth: int = 0) -> list:
    """Components of path inside the archive, following the archive's own symlinks
    (links: member name -> link target). FetchError when it leaves the archive root, so
    chains that each pass the lexical check (a -> sub/.., a/b -> ..) are caught too."""
    if depth > 40:
        raise FetchError(f"unsafe link loop at {path!r} in {archive.name}")
    resolved: list = []
    for part in path.replace("\\", "/").split("/"):
        if part in ("", "."):
            continue
        if part == "..":
            if not resolved:
                raise FetchError(f"unsafe path {path!r} in {archive.name}")
            resolved.pop()
            continue
        resolved.append(part)
        target = links.get("/".join(resolved))
        if target is not None:
            if PurePosixPath(target).is_absolute():
                raise FetchError(f"unsafe link {path!r} in {archive.name}")
            base = resolved[:-1]
            resolved = _resolve_member("/".join([*base, target]), links, archive, depth + 1)
    return resolved


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
                # The data filter (Python 3.11.4+, 3.12+) also guards what lexical
                # checks cannot see, such as links created on disk by earlier members.
                if getattr(tarfile, "data_filter", None) is None:
                    raise FetchError(
                        f"cannot unpack {archive.name}: this Python's tarfile has no data "
                        "filter; use Python 3.11.4+ or 3.12+"
                    )
                members = tf.getmembers()
                links: dict = {}
                for m in members:
                    _check_member(m.name, archive)
                    parent = _resolve_member(posixpath.dirname(m.name), links, archive)
                    if m.issym() or m.islnk():
                        if PurePosixPath(m.linkname).is_absolute():
                            raise FetchError(f"unsafe link {m.name!r} in {archive.name}")
                        base = posixpath.dirname(m.name) if m.issym() else ""
                        _check_member(posixpath.join(base, m.linkname), archive)
                        if m.issym():
                            _resolve_member("/".join([*parent, m.linkname]), links, archive)
                            links["/".join([*parent, posixpath.basename(m.name)])] = m.linkname
                        else:
                            _resolve_member(m.linkname, links, archive)
                    elif not (m.isfile() or m.isdir()):
                        raise FetchError(f"unsafe member type {m.name!r} in {archive.name}")
                tf.extractall(into, members=members, filter="data")
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
    files: dict = field(default_factory=dict)  # name -> verified download
    deps_dir: Path = SCRIPT_DIR
    log: Callable = print
    launchers_dir: Path = SCRIPT_DIR / "jdtls"


def host_target() -> str:
    """<os>-<arch> of the machine running the fetcher."""
    return target_name(normalize_os(sys.platform), normalize_arch(platform.machine()))


# The JRE serves the daemon and jdtls only (projects build with the user's JDK).
# Daemon (jdeps on the shadow jar): java.se, jdk.net, jdk.security.auth,
# jdk.unsupported; Gradle Tooling API, JDBC and keyring need jdk.crypto.ec,
# jdk.zipfs and jdk.management. jdtls starts with --add-modules=ALL-SYSTEM and
# needs jdk.jdi, jdk.attach and jdk.jdwp.agent for java-debug, java.instrument
# for the Lombok javaagent, jdk.localedata and jdk.charsets; jdk.compiler is
# looked up by some plugins. See docs/superpowers/plans/m6-gap-analysis.md 4.2.
JRE_MODULES = (
    "java.se",
    "jdk.unsupported",
    "jdk.crypto.ec",
    "jdk.zipfs",
    "jdk.net",
    "jdk.management",
    "jdk.security.auth",
    "jdk.jdi",
    "jdk.attach",
    "jdk.jdwp.agent",
    "java.instrument",
    "jdk.localedata",
    "jdk.charsets",
    "jdk.jfr",
    "jdk.httpserver",
    "jdk.compiler",
    # Loaded by JDBC drivers through services or reflection, so jdeps cannot see them:
    # PKCS#11 keys, JNDI DNS/SRV lookups and Kerberos (e.g. mssql-jdbc integrated auth).
    "jdk.crypto.cryptoki",
    "jdk.naming.dns",
    "jdk.security.jgss",
)
# Modules that exist only in some OS's JDK: the Windows-ROOT / Windows-MY keystores
# (corporate TLS roots, client certificates).
JRE_MODULES_BY_OS = {"windows": ("jdk.crypto.mscapi",)}


def jre_modules(os_name: str) -> tuple:
    return JRE_MODULES + JRE_MODULES_BY_OS.get(os_name, ())


JLINK_FLAGS = ("--strip-debug", "--no-man-pages", "--no-header-files", "--compress=zip-6")

# jdtls's shared configuration per target: the first is the one the launcher uses,
# the second the syntax server's. Every other config_* directory is dropped.
JDTLS_CONFIGS = {
    "windows-x64": ("config_win", "config_ss_win"),
    "linux-x64": ("config_linux", "config_ss_linux"),
    "linux-aarch64": ("config_linux_arm", "config_ss_linux_arm"),
    "macos-x64": ("config_mac", "config_ss_mac"),
    "macos-aarch64": ("config_mac_arm", "config_ss_mac_arm"),
}


def _exe(ctx: BundleContext) -> str:
    return ".exe" if ctx.os == "windows" else ""


def _run_checked(run: Callable, cmd: list, what: str) -> subprocess.CompletedProcess:
    try:
        proc = run([str(c) for c in cmd], capture_output=True, text=True, timeout=900)
    except (OSError, subprocess.SubprocessError) as err:
        raise FetchError(f"{what} failed to start: {err}") from err
    if proc.returncode != 0:
        output = "\n".join(s for s in (proc.stdout, proc.stderr) if s).strip()
        raise FetchError(f"{what} exited with {proc.returncode}: {output}")
    return proc


def build_jre(ctx: BundleContext, run: Callable = subprocess.run, host: str | None = None) -> None:
    """jlink ctx.work["jdk"] into ctx.bundle / "jre" (legal/ kept) and check the image runs.

    jlink only links for the platform it runs on, so the target must be the host."""
    host = host or host_target()
    if host != ctx.target:
        raise FetchError(
            f"jre: jlink must run on the target OS ({ctx.target}); this machine is {host}"
        )
    jdk = ctx.work.get("jdk")
    if jdk is None:
        raise FetchError("jre: no unpacked JDK (artifact jdk with into = \"work\")")
    exe = _exe(ctx)
    jlink = Path(jdk) / "bin" / f"jlink{exe}"
    jmods = Path(jdk) / "jmods"
    jre = ctx.bundle / "jre"
    if jre.exists():
        raise FetchError(f"jre: {jre} already exists in the bundle")
    modules = jre_modules(ctx.os)
    ctx.log(f"  jlink   {len(modules)} modules -> jre/")
    _run_checked(
        run,
        [jlink, "--module-path", jmods, "--add-modules", ",".join(modules), *JLINK_FLAGS,
         "--output", jre],
        "jlink",
    )
    if not (jre / "release").is_file():
        raise FetchError(f"jre: jlink produced no {jre / 'release'}")
    if not (jre / "legal" / "java.base").is_dir():
        raise FetchError("jre: jlink produced no legal/java.base")
    java = jre / "bin" / f"java{exe}"
    version = _run_checked(run, [java, "-version"], "jre/bin/java -version")
    first = (version.stderr or version.stdout).strip().splitlines()
    ctx.log(f"  jre     {first[0] if first else 'java -version ok'}")
    listing = _run_checked(run, [java, "--list-modules"], "jre/bin/java --list-modules")
    present = {line.split("@", 1)[0].strip() for line in listing.stdout.splitlines()}
    missing = [m for m in modules if m not in present]
    if missing:
        raise FetchError(f"jre: image lacks modules {', '.join(missing)}")


def write_jdtls_launchers(ctx: BundleContext) -> None:
    """Install the Brewery jdtls launcher for the target and drop other platforms' configs.

    Windows gets brewery-jdtls.cmd (CRLF, which cmd.exe's label lookup wants), every
    other OS brewery-jdtls (LF, executable). The repo copies are LF."""
    jdtls = ctx.bundle / "jdtls"
    configs = JDTLS_CONFIGS.get(ctx.target)
    if configs is None:
        raise FetchError(f"jdtls: no config directory known for {ctx.target}")
    if not (jdtls / configs[0]).is_dir():
        raise FetchError(f"jdtls: {configs[0]} missing from the jdtls archive")
    for entry in jdtls.iterdir():
        if entry.is_dir() and entry.name.startswith("config_") and entry.name not in configs:
            rmtree(entry)
    if ctx.os == "windows":
        name, newline = "brewery-jdtls.cmd", b"\r\n"
    else:
        name, newline = "brewery-jdtls", b"\n"
    src = Path(ctx.launchers_dir) / name
    if not src.is_file():
        raise FetchError(f"jdtls: launcher {src} not found")
    lines = src.read_bytes().replace(b"\r\n", b"\n").split(b"\n")
    dest = jdtls / name
    dest.write_bytes(newline.join(lines))
    if ctx.os != "windows":
        _make_executable(dest)
    ctx.log(f"  launch  jdtls/{name}")


def _license_text(ctx: BundleContext, art: Artifact) -> dict:
    """{file name: bytes} for one artifact's licence; FetchError when it has none."""
    sources = [
        s for s in (art.license_file, art.license_member, art.license_dir, art.license_members) if s
    ]
    if len(sources) != 1:
        raise FetchError(
            f"licenses: artifact {art.name} needs exactly one of license_file, "
            "license_member, license_dir, license_members in the deps file"
        )
    if art.license_members:
        return _archive_members(art, ctx.files.get(art.name))
    if art.license_file:
        path = Path(ctx.deps_dir) / art.license_file
        if not path.is_file():
            raise FetchError(f"licenses: {art.name}: {path} not found")
        return {"LICENSE": path.read_bytes()}
    if art.license_member:
        jar = ctx.bundle / art.dest
        try:
            with zipfile.ZipFile(jar) as zf:
                return {"LICENSE": zf.read(art.license_member)}
        except (OSError, KeyError, zipfile.BadZipFile) as err:
            raise FetchError(f"licenses: {art.name}: no {art.license_member} in {jar.name}: {err}")
    legal = ctx.bundle / art.license_dir
    if not (legal / "LICENSE").is_file():
        raise FetchError(f"licenses: {art.name}: {art.license_dir}/LICENSE not found (legal/ missing?)")
    # copyfile-style reads follow the Linux image's relative symlinks.
    return {p.name: p.read_bytes() for p in sorted(legal.iterdir()) if p.is_file()}


def _archive_members(art: Artifact, archive: Path | None) -> dict:
    """{base name: bytes} of art.license_members, read from its downloaded archive."""
    if archive is None:
        raise FetchError(f"licenses: {art.name}: its download is not known")
    texts = {}
    try:
        if art.format == "zip":
            with zipfile.ZipFile(archive) as zf:
                for member in art.license_members:
                    texts[PurePosixPath(member).name] = zf.read(member)
        else:
            with tarfile.open(archive, "r:gz") as tf:
                for member in art.license_members:
                    f = tf.extractfile(member)
                    if f is None:
                        raise KeyError(member)
                    texts[PurePosixPath(member).name] = f.read()
    except (OSError, KeyError, tarfile.TarError, zipfile.BadZipFile) as err:
        raise FetchError(f"licenses: {art.name}: {err} not found in {archive.name}") from err
    return texts


def collect_licenses(ctx: BundleContext) -> None:
    """licenses/<component>/ with each licence text, plus README.txt indexing them.

    The JRE keeps its own legal/ too; licenses/jre/SOURCE.txt names the GPLv2
    corresponding source. The daemon's THIRD-PARTY-NOTICES.txt comes out of its jar."""
    lic = ctx.bundle / "licenses"
    lic.mkdir(exist_ok=True)
    index = [
        "Third-party components bundled with Brewery",
        "",
        "Each directory here holds the licence of one component. The Java runtime in",
        "jre/ also carries its per-module notices in jre/legal/.",
        "",
    ]
    for art in ctx.artifacts.values():
        component = "jre" if art.name == "jdk" else art.name
        texts = _license_text(ctx, art)
        dest = lic / component
        dest.mkdir(exist_ok=True)
        for file_name, data in texts.items():
            (dest / file_name).write_bytes(data)
        index += [f"{component} {art.version}", f"  License: {art.license}", f"  Source:  {art.source}"]
        if art.source_code or art.source_tag:
            if art.name == "jdk":
                source = [
                    f"Corresponding source for the Java runtime in jre/ ({art.version}),",
                    "built from the Eclipse Temurin JDK with jlink:",
                    "",
                ]
            else:
                source = [f"Source code of {component} {art.version}:", ""]
            if art.source_code:
                source.append(f"Source archive: {art.source_code}")
                index.append(f"  Corresponding source: {art.source_code}")
            if art.source_tag:
                source.append(f"Source tag:     {art.source_tag}")
                if not art.source_code:
                    index.append(f"  Source code: {art.source_tag}")
            source.append(f"Binaries:       {art.source}")
            (dest / "SOURCE.txt").write_text("\n".join(source) + "\n", encoding="utf-8")
        index.append(f"  Files:   licenses/{component}/")
        index.append("")
    daemon = ctx.bundle / "brewery-daemon" / "brewery-daemon.jar"
    if daemon.is_file():
        try:
            with zipfile.ZipFile(daemon) as zf:
                notices = zf.read("THIRD-PARTY-NOTICES.txt")
        except (OSError, KeyError, zipfile.BadZipFile) as err:
            raise FetchError(f"licenses: no THIRD-PARTY-NOTICES.txt in {daemon.name}: {err}")
        (lic / "brewery-daemon").mkdir(exist_ok=True)
        (lic / "brewery-daemon" / "THIRD-PARTY-NOTICES.txt").write_bytes(notices)
        index += [
            "brewery-daemon",
            "  License: Apache-2.0 (Brewery); its dependencies are listed with their",
            "  licences in licenses/brewery-daemon/THIRD-PARTY-NOTICES.txt",
            "",
        ]
    (lic / "README.txt").write_text("\n".join(index), encoding="utf-8", newline="\n")
    ctx.log(f"  legal   licenses/ ({len(ctx.artifacts)} components)")


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
            files=files,
            deps_dir=Path(deps.path).parent,
            log=log,
        )
        # A directory artifact (jdtls/) goes in before the files placed inside it
        # (jdtls/tools/...), whatever order the pins file lists them in.
        for art in sorted(resolved, key=lambda a: len(PurePosixPath(a.dest).parts)):
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
    parser.add_argument(
        "--no-daemon", action="store_true",
        help="build without brewery-daemon/ (not for releases: the updater needs it)",
    )
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args(argv)
    if args.daemon_jar is None and not args.no_daemon:
        parser.error("--daemon-jar is required (or --no-daemon for a bundle without the daemon)")
    log = (lambda *_: None) if args.quiet else (lambda msg: print(msg, flush=True))
    try:
        target = target_name(normalize_os(args.os), normalize_arch(args.arch))
        deps = load_deps(args.deps)
        build_bundle(
            deps, target, args.out, args.cache, daemon_jar=args.daemon_jar,
            steps=DEFAULT_STEPS, log=log,
        )
    except (DepsError, FetchError) as err:
        print(f"error: {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
