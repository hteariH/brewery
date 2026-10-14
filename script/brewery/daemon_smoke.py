#!/usr/bin/env python3
"""Start brewery-daemon and read a project's model, to smoke-test the offline bundle.

    python script/brewery/daemon_smoke.py --java <bundle>/jre/bin/java[.exe]
        --jar <bundle>/brewery-daemon/brewery-daemon.jar --project <dir>
        [--expect-build-tool maven] [--expect-modules 3] [--timeout 120]
    python script/brewery/daemon_smoke.py --project <dir> -- <daemon command...>

Speaks the daemon's JSON-RPC (LSP framing): initialize (protocol version must
match), project/open, project/model, shutdown. Exit 0 on success. Python 3.11+
standard library only.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import lsp_smoke

# Must match PROTOCOL_VERSION in brewery-daemon Protocol.kt and brewery_daemon_client.
PROTOCOL_VERSION = 1


def daemon_command(java: str, jar: str) -> list:
    return [str(java), "-jar", str(jar)]


def run(args) -> None:
    project = Path(args.project).resolve()
    if args.command:
        cmd = args.command
    elif args.java and args.jar:
        cmd = daemon_command(Path(args.java).resolve(), Path(args.jar).resolve())
    else:
        raise lsp_smoke.SmokeError("give --java and --jar, or a daemon command after --")
    lsp_smoke.log(f"starting {' '.join(cmd)}")
    started = time.monotonic()
    deadline = started + args.timeout
    # stderr goes to a file: a full pipe would block a chatty daemon.
    with tempfile.TemporaryFile() as stderr:
        try:
            proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr)
        except OSError as err:
            raise lsp_smoke.SmokeError(f"cannot start {cmd[0]}: {err}") from err
        failed = True
        try:
            check(args, proc, project, started, deadline)
            failed = False
        finally:
            try:
                proc.stdin.close()
            except OSError:
                pass
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=10)
            if failed:
                stderr.seek(0)
                tail = stderr.read()[-4000:].decode("utf-8", "replace")
                if tail.strip():
                    lsp_smoke.log("daemon stderr (tail):\n" + tail)
    lsp_smoke.log(f"done in {time.monotonic() - started:.1f}s")


def check(args, proc: subprocess.Popen, project: Path, started: float, deadline: float) -> None:
    client = lsp_smoke.Client(proc, project.as_uri(), [])
    init = client.request("initialize", {"protocolVersion": PROTOCOL_VERSION}, deadline)
    if not isinstance(init, dict) or init.get("protocolVersion") != PROTOCOL_VERSION:
        raise lsp_smoke.SmokeError(f"daemon speaks protocol {init!r}, expected {PROTOCOL_VERSION}")
    lsp_smoke.log(f"initialize ok after {time.monotonic() - started:.1f}s: daemon {init.get('daemonVersion')}")
    client.request("project/open", {"rootDir": str(project)}, deadline)
    model = client.request("project/model", {}, deadline) or {}
    tool = model.get("buildTool")
    modules = [m.get("name") for m in model.get("modules", [])]
    lsp_smoke.log(f"project/model: {tool}, modules {', '.join(map(str, modules))}")
    if args.expect_build_tool and tool != args.expect_build_tool:
        raise lsp_smoke.SmokeError(f"build tool {tool!r}, expected {args.expect_build_tool!r}")
    if args.expect_modules is not None and len(modules) != args.expect_modules:
        raise lsp_smoke.SmokeError(f"{len(modules)} modules, expected {args.expect_modules}")
    client.request("shutdown", None, deadline)


def main(argv: list | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    command = []
    if "--" in argv:
        split = argv.index("--")
        argv, command = argv[:split], argv[split + 1 :]
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--project", required=True, help="project directory to open")
    parser.add_argument("--java", help="java executable (the bundled jre/bin/java)")
    parser.add_argument("--jar", help="brewery-daemon.jar")
    parser.add_argument("--expect-build-tool", help="maven, gradle, ...")
    parser.add_argument("--expect-modules", type=int, help="number of modules in the model")
    parser.add_argument("--timeout", type=float, default=120, help="seconds for the whole run")
    args = parser.parse_args(argv)
    args.command = command
    try:
        run(args)
    except lsp_smoke.SmokeError as err:
        print(f"daemon-smoke: FAILED: {err}", file=sys.stderr, flush=True)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
