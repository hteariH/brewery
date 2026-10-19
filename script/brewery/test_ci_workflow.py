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


if __name__ == "__main__":
    unittest.main()
