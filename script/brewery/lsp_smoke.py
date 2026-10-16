#!/usr/bin/env python3
"""Start a language server and check that it answers LSP `initialize`.

Used to smoke-test the offline bundle: jdtls started through the Brewery
launcher (optionally behind java-lsp-proxy, exactly as the Java extension
starts it) against a fixture project. Python 3.11+ standard library only.

    python script/brewery/lsp_smoke.py --project <dir> --launcher <bundle>/jdtls/brewery-jdtls[.cmd]
        [--proxy <bundle>/jdtls/tools/java-lsp-proxy[.exe]] [--lombok <bundle>/jdtls/tools/lombok.jar]
        [--java-debug <bundle>/jdtls/tools/java-debug.jar] [--wait-ready] [--open src/Main.java]
        [--hover src/Main.java:3:5:Text] [--change-build-file build.gradle] [--clean-build-files]
        [--unchanged <bundle>] [--cache <dir>] [--copy] [--timeout 300]
    python script/brewery/lsp_smoke.py --project <dir> -- <server command...>

Checks, in order: the server writes only LSP to stdout; `initialize` returns
capabilities; with --wait-ready, jdtls reports ServiceReady; with --java-debug,
the java-debug bundle loads and `vscode.java.startDebugSession` returns a port;
with --open, `textDocument/documentSymbol` lists symbols; with --hover, the hover
at a position has the expected text; with --change-build-file, the file is edited
(a newline appended), jdtls is told, its offer to synchronize is accepted and the
project update must succeed; with --clean-build-files, no build file
(build.gradle[.kts], settings.gradle[.kts], pom.xml) has error diagnostics, so the
project imported; with --unchanged, the directory (the bundle) is byte-for-byte
untouched afterwards. Exit 0 on success.
"""

from __future__ import annotations

import argparse
import collections
import json
import os
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path


class SmokeError(Exception):
    pass


def log(msg: str) -> None:
    # Server messages can hold characters the console's code page lacks (Windows).
    line = f"lsp-smoke: {msg}"
    encoding = getattr(sys.stdout, "encoding", None) or "utf-8"
    print(line.encode(encoding, "replace").decode(encoding), flush=True)


# --- framing ----------------------------------------------------------------


def encode(msg: dict) -> bytes:
    body = json.dumps(msg).encode("utf-8")
    return b"Content-Length: %d\r\n\r\n" % len(body) + body


def reader(stream, messages: queue.Queue) -> None:
    """Parse LSP frames from stream into messages; anything else is an error."""
    try:
        while True:
            length = None
            while True:
                line = stream.readline()
                if not line:
                    messages.put(("eof", None))
                    return
                stripped = line.strip()
                if not stripped:
                    if length is None:
                        continue
                    break
                name, sep, value = stripped.partition(b":")
                if not sep or name.lower() not in (b"content-length", b"content-type"):
                    messages.put(("noise", line))
                    return
                if name.lower() == b"content-length":
                    length = int(value.strip())
            body = stream.read(length)
            messages.put(("message", json.loads(body)))
    except (OSError, ValueError) as err:
        messages.put(("error", err))


class Client:
    def __init__(self, proc: subprocess.Popen, root_uri: str, folders: list):
        self.proc = proc
        self.root_uri = root_uri
        self.folders = folders
        self.messages: queue.Queue = queue.Queue()
        self.next_id = 1
        self.statuses: list = []
        # The latest textDocument/publishDiagnostics per document URI.
        self.diagnostics: dict = {}
        # The server's last window/logMessage lines, shown when a check fails.
        self.server_log: collections.deque = collections.deque(maxlen=40)
        threading.Thread(target=reader, args=(proc.stdout, self.messages), daemon=True).start()

    def send(self, msg: dict) -> None:
        msg = {"jsonrpc": "2.0", **msg}
        try:
            self.proc.stdin.write(encode(msg))
            self.proc.stdin.flush()
        except OSError as err:
            raise SmokeError(f"cannot write to the server: {err}") from err

    def request(self, method: str, params, deadline: float):
        rid = self.next_id
        self.next_id += 1
        self.send({"id": rid, "method": method, "params": params})
        return self.wait(lambda m: m.get("id") == rid and "method" not in m, deadline, method)

    def notify(self, method: str, params) -> None:
        self.send({"method": method, "params": params})

    def answer(self, msg: dict) -> None:
        """Reply to a server-to-client request so the server never blocks on us."""
        method = msg["method"]
        if method == "workspace/configuration":
            result = [None for _ in msg.get("params", {}).get("items", [])]
        elif method == "workspace/workspaceFolders":
            result = self.folders
        else:
            result = None
        self.send({"id": msg["id"], "result": result})

    def wait(self, match, deadline: float, what: str):
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                raise SmokeError(f"timed out waiting for {what}")
            try:
                kind, value = self.messages.get(timeout=min(left, 1.0))
            except queue.Empty:
                if self.proc.poll() is not None:
                    raise SmokeError(f"server exited with {self.proc.returncode} before {what}")
                continue
            if kind == "noise":
                raise SmokeError(f"non-LSP output on the server's stdout: {value[:200]!r}")
            if kind == "eof":
                raise SmokeError(f"server closed stdout before {what}")
            if kind == "error":
                raise SmokeError(f"bad LSP frame: {value}")
            msg = value
            if "method" in msg and "id" in msg:
                self.answer(msg)
                continue
            if msg.get("method") == "textDocument/publishDiagnostics":
                params = msg.get("params", {})
                self.diagnostics[params.get("uri", "")] = params.get("diagnostics", [])
            if msg.get("method") == "window/logMessage":
                self.server_log.append(str(msg.get("params", {}).get("message", ""))[:500])
            if msg.get("method") == "language/status":
                status = msg.get("params", {})
                self.statuses.append(status.get("type"))
                if status.get("type") in ("ServiceReady", "Error"):
                    log(f"status {status.get('type')}: {status.get('message')}")
            if match(msg):
                if "error" in msg:
                    raise SmokeError(f"{what} failed: {msg['error']}")
                return msg.get("result", msg.get("params"))


# --- bundle checks ------------------------------------------------------------


def snapshot(root: Path) -> dict:
    """{relative path: (size, mtime_ns)} of every file below root."""
    root = Path(root)
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        for name in filenames:
            p = Path(dirpath) / name
            st = p.stat()
            out[p.relative_to(root).as_posix()] = (st.st_size, st.st_mtime_ns)
    return out


def diff_snapshots(before: dict, after: dict) -> list:
    changes = [f"added {p}" for p in sorted(after.keys() - before.keys())]
    changes += [f"removed {p}" for p in sorted(before.keys() - after.keys())]
    changes += [f"changed {p}" for p in sorted(before.keys() & after.keys()) if before[p] != after[p]]
    return changes


# --- process ------------------------------------------------------------------


def launcher_command(args, workdir: Path) -> list:
    launcher = str(Path(args.launcher).resolve())
    extra = []
    if args.lombok:
        extra.append(f"--jvm-arg=-javaagent:{Path(args.lombok).resolve()}")
    if args.data:
        extra += ["-data", str(Path(args.data).resolve())]
    if args.proxy:
        # What jdtls_server.rs hands Zed: <proxy> <extension workdir> <launcher> [args].
        return [str(Path(args.proxy).resolve()), str(workdir), launcher, *extra]
    if os.name == "nt" and launcher.lower().endswith((".cmd", ".bat")):
        # java-lsp-proxy runs batch launchers as `cmd /C <bin> <args>`.
        return ["cmd", "/C", launcher, *extra]
    return [launcher, *extra]


def stop(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"], capture_output=True)
    else:
        proc.kill()
    try:
        proc.wait(timeout=30)
    except subprocess.TimeoutExpired:
        pass


def run(args) -> None:
    project = Path(args.project).resolve()
    if not project.is_dir():
        raise SmokeError(f"project {project} is not a directory")
    scratch = Path(tempfile.mkdtemp(prefix="lsp-smoke-"))
    try:
        if args.copy:
            # Work on a copy so the server cannot touch the checked-in fixture.
            copy = scratch / "project" / project.name
            shutil.copytree(project, copy, ignore=shutil.ignore_patterns(".gradle", "build", "target"))
            project = copy
        if args.command:
            cmd = args.command
        elif args.launcher:
            cmd = launcher_command(args, scratch / "extension-workdir")
            (scratch / "extension-workdir").mkdir()
        else:
            raise SmokeError("give --launcher or a server command after --")
        env = dict(os.environ)
        env["BREWERY_JDTLS_CACHE_DIR"] = str(Path(args.cache).resolve() if args.cache else scratch / "cache")
        before = snapshot(Path(args.unchanged)) if args.unchanged else None

        log(f"starting {' '.join(cmd)} in {project}")
        started = time.monotonic()
        stderr_path = scratch / "server-stderr.log"
        with open(stderr_path, "wb") as stderr:
            proc = subprocess.Popen(
                cmd, cwd=project, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=stderr,
            )
            failed = True
            try:
                check(args, proc, project, started)
                failed = False
            finally:
                stop(proc)
                if args.verbose or failed:
                    stderr.flush()
                    tail = stderr_path.read_bytes()[-4000:].decode("utf-8", "replace")
                    if tail.strip():
                        log(f"server stderr (tail):\n{tail}")
                if failed:
                    client = getattr(proc, "brewery_client", None)
                    if client is not None and client.server_log:
                        log("server log messages:\n" + "\n".join(client.server_log))
                    # jdtls's own log, in its workspace under the cache dir.
                    cache = Path(env["BREWERY_JDTLS_CACHE_DIR"])
                    for jdtls_log in cache.glob("workspace/*/.metadata/.log"):
                        text = jdtls_log.read_bytes()[-6000:].decode("utf-8", "replace")
                        log(f"{jdtls_log} (tail):\n{text}")
                if proc.stdin:
                    try:
                        proc.stdin.close()
                    except OSError:
                        pass

        if before is not None:
            changes = diff_snapshots(before, snapshot(Path(args.unchanged)))
            if changes:
                raise SmokeError(f"{args.unchanged} was modified: {', '.join(changes[:10])}")
            log(f"{args.unchanged} unchanged ({len(before)} files)")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def check(args, proc: subprocess.Popen, project: Path, started: float) -> None:
    root_uri = project.as_uri()
    folders = [{"uri": root_uri, "name": project.name}]
    client = Client(proc, root_uri, folders)
    proc.brewery_client = client  # for the failure report in run()
    deadline = started + args.timeout
    init_options = {
        "workspaceFolders": [root_uri],
        "extendedClientCapabilities": {
            "classFileContentsSupport": True,
            "resolveAdditionalTextEditsSupport": True,
        },
        "settings": {"java": {}},
    }
    if args.java_debug:
        init_options["bundles"] = [str(Path(args.java_debug).resolve())]
    result = client.request(
        "initialize",
        {
            "processId": os.getpid(),
            "clientInfo": {"name": "brewery-lsp-smoke"},
            "rootUri": root_uri,
            "rootPath": str(project),
            "workspaceFolders": folders,
            "capabilities": {
                "workspace": {"configuration": True, "workspaceFolders": True},
                "textDocument": {"documentSymbol": {"hierarchicalDocumentSymbolSupport": True}},
                "window": {"workDoneProgress": True},
            },
            "initializationOptions": init_options,
        },
        deadline,
    )
    if not isinstance(result, dict) or not isinstance(result.get("capabilities"), dict):
        raise SmokeError(f"initialize returned no capabilities: {result!r}")
    elapsed = time.monotonic() - started
    info = result.get("serverInfo") or {}
    log(
        f"initialize ok after {elapsed:.1f}s: {info.get('name', '?')} {info.get('version', '')}; "
        f"capabilities: {', '.join(sorted(result['capabilities']))}"
    )
    client.notify("initialized", {})

    if args.wait_ready or args.java_debug:
        if "ServiceReady" not in client.statuses:
            client.wait(
                lambda m: m.get("method") == "language/status"
                and m.get("params", {}).get("type") == "ServiceReady",
                deadline,
                "ServiceReady",
            )
        log(f"ServiceReady after {time.monotonic() - started:.1f}s")
    if args.java_debug:
        port = client.request(
            "workspace/executeCommand",
            {"command": "vscode.java.startDebugSession", "arguments": []},
            deadline,
        )
        if not isinstance(port, int) or port <= 0:
            raise SmokeError(f"vscode.java.startDebugSession returned {port!r}, not a port")
        log(f"java-debug loaded: startDebugSession -> port {port}")
    if args.open:
        path = (project / args.open).resolve()
        uri = path.as_uri()
        client.notify(
            "textDocument/didOpen",
            {"textDocument": {"uri": uri, "languageId": "java", "version": 1,
                              "text": path.read_text(encoding="utf-8")}},
        )
        # jdtls answers [] until the file's project is imported, which can take longer
        # than ServiceReady (offline Maven import on a fast runner), so ask again.
        symbols_until = min(deadline, time.monotonic() + 120)
        while True:
            symbols = client.request(
                "textDocument/documentSymbol", {"textDocument": {"uri": uri}}, deadline
            )
            names = [s.get("name") for s in symbols or []]
            if names:
                break
            if time.monotonic() + 1 > symbols_until:
                raise SmokeError(f"no document symbols for {args.open}")
            time.sleep(0.5)
        log(f"documentSymbol {args.open}: {', '.join(names)}")
    if args.hover:
        check_hover(client, project, args.hover, deadline)
    if args.change_build_file:
        change_build_file(client, project, args.change_build_file, deadline)
    if args.clean_build_files:
        check_build_files(client, deadline)

    client.request("shutdown", None, deadline)
    client.notify("exit", None)
    # jdtls finishes once its input closes, as it does when the editor goes away.
    try:
        proc.stdin.close()
    except OSError:
        pass
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        # jdtls only forces its exit a minute after `exit`; editors kill it too.
        log("server still running 10s after `exit` (jdtls forces its exit after 1 min); stopping it")
    log(f"done in {time.monotonic() - started:.1f}s")


BUILD_FILES = ("build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts", "pom.xml")


def check_hover(client: Client, project: Path, spec: str, deadline: float) -> None:
    """spec is FILE:LINE:COLUMN:TEXT (1-based line and column); the hover must contain TEXT."""
    try:
        name, line, column, expected = spec.split(":", 3)
        line, column = int(line), int(column)
    except ValueError as err:
        raise SmokeError(f"--hover wants FILE:LINE:COLUMN:TEXT, not {spec!r}") from err
    path = (project / name).resolve()
    uri = path.as_uri()
    client.notify(
        "textDocument/didOpen",
        {"textDocument": {"uri": uri, "languageId": "java", "version": 1,
                          "text": path.read_text(encoding="utf-8")}},
    )
    # Like documentSymbol, hover is empty until the project is imported.
    hover_until = min(deadline, time.monotonic() + 120)
    text = ""
    while True:
        hover = client.request(
            "textDocument/hover",
            {"textDocument": {"uri": uri}, "position": {"line": line - 1, "character": column - 1}},
            deadline,
        )
        text = json.dumps((hover or {}).get("contents", ""))
        if expected in text:
            break
        if time.monotonic() + 1 > hover_until:
            raise SmokeError(f"hover at {name}:{line}:{column} has no {expected!r}: {text[:300]}")
        time.sleep(0.5)
    log(f"hover {name}:{line}:{column}: {text[:120]}")


def change_build_file(client: Client, project: Path, name: str, deadline: float) -> None:
    """Edit a build file the way a user saves it and accept jdtls's offer to synchronize."""
    path = (project / name).resolve()
    with open(path, "a", encoding="utf-8", newline="\n") as f:
        f.write("\n")
    client.notify(
        "workspace/didChangeWatchedFiles", {"changes": [{"uri": path.as_uri(), "type": 2}]}
    )
    # "A build file was modified. Do you want to synchronize ...?" -- answer Yes.
    client.wait(
        lambda m: m.get("method") == "language/actionableNotification"
        and any(c.get("command") == "java.projectConfiguration.status"
                for c in m.get("params", {}).get("commands", [])),
        min(deadline, time.monotonic() + 60),
        f"the offer to synchronize after changing {name}",
    )
    # The Yes button is a client command; it sends jdtls this notification.
    client.notify("java/projectConfigurationUpdate", {"uri": path.as_uri()})
    # A good update ends with a ProjectStatus; a failed one only logs an error.
    done = client.wait(
        lambda m: (m.get("method") == "language/status"
                   and m.get("params", {}).get("type") == "ProjectStatus")
        or (m.get("method") == "window/logMessage" and m.get("params", {}).get("type") == 1
            and "Updating project configurations" in m.get("params", {}).get("message", "")),
        min(deadline, time.monotonic() + 300),
        f"the project update after changing {name}",
    )
    if done.get("type") == 1:
        lines = [line.strip() for line in done.get("message", "").splitlines()]
        causes = [line for line in lines if line.startswith("Caused by")]
        summary = "\n  ".join(lines[1:2] + causes[-1:])
        raise SmokeError(f"updating the project after changing {name} failed:\n  {summary}")
    log(f"changed {name}: project updated ({done.get('message')})")


def check_build_files(client: Client, deadline: float, settle: float = 5.0) -> None:
    """Fail on error diagnostics on build files, e.g. a failed Gradle import."""
    # Import problems arrive as publishDiagnostics on the build file; give late ones a moment.
    try:
        client.wait(lambda m: False, min(deadline, time.monotonic() + settle), "late diagnostics")
    except SmokeError as err:
        if not str(err).startswith("timed out"):
            raise
    errors = [
        f"{uri.rsplit('/', 1)[-1]}: {d.get('message', '').strip()[:400]}"
        for uri, diagnostics in client.diagnostics.items()
        if uri.rsplit("/", 1)[-1] in BUILD_FILES
        for d in diagnostics
        if d.get("severity", 1) == 1
    ]
    if errors:
        raise SmokeError("build file errors:\n  " + "\n  ".join(errors))
    log("no errors on build files")


def main(argv: list | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    command = []
    if "--" in argv:
        split = argv.index("--")
        argv, command = argv[:split], argv[split + 1 :]
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--project", required=True, help="project directory (the server's cwd)")
    parser.add_argument("--copy", action="store_true", help="run on a temp copy of --project")
    parser.add_argument("--launcher", help="brewery-jdtls launcher")
    parser.add_argument("--proxy", help="java-lsp-proxy to start the launcher through")
    parser.add_argument("--lombok", help="pass --jvm-arg=-javaagent:<jar>, like the extension")
    parser.add_argument("--data", help="pass -data <dir>, like the extension's data_directory")
    parser.add_argument("--java-debug", help="java-debug plugin jar to load as a jdtls bundle")
    parser.add_argument("--wait-ready", action="store_true", help="wait for ServiceReady")
    parser.add_argument("--open", help="project-relative Java file to ask documentSymbol for")
    parser.add_argument("--hover", help="FILE:LINE:COLUMN:TEXT; the hover there must contain TEXT")
    parser.add_argument(
        "--change-build-file",
        help="project-relative build file to edit (and report as changed) once jdtls is ready",
    )
    parser.add_argument(
        "--clean-build-files", action="store_true",
        help="fail on error diagnostics on build files (a failed Gradle or Maven import)",
    )
    parser.add_argument("--unchanged", help="directory that must not be modified (the bundle)")
    parser.add_argument("--cache", help="BREWERY_JDTLS_CACHE_DIR (default: a temp dir)")
    parser.add_argument("--timeout", type=float, default=300, help="seconds for the whole run")
    parser.add_argument("--verbose", action="store_true", help="print the server's stderr")
    args = parser.parse_args(argv)
    args.command = command
    try:
        run(args)
    except SmokeError as err:
        print(f"lsp-smoke: FAILED: {err}", file=sys.stderr, flush=True)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
