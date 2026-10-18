"""Recovery shell failure boundaries with a fake Docker; no daemon is accessible."""
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parents[1]
FAKE_DOCKER = '''#!/usr/bin/env python3
import json,os,pathlib,sys
p=pathlib.Path(os.environ['FAKE_DOCKER_STATE'])
s=json.loads(p.read_text()); a=sys.argv[1:]; s['calls'].append(a)
p.write_text(json.dumps(s))
if a[0]=='info': pass
elif a[:2]==['volume','inspect']:
    if not s.get('volume_exists',True): sys.exit(1)
elif a[:2]==['image','inspect']: print('expected-image')
elif a[:2]==['container','inspect']:
    if not s.get('container_exists',True): sys.exit(1)
elif a[0]=='inspect':
    f=a[a.index('--format')+1]
    if f=='{{.Image}}': print(s.get('image','expected-image'))
    elif f=='{{.State.Status}}': print(s.get('status','created'))
    elif '.Mounts' in f: print(s.get('volume','gamer-production-data'))
    else: sys.exit(92)
elif a[0]=='ps':
    if s.get('busy') and any('volume=' in x for x in a): print('another-active-container')
elif a[0]=='run':
    if 'Runtime preflight passed' in ' '.join(a) and s.get('preflight_fails'): sys.exit(93)
    if a[-1]=='verify' and s.get('verify_fails'): sys.exit(94)
    if '-d' in a: print('new-container-id')
elif a[0]=='update': pass
elif a[0]=='rename': s['container_exists']=False; p.write_text(json.dumps(s))
else: sys.exit(95)
'''


@unittest.skipUnless(os.name == 'posix' and shutil.which('bash'), 'Linux shell tests')
class RecoveryStartTests(unittest.TestCase):
    def scenario(self, cookie_secure=None, **state):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            script = root / 'restore.sh'
            script.write_bytes((REPO / 'deploy/docker/restore.sh').read_bytes())
            stub = root / 'docker'
            stub.write_text(FAKE_DOCKER)
            stub.chmod(0o700)
            state_path = root / 'state.json'
            state_path.write_text(json.dumps({'calls': [], **state}))
            environment = {**os.environ, 'PATH': str(root) + ':' + os.environ['PATH'],
                           'FAKE_DOCKER_STATE': str(state_path)}
            environment.pop('GAMER_COOKIE_SECURE', None)
            if cookie_secure is not None:
                environment['GAMER_COOKIE_SECURE'] = cookie_secure
            result = subprocess.run(['bash', str(script), 'start'], capture_output=True, text=True,
                                    env=environment)
            return result, json.loads(state_path.read_text())['calls']

    def assert_no_data_or_container_mutation(self, calls):
        self.assertFalse(any(c[0] in ('update', 'rename', 'stop', 'start', 'rm', 'pull') for c in calls))
        self.assertFalse(any(c[0]=='run' and ('-d' in c or c[-1] in ('restore', 'activate')) for c in calls))

    def test_success_reuses_volume_and_retains_previous_container(self):
        result, calls = self.scenario()
        self.assertEqual(result.returncode, 0, result.stderr)
        verify = next(c for c in calls if c[0]=='run' and c[-1]=='verify')
        self.assertIn('type=volume,src=gamer-production-data,dst=/data,readonly', verify)
        self.assertIn('512m', verify)
        rename = next(c for c in calls if c[0]=='rename')
        self.assertTrue(rename[2].startswith('gamer-start-failed-'))
        self.assertIn(['update', '--restart=no', 'gamer'], calls)
        launch = next(c for c in calls if c[0]=='run' and '-d' in c)
        self.assertIn('type=volume,src=gamer-production-data,dst=/data', launch)
        self.assertEqual(launch[-1], 'posase/gamer:v1.0.2')
        self.assertFalse(any(c[0]=='run' and c[-1] in ('restore','activate') for c in calls))

    def test_runtime_or_verification_failure_does_not_rename_or_start(self):
        for failure in ('preflight_fails', 'verify_fails'):
            with self.subTest(failure=failure):
                result, calls = self.scenario(**{failure: True})
                self.assertNotEqual(result.returncode, 0)
                self.assert_no_data_or_container_mutation(calls)

    def test_foreign_live_or_busy_volume_is_refused_without_mutation(self):
        for state in ({'image':'foreign'}, {'volume':'foreign'}, {'status':'running'},
                      {'busy':True}, {'volume_exists':False}):
            with self.subTest(state=state):
                result, calls = self.scenario(**state)
                self.assertNotEqual(result.returncode, 0)
                self.assert_no_data_or_container_mutation(calls)

    def test_missing_container_does_not_prevent_verified_existing_volume_start(self):
        result, calls = self.scenario(container_exists=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any(c[0]=='rename' for c in calls))
        self.assertTrue(any(c[0]=='run' and '-d' in c for c in calls))

    def test_explicit_cookie_policy_is_forwarded_when_creating_the_container(self):
        for value in ('false', 'true'):
            with self.subTest(value=value):
                result, calls = self.scenario(cookie_secure=value)
                self.assertEqual(result.returncode, 0, result.stderr)
                launch = next(c for c in calls if c[0]=='run' and '-d' in c)
                self.assertIn('GAMER_COOKIE_SECURE=' + value, launch)


if __name__ == '__main__':
    unittest.main()
