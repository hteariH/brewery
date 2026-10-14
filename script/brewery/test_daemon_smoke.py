"""Tests for daemon_smoke.py against a fake daemon (no Java needed).

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

import daemon_smoke
import fetch_bundle_deps as fbd

FAKE_DAEMON = textwrap.dedent(
    r'''
    import json, sys

    MODE = sys.argv[1]
    out, inp = sys.stdout.buffer, sys.stdin.buffer

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

    while True:
        msg = read()
        method = msg.get("method")
        if method == "initialize":
            assert msg["params"] == {"protocolVersion": 1}, msg
            version = 2 if MODE == "wrong-protocol" else 1
            send({"jsonrpc": "2.0", "id": msg["id"],
                  "result": {"protocolVersion": version, "daemonVersion": "9.9.9"}})
        elif method == "project/open":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
        elif method == "project/model":
            send({"jsonrpc": "2.0", "id": msg["id"],
                  "result": {"buildTool": "maven",
                             "modules": [{"name": "a"}, {"name": "b"}, {"name": "c"}]}})
        elif method == "shutdown":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    '''
)


class DaemonSmokeTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="daemon-smoke-test-"))
        self.addCleanup(fbd.rmtree, self.tmp)
        self.fake = self.tmp / "fake_daemon.py"
        self.fake.write_text(FAKE_DAEMON, encoding="utf-8")

    def smoke(self, mode, *extra):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = daemon_smoke.main(
                ["--project", str(self.tmp), "--timeout", "20", *extra,
                 "--", sys.executable, "-I", str(self.fake), mode]
            )
        return code, out.getvalue()

    def test_reads_the_model(self):
        code, log = self.smoke("ok", "--expect-build-tool", "maven", "--expect-modules", "3")
        self.assertEqual(code, 0, log)
        self.assertIn("9.9.9", log)
        self.assertIn("maven", log)

    def test_wrong_module_count_fails(self):
        code, log = self.smoke("ok", "--expect-modules", "2")
        self.assertNotEqual(code, 0)
        self.assertIn("modules", log)

    def test_wrong_build_tool_fails(self):
        code, log = self.smoke("ok", "--expect-build-tool", "gradle")
        self.assertNotEqual(code, 0)

    def test_protocol_mismatch_fails(self):
        code, log = self.smoke("wrong-protocol")
        self.assertNotEqual(code, 0)
        self.assertIn("protocol", log)

    def test_missing_java_is_a_clean_failure(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = daemon_smoke.main(
                ["--project", str(self.tmp), "--java", str(self.tmp / "no-java"),
                 "--jar", str(self.tmp / "no.jar")]
            )
        self.assertEqual(code, 1)
        self.assertIn("cannot start", out.getvalue())

    def test_java_and_jar_build_the_command(self):
        self.assertEqual(
            daemon_smoke.daemon_command("/b/jre/bin/java", "/b/brewery-daemon/brewery-daemon.jar"),
            ["/b/jre/bin/java", "-jar", "/b/brewery-daemon/brewery-daemon.jar"],
        )


if __name__ == "__main__":
    unittest.main()
