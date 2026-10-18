"""Data-loss regression coverage for compact, read-only Docker migration."""
import argparse
import contextlib
import importlib.util
import io
import json
import pathlib
import tarfile
import tempfile
import unittest
from unittest import mock

import gamer_docker

spec = importlib.util.spec_from_file_location("volume_transfer", pathlib.Path(__file__).parents[1] / "deploy/docker/volume-transfer.py")
transfer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(transfer)


class VolumeTransferTests(unittest.TestCase):
    def test_production_preserves_sessions_and_all_application_history(self):
        keep = ["gamer.db", "gamer.db-wal", "config.toml", "state/credentials.key",
                "state/home/.config/gh/hosts.yml", "state/browser-sessions.enc",
                "browser-profiles/account/Local State",
                "browser-profiles/account/Default/Cookies",
                "browser-profiles/account/Default/Network/Cookies",
                "browser-profiles/account/Default/Local Storage/leveldb/data",
                "browser-profiles/account/Default/IndexedDB/data",
                "browser-profiles/account/Default/Session Storage/data",
                "browser-profiles/account/Default/Service Worker/CacheStorage/data",
                "extension-data/gamer-ai/private/settings.dat",
                "extension-data/gamer-yaml/drafts/old-version",
                "extensions/immutable/old-version.wasm", "runtime-traces/run/frame.png",
                "retained-traces/run/frame.png", "media/asset/original.mp4",
                "package-publisher/transaction.json", "packages/game/script.yaml",
                "unknown-future-data/Cache/keep", "plugin-data/historical-candidate"]
        skip = ["_backups/old/data", "migration-originals/windows-browser-profiles/Cookies",
                "hsr-supervision/capture.png", "live-workflow-20261015/fixture",
                "live-workflow-20261016/fixture", "qa-autonomy/fixture", "qa-cursor-lock/fixture",
                "browser-profiles/account/Default/Cache/data",
                "browser-profiles/account/Default/Code Cache/data",
                "browser-profiles/account/ShaderCache/data",
                "browser-profiles/account/DevToolsActivePort"]
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            original = {}
            for relative in keep + skip:
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                original[relative] = ("original:" + relative).encode()
                path.write_bytes(original[relative])
            for profile in ("production", "full"):
                output = io.BytesIO()
                transfer.export(root, profile, output, compressed=True)
                output.seek(0)
                with tarfile.open(fileobj=output, mode="r:gz") as archive:
                    actual = {member.name: archive.extractfile(member).read() for member in archive if member.isfile()}
                expected = keep if profile == "production" else keep + skip
                self.assertEqual(actual, {name: original[name] for name in expected})
            with contextlib.redirect_stdout(io.StringIO()) as captured:
                transfer.report(root, "production")
            report = json.loads(captured.getvalue())
            self.assertEqual(report["included_bytes"], sum(len(original[name]) for name in keep))
            self.assertEqual(report["excluded_bytes"], sum(len(original[name]) for name in skip))
            self.assertEqual(report["included_files"], len(keep))
            for name, data in original.items():
                self.assertEqual((root / name).read_bytes(), data)

    def test_live_volume_export_is_rejected_before_creating_a_file(self):
        with tempfile.TemporaryDirectory() as folder:
            target = pathlib.Path(folder) / "backup.tar.gz"
            args = argparse.Namespace(volume="fixture", image="fixture", output=str(target), profile="production", report=False)
            with mock.patch.object(gamer_docker.subprocess, "run") as run, \
                    mock.patch.object(gamer_docker.subprocess, "check_output", return_value="active-container"):
                with self.assertRaisesRegex(ValueError, "Stop containers"):
                    gamer_docker.backup(args)
                self.assertEqual(run.call_count, 1)  # volume inspect only
            self.assertFalse(target.exists())
            self.assertFalse(target.with_name(target.name + ".partial").exists())

    def test_failed_export_never_publishes_a_backup(self):
        with tempfile.TemporaryDirectory() as folder:
            target = pathlib.Path(folder) / "backup.tar.gz"
            args = argparse.Namespace(volume="fixture", image="fixture", output=str(target), profile="production", report=False)
            with mock.patch.object(gamer_docker, "private_permissions"), \
                    mock.patch.object(gamer_docker.subprocess, "run", side_effect=[None, RuntimeError("fixture failure")]), \
                    mock.patch.object(gamer_docker.subprocess, "check_output", return_value=""):
                with self.assertRaisesRegex(RuntimeError, "fixture failure"):
                    gamer_docker.backup(args)
            self.assertFalse(target.exists())
            self.assertFalse(target.with_name(target.name + ".partial").exists())


if __name__ == "__main__":
    unittest.main()
