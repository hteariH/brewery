"""Tests for lsp_smoke.py against a fake language server (no Java needed).

Run from the repo root:
    python -m unittest discover script/brewery
"""

import contextlib
import io
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

import fetch_bundle_deps as fbd
import lsp_smoke

FAKE_SERVER = textwrap.dedent(
    r'''
    import json, sys

    MODE = sys.argv[1]
    out = sys.stdout.buffer
    inp = sys.stdin.buffer

    def send(msg):
        body = json.dumps(msg).encode()
        out.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        out.flush()

    def read():
        length = None
        while True:
            line = inp.readline()
            if not line:
                sys.exit(0)
            line = line.strip()
            if not line:
                break
            if line.lower().startswith(b"content-length:"):
                length = int(line.split(b":")[1])
        return json.loads(inp.read(length))

    if MODE == "noise":
        out.write(b"Picked up JAVA_TOOL_OPTIONS\n")
        out.flush()
    while True:
        msg = read()
        method = msg.get("method")
        if method == "initialize":
            if MODE == "silent":
                continue
            # Ask the client something first; a real jdtls does this too.
            send({"jsonrpc": "2.0", "id": "s1", "method": "workspace/configuration",
                  "params": {"items": [{"section": "java"}]}})
            reply = read()
            assert reply.get("id") == "s1" and reply.get("result") == [None], reply
            bundles = msg["params"]["initializationOptions"].get("bundles", [])
            send({"jsonrpc": "2.0", "id": msg["id"],
                  "result": {"capabilities": {"documentSymbolProvider": True},
                             "serverInfo": {"name": "fake", "version": str(len(bundles))}}})
        elif method == "initialized":
            send({"jsonrpc": "2.0", "method": "language/status",
                  "params": {"type": "ServiceReady", "message": "ServiceReady"}})
        elif method == "workspace/executeCommand":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": 50505})
        elif method == "textDocument/documentSymbol":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": [{"name": "Owner", "kind": 5}]})
        elif method == "shutdown":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
        elif method == "exit":
            sys.exit(0)
    '''
)


class LspSmokeTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="lsp-smoke-test-"))
        self.addCleanup(fbd.rmtree, self.tmp)
        self.server = self.tmp / "fake_server.py"
        self.server.write_text(FAKE_SERVER, encoding="utf-8")
        self.project = self.tmp / "project"
        (self.project / "src").mkdir(parents=True)
        (self.project / "src" / "Owner.java").write_text("class Owner {}\n", encoding="utf-8")

    def smoke(self, mode, *extra):
        err = io.StringIO()
        out = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(out):
            code = lsp_smoke.main(
                ["--project", str(self.project), "--timeout", "20", *extra,
                 "--", sys.executable, "-I", str(self.server), mode]
            )
        return code, out.getvalue() + err.getvalue()

    def test_initialize_result_is_success(self):
        code, log = self.smoke("ok")
        self.assertEqual(code, 0, log)
        self.assertIn("initialize", log)
        self.assertIn("documentSymbolProvider", log)

    def test_ready_debug_session_and_symbols(self):
        jar = self.tmp / "java-debug.jar"
        jar.write_bytes(b"jar")
        code, log = self.smoke(
            "ok", "--wait-ready", "--java-debug", str(jar), "--open", "src/Owner.java"
        )
        self.assertEqual(code, 0, log)
        self.assertIn("ServiceReady", log)
        self.assertIn("50505", log)
        self.assertIn("Owner", log)

    def test_copy_leaves_the_project_alone(self):
        before = lsp_smoke.snapshot(self.project)
        code, log = self.smoke("ok", "--copy", "--open", "src/Owner.java")
        self.assertEqual(code, 0, log)
        self.assertIn("Owner", log)
        self.assertEqual(lsp_smoke.snapshot(self.project), before)

    def test_noise_on_stdout_fails(self):
        code, log = self.smoke("noise")
        self.assertNotEqual(code, 0)
        self.assertIn("stdout", log)

    def test_no_answer_times_out(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            code = lsp_smoke.main(
                ["--project", str(self.project), "--timeout", "2",
                 "--", sys.executable, "-I", str(self.server), "silent"]
            )
        self.assertNotEqual(code, 0)
        self.assertIn("timed out", err.getvalue())

    def test_unchanged_dir_check(self):
        watched = self.tmp / "bundle"
        watched.mkdir()
        (watched / "a.txt").write_text("a", encoding="utf-8")
        code, log = self.smoke("ok", "--unchanged", str(watched))
        self.assertEqual(code, 0, log)
        self.assertEqual(lsp_smoke.snapshot(watched), lsp_smoke.snapshot(watched))
        before = lsp_smoke.snapshot(watched)
        (watched / "b.txt").write_text("b", encoding="utf-8")
        self.assertEqual(lsp_smoke.diff_snapshots(before, lsp_smoke.snapshot(watched)), ["added b.txt"])


if __name__ == "__main__":
    unittest.main()
