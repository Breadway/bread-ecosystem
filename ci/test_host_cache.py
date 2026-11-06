import fcntl
import io
import json
import os
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import host_cache


class KeyTests(unittest.TestCase):
    def test_keys_are_24_hex_and_stable(self):
        key = host_cache.key_for("breadbar")
        self.assertEqual(len(key), 24)
        self.assertTrue(all(c in "0123456789abcdef" for c in key))
        self.assertEqual(key, host_cache.key_for("breadbar"))
        self.assertNotEqual(key, host_cache.key_for("breadpad"))

    def test_lease_paths_reject_an_invalid_key(self):
        with self.assertRaises(RuntimeError):
            host_cache.lease_paths(Path("/tmp"), "not-a-key")


class MountValidationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()

    def tearDown(self):
        self.temporary.cleanup()

    def test_accepts_the_expected_mount(self):
        host_cache.check_root(self.root, "uuid-1", uuid_lookup=lambda _: "uuid-1")
        self.assertTrue((self.root / "targets").is_dir())
        self.assertTrue((self.root / "state").is_dir())

    def test_refuses_a_missing_mount(self):
        with self.assertRaisesRegex(RuntimeError, "not on the expected filesystem"):
            host_cache.check_root(self.root, "uuid-1", uuid_lookup=lambda _: None)

    def test_refuses_a_different_filesystem(self):
        with self.assertRaisesRegex(RuntimeError, "not on the expected filesystem"):
            host_cache.check_root(self.root, "uuid-1", uuid_lookup=lambda _: "uuid-2")

    def test_refuses_an_unset_uuid(self):
        with self.assertRaisesRegex(RuntimeError, "UUID is not set"):
            host_cache.check_root(self.root, "", uuid_lookup=lambda _: "uuid-1")

    def test_refuses_a_symlink_root(self):
        link = self.root / "link"
        link.symlink_to(self.root)
        with self.assertRaisesRegex(RuntimeError, "must not be a symlink"):
            host_cache.check_root(link, "uuid-1", uuid_lookup=lambda _: "uuid-1")

    def test_refuses_a_group_writable_root(self):
        self.root.chmod(0o775)
        with self.assertRaisesRegex(RuntimeError, "writable by another user"):
            host_cache.check_root(self.root, "uuid-1", uuid_lookup=lambda _: "uuid-1")


class LeaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        host_cache.check_root(self.root, "uuid-1", uuid_lookup=lambda _: "uuid-1")
        self.key = host_cache.key_for("breadbar")
        self.target = self.root / "targets" / self.key
        self.target.mkdir()
        (self.target / "artifact").write_text("synthetic", encoding="utf-8")
        host_cache.touch(self.root, self.key, self.target)

    def tearDown(self):
        self.temporary.cleanup()

    def record(self):
        return json.loads((self.root / "state" / (self.key + ".json")).read_text())

    def test_touch_writes_the_shared_record_shape(self):
        record = self.record()
        self.assertEqual(record["target"], str(self.target))
        self.assertIn("last_used", record)
        self.assertEqual(record["manifest"], "ci:" + self.key)

    def test_touch_refreshes_last_used(self):
        before = self.record()["last_used"]
        host_cache.touch(self.root, self.key, self.target)
        self.assertGreaterEqual(self.record()["last_used"], before)

    def test_active_lease_is_never_deleted(self):
        lock = self.root / "state" / (self.key + ".lock")
        with lock.open("a") as handle:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
            output = io.StringIO()
            with redirect_stdout(output):
                host_cache.retain(self.root, idle_days=0, free_floor_gib=0, apply=True)
            self.assertIn("SKIP active lease", output.getvalue())
        self.assertTrue(self.target.exists())

    def test_idle_unleased_target_is_deleted(self):
        output = io.StringIO()
        with redirect_stdout(output):
            deleted = host_cache.retain(
                self.root, idle_days=0, free_floor_gib=0, apply=True
            )
        self.assertEqual(deleted, 1)
        self.assertFalse(self.target.exists())
        self.assertIn("DELETE", output.getvalue())

    def test_recent_target_is_kept(self):
        output = io.StringIO()
        with redirect_stdout(output):
            deleted = host_cache.retain(
                self.root, idle_days=14, free_floor_gib=0, apply=True
            )
        self.assertEqual(deleted, 0)
        self.assertTrue(self.target.exists())

    def test_a_record_pointing_elsewhere_is_skipped(self):
        record_path = self.root / "state" / (self.key + ".json")
        record = self.record()
        record["target"] = str(self.root / "somewhere-else")
        record_path.write_text(json.dumps(record), encoding="utf-8")
        output = io.StringIO()
        with redirect_stdout(output):
            host_cache.retain(self.root, idle_days=0, free_floor_gib=0, apply=True)
        self.assertIn("does not match its key", output.getvalue())
        self.assertTrue(self.target.exists())

    def test_deletion_stops_after_one_idle_target(self):
        second_key = host_cache.key_for("breadpad")
        second_target = self.root / "targets" / second_key
        second_target.mkdir()
        (second_target / "artifact").write_text("synthetic", encoding="utf-8")
        host_cache.touch(self.root, second_key, second_target)
        with redirect_stdout(io.StringIO()):
            deleted = host_cache.retain(
                self.root, idle_days=0, free_floor_gib=0, apply=True
            )
        self.assertEqual(deleted, 2)
        self.assertFalse(self.target.exists())
        self.assertFalse(second_target.exists())


if __name__ == "__main__":
    unittest.main()
