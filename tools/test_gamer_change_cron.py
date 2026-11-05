"""Isolated cron maintenance regressions; no HTTP, SQLite or device I/O."""
import copy
import unittest

from gamer_change_cron import change_cron


class FakeClient:
    def __init__(self):
        self.calls = []
        self.task = {'id': 'monthly', 'name': '每月兑换', 'updated_at': '2026-11-05T14:00:00Z',
                     'schedule': {'provider_id': 'cron', 'config': {'expression': '17 4 6 * *'}},
                     'app': {'device_id': 'browser-1', 'content_package': 'hsr'},
                     'runner_id': 'gamer-yaml', 'entrypoint': 'hsr/monthly.yaml',
                     'payload': {'args': {'notify_success': False}}, 'enabled': True, 'state': 'active',
                     'inputs': [{'id': 'one', 'device_id': 'browser-1'}, {'id': 'two', 'device_id': 'browser-2'}],
                     'concurrency': 2}
        self.conflict = False

    def tool(self, name, args):
        self.calls.append((name, copy.deepcopy(args)))
        if name.endswith('task_get'):
            return copy.deepcopy(self.task)
        if name.endswith('schedule_preview'):
            return {'valid': True, 'server_now_local': '2026-11-05T22:00:00+08:00',
                    'next_times': ['2026-11-05T21:00:00Z'],
                    'next_times_local': ['2026-11-06T05:00:00+08:00']}
        if self.conflict:
            raise RuntimeError('task_changed')
        saved = copy.deepcopy(args)
        saved['next_wakeup'] = None
        return saved


class ChangeCronTests(unittest.TestCase):
    def test_default_previews_in_server_timezone_without_writing(self):
        client = FakeClient()
        result = change_cron(client, 'monthly', '17 4 6 * *', '0 5 6 * *')
        self.assertFalse(result['applied'])
        self.assertEqual(result['next_times_local'], ['2026-11-06T05:00:00+08:00'])
        self.assertEqual([name for name, _ in client.calls], ['gamer.core.task_get', 'gamer.core.schedule_preview'])

    def test_apply_uses_snapshot_cas_and_preserves_multi_input_configuration(self):
        client = FakeClient()
        result = change_cron(client, 'monthly', '17 4 6 * *', '0 5 6 * *', True, 'change-monthly-once')
        args = client.calls[-1][1]
        self.assertTrue(result['applied'])
        self.assertEqual(args['expected_updated_at'], client.task['updated_at'])
        for field in ('inputs', 'concurrency', 'enabled', 'payload', 'runner_id', 'entrypoint'):
            self.assertEqual(args[field], client.task[field])
        self.assertEqual(args['operation_id'], 'change-monthly-once')
        self.assertNotIn('next_wakeup', args)

    def test_changed_expression_fails_before_preview_or_write(self):
        client = FakeClient()
        with self.assertRaisesRegex(ValueError, 'cron changed'):
            change_cron(client, 'monthly', 'unexpected', '0 5 6 * *', True)
        self.assertEqual(len(client.calls), 1)

    def test_conflict_is_not_retried(self):
        client = FakeClient()
        client.conflict = True
        with self.assertRaisesRegex(RuntimeError, 'task_changed'):
            change_cron(client, 'monthly', '17 4 6 * *', '0 5 6 * *', True, 'conflicted-once')
        self.assertEqual(sum(name.endswith('task_save') for name, _ in client.calls), 1)

    def test_enabled_suspended_task_is_not_implicitly_resumed(self):
        client = FakeClient()
        client.task['state'] = 'suspended'
        with self.assertRaisesRegex(ValueError, 'implicitly resume'):
            change_cron(client, 'monthly', '17 4 6 * *', '0 5 6 * *', True)
        self.assertEqual(len(client.calls), 1)


if __name__ == '__main__':
    unittest.main()
