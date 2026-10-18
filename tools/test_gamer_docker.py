"""Migration invariants using synthetic data only; no live account or source data."""
import argparse
import contextlib
import hashlib
import io
import json
import pathlib
import sqlite3
import tempfile
import unittest

import gamer_docker as migration


class MigrationTests(unittest.TestCase):
    def test_snapshot_preserves_history_and_originals_while_converting_active_credentials(self):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            source = root / "source"
            source.mkdir()
            with contextlib.closing(sqlite3.connect(source / "gamer.db")) as db, db:
                db.executescript("PRAGMA user_version=7; CREATE TABLE browser_targets(config TEXT);"
                                 "CREATE TABLE timer_tasks(id TEXT,enabled INTEGER);"
                                 "INSERT INTO timer_tasks VALUES('daily',1);")
                db.execute("INSERT INTO browser_targets VALUES(?)", (json.dumps({"profile_id": "fixture"}),))
            files = {
                "packages/game/plugins/gamer-yaml/automations/daily.yaml": b"fixture script",
                "runtime-traces/old/frame.png": b"original evidence",
                "extension-data/gamer-ai/private/settings.dat": b"retired opaque data",
                "extensions/immutable/version.wasm": b"immutable version",
                "browser-profiles/fixture/Default/Local Storage/leveldb/000001.log": b"site state",
                "browser-profiles/fixture/Default/Network/Cookies": b"Windows opaque cookie database",
            }
            for relative, data in files.items():
                path = source / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            old_key = b"k" * 32
            old_key_path = source / "state/credentials.key"
            old_key_path.parent.mkdir()
            old_key_path.write_bytes(old_key)
            private = source / migration.PRIVATE_FILES[1]
            private.parent.mkdir(parents=True)
            original = migration.seal(old_key, json.dumps({"engine_path": "C:/tesseract.exe", "tessdata_path": "C:/data"}).encode())
            private.write_bytes(original)
            config = root / "config.toml"
            config.write_text('port=8443\nbrowser_idle_secs=1800\n[auth]\npassword_hash="$argon2id$synthetic-fixture"\n', encoding="utf-8")
            destination = root / "prepared"
            with contextlib.redirect_stdout(io.StringIO()):
                migration.prepare(argparse.Namespace(source=str(source), config=str(config), destination=str(destination)))
            backup, working = destination / "backup", destination / "data"
            manifest = json.loads((destination / "backup-manifest.json").read_text())
            for relative, data in files.items():
                self.assertEqual((source / relative).read_bytes(), data)
                self.assertEqual((backup / relative).read_bytes(), data)
                self.assertEqual(manifest[relative]["sha256"], hashlib.sha256(data).hexdigest())
                if not relative.startswith("browser-profiles/"):
                    self.assertEqual((working / relative).read_bytes(), data)
            self.assertEqual(private.read_bytes(), original)
            self.assertEqual((working / "migration-originals/state/credentials.key").read_bytes(), old_key)
            self.assertEqual((backup / migration.PRIVATE_FILES[1]).read_bytes(), original)
            converted = migration.unseal((working / "state/credentials.key").read_bytes(),
                                         (working / migration.PRIVATE_FILES[1]).read_bytes())
            self.assertEqual(json.loads(converted)["engine_path"], "/usr/bin/tesseract")
            self.assertEqual((working / "browser-profiles/fixture/Default/Local Storage/leveldb/000001.log").read_bytes(), b"site state")
            self.assertFalse((working / "browser-profiles/fixture/Default/Network/Cookies").exists())
            self.assertEqual((working / "migration-originals/windows-browser-profiles/fixture/Default/Network/Cookies").read_bytes(), b"Windows opaque cookie database")
            self.assertIn("scheduler_enabled = false", (working / "config.toml").read_text())
            with contextlib.closing(sqlite3.connect(working / "gamer.db")) as db:
                self.assertEqual(db.execute("SELECT id,enabled FROM timer_tasks").fetchall(), [("daily", 1)])
            with self.assertRaises(ValueError):
                migration.prepare(argparse.Namespace(source=str(source), config=str(config), destination=str(destination)))


if __name__ == "__main__":
    unittest.main()
