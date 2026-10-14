"""Tests for the jdtls launchers (script/brewery/jdtls/brewery-jdtls{,.cmd}).

The launcher is installed into a fake bundle by the fetcher's own step, and the
equinox launcher jar is replaced by a tiny Java program that prints what it was
started with (JVM options, system properties, program arguments) and doubles
as a javaagent. The test runs the host's launcher the way java-lsp-proxy does
(`cmd /C <launcher> ...` on Windows, the script itself elsewhere).

Needs a JDK 21 (javac, jar) in BREWERY_TEST_JDK or JAVA_HOME; skipped otherwise.
Run from the repo root:
    python -m unittest discover script/brewery
"""

import os
import shutil
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

import fetch_bundle_deps as fbd

EXE = ".exe" if os.name == "nt" else ""
WINDOWS = os.name == "nt"


def find_jdk() -> Path | None:
    for var in ("BREWERY_TEST_JDK", "JAVA_HOME"):
        home = os.environ.get(var)
        if home and (Path(home) / "bin" / f"javac{EXE}").is_file():
            return Path(home)
    return None


JDK = find_jdk()

FAKE_LAUNCHER = textwrap.dedent(
    """
    import java.lang.instrument.Instrumentation;
    import java.lang.management.ManagementFactory;

    public class FakeLauncher {
        public static void premain(String args, Instrumentation inst) {
            System.setProperty("brewery.agent", "loaded");
        }

        public static void main(String[] args) {
            StringBuilder out = new StringBuilder();
            for (String jvm : ManagementFactory.getRuntimeMXBean().getInputArguments()) {
                out.append("jvm=").append(jvm).append('\\n');
            }
            for (String key : new String[] {"java.home", "user.dir", "brewery.agent", "brewery.java",
                    "osgi.configuration.area", "osgi.sharedConfiguration.area"}) {
                out.append("prop:").append(key).append('=')
                    .append(System.getProperty(key)).append('\\n');
            }
            for (String arg : args) {
                out.append("arg=").append(arg).append('\\n');
            }
            System.out.print(out);
            System.out.flush();
        }
    }
    """
)


def norm(path) -> str:
    # realpath: on macOS the temp dir is /var/..., which the launcher sees as /private/var.
    return os.path.normcase(os.path.realpath(str(path)))


@unittest.skipUnless(JDK, "needs a JDK 21 in BREWERY_TEST_JDK or JAVA_HOME")
class LauncherTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.build = Path(tempfile.mkdtemp(prefix="jdtls-launcher-build-"))
        src = cls.build / "FakeLauncher.java"
        src.write_text(FAKE_LAUNCHER, encoding="utf-8")
        subprocess.run(
            [JDK / "bin" / f"javac{EXE}", "-d", cls.build / "classes", src],
            check=True, capture_output=True,
        )
        manifest = cls.build / "manifest.txt"
        manifest.write_text("Main-Class: FakeLauncher\nPremain-Class: FakeLauncher\n", encoding="utf-8")
        cls.jar = cls.build / "fake-launcher.jar"
        subprocess.run(
            [JDK / "bin" / f"jar{EXE}", "--create", "--file", cls.jar, "--manifest", manifest,
             "-C", cls.build / "classes", "."],
            check=True, capture_output=True,
        )

    @classmethod
    def tearDownClass(cls):
        fbd.rmtree(cls.build)

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="jdtls-launcher-"))
        self.addCleanup(fbd.rmtree, self.tmp)
        self.cache = self.tmp / "cache"
        self.project = self.tmp / "project"
        self.project.mkdir()

    def make_bundle(self, root: Path, with_jre_link=False) -> Path:
        """A bundle with the real launcher, a platform config dir and the fake launcher jar."""
        target = fbd.host_target()
        os_name, _, arch = target.partition("-")
        jdtls = root / "jdtls"
        (jdtls / "plugins").mkdir(parents=True)
        shutil.copyfile(self.jar, jdtls / "plugins" / "org.eclipse.equinox.launcher_1.8.0.v1.jar")
        for config in fbd.JDTLS_CONFIGS[target]:
            (jdtls / config).mkdir()
            (jdtls / config / "config.ini").write_text("osgi.bundles=\n", encoding="utf-8")
        ctx = fbd.BundleContext(target=target, os=os_name, arch=arch, bundle=root, log=lambda *_: None)
        fbd.write_jdtls_launchers(ctx)
        if with_jre_link:
            link = root / "jre"
            if WINDOWS:
                made = subprocess.run(
                    ["cmd", "/C", "mklink", "/J", str(link), str(JDK)], capture_output=True
                )
                if made.returncode != 0:
                    self.skipTest("cannot create a directory junction")
                # Remove the link itself first so cleanup can never walk into the JDK.
                self.addCleanup(os.rmdir, link)
            else:
                # java resolves symlinks to find its home, so use a wrapper that tags the JVM.
                java = link / "bin" / "java"
                java.parent.mkdir(parents=True)
                java.write_text(
                    f'#!/bin/sh\nexec "{JDK}/bin/java" -Dbrewery.java=bundled "$@"\n', encoding="utf-8"
                )
                java.chmod(0o755)
        return jdtls / ("brewery-jdtls.cmd" if WINDOWS else "brewery-jdtls")

    def env(self, **overrides):
        env = dict(os.environ)
        env["BREWERY_JDTLS_CACHE_DIR"] = str(self.cache)
        env["JAVA_HOME"] = str(JDK)
        for key, value in overrides.items():
            if value is None:
                env.pop(key, None)
            else:
                env[key] = value
        return env

    def run_launcher(self, launcher: Path, *args, env=None, cwd=None, check=True):
        # java-lsp-proxy (src/main.rs) runs `.cmd`/`.bat` launchers as `cmd /C <bin> <args>`.
        cmd = (["cmd", "/C", str(launcher)] if WINDOWS else [str(launcher)]) + [str(a) for a in args]
        proc = subprocess.run(
            cmd, cwd=cwd or self.project, env=env or self.env(), capture_output=True, text=True,
            stdin=subprocess.DEVNULL, timeout=120,
        )
        if check:
            self.assertEqual(proc.returncode, 0, f"stdout={proc.stdout!r}\nstderr={proc.stderr!r}")
        return proc

    def parse(self, stdout: str):
        jvm, props, args = [], {}, []
        for line in stdout.splitlines():
            if line.startswith("jvm="):
                jvm.append(line[4:])
            elif line.startswith("prop:"):
                key, _, value = line[5:].partition("=")
                props[key] = value
            elif line.startswith("arg="):
                args.append(line[4:])
            else:
                self.fail(f"unexpected stdout line {line!r} (the launcher must keep stdout for LSP)")
        return jvm, props, args

    def test_default_launch_matches_the_extension_args(self):
        launcher = self.make_bundle(self.tmp / "bundle")
        jvm, props, args = self.parse(self.run_launcher(launcher).stdout)
        jdtls = self.tmp / "bundle" / "jdtls"
        config = fbd.JDTLS_CONFIGS[fbd.host_target()][0]
        for expected in (
            "-Declipse.application=org.eclipse.jdt.ls.core.id1",
            "-Dosgi.bundles.defaultStartLevel=4",
            "-Declipse.product=org.eclipse.jdt.ls.core.product",
            "-Dosgi.checkConfiguration=true",
            "-Dosgi.sharedConfiguration.area.readOnly=true",
            "-Dosgi.configuration.cascaded=true",
            "-Djava.import.generatesMetadataFilesAtProjectRoot=false",
            "-Xms1G",
            "--add-modules=ALL-SYSTEM",
            # The JVM reports "--add-opens X" as "--add-opens=X".
            "--add-opens=java.base/java.util=ALL-UNNAMED",
            "--add-opens=java.base/java.lang=ALL-UNNAMED",
        ):
            self.assertIn(expected, jvm)
        # Shared config read-only from the bundle, the writable one in the user cache.
        self.assertEqual(norm(props["osgi.sharedConfiguration.area"]), norm(jdtls / config))
        area = Path(props["osgi.configuration.area"])
        self.assertEqual(norm(area.parent), norm(self.cache / "config"))
        self.assertRegex(area.name, r"^[0-9a-f]{40}$")
        # Without -data the workspace is per project directory, in the user cache.
        self.assertEqual(args[0], "-data")
        data = Path(args[1])
        self.assertEqual(norm(data.parent), norm(self.cache / "workspace"))
        self.assertRegex(data.name, r"^jdtls-[0-9a-f]{40}$")
        self.assertEqual(len(args), 2)
        self.assertEqual(norm(props["user.dir"]), norm(self.project))
        self.assertEqual(props["brewery.agent"], "null")

    def test_workspace_differs_per_project_and_is_stable(self):
        launcher = self.make_bundle(self.tmp / "bundle")
        other = self.tmp / "other"
        other.mkdir()
        first = self.parse(self.run_launcher(launcher).stdout)[2][1]
        again = self.parse(self.run_launcher(launcher).stdout)[2][1]
        second = self.parse(self.run_launcher(launcher, cwd=other).stdout)[2][1]
        self.assertEqual(first, again)
        self.assertNotEqual(first, second)

    def test_config_area_differs_per_bundle_location(self):
        a = self.parse(self.run_launcher(self.make_bundle(self.tmp / "a")).stdout)[1]
        b = self.parse(self.run_launcher(self.make_bundle(self.tmp / "b")).stdout)[1]
        self.assertNotEqual(a["osgi.configuration.area"], b["osgi.configuration.area"])

    def test_jvm_arg_and_data_from_the_extension(self):
        # What jdtls_server.rs appends: --jvm-arg=-javaagent:<lombok> and -data <dir>.
        launcher = self.make_bundle(self.tmp / "bundle")
        agent = self.tmp / "agent" / "lombok.jar"
        agent.parent.mkdir()
        shutil.copyfile(self.jar, agent)
        data = self.tmp / "data" / "jdtls-abc"
        jvm, props, args = self.parse(
            self.run_launcher(launcher, f"--jvm-arg=-javaagent:{agent}", "-data", data).stdout
        )
        self.assertEqual(props["brewery.agent"], "loaded")
        self.assertIn(f"-javaagent:{agent}", jvm)
        self.assertEqual(args, ["-data", str(data)])

    def test_jvm_arg_and_data_with_spaces(self):
        launcher = self.make_bundle(self.tmp / "bundle")
        agent = self.tmp / "agent dir" / "lombok.jar"
        agent.parent.mkdir()
        shutil.copyfile(self.jar, agent)
        data = self.tmp / "data dir" / "jdtls-abc"
        jvm, props, args = self.parse(
            self.run_launcher(launcher, f"--jvm-arg=-javaagent:{agent}", "-data", data).stdout
        )
        self.assertEqual(props["brewery.agent"], "loaded")
        self.assertEqual(args, ["-data", str(data)])

    def test_several_jvm_args_and_extra_jdtls_args(self):
        launcher = self.make_bundle(self.tmp / "bundle")
        jvm, _, args = self.parse(
            self.run_launcher(launcher, "--jvm-arg=-Xss2m", "--jvm-arg=-Xmx3G", "--extra").stdout
        )
        self.assertIn("-Xss2m", jvm)
        self.assertIn("-Xmx3G", jvm)
        # JVM options from --jvm-arg come after the defaults, so they win (-Xmx, -Xms).
        self.assertGreater(jvm.index("-Xmx3G"), jvm.index("-Xms1G"))
        self.assertEqual(args[-1], "--extra")
        self.assertEqual(args[0], "-data")

    def test_launcher_in_a_directory_with_spaces(self):
        launcher = self.make_bundle(self.tmp / "Brewery bundle")
        jvm, props, args = self.parse(self.run_launcher(launcher).stdout)
        self.assertEqual(
            norm(props["osgi.sharedConfiguration.area"]),
            norm(self.tmp / "Brewery bundle" / "jdtls" / fbd.JDTLS_CONFIGS[fbd.host_target()][0]),
        )
        self.assertEqual(args[0], "-data")

    def test_bundled_jre_wins_over_java_home(self):
        launcher = self.make_bundle(self.tmp / "bundle", with_jre_link=True)
        env = self.env(JAVA_HOME=str(self.tmp / "no-such-jdk"))
        _, props, _ = self.parse(self.run_launcher(launcher, env=env).stdout)
        if WINDOWS:
            self.assertEqual(norm(props["java.home"]), norm(self.tmp / "bundle" / "jre"))
        else:
            self.assertEqual(props["brewery.java"], "bundled")

    def test_java_home_without_bundled_jre(self):
        launcher = self.make_bundle(self.tmp / "bundle")
        _, props, _ = self.parse(self.run_launcher(launcher).stdout)
        self.assertEqual(norm(Path(props["java.home"]).resolve()), norm(JDK.resolve()))

    def test_default_cache_is_the_user_cache_dir(self):
        launcher = self.make_bundle(self.tmp / "bundle")
        home = self.tmp / "home"
        home.mkdir()
        if WINDOWS:
            env = self.env(BREWERY_JDTLS_CACHE_DIR=None, LOCALAPPDATA=str(home))
            expected = home / "Brewery" / "jdtls"
        elif fbd.host_target().startswith("macos"):
            env = self.env(BREWERY_JDTLS_CACHE_DIR=None, HOME=str(home))
            expected = home / "Library" / "Caches" / "Brewery" / "jdtls"
        else:
            env = self.env(BREWERY_JDTLS_CACHE_DIR=None, XDG_CACHE_HOME=str(home / "xdg"))
            expected = home / "xdg" / "brewery" / "jdtls"
        _, props, args = self.parse(self.run_launcher(launcher, env=env).stdout)
        self.assertEqual(norm(Path(props["osgi.configuration.area"]).parent.parent), norm(expected))
        self.assertEqual(norm(Path(args[1]).parent.parent), norm(expected))

    def test_missing_equinox_launcher_fails_on_stderr(self):
        launcher = self.make_bundle(self.tmp / "bundle")
        for jar in (self.tmp / "bundle" / "jdtls" / "plugins").iterdir():
            jar.unlink()
        proc = self.run_launcher(launcher, check=False)
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual(proc.stdout, "")
        self.assertIn("equinox", proc.stderr.lower())

    def test_exit_code_of_jdtls_is_passed_on(self):
        launcher = self.make_bundle(self.tmp / "bundle")
        # An unknown JVM option makes java exit non-zero before main runs.
        proc = self.run_launcher(launcher, "--jvm-arg=-XX:+NoSuchBreweryOption", check=False)
        self.assertNotEqual(proc.returncode, 0)


if __name__ == "__main__":
    unittest.main()
