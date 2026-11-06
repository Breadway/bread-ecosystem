import importlib.util
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check-stale-tracks.py")
SPEC = importlib.util.spec_from_file_location("check_stale_tracks", SCRIPT)
CHECKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


class StaleTrackTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.repo = Path(self.temp_dir.name) / "bread-demo"
        self.repo.mkdir()
        self.run_git("init", "-q")
        self.run_git("config", "user.name", "Test")
        self.run_git("config", "user.email", "test@example.com")
        workflows = self.repo / ".forgejo" / "workflows"
        workflows.mkdir(parents=True)
        (workflows / "dev-release.yml").write_text("name: dev\n")
        self.commit("promotion", 1_800_000_000)
        self.run_git("tag", "v1.0.0")
        self.run_git("branch", "-M", "main")
        self.run_git("update-ref", "refs/remotes/origin/main", "HEAD")

    def tearDown(self):
        self.temp_dir.cleanup()

    def run_git(self, *args, env=None):
        merged_env = os.environ.copy()
        if env:
            merged_env.update(env)
        return subprocess.run(
            ["git", "-C", str(self.repo), *args],
            check=True,
            capture_output=True,
            text=True,
            env=merged_env,
        )

    def commit(self, message, epoch):
        path = self.repo / "work.txt"
        path.write_text(path.read_text() + message + "\n" if path.exists() else message + "\n")
        self.run_git("add", "work.txt")
        date = str(epoch)
        self.run_git(
            "commit",
            "-qm",
            message,
            env={"GIT_AUTHOR_DATE": f"@{date} +0000", "GIT_COMMITTER_DATE": f"@{date} +0000"},
        )
        self.run_git("update-ref", "refs/remotes/origin/main", "HEAD")

    def test_oldest_unpromoted_commit_triggers_at_fourteen_days(self):
        now = 1_900_000_000
        self.commit("old work", now - CHECKER.AGE_THRESHOLD_SECONDS)
        warning = CHECKER.inspect_repo(self.repo, now)
        self.assertTrue(warning["age_triggered"])
        self.assertFalse(warning["commit_triggered"])

    def test_twenty_commits_trigger_even_when_recent(self):
        now = 1_900_000_000
        for index in range(CHECKER.COMMIT_THRESHOLD):
            self.commit(f"work {index}", now - 60)
        warning = CHECKER.inspect_repo(self.repo, now)
        self.assertFalse(warning["age_triggered"])
        self.assertTrue(warning["commit_triggered"])

    def test_recent_single_commit_does_not_trigger(self):
        now = 1_900_000_000
        self.commit("recent work", now - CHECKER.AGE_THRESHOLD_SECONDS + 1)
        self.assertIsNone(CHECKER.inspect_repo(self.repo, now))

    def test_no_dev_workflow_is_ignored(self):
        (self.repo / ".forgejo" / "workflows" / "dev-release.yml").unlink()
        self.assertIsNone(CHECKER.inspect_repo(self.repo, 1_900_000_000))

    def test_release_docs_wire_checker_into_report_only_sweep(self):
        docs = SCRIPT.parent.parent / "docs" / "release-channels.md"
        text = docs.read_text()
        self.assertIn("At the end of each normal ecosystem sweep", text)
        self.assertIn(
            "python3 ~/Projects/bread-ecosystem/scripts/check-stale-tracks.py ~/Projects",
            text,
        )
        self.assertIn("do not file backlog issues", text)
        self.assertIn("create tags, or publish artifacts", text)


if __name__ == "__main__":
    unittest.main()
