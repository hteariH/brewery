"""Tests for fetch_bundle_deps.py.

Run from the repo root:
    python -m unittest discover script/brewery
"""

import contextlib
import hashlib
import io
import os
import re
import tarfile
import tempfile
import threading
import subprocess
import unittest
import zipfile
from unittest import mock
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import fetch_bundle_deps as fbd

REAL_DEPS = Path(__file__).with_name("bundle-deps.toml")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def make_tar_gz(files: dict) -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        for name, content in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = 0o755
            tar.addfile(info, io.BytesIO(content))
    return buf.getvalue()


def make_zip(files: dict) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        for name, content in files.items():
            zf.writestr(name, content)
    return buf.getvalue()


class CountingHandler(SimpleHTTPRequestHandler):
    hits: dict = {}

    def do_GET(self):
        CountingHandler.hits[self.path] = CountingHandler.hits.get(self.path, 0) + 1
        super().do_GET()

    def log_message(self, *args):
        pass


class FixtureServer:
    """Serves a temp directory over HTTP on a free local port."""

    def __init__(self, root: Path):
        CountingHandler.hits = {}
        handler = partial(CountingHandler, directory=str(root))
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.url = f"http://127.0.0.1:{self.httpd.server_address[1]}"
        self.thread = threading.Thread(target=self.httpd.serve_forever, args=(0.05,), daemon=True)
        self.thread.start()

    def hits(self, path: str) -> int:
        return CountingHandler.hits.get(path, 0)

    def total_hits(self) -> int:
        return sum(CountingHandler.hits.values())

    def close(self):
        self.httpd.shutdown()
        self.httpd.server_close()


class BundleFixture(unittest.TestCase):
    """Builds a small fake set of artifacts, a deps file and a server."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="fbd-test-"))
        self.addCleanup(fbd.rmtree, self.tmp)
        self.served = self.tmp / "served"
        self.served.mkdir()
        self.out = self.tmp / "out"
        self.cache = self.tmp / "cache"
        self.artifacts = {
            "jdtls.tar.gz": make_tar_gz(
                {
                    "plugins/org.eclipse.equinox.launcher_1.0.jar": b"launcher",
                    "config_win/config.ini": b"win",
                    "config_linux/config.ini": b"linux",
                }
            ),
            "lombok-1.0.jar": make_zip({"LICENSE": b"lombok MIT text", "lombok/Main.class": b"x"}),
            "java-debug-0.1.jar": b"debug-bytes",
            "proxy-windows.zip": make_zip({"java-lsp-proxy.exe": b"win-proxy"}),
            "proxy-linux.tar.gz": make_tar_gz({"java-lsp-proxy": b"linux-proxy"}),
            # No file extension, like the Zed gallery endpoint.
            "ext/download": make_tar_gz(
                {"extension.toml": b'id = "java"\n', "grammars/java.wasm": b"wasm"}
            ),
            "jdk-windows.zip": make_zip(
                {"jdk-21+1/bin/java.exe": b"java", "jdk-21+1/release": b"JAVA_VERSION"}
            ),
            "jdk-linux.tar.gz": make_tar_gz(
                {"jdk-21+1/bin/java": b"java", "jdk-21+1/release": b"JAVA_VERSION"}
            ),
        }
        for name, data in self.artifacts.items():
            path = self.served / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        self.server = FixtureServer(self.served)
        self.addCleanup(self.server.close)
        self.deps_path = self.tmp / "deps.toml"
        lic = self.tmp / "licenses"
        lic.mkdir()
        (lic / "jdtls.txt").write_text("EPL-2.0 text\n", encoding="utf-8")
        (lic / "java-debug.txt").write_text("EPL-1.0 text\n", encoding="utf-8")
        (lic / "apache.txt").write_text("Apache-2.0 text\n", encoding="utf-8")
        self.write_deps()

    def h(self, name: str) -> str:
        return sha256(self.artifacts[name])

    def write_deps(self, overrides: dict | None = None):
        overrides = overrides or {}
        u = self.server.url

        def sha(name):
            return overrides.get(name, self.h(name))

        text = f"""
schema = 1
targets = ["windows-x64", "linux-x64"]

[artifacts.jdk]
version = "21+1"
license = "GPL-2.0-only WITH Classpath-exception-2.0"
source = "https://example.invalid/adoptium"
source_code = "https://example.invalid/adoptium/jdk-sources.tar.gz"
source_tag = "https://example.invalid/adoptium/tree/jdk-21+1"
license_dir = "jre/legal/java.base"
into = "work"

[artifacts.jdk.windows-x64]
url = "{u}/jdk-windows.zip"
sha256 = "{sha('jdk-windows.zip')}"
root = "jdk-21+1"

[artifacts.jdk.linux-x64]
url = "{u}/jdk-linux.tar.gz"
sha256 = "{sha('jdk-linux.tar.gz')}"
root = "jdk-21+1"

[artifacts.jdtls]
version = "1.0"
license = "EPL-2.0"
source = "https://example.invalid/jdtls"
license_file = "licenses/jdtls.txt"
dest = "jdtls"

[artifacts.jdtls.any]
url = "{u}/jdtls.tar.gz"
sha256 = "{sha('jdtls.tar.gz')}"

[artifacts.lombok]
version = "1.0"
license = "MIT"
source = "https://example.invalid/lombok"
license_member = "LICENSE"
dest = "lombok.jar"

[artifacts.lombok.any]
url = "{u}/lombok-1.0.jar"
sha256 = "{sha('lombok-1.0.jar')}"

[artifacts.java-debug]
version = "0.1"
license = "EPL-1.0"
source = "https://example.invalid/java-debug"
license_file = "licenses/java-debug.txt"
dest = "java-debug.jar"

[artifacts.java-debug.any]
url = "{u}/java-debug-0.1.jar"
sha256 = "{sha('java-debug-0.1.jar')}"

[artifacts.java-lsp-proxy]
version = "v1"
license = "Apache-2.0"
source = "https://example.invalid/proxy"
license_file = "licenses/apache.txt"
dest = "java-lsp-proxy{{exe}}"
member = "java-lsp-proxy{{exe}}"
executable = true

[artifacts.java-lsp-proxy.windows-x64]
url = "{u}/proxy-windows.zip"
sha256 = "{sha('proxy-windows.zip')}"

[artifacts.java-lsp-proxy.linux-x64]
url = "{u}/proxy-linux.tar.gz"
sha256 = "{sha('proxy-linux.tar.gz')}"

[artifacts.java-extension]
version = "1.0"
license = "Apache-2.0"
source = "https://example.invalid/gallery"
license_file = "licenses/apache.txt"
dest = "extensions/java"
format = "tar.gz"

[artifacts.java-extension.any]
url = "{u}/ext/download"
sha256 = "{sha('ext/download')}"
"""
        self.deps_path.write_text(text, encoding="utf-8")

    def build(self, target="windows-x64", steps=(), **kwargs):
        deps = fbd.load_deps(self.deps_path)
        return fbd.build_bundle(
            deps, target, self.out, self.cache, steps=steps, log=lambda *_: None, **kwargs
        )


class LoadDepsTest(unittest.TestCase):
    def write(self, text: str) -> Path:
        tmp = Path(tempfile.mkdtemp(prefix="fbd-toml-"))
        self.addCleanup(fbd.rmtree, tmp)
        path = tmp / "deps.toml"
        path.write_text(text, encoding="utf-8")
        return path

    def test_real_deps_file_pins_every_artifact_for_every_target(self):
        deps = fbd.load_deps(REAL_DEPS)
        self.assertEqual(
            set(deps.targets),
            {"windows-x64", "linux-x64", "linux-aarch64", "macos-aarch64", "macos-x64"},
        )
        self.assertEqual(
            set(deps.artifacts),
            {"jdk", "jdtls", "lombok", "java-debug", "java-lsp-proxy", "java-extension"},
        )
        for target in deps.targets:
            resolved = fbd.select(deps, target)
            self.assertEqual(len(resolved), len(deps.artifacts))
            for art in resolved:
                self.assertTrue(art.url.startswith("https://"), art.url)
                self.assertRegex(art.sha256, r"^[0-9a-f]{64}$")
                self.assertTrue(art.version)
                self.assertTrue(art.license)
                self.assertTrue(art.source.startswith("https://"), art.source)

    def test_real_deps_layout_matches_plan(self):
        deps = fbd.load_deps(REAL_DEPS)
        win = {a.name: a for a in fbd.select(deps, "windows-x64")}
        lin = {a.name: a for a in fbd.select(deps, "linux-x64")}
        self.assertEqual(win["jdk"].into, "work")
        self.assertEqual(win["jdtls"].dest, "jdtls")
        self.assertEqual(win["lombok"].dest, "lombok.jar")
        self.assertEqual(win["java-debug"].dest, "java-debug.jar")
        self.assertEqual(win["java-lsp-proxy"].dest, "java-lsp-proxy.exe")
        self.assertEqual(lin["java-lsp-proxy"].dest, "java-lsp-proxy")
        self.assertEqual(win["java-extension"].dest, "extensions/java")
        self.assertEqual(win["java-extension"].format, "tar.gz")
        self.assertEqual(win["jdk"].format, "zip")
        self.assertEqual(lin["jdk"].format, "tar.gz")

    def test_rejects_bad_sha256(self):
        path = self.write(
            """
schema = 1
targets = ["linux-x64"]
[artifacts.lombok]
version = "1"
license = "MIT"
source = "https://x"
dest = "lombok.jar"
[artifacts.lombok.any]
url = "https://x/lombok.jar"
sha256 = "abc"
"""
        )
        with self.assertRaisesRegex(fbd.DepsError, "sha256"):
            fbd.load_deps(path)

    def test_rejects_missing_target_pin(self):
        path = self.write(
            """
schema = 1
targets = ["linux-x64", "windows-x64"]
[artifacts.proxy]
version = "1"
license = "Apache-2.0"
source = "https://x"
dest = "p"
[artifacts.proxy.linux-x64]
url = "https://x/p.tar.gz"
sha256 = "%s"
"""
            % ("0" * 64)
        )
        with self.assertRaisesRegex(fbd.DepsError, "proxy.*windows-x64"):
            fbd.load_deps(path)

    def test_rejects_unknown_schema(self):
        path = self.write('schema = 2\ntargets = []\n[artifacts]\n')
        with self.assertRaisesRegex(fbd.DepsError, "schema"):
            fbd.load_deps(path)

    def test_rejects_dest_escaping_bundle(self):
        path = self.write(
            """
schema = 1
targets = ["linux-x64"]
[artifacts.evil]
version = "1"
license = "MIT"
source = "https://x"
dest = "../evil.jar"
[artifacts.evil.any]
url = "https://x/e.jar"
sha256 = "%s"
"""
            % ("0" * 64)
        )
        with self.assertRaisesRegex(fbd.DepsError, "dest"):
            fbd.load_deps(path)


class SelectTest(BundleFixture):
    def test_selects_per_target_pin_and_any(self):
        deps = fbd.load_deps(self.deps_path)
        win = {a.name: a for a in fbd.select(deps, "windows-x64")}
        lin = {a.name: a for a in fbd.select(deps, "linux-x64")}
        self.assertTrue(win["java-lsp-proxy"].url.endswith("/proxy-windows.zip"))
        self.assertEqual(win["java-lsp-proxy"].format, "zip")
        self.assertEqual(win["java-lsp-proxy"].dest, "java-lsp-proxy.exe")
        self.assertEqual(win["java-lsp-proxy"].member, "java-lsp-proxy.exe")
        self.assertTrue(lin["java-lsp-proxy"].url.endswith("/proxy-linux.tar.gz"))
        self.assertEqual(lin["java-lsp-proxy"].format, "tar.gz")
        self.assertEqual(lin["java-lsp-proxy"].dest, "java-lsp-proxy")
        self.assertEqual(win["lombok"].url, lin["lombok"].url)
        self.assertEqual(win["lombok"].format, "file")
        self.assertEqual(win["java-extension"].format, "tar.gz")
        self.assertEqual(win["jdk"].root, "jdk-21+1")

    def test_unknown_target_is_an_error(self):
        deps = fbd.load_deps(self.deps_path)
        with self.assertRaisesRegex(fbd.DepsError, "macos-aarch64"):
            fbd.select(deps, "macos-aarch64")

    def test_parse_target(self):
        self.assertEqual(fbd.target_name("windows", "x64"), "windows-x64")
        self.assertEqual(fbd.normalize_arch("AMD64"), "x64")
        self.assertEqual(fbd.normalize_arch("x86_64"), "x64")
        self.assertEqual(fbd.normalize_arch("arm64"), "aarch64")
        self.assertEqual(fbd.normalize_os("darwin"), "macos")
        self.assertEqual(fbd.normalize_os("win32"), "windows")
        self.assertEqual(fbd.normalize_os("linux"), "linux")


class BuildTest(BundleFixture):
    def test_layout_of_out(self):
        jar = self.tmp / "brewery-daemon-all.jar"
        jar.write_bytes(b"daemon")
        seen = []

        def step(ctx):
            # Work dirs (the unpacked JDK) exist only while steps run.
            seen.append((ctx, (ctx.work["jdk"] / "bin/java.exe").is_file(), ctx.bundle.is_dir()))

        bundle = self.build("windows-x64", steps=[step], daemon_jar=jar)
        self.assertEqual(bundle, self.out / "windows-x64")
        b = bundle
        self.assertEqual(
            (b / "jdtls/plugins/org.eclipse.equinox.launcher_1.0.jar").read_bytes(),
            b"launcher",
        )
        self.assertTrue((b / "jdtls/config_win/config.ini").is_file())
        self.assertEqual((b / "lombok.jar").read_bytes(), self.artifacts["lombok-1.0.jar"])
        self.assertEqual((b / "java-debug.jar").read_bytes(), b"debug-bytes")
        self.assertEqual((b / "java-lsp-proxy.exe").read_bytes(), b"win-proxy")
        self.assertTrue((b / "extensions/java/extension.toml").is_file())
        self.assertTrue((b / "extensions/java/grammars/java.wasm").is_file())
        self.assertEqual((b / "brewery-daemon/brewery-daemon.jar").read_bytes(), b"daemon")
        self.assertTrue((b / "licenses").is_dir())
        # The JDK is a build input for jlink, not part of the bundle.
        self.assertFalse((b / "jdk").exists())
        self.assertFalse(any(p.name.startswith("jdk-21") for p in b.rglob("*")))
        # Only the finished bundle is left in --out.
        self.assertEqual(sorted(p.name for p in self.out.iterdir()), ["windows-x64"])
        # Post steps (jlink, launchers, licences) receive the context.
        self.assertEqual(len(seen), 1)
        ctx, jdk_present, bundle_present = seen[0]
        self.assertEqual(ctx.target, "windows-x64")
        self.assertEqual(ctx.os, "windows")
        self.assertEqual(ctx.arch, "x64")
        self.assertTrue(jdk_present)
        self.assertTrue(bundle_present)
        self.assertFalse(ctx.work["jdk"].exists())
        self.assertEqual(ctx.artifacts["jdtls"].version, "1.0")

    def test_linux_layout_uses_tar_and_no_exe(self):
        bundle = self.build("linux-x64")
        self.assertEqual((bundle / "java-lsp-proxy").read_bytes(), b"linux-proxy")
        self.assertFalse((bundle / "java-lsp-proxy.exe").exists())
        if os.name != "nt":
            self.assertTrue(os.access(bundle / "java-lsp-proxy", os.X_OK))

    def test_checksum_mismatch_fails_with_no_partial_output(self):
        self.write_deps({"java-debug-0.1.jar": "1" * 64})
        with self.assertRaisesRegex(fbd.FetchError, "SHA-256 mismatch"):
            self.build()
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))
        cached = [p for p in self.cache.rglob("*") if p.is_file()] if self.cache.exists() else []
        self.assertFalse(any("java-debug" in p.name for p in cached), cached)
        self.assertFalse(any(p.name.endswith(".part") for p in cached), cached)

    def test_checksum_mismatch_keeps_previous_bundle(self):
        self.build()
        marker = self.out / "windows-x64" / "lombok.jar"
        self.assertTrue(marker.is_file())
        self.write_deps({"lombok-1.0.jar": "2" * 64})
        fbd.rmtree(self.cache)
        with self.assertRaises(fbd.FetchError):
            self.build()
        self.assertTrue(marker.is_file())
        self.assertEqual(sorted(p.name for p in self.out.iterdir()), ["windows-x64"])

    def test_failing_step_leaves_no_partial_output(self):
        def boom(ctx):
            raise RuntimeError("jlink failed")

        with self.assertRaisesRegex(RuntimeError, "jlink failed"):
            self.build(steps=[boom])
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))

    def test_cache_hit_skips_download(self):
        self.build()
        first = self.server.total_hits()
        self.assertGreater(first, 0)
        fbd.rmtree(self.out)
        self.build()
        self.assertEqual(self.server.total_hits(), first)
        self.assertTrue((self.out / "windows-x64" / "lombok.jar").is_file())

    def test_corrupt_cache_entry_is_downloaded_again(self):
        self.build()
        before = self.server.hits("/lombok-1.0.jar")
        entry = fbd.cache_path(self.cache, self.h("lombok-1.0.jar"), self.server.url + "/lombok-1.0.jar")
        entry.write_bytes(b"tampered")
        self.build()
        self.assertEqual(self.server.hits("/lombok-1.0.jar"), before + 1)
        self.assertEqual(entry.read_bytes(), self.artifacts["lombok-1.0.jar"])

    def test_rebuild_is_idempotent_and_drops_stale_files(self):
        bundle = self.build()
        (bundle / "stale.txt").write_text("old")
        bundle = self.build()
        self.assertFalse((bundle / "stale.txt").exists())
        self.assertTrue((bundle / "lombok.jar").is_file())
        self.assertEqual(sorted(p.name for p in self.out.iterdir()), ["windows-x64"])

    def test_archive_path_traversal_is_rejected(self):
        evil = make_tar_gz({"../escape.txt": b"x"})
        self.artifacts["jdtls.tar.gz"] = evil
        (self.served / "jdtls.tar.gz").write_bytes(evil)
        self.write_deps()
        with self.assertRaisesRegex(fbd.FetchError, "unsafe"):
            self.build()
        self.assertFalse((self.tmp / "escape.txt").exists())
        self.assertFalse((self.out / "escape.txt").exists())
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))

    def test_missing_member_is_an_error(self):
        self.artifacts["proxy-windows.zip"] = make_zip({"other.exe": b"x"})
        (self.served / "proxy-windows.zip").write_bytes(self.artifacts["proxy-windows.zip"])
        self.write_deps()
        with self.assertRaisesRegex(fbd.FetchError, "java-lsp-proxy.exe"):
            self.build()

    def test_http_error_is_a_fetch_error(self):
        (self.served / "lombok-1.0.jar").unlink()
        with self.assertRaisesRegex(fbd.FetchError, "lombok"):
            self.build()
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))


def find_test_jdk() -> Path | None:
    """A full JDK 21+ (with jlink and jmods) for the real-jlink tests, or None."""
    exe = ".exe" if os.name == "nt" else ""
    for var in ("BREWERY_TEST_JDK", "JAVA_HOME"):
        home = os.environ.get(var)
        if home and (Path(home) / "bin" / f"jlink{exe}").is_file() and (Path(home) / "jmods").is_dir():
            return Path(home)
    return None


class FakeJlink:
    """Stands in for subprocess.run: records commands and fakes jlink and java."""

    def __init__(self, modules=None, jlink_rc=0, make_release=True):
        self.calls = []
        self.modules = fbd.JRE_MODULES if modules is None else modules
        self.jlink_rc = jlink_rc
        self.make_release = make_release

    def __call__(self, cmd, **kwargs):
        cmd = [str(c) for c in cmd]
        self.calls.append(cmd)
        name = Path(cmd[0]).name
        if name.startswith("jlink"):
            if self.jlink_rc:
                return subprocess.CompletedProcess(cmd, self.jlink_rc, "", "Error: module not found: jdk.bogus")
            out = Path(cmd[cmd.index("--output") + 1])
            (out / "bin").mkdir(parents=True)
            (out / "bin" / "java").write_bytes(b"")
            (out / "bin" / "java.exe").write_bytes(b"")
            legal = out / "legal" / "java.base"
            legal.mkdir(parents=True)
            (legal / "LICENSE").write_text("GPLv2 text\n", encoding="utf-8")
            (legal / "ASSEMBLY_EXCEPTION").write_text("assembly exception\n", encoding="utf-8")
            if self.make_release:
                (out / "release").write_text('JAVA_VERSION="21.0.12"\n', encoding="utf-8")
            return subprocess.CompletedProcess(cmd, 0, "", "")
        if "-version" in cmd:
            return subprocess.CompletedProcess(cmd, 0, "", 'openjdk version "21.0.12" 2026-07-21 LTS\n')
        if "--list-modules" in cmd:
            listing = "".join(f"{m}@21.0.12\n" for m in self.modules)
            return subprocess.CompletedProcess(cmd, 0, listing, "")
        raise AssertionError(f"unexpected command {cmd}")


class JreStepTest(BundleFixture):
    def run_step(self, fake, target="windows-x64", host=None):
        def step(ctx):
            fbd.build_jre(ctx, run=fake, host=host or target)

        return self.build(target, steps=[step])

    def test_jlink_gets_the_module_list_and_flags(self):
        fake = FakeJlink()
        bundle = self.run_step(fake)
        jlink = fake.calls[0]
        self.assertTrue(Path(jlink[0]).name.startswith("jlink"))
        # jlink comes from the unpacked JDK (a work dir), not from PATH.
        self.assertEqual(Path(jlink[0]).parent.parent.name, "jdk")
        modules = jlink[jlink.index("--add-modules") + 1].split(",")
        for m in (
            "java.se", "jdk.unsupported", "jdk.crypto.ec", "jdk.zipfs", "jdk.net",
            "jdk.management", "jdk.jdi", "jdk.attach", "jdk.jdwp.agent", "java.instrument",
            "jdk.localedata", "jdk.charsets", "jdk.jfr", "jdk.httpserver", "jdk.compiler",
            "jdk.security.auth",
        ):
            self.assertIn(m, modules)
        for flag in ("--strip-debug", "--no-man-pages", "--no-header-files", "--compress=zip-6"):
            self.assertIn(flag, jlink)
        self.assertEqual(Path(jlink[jlink.index("--output") + 1]).name, "jre")
        # jre/ lands in the bundle with its legal/ directory.
        self.assertTrue((bundle / "jre" / "release").is_file())
        self.assertTrue((bundle / "jre" / "legal" / "java.base" / "LICENSE").is_file())
        # The image is checked by running it.
        self.assertTrue(any("-version" in c for c in fake.calls[1:]))
        self.assertTrue(any("--list-modules" in c for c in fake.calls[1:]))

    def test_refuses_to_cross_link(self):
        with self.assertRaisesRegex(fbd.FetchError, "target OS"):
            self.run_step(FakeJlink(), target="linux-x64", host="windows-x64")
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))

    def test_jlink_failure_is_a_fetch_error_with_its_output(self):
        with self.assertRaisesRegex(fbd.FetchError, "jdk.bogus"):
            self.run_step(FakeJlink(jlink_rc=1))
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))

    def test_image_without_release_file_is_rejected(self):
        with self.assertRaisesRegex(fbd.FetchError, "release"):
            self.run_step(FakeJlink(make_release=False))

    def test_image_missing_a_module_is_rejected(self):
        fake = FakeJlink(modules=[m for m in fbd.JRE_MODULES if m != "jdk.jdi"])
        with self.assertRaisesRegex(fbd.FetchError, "jdk.jdi"):
            self.run_step(fake)


@unittest.skipUnless(find_test_jdk(), "needs a full JDK 21 in BREWERY_TEST_JDK or JAVA_HOME")
class RealJlinkTest(unittest.TestCase):
    def test_real_jlink_builds_a_runnable_jre(self):
        jdk = find_test_jdk()
        tmp = Path(tempfile.mkdtemp(prefix="fbd-jlink-"))
        self.addCleanup(fbd.rmtree, tmp)
        bundle = tmp / "bundle"
        bundle.mkdir()
        target = fbd.host_target()
        os_name, _, arch = target.partition("-")
        ctx = fbd.BundleContext(
            target=target, os=os_name, arch=arch, bundle=bundle,
            work={"jdk": jdk}, log=lambda *_: None,
        )
        fbd.build_jre(ctx)
        exe = ".exe" if os.name == "nt" else ""
        java = bundle / "jre" / "bin" / f"java{exe}"
        out = subprocess.run([java, "-version"], capture_output=True, text=True)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn("version", out.stderr)
        self.assertTrue((bundle / "jre" / "legal" / "java.base").is_dir())
        self.assertFalse((bundle / "jre" / "include").exists())
        self.assertFalse((bundle / "jre" / "jmods").exists())


class LauncherStepTest(BundleFixture):
    def setUp(self):
        super().setUp()
        jdtls = make_tar_gz(
            {
                "plugins/org.eclipse.equinox.launcher_1.0.jar": b"launcher",
                "config_win/config.ini": b"win",
                "config_ss_win/config.ini": b"ss-win",
                "config_linux/config.ini": b"linux",
                "config_linux_arm/config.ini": b"linux-arm",
                "config_mac/config.ini": b"mac",
                "bin/jdtls.py": b"python",
            }
        )
        self.artifacts["jdtls.tar.gz"] = jdtls
        (self.served / "jdtls.tar.gz").write_bytes(jdtls)
        self.write_deps()

    def configs(self, bundle):
        return sorted(p.name for p in (bundle / "jdtls").iterdir() if p.name.startswith("config_"))

    def test_windows_gets_the_cmd_launcher_with_crlf_and_only_its_config(self):
        bundle = self.build("windows-x64", steps=[fbd.write_jdtls_launchers])
        cmd = bundle / "jdtls" / "brewery-jdtls.cmd"
        data = cmd.read_bytes()
        self.assertGreater(data.count(b"\r\n"), 10)
        self.assertEqual(data.count(b"\n"), data.count(b"\r\n"), "every line ends in CRLF")
        self.assertFalse((bundle / "jdtls" / "brewery-jdtls").exists())
        self.assertEqual(self.configs(bundle), ["config_ss_win", "config_win"])

    def test_linux_gets_the_sh_launcher_with_lf_and_exec_bit(self):
        bundle = self.build("linux-x64", steps=[fbd.write_jdtls_launchers])
        sh = bundle / "jdtls" / "brewery-jdtls"
        data = sh.read_bytes()
        self.assertTrue(data.startswith(b"#!/bin/sh\n"))
        self.assertNotIn(b"\r", data)
        if os.name != "nt":
            self.assertTrue(os.access(sh, os.X_OK))
        self.assertFalse((bundle / "jdtls" / "brewery-jdtls.cmd").exists())
        self.assertEqual(self.configs(bundle), ["config_linux"])

    def test_missing_platform_config_is_an_error(self):
        def step(ctx):
            # The fixture has no config_mac_arm.
            ctx.target, ctx.os, ctx.arch = "macos-aarch64", "macos", "aarch64"
            fbd.write_jdtls_launchers(ctx)

        with self.assertRaisesRegex(fbd.FetchError, "config_mac_arm"):
            self.build("linux-x64", steps=[step])


class LicenseStepTest(BundleFixture):
    def fake_jre(self, ctx):
        legal = ctx.bundle / "jre" / "legal" / "java.base"
        legal.mkdir(parents=True)
        (legal / "LICENSE").write_text("GPLv2 text\n", encoding="utf-8")
        (legal / "ASSEMBLY_EXCEPTION").write_text("assembly exception\n", encoding="utf-8")
        (legal / "ADDITIONAL_LICENSE_INFO").write_text("additional info\n", encoding="utf-8")

    def daemon_jar(self, with_notices=True) -> Path:
        files = {"dev/brewery/daemon/MainKt.class": b"x"}
        if with_notices:
            files["THIRD-PARTY-NOTICES.txt"] = b"Third-party notices for the dependencies\n"
        jar = self.tmp / "brewery-daemon-all.jar"
        jar.write_bytes(make_zip(files))
        return jar

    def test_every_component_gets_its_licence_and_an_index(self):
        bundle = self.build(steps=[self.fake_jre, fbd.collect_licenses], daemon_jar=self.daemon_jar())
        lic = bundle / "licenses"
        self.assertEqual((lic / "jre" / "LICENSE").read_text(encoding="utf-8"), "GPLv2 text\n")
        self.assertTrue((lic / "jre" / "ASSEMBLY_EXCEPTION").is_file())
        self.assertTrue((lic / "jre" / "ADDITIONAL_LICENSE_INFO").is_file())
        source = (lic / "jre" / "SOURCE.txt").read_text(encoding="utf-8")
        self.assertIn("https://example.invalid/adoptium/jdk-sources.tar.gz", source)
        self.assertIn("https://example.invalid/adoptium/tree/jdk-21+1", source)
        self.assertEqual((lic / "jdtls" / "LICENSE").read_text(encoding="utf-8"), "EPL-2.0 text\n")
        self.assertEqual((lic / "java-debug" / "LICENSE").read_text(encoding="utf-8"), "EPL-1.0 text\n")
        self.assertEqual((lic / "lombok" / "LICENSE").read_bytes(), b"lombok MIT text")
        self.assertEqual(
            (lic / "java-lsp-proxy" / "LICENSE").read_text(encoding="utf-8"), "Apache-2.0 text\n"
        )
        self.assertEqual(
            (lic / "java-extension" / "LICENSE").read_text(encoding="utf-8"), "Apache-2.0 text\n"
        )
        self.assertIn(
            b"Third-party notices",
            (lic / "brewery-daemon" / "THIRD-PARTY-NOTICES.txt").read_bytes(),
        )
        index = (lic / "README.txt").read_text(encoding="utf-8")
        for name, version, license in (
            ("jre", "21+1", "GPL-2.0-only WITH Classpath-exception-2.0"),
            ("jdtls", "1.0", "EPL-2.0"),
            ("lombok", "1.0", "MIT"),
            ("java-debug", "0.1", "EPL-1.0"),
            ("java-lsp-proxy", "v1", "Apache-2.0"),
            ("java-extension", "1.0", "Apache-2.0"),
        ):
            self.assertRegex(index, rf"(?m)^{re.escape(name)} {re.escape(version)}$")
            self.assertIn(license, index)
        self.assertIn("brewery-daemon", index)

    def test_without_daemon_jar_there_are_no_daemon_notices(self):
        bundle = self.build(steps=[self.fake_jre, fbd.collect_licenses])
        self.assertFalse((bundle / "licenses" / "brewery-daemon").exists())

    def test_daemon_jar_without_notices_is_an_error(self):
        with self.assertRaisesRegex(fbd.FetchError, "THIRD-PARTY-NOTICES"):
            self.build(steps=[self.fake_jre, fbd.collect_licenses], daemon_jar=self.daemon_jar(False))

    def test_missing_jre_legal_is_an_error(self):
        with self.assertRaisesRegex(fbd.FetchError, "legal"):
            self.build(steps=[fbd.collect_licenses])

    def test_artifact_without_licence_source_is_an_error(self):
        text = self.deps_path.read_text(encoding="utf-8").replace(
            'license_file = "licenses/java-debug.txt"\n', ""
        )
        self.deps_path.write_text(text, encoding="utf-8")
        with self.assertRaisesRegex(fbd.FetchError, "java-debug"):
            self.build(steps=[self.fake_jre, fbd.collect_licenses])

    def test_real_deps_name_a_licence_for_every_artifact(self):
        deps = fbd.load_deps(REAL_DEPS)
        for target in deps.targets:
            for art in fbd.select(deps, target):
                sources = [s for s in (art.license_file, art.license_member, art.license_dir) if s]
                self.assertEqual(len(sources), 1, art.name)
                if art.license_file:
                    path = REAL_DEPS.parent / art.license_file
                    self.assertTrue(path.is_file(), path)
                    self.assertNotIn(b"\r", path.read_bytes())
        jdk = fbd.select(deps, "linux-x64")[0]
        self.assertEqual(jdk.name, "jdk")
        self.assertTrue(jdk.source_code.startswith("https://"))
        self.assertTrue(jdk.source_tag.startswith("https://"))


class CliTest(BundleFixture):
    def setUp(self):
        super().setUp()
        # The fixture JDK is fake: the CLI tests cover argument handling, not jlink.
        patcher = mock.patch.object(fbd, "DEFAULT_STEPS", ())
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_main_builds_requested_target(self):
        code = fbd.main(
            [
                "--deps", str(self.deps_path),
                "--os", "linux",
                "--arch", "x64",
                "--out", str(self.out),
                "--cache", str(self.cache),
                "--quiet",
            ]
        )
        self.assertEqual(code, 0)
        self.assertTrue((self.out / "linux-x64" / "java-lsp-proxy").is_file())

    def test_main_reports_mismatch_with_exit_code(self):
        self.write_deps({"lombok-1.0.jar": "3" * 64})
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            code = fbd.main(
                [
                    "--deps", str(self.deps_path),
                    "--os", "windows",
                    "--arch", "x64",
                    "--out", str(self.out),
                    "--cache", str(self.cache),
                    "--quiet",
                ]
            )
        self.assertNotEqual(code, 0)
        self.assertIn("SHA-256 mismatch", err.getvalue())
        self.assertFalse((self.out / "windows-x64").exists())


if __name__ == "__main__":
    unittest.main()
