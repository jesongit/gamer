"""Synthetic tests for the simple migration bundle; never touches production."""
import contextlib
import importlib.util
import io
import pathlib
import sqlite3
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("restore_volume", pathlib.Path(__file__).parents[1] / "deploy/docker/restore-volume.py")
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class RestoreTests(unittest.TestCase):
    def fixture(self, base):
        source = base / "source"
        source.mkdir()
        (source / "state").mkdir()
        (source / "state/credentials.key").write_bytes(b"k" * 32)
        (source / "config.toml").write_text('scheduler_enabled = true\n[auth]\npassword_hash = "$argon2id$fixture"\n', encoding="utf-8")
        with contextlib.closing(sqlite3.connect(source / "gamer.db")) as db, db:
            db.execute("CREATE TABLE timer_tasks(id TEXT,enabled INTEGER)")
            db.execute("INSERT INTO timer_tasks VALUES('daily',1)")
        (source / "history.bin").write_bytes(b"original history")
        archive = base / "gamer-data.tar"
        with tarfile.open(archive, "w") as output:
            output.add(source, arcname=".")
        return archive

    def test_restore_preserves_data_and_disables_only_automatic_scheduling(self):
        with tempfile.TemporaryDirectory() as name:
            base = pathlib.Path(name)
            archive = self.fixture(base)
            dest = base / "destination"
            dest.mkdir()
            with contextlib.redirect_stdout(io.StringIO()):
                helper.restore(archive, dest)
            self.assertEqual((dest / "state/credentials.key").read_bytes(), b"k" * 32)
            self.assertEqual((dest / "history.bin").read_bytes(), b"original history")
            self.assertIn("scheduler_enabled = false", (dest / "config.toml").read_text())
            with contextlib.closing(sqlite3.connect(dest / "gamer.db")) as db:
                self.assertEqual(db.execute("SELECT id,enabled FROM timer_tasks").fetchall(), [("daily", 1)])
            helper.schedule(dest, True)
            self.assertIn("scheduler_enabled = true", (dest / "config.toml").read_text())

    def test_existing_volume_is_refused_without_modification(self):
        with tempfile.TemporaryDirectory() as name:
            base = pathlib.Path(name)
            archive = self.fixture(base)
            dest = base / "destination"
            dest.mkdir()
            sentinel = dest / "keep"
            sentinel.write_bytes(b"existing data")
            with self.assertRaisesRegex(ValueError, "not empty"):
                helper.restore(archive, dest)
            self.assertEqual(sentinel.read_bytes(), b"existing data")
            self.assertFalse((dest / "gamer.db").exists())

    def test_recovery_verify_preserves_database_key_config_and_history(self):
        with tempfile.TemporaryDirectory() as name:
            base = pathlib.Path(name)
            self.fixture(base)
            root = base / "source"
            helper.schedule(root, False)
            before = {str(p.relative_to(root)): p.read_bytes() for p in root.rglob('*') if p.is_file()}
            with contextlib.redirect_stdout(io.StringIO()):
                helper.verify(root)
            after = {str(p.relative_to(root)): p.read_bytes() for p in root.rglob('*') if p.is_file()}
            self.assertEqual(before, after)

    def test_recovery_refuses_enabled_scheduling_without_changing_it(self):
        with tempfile.TemporaryDirectory() as name:
            base = pathlib.Path(name)
            self.fixture(base)
            root = base / "source"
            config = (root / 'config.toml').read_bytes()
            with self.assertRaisesRegex(ValueError, "remain disabled"):
                helper.verify(root)
            self.assertEqual((root / 'config.toml').read_bytes(), config)

    def test_recovery_refuses_missing_original_key_without_generating_one(self):
        with tempfile.TemporaryDirectory() as name:
            base = pathlib.Path(name)
            self.fixture(base)
            root = base / 'source'
            key = root / 'state/credentials.key'
            key.unlink()
            with self.assertRaisesRegex(ValueError, "original 32-byte"):
                helper.verify(root)
            self.assertFalse(key.exists())


if __name__ == "__main__":
    unittest.main()
