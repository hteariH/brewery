"""Invariants of .github/workflows/brewery_ci.yml that a CI run alone wouldn't catch.

Run from the repo root (needs PyYAML; skipped without it):
    python -m unittest discover script/brewery
"""

import re
import unittest
from pathlib import Path

try:
    import yaml
except ImportError:  # setup-python's interpreter on CI has no PyYAML
    yaml = None

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "brewery_ci.yml"
ACTIONS = ROOT / ".github" / "actions"


def load(path):
    return yaml.safe_load(path.read_text(encoding="utf-8"))


def steps():
    """(job id, step) for every step of the workflow and of its local composite actions."""
    for job_id, job in load(WORKFLOW)["jobs"].items():
        for step in job.get("steps", []):
            yield job_id, step
    for action in sorted(ACTIONS.glob("*/action.yml")):
        for step in load(action)["runs"].get("steps", []):
            yield f"action {action.parent.name}", step


@unittest.skipIf(yaml is None, "PyYAML is not installed")
class CacheWriteTests(unittest.TestCase):
    """Caches are written from main (and tags) only: SAVE_CACHE (#47)."""

    def test_setup_java_does_not_cache(self):
        # setup-java's cache saves on every ref, so branch caches evict main's.
        for job_id, step in steps():
            if step.get("uses", "").startswith("actions/setup-java@"):
                self.assertNotIn("cache", step.get("with", {}), job_id)

    def test_cache_saves_are_gated_on_save_cache(self):
        saves = 0
        for job_id, step in steps():
            uses = step.get("uses", "")
            # actions/cache (restore and save in one) can't be gated; use restore + save.
            self.assertFalse(re.match(r"actions/cache@", uses), f"{job_id}: {uses}")
            if uses.startswith("actions/cache/save@"):
                saves += 1
                self.assertIn("env.SAVE_CACHE == 'true'", step.get("if", ""), f"{job_id}: {step.get('name')}")
        self.assertGreater(saves, 0)

    def test_gradle_cache_restore_and_save_paths_match(self):
        # The path list is part of the cache version: a different list never restores.
        action = load(ACTIONS / "save-gradle-cache" / "action.yml")
        (saved,) = [s["with"]["path"] for s in action["runs"]["steps"] if "with" in s]
        restores = [
            (job_id, step["with"]["path"])
            for job_id, step in steps()
            if step.get("name") == "Restore the Gradle cache"
        ]
        self.assertEqual({job_id for job_id, _ in restores}, {"daemon", "bundle"})
        for job_id, path in restores:
            self.assertEqual(path, saved, job_id)


def glob_regex(pattern):
    """dorny/paths-filter (picomatch) globs, as far as brewery_ci.yml uses them."""
    out = ""
    for part in re.split(r"(\*\*/|\*\*|\*)", pattern):
        out += {"**/": "(?:.*/)?", "**": ".*", "*": "[^/]*"}.get(part, re.escape(part))
    return re.compile(out + r"\Z")


def filter_matches(name, path):
    changes = load(WORKFLOW)["jobs"]["changes"]
    (step,) = [s for s in changes["steps"] if s.get("id") == "filter"]
    patterns = yaml.safe_load(step["with"]["filters"])[name]
    return any(glob_regex(p).match(path) for p in patterns)


@unittest.skipIf(yaml is None, "PyYAML is not installed")
class PathsFilterTests(unittest.TestCase):
    def test_bundle_filter_covers_the_packaging_inputs(self):
        # The package jobs (and package-winget) run only when "bundle" changed (#46).
        for path in [
            "packaging/nfpm.yaml",
            "packaging/winget/Brewery.installer.yaml.in",
            "packaging/aur/PKGBUILD.in",
            "packaging/linux/postinstall.sh",
            "crates/zed/resources/windows/brewery.iss",
            "crates/zed/resources/brewery.desktop.in",
            "crates/zed/resources/info/DocumentTypes.plist",
            "script/brewery/bundle-linux",
            "brewery-daemon/build.gradle.kts",
        ]:
            self.assertTrue(filter_matches("bundle", path), path)

    def test_bundle_filter_skips_unrelated_paths(self):
        for path in ["crates/editor/src/editor.rs", "docs/src/README.md", "packaging"]:
            self.assertFalse(filter_matches("bundle", path), path)


@unittest.skipIf(yaml is None, "PyYAML is not installed")
class LicenceTests(unittest.TestCase):
    """cargo-about runs once per run, in the Licences job, and only on a cache miss (#49)."""

    def test_licences_are_generated_only_in_the_licences_job(self):
        generating = {
            job_id
            for job_id, step in steps()
            if "generate-licenses" in step.get("run", "") or "cargo-about" in str(step.get("with", {}).get("tool", ""))
        }
        self.assertEqual(generating, {"licenses"})

    def test_generation_is_skipped_on_a_cache_hit(self):
        job = load(WORKFLOW)["jobs"]["licenses"]
        for step in job["steps"]:
            if "generate-licenses" in step.get("run", "") or "cargo-about" in str(step.get("with", {}).get("tool", "")):
                self.assertIn("steps.cache.outputs.cache-hit != 'true'", step.get("if", ""))

    def test_build_jobs_take_the_licences_from_the_licences_job(self):
        jobs = load(WORKFLOW)["jobs"]
        for job_id in ["build-windows", "build-other"]:
            self.assertIn("licenses", jobs[job_id]["needs"], job_id)
            downloads = [
                s for s in jobs[job_id]["steps"]
                if s.get("uses", "").startswith("actions/download-artifact@")
                and s["with"]["name"] == "rust-licenses"
            ]
            self.assertEqual(len(downloads), 1, job_id)
            self.assertEqual(downloads[0]["with"]["path"], "assets", job_id)


@unittest.skipIf(yaml is None, "PyYAML is not installed")
class SpectreTests(unittest.TestCase):
    """The Visual Studio installer runs only on a cache miss (#48)."""

    def test_windows_rust_jobs_use_the_cached_spectre_libraries(self):
        jobs = load(WORKFLOW)["jobs"]
        for job_id in ["check-windows", "build-windows"]:
            uses = [s.get("uses") for s in jobs[job_id]["steps"]]
            self.assertIn("./.github/actions/msvc-spectre", uses, job_id)

    def test_the_installer_runs_only_on_a_miss(self):
        installs = [
            (job_id, step) for job_id, step in steps() if "vs_installer" in step.get("run", "")
        ]
        self.assertEqual([job_id for job_id, _ in installs], ["action msvc-spectre"])
        (_, step) = installs[0]
        run = step["run"]
        self.assertLess(run.index("CACHE_HIT"), run.index("vs_installer"))


if __name__ == "__main__":
    unittest.main()
