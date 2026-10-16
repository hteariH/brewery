#!/usr/bin/env python3
"""Smoke-test a built offline bundle: the daemon and jdtls on the bundled pieces.

    python script/brewery/smoke_bundle.py target/bundle/<os>-<arch> [--require-offline]
        [--checks basic,gradle,gradle-old]

basic (the default):
1. daemon_smoke: <bundle>/jre/bin/java -jar <bundle>/brewery-daemon/brewery-daemon.jar
   reads the maven-multi fixture (maven, 3 modules).
2. lsp_smoke: jdtls started the way the Java extension starts it
   (java-lsp-proxy -> jdtls/brewery-jdtls[.cmd] --jvm-arg=-javaagent:jdtls/tools/lombok.jar)
   on a plain folder with one Java file (no build files): initialize, ServiceReady,
   java-debug's startDebugSession, documentSymbol; the bundle must stay unchanged.

gradle (needs the network once, for the Gradle distribution of the fixture's wrapper):
3. jdtls imports a two-project Gradle build (app uses lib): no errors on the build
   files, documentSymbol, and a hover in app that resolves lib's class. Then the
   bundle's jdtls is "updated" (a copy of it gets new file times, as a Brewery update
   brings) and started again on the same workspace: after a build file change jdtls
   must still synchronize the build. A workspace that outlives its jdtls keeps the
   old jdtls's Gradle init script path, which used to break exactly that.

gradle-old (needs the network once, and a JDK 17 in BREWERY_SMOKE_JDK17 or the hosted
runners' JAVA_HOME_17_X64 / JAVA_HOME_17_arm64):
4. The same build on Gradle 8.4, which cannot run on the bundled Java 21, with the JDK 17
   as JAVA_HOME: the daemon reads its model, and jdtls (on the bundled JRE) imports it
   with Gradle running on JAVA_HOME.

--require-offline first checks that the network is unreachable, so a run inside
`unshare -n` / `docker run --network none` proves the bundle needs no downloads.
"""

from __future__ import annotations

import argparse
import os
import shutil
import socket
import sys
import tempfile
import time
from pathlib import Path

import daemon_smoke
import lsp_smoke

REPO = Path(__file__).resolve().parent.parent.parent
FIXTURE = REPO / "brewery-daemon" / "src" / "test" / "resources" / "projects" / "maven-multi"
# jdtls gets a plain source folder (no pom.xml): importing a Maven project needs Maven's
# default plugins in ~/.m2, which a fresh, offline machine does not have, and that is
# the user project's business, not the bundle's.
JAVA_FILE = "src/demo/Owner.java"
JAVA_SOURCE = """package demo;

public class Owner {
    private String name;

    public String getName() {
        return name;
    }
}
"""

# A Gradle build without external dependencies: only the wrapper's distribution is
# downloaded. No gradle-wrapper.jar: the Tooling API reads gradle-wrapper.properties.
GRADLE_VERSION = "9.4.0"
# The newest Gradle that cannot run on Java 21 (the bundled JRE): it needs the user's JDK.
OLD_GRADLE_VERSION = "8.4"
# Where the old-Gradle check looks for a JDK 17 (GitHub's hosted runners preinstall one).
OLD_GRADLE_JDK_VARS = ("BREWERY_SMOKE_JDK17", "JAVA_HOME_17_X64", "JAVA_HOME_17_arm64", "JAVA_HOME_17_ARM64")
GRADLE_MAIN = "app/src/main/java/demo/app/Main.java"
# The root build writes the JVM Gradle configures it on, for the checks to compare.
GRADLE_JAVA_HOME_FILE = "gradle-java-home.txt"
# Brewery's lsp.jdtls.initialization_options with the bundled launcher
# (crates/brewery_bundle/src/java_setup.rs, java_initialization_options_for).
INIT_SETTINGS = '{"java": {"import": {"gradle": {"java": {"home": "${brewery.gradle.java.home}"}}}}}'


def gradle_files(version: str) -> dict:
    return {
        "settings.gradle.kts": 'rootProject.name = "gradle-smoke"\ninclude("app", "lib")\n',
        "build.gradle.kts": (
            "plugins { java }\n\n"
            f'file("{GRADLE_JAVA_HOME_FILE}").writeText(System.getProperty("java.home"))\n'
        ),
        "lib/build.gradle.kts": "plugins { java }\n",
        "app/build.gradle.kts": 'plugins { java }\n\ndependencies {\n    implementation(project(":lib"))\n}\n',
        "gradle/wrapper/gradle-wrapper.properties": (
            "distributionBase=GRADLE_USER_HOME\n"
            "distributionPath=wrapper/dists\n"
            f"distributionUrl=https\\://services.gradle.org/distributions/gradle-{version}-bin.zip\n"
            "zipStoreBase=GRADLE_USER_HOME\n"
            "zipStorePath=wrapper/dists\n"
        ),
        **GRADLE_SOURCES,
    }


GRADLE_SOURCES = {
    "lib/src/main/java/demo/lib/Greeter.java": (
        "package demo.lib;\n\n/** Says hello. */\npublic class Greeter {\n"
        "    public String greet(String name) {\n        return \"Hello, \" + name;\n    }\n}\n"
    ),
    GRADLE_MAIN: (
        "package demo.app;\n\nimport demo.lib.Greeter;\n\npublic class Main {\n"
        "    public static void main(String[] args) {\n"
        "        System.out.println(new Greeter().greet(\"Brewery\"));\n    }\n}\n"
    ),
}
# Line 7, on "Greeter" in `new Greeter()`: the hover shows lib's class and its javadoc.
GRADLE_HOVER = f"{GRADLE_MAIN}:7:34:Says hello."

CHECKS = ("basic", "gradle", "gradle-old")
ONLINE_CHECKS = ("gradle", "gradle-old")


def network_reachable() -> bool:
    for host in ("repo.maven.apache.org", "github.com", "1.1.1.1"):
        try:
            with socket.create_connection((host, 443), timeout=5):
                return True
        except OSError:
            continue
    return False


def basic(bundle: Path, windows: bool, scratch: Path, timeout: float) -> int:
    exe = ".exe" if windows else ""
    code = daemon_smoke.main(
        ["--project", str(FIXTURE), "--java", str(bundle / "jre" / "bin" / f"java{exe}"),
         "--jar", str(bundle / "brewery-daemon" / "brewery-daemon.jar"),
         "--expect-build-tool", "maven", "--expect-modules", "3"]
    )
    if code != 0:
        return code
    project = scratch / "plain-java"
    java_file = project / JAVA_FILE
    java_file.parent.mkdir(parents=True)
    java_file.write_text(JAVA_SOURCE, encoding="utf-8")
    launcher = bundle / "jdtls" / ("brewery-jdtls.cmd" if windows else "brewery-jdtls")
    return lsp_smoke.main(
        ["--project", str(project),
         "--proxy", str(bundle / "jdtls" / "tools" / f"java-lsp-proxy{exe}"),
         "--launcher", str(launcher),
         "--lombok", str(bundle / "jdtls" / "tools" / "lombok.jar"),
         "--java-debug", str(bundle / "jdtls" / "tools" / "java-debug.jar"),
         "--open", JAVA_FILE,
         "--unchanged", str(bundle),
         "--cache", str(scratch / "jdtls-cache"),
         "--timeout", str(timeout)]
    )


def write_gradle_project(project: Path, version: str) -> None:
    for name, text in gradle_files(version).items():
        path = project / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8", newline="\n")


def jdtls_args(bundle: Path, windows: bool, launcher_dir: Path, project: Path, cache: Path,
               timeout: float) -> list:
    """lsp_smoke arguments for a Gradle import, started the way Brewery starts jdtls."""
    exe = ".exe" if windows else ""
    return [
        "--project", str(project),
        "--proxy", str(bundle / "jdtls" / "tools" / f"java-lsp-proxy{exe}"),
        "--launcher", str(launcher_dir / ("brewery-jdtls.cmd" if windows else "brewery-jdtls")),
        "--lombok", str(bundle / "jdtls" / "tools" / "lombok.jar"),
        "--init-settings", INIT_SETTINGS,
        "--wait-ready", "--open", GRADLE_MAIN, "--clean-build-files",
        "--cache", str(cache),
        "--timeout", str(timeout),
    ]


def gradle(bundle: Path, windows: bool, scratch: Path, timeout: float) -> int:
    project = scratch / "gradle-smoke"
    write_gradle_project(project, GRADLE_VERSION)
    # A copy of jdtls whose files can be "updated" without touching the bundle. Next to
    # no jre/, the launcher takes JAVA_HOME: the bundled JRE.
    jdtls = scratch / "installed" / "jdtls"
    shutil.copytree(bundle / "jdtls", jdtls, symlinks=True)
    os.environ["JAVA_HOME"] = str(bundle / "jre")
    common = jdtls_args(bundle, windows, jdtls, project, scratch / "gradle-jdtls-cache", timeout)
    lsp_smoke.log(f"gradle: first start, import of the fixture (Gradle {GRADLE_VERSION})")
    code = lsp_smoke.main([*common, "--hover", GRADLE_HOVER])
    if code != 0:
        return code
    # What an update of Brewery does to jdtls's files: same paths, newer times.
    later = time.time() + 86400
    for path in jdtls.rglob("*"):
        if path.is_file() and not path.is_symlink():
            os.utime(path, (later, later))
    lsp_smoke.log("gradle: second start after a jdtls update, then a build file change")
    return lsp_smoke.main([*common, "--change-build-file", "app/build.gradle.kts"])


def find_jdk17() -> Path | None:
    for var in OLD_GRADLE_JDK_VARS:
        home = os.environ.get(var)
        if home and (Path(home) / "release").is_file():
            return Path(home)
    return None


def same_dir(a: str, b: Path) -> bool:
    return os.path.normcase(os.path.realpath(a)) == os.path.normcase(os.path.realpath(str(b)))


def gradle_old(bundle: Path, windows: bool, scratch: Path, timeout: float) -> int:
    jdk = find_jdk17()
    if jdk is None:
        lsp_smoke.log(f"gradle-old: FAILED: no JDK 17 in {', '.join(OLD_GRADLE_JDK_VARS)}")
        return 1
    exe = ".exe" if windows else ""
    project = scratch / "gradle-old"
    write_gradle_project(project, OLD_GRADLE_VERSION)
    # The user's JDK in JAVA_HOME, as a shell or java_home gives it to the daemon and
    # the launcher; jdtls itself still runs on the bundled JRE, so jdtls/ gets a jre/.
    os.environ["JAVA_HOME"] = str(jdk)
    lsp_smoke.log(f"gradle-old: the daemon reads a Gradle {OLD_GRADLE_VERSION} build, JAVA_HOME {jdk}")
    code = daemon_smoke.main(
        ["--project", str(project), "--java", str(bundle / "jre" / "bin" / f"java{exe}"),
         "--jar", str(bundle / "brewery-daemon" / "brewery-daemon.jar"),
         "--expect-build-tool", "gradle", "--expect-modules", "3", "--timeout", str(timeout)]
    )
    if code != 0:
        return code
    root = scratch / "old-gradle-bundle"
    shutil.copytree(bundle / "jre", root / "jre", symlinks=True)
    shutil.copytree(bundle / "jdtls", root / "jdtls", symlinks=True)
    lsp_smoke.log(f"gradle-old: jdtls imports it (Gradle {OLD_GRADLE_VERSION} cannot run on Java 21)")
    (project / GRADLE_JAVA_HOME_FILE).unlink(missing_ok=True)
    code = lsp_smoke.main(
        [*jdtls_args(bundle, windows, root / "jdtls", project, scratch / "old-jdtls-cache", timeout),
         "--hover", GRADLE_HOVER]
    )
    if code != 0:
        return code
    used = (project / GRADLE_JAVA_HOME_FILE).read_text(encoding="utf-8").strip()
    if not same_dir(used, jdk):
        lsp_smoke.log(f"gradle-old: FAILED: jdtls ran Gradle on {used}, not JAVA_HOME {jdk}")
        return 1
    lsp_smoke.log(f"gradle-old: jdtls ran Gradle on JAVA_HOME ({used})")
    return 0


def main(argv: list | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("bundle", help="target/bundle/<os>-<arch>")
    parser.add_argument("--require-offline", action="store_true", help="fail if the network is reachable")
    parser.add_argument(
        "--checks", default="basic",
        help=f"comma-separated, of {', '.join(CHECKS)} (default: basic; "
        f"{' and '.join(ONLINE_CHECKS)} download Gradle)",
    )
    parser.add_argument("--timeout", type=float, default=400)
    args = parser.parse_args(argv)
    checks = [c.strip() for c in args.checks.split(",") if c.strip()]
    unknown = sorted(set(checks) - set(CHECKS))
    if unknown or not checks:
        parser.error(f"--checks: unknown {', '.join(unknown) or '(empty)'}")
    bundle = Path(args.bundle).resolve()
    windows = (bundle / "jre" / "bin" / "java.exe").is_file()

    if args.require_offline:
        online = [c for c in checks if c in ONLINE_CHECKS]
        if online:
            parser.error(f"{', '.join(online)} download Gradle; they cannot run with --require-offline")
        if network_reachable():
            print("smoke-bundle: FAILED: the network is reachable, not an offline run", file=sys.stderr)
            return 1
        lsp_smoke.log("network unreachable: offline run")

    scratch = Path(tempfile.mkdtemp(prefix="smoke-bundle-"))
    try:
        steps = {"basic": basic, "gradle": gradle, "gradle-old": gradle_old}
        for check in CHECKS:
            if check in checks:
                code = steps[check](bundle, windows, scratch, args.timeout)
                if code != 0:
                    return code
        return 0
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
