"""Isolated Windows E2E: real executable, local artifacts, success and rollback.

The baseline uses the current executable under an older installation pointer;
this tests the update transaction, not compatibility with an old application.
No running development installation or user data is accessed.
"""
import argparse
import copy
import hashlib
import io
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import urllib.request
import zipfile

REPO = Path(__file__).resolve().parents[2]

def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding='utf-8')

def wait_for(fn, timeout=600):
    end = time.monotonic() + timeout
    last = None
    while time.monotonic() < end:
        try:
            value = fn()
            if value:
                return value
        except Exception as error:
            last = error
        time.sleep(.3)
    raise AssertionError(f'timed out: {last}')

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', type=Path, default=REPO / 'server/target/debug/gamer-server.exe')
    parser.add_argument('--port', type=int, default=18459)
    parser.add_argument('--official-bundle', type=Path)
    parser.add_argument('--installed-plugin', type=Path)
    args = parser.parse_args()
    assert bool(args.official_bundle) == bool(args.installed_plugin), 'Supply both plugin arguments'
    assert args.port != 8443
    root = Path(tempfile.mkdtemp(prefix='gamer-portable-e2e-')).resolve()
    print(f'Isolated installation: {root}', flush=True)
    version = json.loads((REPO / 'web/package.json').read_text(encoding='utf-8'))['version']
    baseline = '0.1.0'
    target = root / 'versions' / baseline
    target.mkdir(parents=True)
    shutil.copy2(args.exe, target / 'gamer-server.exe')
    shutil.copy2(args.exe, root / 'Gamer.exe')
    # A harmless PE overlay makes the old root entry different, exercising real replacement.
    with (root / 'Gamer.exe').open('ab') as entry:
        entry.write(b'portable-e2e-old-entry')
    web_source = args.exe.parent / 'web-dist'
    shutil.copytree(web_source if web_source.is_dir() else REPO / 'server/web-dist', target / 'web-dist')
    (target / 'assets').mkdir()
    shutil.copy2(REPO / 'server/assets/scrcpy-server.jar', target / 'assets/scrcpy-server.jar')
    # Locked dependencies already fetched by the packaging pipeline.
    import tomllib
    lock = tomllib.loads((REPO / 'release/dependencies.lock.toml').read_text(encoding='utf-8-sig'))
    model = json.loads((REPO / 'release/contracts/fixtures/manifest/valid/manifest-valid-basic.json').read_text())
    schema = int(re.search(r'TARGET_SCHEMA:\s*i64\s*=\s*(\d+)', (REPO / 'server/src/migrations.rs').read_text(encoding='utf-8')).group(1))
    model['release'].update(version=baseline, data_schema=schema, minimum_updater_version='0.1.0')
    platform = model['platforms']['windows-x86_64']
    platform['components'] = []
    for component in lock['component']:
        if component['id'] not in ['adb', 'ffmpeg']:
            continue
        name = f"gamer-{component['id']}-{component['version']}-windows-x64.zip"
        archive = REPO / 'release/dist' / name
        with zipfile.ZipFile(archive) as z:
            z.extractall(root / 'runtime' / component['id'] / component['version'])
        artifact = dict(name=name, url=f'https://example.invalid/{name}', size=archive.stat().st_size, sha256=digest(archive))
        platform['components'].append(dict(id=component['id'], version=component['version'], artifact=artifact, required_files=[{k: f[k] for k in ['path', 'size', 'sha256']} for f in component['files']]))
    platform['resources']['scrcpy_server']['sha256'] = digest(target / 'assets/scrcpy-server.jar')
    (root / 'seeds').mkdir()
    archive = root / 'seeds' / f'gamer-app-{version}-windows-x64.zip'
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=1) as z:
        for f in target.rglob('*'):
            if f.is_file():
                rel = f.relative_to(target).as_posix()
                z.write(f, rel)
    platform['app']['artifact'] = dict(name=archive.name, url=f'https://example.invalid/{archive.name}', size=archive.stat().st_size, sha256=digest(archive))
    write_json(root / f'manifests/{baseline}.json', model)
    candidate = copy.deepcopy(model)
    candidate['release']['version'] = version
    expected_plugin = None
    if args.official_bundle:
        bundle = root / 'seeds' / args.official_bundle.name
        shutil.copy2(args.official_bundle, bundle)
        with zipfile.ZipFile(args.installed_plugin) as old:
            old_meta = tomllib.loads(old.read('manifest.toml').decode('utf-8'))
        files = []
        with zipfile.ZipFile(bundle) as packaged:
            for member in packaged.infolist():
                if member.is_dir():
                    continue
                data = packaged.read(member)
                files.append(dict(path=member.filename, size=len(data), sha256=hashlib.sha256(data).hexdigest()))
                if member.filename.endswith('.gplugin'):
                    with zipfile.ZipFile(io.BytesIO(data)) as plugin:
                        meta = tomllib.loads(plugin.read('manifest.toml').decode('utf-8'))
                    if meta['id'] == old_meta['id']:
                        expected_plugin = (meta['id'], meta['version'])
        assert expected_plugin and expected_plugin[1] != old_meta['version']
        candidate['platforms']['windows-x86_64']['components'].append(dict(
            id='official-plugins', version=version, required_files=files,
            artifact=dict(name=bundle.name, url=f'https://example.invalid/{bundle.name}', size=bundle.stat().st_size, sha256=digest(bundle))))
    source = root / 'candidate.json'
    write_json(source, candidate)
    write_json(root / 'state/current.json', dict(schema_version=1, current=baseline, previous=None))
    (root / 'config').mkdir()
    config = (REPO / 'release/config.default.toml').read_text(encoding='utf-8').replace('port = 8443', f'port = {args.port}')
    (root / 'config/config.toml').write_text(config, encoding='utf-8')
    env = os.environ | dict(GAMER_INSTALL_ROOT=str(root), GAMER_PORTABLE_CHILD='1', GAMER_LOCAL_ONLY='1', ADB_MDNS='0', GAMER_RELEASE_MANIFEST=str(source))
    log = (root / 'e2e-process.log').open('wb')
    child = subprocess.Popen([str(root / 'Gamer.exe')], cwd=root, env=env, stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
    token = ''
    def request(path, method='GET', body=None, extra_headers=None):
        headers = {'X-Admin-Token': token, 'Content-Type': 'application/json'}
        headers.update(extra_headers or {})
        data = body if isinstance(body, bytes) else json.dumps(body).encode() if body is not None else (b'' if method != 'GET' else None)
        req = urllib.request.Request(f'http://127.0.0.1:{args.port}{path}', data=data, headers=headers, method=method)
        with urllib.request.urlopen(req, timeout=10) as response:
            raw = response.read()
            return json.loads(raw) if raw else None
    def status():
        return request('/api/system/update')
    try:
        wait_for(lambda: request('/health/ready').get('ready'))
        token = json.loads((root / 'state/admin-token').read_text())['token']
        first = request('/api/system/info')
        assert first['deployment']['mode'] == 'portable' and first['capabilities']['install']
        if args.installed_plugin:
            installed = request('/api/extensions', 'POST', args.installed_plugin.read_bytes(), {
                'Content-Type': 'application/zip', 'X-Expected-Sha256': digest(args.installed_plugin),
                'X-Gamer-Extension-Source': 'official', 'X-Gamer-Permission-Confirm': '1'})
            assert installed['state'] == 'running', installed
        request('/api/system/update/policy', 'PUT', dict(strategy='off', maintenance_window=dict(start='02:00', end='06:00'), freeze_window_minutes=0))
        (root / 'data/e2e-preserve.txt').write_text('keep me')
        request('/api/system/update/check', 'POST', {})
        wait_for(lambda: status()['state'] == 'available')
        request('/api/system/update/apply', 'POST', {})
        wait_for(lambda: json.loads((root / 'state/current.json').read_text())['current'] == version)
        final = wait_for(lambda: (lambda s: s if s['state'] in ['idle', 'failed', 'manual_recovery'] else None)(status()))
        assert final['state'] == 'idle' and not final.get('last_error'), final
        info = request('/api/system/info')
        assert info['startup']['boot_id'] != first['startup']['boot_id']
        assert digest(root / 'Gamer.exe') == digest(root / f'versions/{version}/gamer-server.exe')
        assert (root / 'data/e2e-preserve.txt').read_text() == 'keep me'
        if expected_plugin:
            installed = request('/api/extensions')['extensions']
            updated = next(p for p in installed if p['id'] == expected_plugin[0])
            assert updated['active_version'] == expected_plugin[1] and updated['state'] == 'running', updated
            print(f'PASS paired plugin update: {expected_plugin[0]} {old_meta["version"]} -> {expected_plugin[1]}, enabled intent retained', flush=True)
        print('PASS update: download, snapshot, real restart, current pointer, entrypoint and data preservation', flush=True)
        # The wrong identity must fail readiness and restore the preceding data snapshot.
        bad = copy.deepcopy(candidate)
        bad['release']['version'] = '99.0.0'
        write_json(source, bad)
        # Restart with the local source override (minimal candidate env deliberately does not inherit arbitrary vars).
        request('/api/shutdown', 'POST')
        wait_for(lambda: not (root / 'state/server/launcher.lock').exists())
        child = subprocess.Popen([str(root / 'Gamer.exe')], cwd=root, env=env, stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
        wait_for(lambda: request('/health/ready').get('ready'))
        request('/api/system/update/check', 'POST', {})
        wait_for(lambda: status()['state'] == 'available')
        request('/api/system/update/apply', 'POST', {})
        wait_for(lambda: status()['state'] == 'failed', 600)
        assert json.loads((root / 'state/current.json').read_text())['current'] == version
        assert (root / 'data/e2e-preserve.txt').read_text() == 'keep me'
        assert request('/health/ready')['ready']
        print('PASS rollback: candidate identity mismatch, old service healthy, data preserved', flush=True)
    finally:
        try:
            request('/api/shutdown', 'POST')
        except Exception:
            pass
        try:
            child.wait(timeout=15)
        except subprocess.TimeoutExpired:
            child.terminate()
        log.close()
        print(f'Evidence retained in {root}', flush=True)

if __name__ == '__main__':
    main()
