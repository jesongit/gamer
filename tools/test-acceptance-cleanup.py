"""Verify isolated executable cleanup without deleting real files."""
import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

repo = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('acceptance_cleanup_subject', repo / 'tools/verify-ai-automation-local.py')
subject = importlib.util.module_from_spec(spec)
spec.loader.exec_module(subject)


class CleanupTests(unittest.TestCase):
    def harness(self):
        source = repo / 'virtual-source/gamer-server.exe'
        work = repo / 'virtual-isolated-run'
        value = subject.Harness(SimpleNamespace(server=source), work)
        value.isolated_binary = work / source.name
        return value

    def test_finished_copy_removed_and_original_preserved(self):
        harness = self.harness()
        harness.process = Mock()
        harness.process.poll.return_value = 0
        with patch.object(Path, 'unlink', autospec=True) as unlink:
            harness.close()
            unlink.assert_called_once_with(harness.isolated_binary, missing_ok=True)
            harness.process.terminate.assert_not_called()

    def test_shutdown_finishes_before_removing_copy(self):
        harness = self.harness()
        events = []
        harness.process = Mock()
        harness.process.poll.side_effect = [None, 0]
        harness.process.terminate.side_effect = lambda: events.append('terminate')
        harness.process.wait.side_effect = lambda **_: events.append('wait')
        with patch.object(Path, 'unlink', autospec=True, side_effect=lambda *_, **__: events.append('unlink')):
            harness.close()
        self.assertEqual(events, ['terminate', 'wait', 'unlink'])

    def test_foreign_or_original_path_is_never_removed(self):
        for path in (repo / 'foreign.exe', repo / 'virtual-source/gamer-server.exe'):
            harness = self.harness()
            harness.isolated_binary = path
            with patch.object(Path, 'unlink', autospec=True) as unlink:
                with self.assertRaisesRegex(RuntimeError, 'non-isolated'):
                    harness.close()
                unlink.assert_not_called()


if __name__ == '__main__':
    unittest.main()
