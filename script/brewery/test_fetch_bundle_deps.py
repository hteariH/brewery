"""Tests for fetch_bundle_deps.py.

Run from the repo root:
    python -m unittest discover script/brewery
"""

import contextlib
import hashlib
import io
import os
import tarfile
import tempfile
import threading
import unittest
import zipfile
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
            "lombok-1.0.jar": b"lombok-bytes",
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
dest = "jdtls"

[artifacts.jdtls.any]
url = "{u}/jdtls.tar.gz"
sha256 = "{sha('jdtls.tar.gz')}"

[artifacts.lombok]
version = "1.0"
license = "MIT"
source = "https://example.invalid/lombok"
dest = "lombok.jar"

[artifacts.lombok.any]
url = "{u}/lombok-1.0.jar"
sha256 = "{sha('lombok-1.0.jar')}"

[artifacts.java-debug]
version = "0.1"
license = "EPL-1.0"
source = "https://example.invalid/java-debug"
dest = "java-debug.jar"

[artifacts.java-debug.any]
url = "{u}/java-debug-0.1.jar"
sha256 = "{sha('java-debug-0.1.jar')}"

[artifacts.java-lsp-proxy]
version = "v1"
license = "Apache-2.0"
source = "https://example.invalid/proxy"
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
        self.assertEqual((b / "lombok.jar").read_bytes(), b"lombok-bytes")
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
        self.assertEqual(entry.read_bytes(), b"lombok-bytes")

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


class CliTest(BundleFixture):
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
