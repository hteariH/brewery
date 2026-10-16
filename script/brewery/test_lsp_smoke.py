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

    symbol_requests = 0
    if MODE == "noise":
        out.write(b"Picked up JAVA_TOOL_OPTIONS\n")
        out.flush()
    while True:
        msg = read()
        method = msg.get("method")
        if method == "initialize":
            if MODE == "silent":
                continue
            root = msg["params"]["rootUri"]
            # Ask the client something first; a real jdtls does this too.
            send({"jsonrpc": "2.0", "id": "s1", "method": "workspace/configuration",
                  "params": {"items": [{"section": "java"}]}})
            reply = read()
            assert reply.get("id") == "s1" and reply.get("result") == [None], reply
            options = msg["params"]["initializationOptions"]
            version = str(len(options.get("bundles", [])))
            if MODE == "settings":
                version = json.dumps(options["settings"], sort_keys=True)
            send({"jsonrpc": "2.0", "id": msg["id"],
                  "result": {"capabilities": {"documentSymbolProvider": True},
                             "serverInfo": {"name": "fake", "version": version}}})
        elif method == "initialized":
            if MODE == "build-error":
                # How jdtls reports a failed Gradle import.
                send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                      "params": {"uri": root + "/build.gradle.kts", "diagnostics": [
                          {"severity": 1, "message": "Could not run phased build action"}]}})
            if MODE == "gradle-jdk":
                # How jdtls reports a Gradle version that cannot run on its Java.
                send({"jsonrpc": "2.0", "method": "language/eventNotification",
                      "params": {"eventType": 300, "data": {
                          "projectUri": root + "/",
                          "message": "Can't use Java 21.0.12.1 and Gradle 8.4 to import Gradle project p.",
                          "highestJavaVersion": "20", "recommendedGradleVersion": "8.9"}}})
            if MODE == "non-project":
                # What a file outside every imported project gets once it is opened.
                send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                      "params": {"uri": root + "/src/Owner.java", "diagnostics": [
                          {"severity": 2, "code": "16",
                           "message": "Owner.java is a non-project file, only syntax errors are reported"}]}})
            # Hints on build files are fine.
            send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                  "params": {"uri": root + "/settings.gradle.kts", "diagnostics": [
                      {"severity": 3, "message": "The build file has been changed"}]}})
            send({"jsonrpc": "2.0", "method": "language/status",
                  "params": {"type": "ServiceReady", "message": "ServiceReady"}})
        elif method == "textDocument/hover":
            send({"jsonrpc": "2.0", "id": msg["id"],
                  "result": {"contents": {"kind": "markdown", "value": "demo.Owner\n\nOwns things."}}})
        elif method == "workspace/didChangeWatchedFiles":
            send({"jsonrpc": "2.0", "method": "language/actionableNotification",
                  "params": {"severity": 3, "message": "A build file was modified.",
                             "commands": [{"title": "Yes",
                                           "command": "java.projectConfiguration.status"}]}})
        elif method == "java/projectConfigurationUpdate":
            if MODE == "update-fails":
                send({"jsonrpc": "2.0", "method": "window/logMessage", "params": {"type": 1,
                      "message": "An internal error occurred during: \"Updating project configurations\".\n"
                                 "org.gradle.tooling.BuildException: Could not run build action\n"
                                 "Caused by: java.lang.IllegalArgumentException: The specified"
                                 " initialization script 'init.gradle' does not exist."}})
            else:
                send({"jsonrpc": "2.0", "method": "language/status",
                      "params": {"type": "ProjectStatus", "message": "OK"}})
        elif method == "workspace/executeCommand":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": 50505})
        elif method == "textDocument/documentSymbol":
            symbol_requests += 1
            # A real jdtls answers [] until the file's project is imported.
            if MODE == "slow-symbols" and symbol_requests < 3:
                send({"jsonrpc": "2.0", "id": msg["id"], "result": []})
            elif MODE == "no-symbols":
                send({"jsonrpc": "2.0", "id": msg["id"], "result": []})
            else:
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

    def test_symbols_are_asked_again_until_the_file_is_imported(self):
        code, log = self.smoke("slow-symbols", "--open", "src/Owner.java")
        self.assertEqual(code, 0, log)
        self.assertIn("Owner", log)

    def test_no_symbols_at_all_fails(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            code = lsp_smoke.main(
                ["--project", str(self.project), "--timeout", "4", "--open", "src/Owner.java",
                 "--", sys.executable, "-I", str(self.server), "no-symbols"]
            )
        self.assertNotEqual(code, 0)
        self.assertIn("no document symbols", err.getvalue())

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

    def test_hover_text(self):
        code, log = self.smoke("ok", "--hover", "src/Owner.java:1:7:Owns things.")
        self.assertEqual(code, 0, log)
        self.assertIn("hover src/Owner.java:1:7", log)
        code, log = self.smoke("ok", "--timeout", "4", "--hover", "src/Owner.java:1:7:Nope")
        self.assertNotEqual(code, 0)
        self.assertIn("has no 'Nope'", log)

    def test_clean_build_files(self):
        code, log = self.smoke("ok", "--wait-ready", "--clean-build-files")
        self.assertEqual(code, 0, log)
        self.assertIn("no errors on build files", log)
        code, log = self.smoke("build-error", "--wait-ready", "--clean-build-files")
        self.assertNotEqual(code, 0)
        self.assertIn("build.gradle.kts: Could not run phased build action", log)

    def test_clean_build_files_fails_when_gradle_cannot_run_on_the_jdk(self):
        code, log = self.smoke("gradle-jdk", "--wait-ready", "--clean-build-files")
        self.assertNotEqual(code, 0)
        self.assertIn("Can't use Java 21.0.12.1 and Gradle 8.4", log)

    def test_clean_build_files_fails_on_a_non_project_file(self):
        code, log = self.smoke("non-project", "--wait-ready", "--clean-build-files")
        self.assertNotEqual(code, 0)
        self.assertIn("Owner.java is a non-project file", log)

    def test_init_settings_are_sent(self):
        settings = '{"java": {"import": {"gradle": {"java": {"home": "x"}}}}}'
        code, log = self.smoke("settings", "--init-settings", settings)
        self.assertEqual(code, 0, log)
        self.assertIn(settings, log)
        code, log = self.smoke("settings")
        self.assertEqual(code, 0, log)
        self.assertIn('{"java": {}}', log)

    def test_change_build_file_updates_the_project(self):
        (self.project / "build.gradle.kts").write_text("plugins { java }\n", encoding="utf-8")
        code, log = self.smoke("ok", "--wait-ready", "--change-build-file", "build.gradle.kts")
        self.assertEqual(code, 0, log)
        self.assertIn("project updated (OK)", log)
        self.assertEqual(
            (self.project / "build.gradle.kts").read_text(encoding="utf-8"), "plugins { java }\n\n"
        )
        code, log = self.smoke("update-fails", "--wait-ready", "--change-build-file", "build.gradle.kts")
        self.assertNotEqual(code, 0)
        self.assertIn("initialization script 'init.gradle' does not exist", log)

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
