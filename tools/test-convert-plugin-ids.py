from contextlib import closing
import importlib.util
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('convert_ids', Path(__file__).with_name('convert-plugin-ids.py'))
converter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(converter)


class ConversionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / 'data'
        self.script = self.root / 'packages/demo/plugins/gamer.yaml/automations/main.yaml'
        self.script.parent.mkdir(parents=True)
        self.script.write_text('steps:\n  - log: gamer.yaml\n', encoding='utf-8')
        (self.root / 'packages/demo/package.toml').write_text('id="demo"\n[plugins."gamer.yaml"]\nrequired=true\n', encoding='utf-8')
        self.state = self.root / 'extensions/state.json'
        self.state.parent.mkdir()
        converter.write_json(self.state, {'plugins': {'gamer.yaml': {'id': 'gamer.yaml', 'active_version': '3.1.2', 'state': 'disabled'}}})
        version = self.root / 'extensions/gamer.yaml/3.1.2'
        version.mkdir(parents=True)
        (version / 'manifest.toml').write_text('id="gamer.yaml"\nversion="3.1.2"\n', encoding='utf-8')
        with closing(sqlite3.connect(self.root / 'gamer.db')) as db:
            db.execute('CREATE TABLE timer_tasks (runner_id TEXT, payload_json TEXT)')
            db.execute('INSERT INTO timer_tasks VALUES (?,?)', ('gamer.yaml', '{"literal":"gamer.yaml"}'))
            db.commit()

    def tearDown(self):
        self.temp.cleanup()

    def test_dry_run_never_changes_original(self):
        result = converter.convert(self.root)
        self.assertTrue(result['dry_run'])
        self.assertTrue(self.script.exists())
        self.assertIn('gamer.yaml', converter.read_json(self.state)['plugins'])
        self.assertEqual(result['changes']['runner_references'], 1)

    def test_offline_swap_preserves_data_and_backup_and_is_idempotent(self):
        result = converter.convert(self.root, apply=True, server_stopped=True)
        backup = Path(result['backup'])
        self.assertTrue((backup / self.script.relative_to(self.root)).exists())
        script = self.root / 'packages/demo/plugins/gamer-yaml/automations/main.yaml'
        self.assertEqual(script.read_text(encoding='utf-8'), 'steps:\n  - log: gamer.yaml\n')
        self.assertEqual(converter.read_json(self.state)['plugins']['gamer-yaml']['state'], 'disabled')
        with closing(sqlite3.connect(self.root / 'gamer.db')) as db:
            self.assertEqual(db.execute('SELECT * FROM timer_tasks').fetchone(), ('gamer-yaml', '{"literal":"gamer.yaml"}'))
        self.assertFalse(converter.convert(self.root, apply=True, server_stopped=True)['applied'])

    def test_conflict_leaves_both_original_directories_untouched(self):
        (self.root / 'packages/demo/plugins/gamer-yaml').mkdir()
        with self.assertRaisesRegex(ValueError, 'overwrite'):
            converter.convert(self.root, apply=True, server_stopped=True)
        self.assertTrue(self.script.exists())
        self.assertTrue((self.root / 'packages/demo/plugins/gamer-yaml').exists())

    def test_apply_requires_offline_acknowledgement(self):
        with self.assertRaisesRegex(ValueError, 'Stop Gamer'):
            converter.convert(self.root, apply=True)


if __name__ == '__main__':
    unittest.main()
