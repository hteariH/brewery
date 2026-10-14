#!/usr/bin/env python3
"""Smoke-test a built offline bundle: the daemon and jdtls on the bundled pieces.

    python script/brewery/smoke_bundle.py target/bundle/<os>-<arch> [--require-offline]

1. daemon_smoke: <bundle>/jre/bin/java -jar <bundle>/brewery-daemon/brewery-daemon.jar
   reads the maven-multi fixture (maven, 3 modules).
2. lsp_smoke: jdtls started the way the Java extension starts it
   (java-lsp-proxy -> jdtls/brewery-jdtls[.cmd] --jvm-arg=-javaagent:lombok.jar)
   on a copy of that fixture plus one Java file: initialize, ServiceReady,
   java-debug's startDebugSession, documentSymbol; the bundle must stay unchanged.

--require-offline first checks that the network is unreachable, so a run inside
`unshare -n` / `docker run --network none` proves the bundle needs no downloads.
"""

from __future__ import annotations

import argparse
import shutil
import socket
import sys
import tempfile
from pathlib import Path

import daemon_smoke
import lsp_smoke

REPO = Path(__file__).resolve().parent.parent.parent
FIXTURE = REPO / "brewery-daemon" / "src" / "test" / "resources" / "projects" / "maven-multi"
JAVA_FILE = "core/src/main/java/demo/Owner.java"
JAVA_SOURCE = """package demo;

public class Owner {
    private String name;

    public String getName() {
        return name;
    }
}
"""


def network_reachable() -> bool:
    for host in ("repo.maven.apache.org", "github.com", "1.1.1.1"):
        try:
            with socket.create_connection((host, 443), timeout=5):
                return True
        except OSError:
            continue
    return False


def main(argv: list | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("bundle", help="target/bundle/<os>-<arch>")
    parser.add_argument("--require-offline", action="store_true", help="fail if the network is reachable")
    parser.add_argument("--timeout", type=float, default=400)
    args = parser.parse_args(argv)
    bundle = Path(args.bundle).resolve()
    windows = (bundle / "jre" / "bin" / "java.exe").is_file()
    exe = ".exe" if windows else ""

    if args.require_offline:
        if network_reachable():
            print("smoke-bundle: FAILED: the network is reachable, not an offline run", file=sys.stderr)
            return 1
        lsp_smoke.log("network unreachable: offline run")

    code = daemon_smoke.main(
        ["--project", str(FIXTURE), "--java", str(bundle / "jre" / "bin" / f"java{exe}"),
         "--jar", str(bundle / "brewery-daemon" / "brewery-daemon.jar"),
         "--expect-build-tool", "maven", "--expect-modules", "3"]
    )
    if code != 0:
        return code

    scratch = Path(tempfile.mkdtemp(prefix="smoke-bundle-"))
    try:
        project = scratch / FIXTURE.name
        shutil.copytree(FIXTURE, project)
        java_file = project / JAVA_FILE
        java_file.parent.mkdir(parents=True)
        java_file.write_text(JAVA_SOURCE, encoding="utf-8")
        launcher = bundle / "jdtls" / ("brewery-jdtls.cmd" if windows else "brewery-jdtls")
        return lsp_smoke.main(
            ["--project", str(project),
             "--proxy", str(bundle / f"java-lsp-proxy{exe}"),
             "--launcher", str(launcher),
             "--lombok", str(bundle / "lombok.jar"),
             "--java-debug", str(bundle / "java-debug.jar"),
             "--open", JAVA_FILE,
             "--unchanged", str(bundle),
             "--cache", str(scratch / "jdtls-cache"),
             "--timeout", str(args.timeout)]
        )
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
