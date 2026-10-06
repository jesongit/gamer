#!/usr/bin/env python3
"""Local HTTP acceptance for the real Gamer server; Python 3.11+, stdlib only.

No build, device, browser, real model, external network destination, production data, or
repository fixture writes. The deterministic provider is a protocol test double,
never evidence of model quality. See docs/testing/ai-automation-local.md.
"""
from __future__ import annotations

import argparse
import base64
import copy
import hashlib
import http.cookiejar
import http.server
import io
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import zipfile
import zlib

YAML = """version: 2
targets:
  claim: {template: claim.png, threshold: 0.99}
  confirm: {template: confirm.png, threshold: 0.99}
  done: {template: done.png, threshold: 0.99}
run:
  - id: claim_button
    wait: claim
    then: [{tap: claim}]
  - optional: {find: confirm, timeout: 250ms, then: [{tap: confirm}]}
  - finish: done
"""
CROPS = [dict(name=name + '.png', sample_id='sample-a', frame_id=frame,
              rect=[20, 15, 9, 9]) for name, frame in [('claim', 'f0'), ('confirm', 'f2'), ('done', 'f4')]]
SELECTED = [dict(sample_id='sample-' + name, plugin_id='gamer-video') for name in 'abc']


def encoded(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode()


def sha(value):
    return hashlib.sha256(value).hexdigest()


def png(width, height, pixel):
    def chunk(kind, body):
        return struct.pack('>I', len(body)) + kind + body + struct.pack('>I', zlib.crc32(kind + body))
    raw = b''.join(b'\0' + bytes(c for x in range(width) for c in pixel(x, y)) for y in range(height))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')


def screen(seed, x0=20, y0=15):
    def pixel(x, y):
        if x0 <= x < x0 + 9 and y0 <= y < y0 + 9:
            x, y = x - x0, y - y0
            v = ((x * 73856093 + y * 19349663 + seed * 83492791) ^ (x * y * seed * 29)) & 255
            return v, (v * 3) & 255, (v + seed * 41) & 255
        return ((x * 17 + y * 3) % 67,) * 3
    return png(64, 48, pixel)


def archive(manifest, files):
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w', compression=zipfile.ZIP_STORED) as bundle:
        for name, body in [('manifest.json', encoded(manifest)), *sorted(files.items())]:
            entry = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            entry.external_attr = 0o100644 << 16
            bundle.writestr(entry, body)
    return out.getvalue()


def fixture(name, popup=True, shifted=False, bad_end=False, origin=None):
    states = [(0, 1), (100000, 1), (250000, 2), (300000, 2), (450000, 3), (500000, 3), (750000, 3), (1000000, 3)] if popup else [(0, 1), (100000, 1), (250000, 3), (500000, 3), (750000, 3), (1000000, 3)]
    if bad_end:
        states[-1] = (states[-1][0], 5)
    x, y = origin or ((32, 25) if shifted else (20, 15))
    files, frames = {}, []
    media_id = 'a' * 64
    for i, (at, seed) in enumerate(states):
        path, body = f'frames/f{i}.png', screen(seed, x, y)
        files[path] = body
        frames.append(dict(id=f'f{i}', path=path, sha256=sha(body), media_id=media_id,
                           media_sha256='b' * 64, frame_index=i, pts_us=at,
                           timeline_us=at, width=64, height=48, rotation=0, role='observation'))
    def action(event, at):
        return dict(schema_version=1, event_id=event, operation_id=event, session_id='recording',
                    source='manual', kind='tap', timeline_us=at, time_domain='recording',
                    coordinate_space='device-display', display_size=dict(width=64, height=48),
                    payload=dict(x=x+5, y=y+5, duration_us=50000), status='accepted')
    actions = [action('claim', 150000)] + ([action('confirm', 350000)] if popup else [])
    windows = [dict(event_id='claim', before_frame_ids=['f0', 'f1'], after_frame_ids=['f2', 'f3'])]
    if popup:
        windows.append(dict(event_id='confirm', before_frame_ids=['f2', 'f3'], after_frame_ids=['f4', 'f5']))
    manifest = dict(schema_version=1, id=name, name='Synthetic HTTP acceptance ' + name,
                    content_sha256='', recording_id='recording', start=dict(timeline_us=0, frame_id='f0'),
                    end=dict(timeline_us=1000000, frame_id=f'f{len(frames)-1}'),
                    goal=dict(description='done pattern visible', confirmed=True),
                    coordinates=dict(space='device-display', width=64, height=48, rotation=0),
                    status='complete', max_frame_gap_us=500000, diagnostics=[], actions=actions,
                    frames=frames, windows=windows,
                    segments=[dict(media_id=media_id, start_us=0, duration_us=1000000, base_pts_us=0)],
                    files=[dict(path=p, size=len(b), sha256=sha(b)) for p, b in sorted(files.items())])
    manifest['content_sha256'] = sha(encoded(manifest))
    return manifest, files


def first_rgb(data_url):
    """Probe PNG is an opaque RGB(A) solid image; decode first pixel, no ML."""
    data = base64.b64decode(data_url.split(',', 1)[1], validate=True)
    assert data.startswith(b'\x89PNG\r\n\x1a\n')
    at, compressed = 8, b''
    while at < len(data):
        n = struct.unpack('>I', data[at:at+4])[0]
        kind, body = data[at+4:at+8], data[at+8:at+8+n]
        if kind == b'IHDR':
            assert body[8] == 8 and body[9] in (2, 6), 'unexpected probe PNG'
        if kind == b'IDAT':
            compressed += body
        at += n + 12
    raw = zlib.decompress(compressed)
    # All PNG filter predictors are zero for the very first RGB pixel.
    return tuple(raw[1:4])


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for v in value.values():
            yield from strings(v)
    elif isinstance(value, list):
        for v in value:
            yield from strings(v)


class Stub:
    """Only deterministic Responses protocol; no remote requests or inference."""
    def __init__(self):
        self.mode = 'valid'
        self.requests = []
        self.started = threading.Event()
        self.release = threading.Event()
        self.release.set()
        owner = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass
            def do_POST(self):
                try:
                    assert self.path == '/v1/responses', 'unexpected provider route'
                    n = int(self.headers['Content-Length'])
                    assert 0 < n < 8 * 1024 * 1024
                    request = json.loads(self.rfile.read(n))
                    assert request.get('stream') is False, 'harness expects nonstreaming API'
                    all_strings = list(strings(request.get('input', [])))
                    prompt = '\n'.join(s for s in all_strings if not s.startswith('data:'))
                    images = [s for s in all_strings if s.startswith('data:image/png;base64,')]
                    calls = []
                    selected_ids = []
                    match = re.search(r'READY_[a-f0-9]+', prompt)
                    if match:
                        kind, text = 'probe.model', match.group()
                    elif 'synthetic image' in prompt or 'Call probe_observe exactly once' in prompt:
                        colors = {(235, 15, 15): 'red', (10, 210, 20): 'green', (10, 20, 235): 'blue', (230, 230, 10): 'yellow'}
                        text = colors[first_rgb(images[-1])]
                        if isinstance(request.get('tool_choice'), dict):
                            marker = re.search(r'marker ([a-f0-9]+)', prompt).group(1)
                            calls = [dict(type='function_call', call_id='local-probe-call', name='probe_observe',
                                          arguments=json.dumps(dict(color=text, marker=marker)), status='completed')]
                        kind = 'probe.images_and_tools'
                    else:
                        kind = 'generation.' + owner.mode
                        owner.started.set()
                        assert owner.release.wait(20), 'local stub hold timeout'
                        assert len(images) >= 3, 'production request must contain actual selected frames'
                        assert not request.get('tools'), 'generation must have no mutation tools'
                        contexts = []
                        for item in all_strings:
                            if item.startswith('{'):
                                try:
                                    decoded = json.loads(item)
                                except ValueError:
                                    continue
                                if isinstance(decoded, dict) and isinstance(decoded.get('samples'), list):
                                    contexts.append(decoded)
                        assert len(contexts) == 1 and contexts[0]['samples'], 'selected evidence context missing'
                        selected_ids = [sample['id'] for sample in contexts[0]['samples']]
                        selected_id = selected_ids[0]
                        crops = [{**crop, 'sample_id': selected_id} for crop in CROPS]
                        proposal = dict(yaml=YAML, templates=crops, explanation='Deterministic local protocol stub, not a real model')
                        if owner.mode == 'bad_action':
                            proposal['yaml'] = YAML.replace('tap: claim', 'tap: [0.9, 0.9]')
                        elif owner.mode == 'drop_sample':
                            proposal['samples'] = ['sample-a', 'sample-c']
                        text = json.dumps(proposal)
                    owner.requests.append(dict(kind=kind, model=request.get('model'), images=len(images), sample_ids=selected_ids))
                    output = calls or [dict(type='message', role='assistant', content=[dict(type='output_text', text=text)])]
                    response = dict(status='completed', output=output, usage=dict(input_tokens=12, output_tokens=4, total_tokens=16))
                    body = encoded(response)
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Expected when cancellation closes a held request.
                except Exception as exc:
                    owner.requests.append(dict(kind='stub_error', error=str(exc)))
                    self.send_error(500, 'local protocol stub error')
        self.httpd = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.httpd.daemon_threads = True
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()
        self.url = f'http://127.0.0.1:{self.httpd.server_port}/v1'

    def hold(self):
        self.mode = 'valid'
        self.started.clear()
        self.release.clear()

    def close(self):
        self.release.set()
        self.httpd.shutdown()
        self.httpd.server_close()
        self.thread.join(timeout=3)


class HTTP:
    def __init__(self, base):
        self.base = base
        # Never inherit proxy configuration for local acceptance requests.
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

    def request(self, method, path, body=None, raw=False, headers=None, expect=(200,)):
        data = body if isinstance(body, bytes) else encoded(body) if body is not None else None
        h = {'Content-Type': 'application/octet-stream' if isinstance(body, bytes) else 'application/json'}
        h.update(headers or {})
        req = urllib.request.Request(self.base + path, data=data, method=method, headers=h)
        try:
            response = self.opener.open(req, timeout=45)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            status, value = response.status, response.read()
        try:
            parsed = json.loads(value)
        except (ValueError, UnicodeDecodeError):
            parsed = value
        assert status in expect, f'{method} {path}: HTTP {status}, expected {expect}; {str(parsed)[:2500]}'
        return value if raw else parsed

    def call(self, action, values=None, plugin='gamer-yaml', expect=(200,)):
        result = self.request('POST', f'/api/extensions/{plugin}/call', dict(action=action, values=values or {}), expect=expect)
        return result.get('data', result) if isinstance(result, dict) else result


class Harness:
    def __init__(self, args, work):
        self.args, self.work = args, work
        self.results = []
        self.process = None
        self.log = None
        self.stub = None
        self.secrets = [secrets.token_urlsafe(30), secrets.token_urlsafe(30)]
        self.package = 'acceptance'

    def check(self, name, test):
        started = time.monotonic()
        try:
            detail = test()
            row = dict(name=name, status='passed', seconds=round(time.monotonic()-started, 3))
            if detail is not None:
                row['detail'] = detail
        except Exception as exc:
            if self.stub:
                self.stub.release.set()
            detail = exc.args[0] if len(exc.args) == 1 else str(exc)
            if isinstance(detail, dict):
                # Full events remain in isolated candidate storage; make failures readable.
                def concise(value):
                    if isinstance(value, dict):
                        return {k: concise(v) for k, v in value.items() if k != 'events'}
                    if isinstance(value, list):
                        return [concise(v) for v in value]
                    return value
                detail = json.dumps(concise(detail), ensure_ascii=False)
            row = dict(name=name, status='failed', seconds=round(time.monotonic()-started, 3), error=str(detail))
        self.results.append(row)
        print(f"[{row['status'].upper()}] {name}", flush=True)
        if row['status'] == 'failed':
            print(self.redact(row['error'])[:3000], flush=True)
        return row['status'] == 'passed'

    def redact(self, text):
        for secret in self.secrets:
            text = text.replace(secret, '<redacted-local-test-secret>')
        return text

    def start(self):
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        self.http = HTTP(f'http://127.0.0.1:{port}')
        data = self.work / 'data'
        data.mkdir()
        # Missing ADB path is intentional: startup cannot scan/control a device.
        cfg = dict(port=port, local_only=True, data_dir=str(data), adb_path=str(self.work/'intentionally-missing-adb'),
                   ffmpeg_path=shutil.which('ffmpeg') or 'ffmpeg', scrcpy_server=str(self.args.repo/'server/assets/scrcpy-server.jar'),
                   threshold=0.85, decode_frames=True, max_size=0, bitrate_mbps=12, fps=15,
                   probe_encoder=False, idle_power_secs=0)
        config = '\n'.join(f'{k} = {json.dumps(v)}' for k, v in cfg.items()) + '\n[update]\nstrategy = "off"\n'
        (self.work/'config.toml').write_text(config, encoding='utf-8')
        env = os.environ.copy()
        # Do not inherit production paths, credentials or proxy destinations.
        for key in list(env):
            if key.startswith('GAMER_') or key.startswith('GB_') or key.lower().endswith('_proxy'):
                env.pop(key)
        env.update(GB_CONFIG=str(self.work/'config.toml'), GB_LOG='stdout', GAMER_PROFILE='dev', GAMER_LOCAL_ONLY='1',
                   GAMER_ADMIN_PASSWORD=self.secrets[0], GAMER_APP_DIR=str(self.args.repo/'server'), ADB_MDNS='0',
                   HOME=str(self.work), XDG_CONFIG_HOME=str(self.work/'config'), XDG_DATA_HOME=str(self.work/'share'))
        # Isolate the running executable too, so subsequent cargo builds are safe.
        binary = self.work/self.args.server.name
        shutil.copy2(self.args.server, binary)
        self.binary_sha256 = sha(binary.read_bytes())
        self.binary_mtime_ns = binary.stat().st_mtime_ns
        self.log = (self.work/'server.log').open('wb')
        self.process = subprocess.Popen([str(binary)], cwd=self.work, env=env, stdout=self.log, stderr=subprocess.STDOUT)
        end = time.monotonic()+90
        while time.monotonic() < end:
            assert self.process.poll() is None, 'server exited during startup; inspect redacted server.log'
            try:
                self.http.request('GET', '/health/live')
                break
            except (OSError, AssertionError):
                time.sleep(.2)
        else:
            raise AssertionError('server startup exceeded 90 seconds')
        self.http.request('GET', '/api/packages', expect=(401,))
        self.http.request('POST', '/api/login', dict(username='admin', password=self.secrets[0]))
        for name in ('acceptance', 'portable', 'other-context'):
            self.http.request('POST', '/api/packages', dict(id=name, name='Local synthetic acceptance', targets=dict(android=dict(packages=['*']))), expect=(201,))
        assert self.http.request('GET', '/api/devices') in ([], {'devices': []}), 'isolated server must have no devices'
        return dict(loopback=self.http.base, server_sha256=self.binary_sha256,
                    executed_binary_mtime_ns=self.binary_mtime_ns, build_label=self.args.build_label,
                    executed_isolated_copy=True)

    def install(self, plugin):
        explicit = dict(item.split('=', 1) for item in self.args.plugin_archive)
        source = self.args.repo/'plugins'/plugin/'manifest.toml'
        manifest = tomllib.loads(source.read_text(encoding='utf-8-sig'))
        path = Path(explicit.get(plugin, self.args.repo/'web/public/plugins'/f'{plugin}-{manifest["version"]}.gplugin'))
        if self.args.yaml_component and plugin not in explicit:
            # Same unsigned archive shape as the official packer. The production
            # installer still performs its complete manifest/component/UI checks.
            assets = {p.relative_to(source.parent/'dist/ui').as_posix(): p.read_bytes()
                      for p in sorted((source.parent/'dist/ui').rglob('*')) if p.is_file()}
            assert 'plugin.js' in assets, f'{plugin}: build UI first; ui/plugin.js missing'
            path = self.work/(plugin+'.gplugin')
            with zipfile.ZipFile(path, 'w', compression=zipfile.ZIP_DEFLATED) as bundle:
                bundle.writestr('manifest.toml', source.read_bytes())
                if manifest.get('entry'):
                    assert plugin == 'gamer-yaml', 'only the YAML component is supplied'
                    component = self.args.yaml_component.read_bytes()
                    assert component.startswith(b'\0asm'), 'expected a freshly componentized WASM file'
                    bundle.writestr(manifest['entry'], component)
                for name, asset in assets.items():
                    bundle.writestr('ui/'+name, asset)
        assert path.is_file(), f'fresh plugin archive required: {path}; pass --plugin-archive {plugin}=PATH or --yaml-component PATH'
        body = path.read_bytes()
        with zipfile.ZipFile(io.BytesIO(body)) as bundle:
            packaged = bundle.read('manifest.toml')
            assert tomllib.loads(packaged.decode('utf-8-sig')) == manifest, f'{plugin}: archive manifest differs from current source'
            wasm_hash = sha(bundle.read(manifest['entry'])) if manifest.get('entry') else None
        result = self.http.request('POST', '/api/extensions', body, headers={'X-Gamer-Extension-Source': 'official', 'X-Gamer-Permission-Confirm': 'true'}, expect=(200, 201))
        if result.get('state') != 'running':
            result = self.http.request('POST', f'/api/extensions/{plugin}/enable')
        assert result.get('state') == 'running', result
        detail = dict(plugin=plugin, version=manifest['version'], archive_sha256=sha(body), wasm_sha256=wasm_hash)
        if plugin == 'gamer-yaml':
            before = self.http.call('settings.get')['values']
            self.replay_settings = dict(before)
            assert self.replay_settings == dict(default_timeout_secs=10, before_click_ms=300, after_click_ms=300), before
            assert self.http.call('settings.get')['values'] == self.replay_settings
            detail.update(settings_before=before, fixture_production_settings=self.replay_settings)
        return detail

    def resource(self, plugin, path, package=None):
        return f'/api/packages/{package or self.package}/plugins/{plugin}/resources/' + urllib.parse.quote(path, safe='/')

    def create_recording_sample(self):
        ffmpeg = shutil.which('ffmpeg')
        assert ffmpeg, 'ffmpeg is required for production media extraction smoke test'
        clip = self.work/'synthetic-recording.mp4'
        subprocess.run([ffmpeg, '-nostdin', '-y', '-v', 'error', '-f', 'lavfi', '-i',
                        'testsrc2=size=64x48:rate=5:duration=2', '-c:v', 'libx264', '-pix_fmt', 'yuv420p',
                        str(clip)], check=True, timeout=30, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        imported = self.http.request('POST', '/api/media/import?name=synthetic-recording.mp4', clip.read_bytes(), expect=(201,))
        media_id = imported['id']
        frames = self.http.request('GET', f'/api/media/{media_id}/frames?pts_us=0')
        end_us = frames['last_pts_us']
        assert end_us > 1000000 and frames['current']['pts_us'] == 0, frames
        recording = self.work/'data'/'media'/media_id/'recording'
        recording.mkdir()
        session_id = 'local-synthetic-recording'
        session = dict(id=session_id, device_id='synthetic-no-device', state='completed',
                       started_at='2026-01-01T00:00:00Z', ended_at='2026-01-01T00:00:02Z',
                       segments=[dict(media_id=media_id, start_us=0, duration_us=end_us, base_pts_us=90000000, reason='normal')],
                       event_count=1, evidence_issues=[], error=None)
        event = dict(schema_version=1, event_id='synthetic-tap', operation_id='synthetic-tap', session_id=session_id,
                     source='manual', kind='tap', timeline_us=250000, time_domain='recording',
                     coordinate_space='device-display', display_size=dict(width=64, height=48),
                     payload=dict(x=25, y=20, duration_us=1000), status='accepted')
        (recording/'session.json').write_bytes(encoded(session))
        (recording/'events-0001.jsonl').write_bytes(encoded(event)+b'\n')
        result = self.http.call('sample.create', dict(package_id=self.package, recording_id=session_id,
                                name='Synthetic recorded MP4; no physical capture', start_us=0, end_us=end_us,
                                goal=dict(description='synthetic END frame confirmed for format test', confirmed=True),
                                include_clips=False), plugin='gamer-video')
        manifest = result['manifest']
        assert manifest['status'] == 'complete' and not manifest['diagnostics'], manifest
        assert manifest['segments'][0]['base_pts_us'] == 90000000
        assert all(f['timeline_us'] == f['pts_us'] for f in manifest['frames']), manifest
        assert len(manifest['windows'][0]['after_frame_ids']) >= 2
        assert manifest['actions'] == [event]
        exported = self.http.request('GET', self.resource('gamer-video', result['path']), raw=True)
        self.http.request('PUT', self.resource('gamer-video', result['path'], 'portable'), exported, expect=(200, 201))
        with_clips = self.http.call('sample.create', dict(package_id=self.package, recording_id=session_id,
                                name='Synthetic MP4 with original clip included', start_us=0, end_us=end_us,
                                goal=dict(description='synthetic END frame confirmed for format test', confirmed=True),
                                include_clips=True), plugin='gamer-video')
        clip_export = self.http.request('GET', self.resource('gamer-video', with_clips['path']), raw=True)
        with zipfile.ZipFile(io.BytesIO(clip_export)) as packed:
            clip_paths = [name for name in packed.namelist() if name.startswith('clips/')]
            assert len(clip_paths) == 1 and packed.read(clip_paths[0]) == clip.read_bytes()
        assert with_clips['manifest']['status'] == 'complete', with_clips
        shutil.rmtree(recording)  # Only this harness's explicitly seeded synthetic recording.
        copied = self.http.call('sample.read', dict(package_id='portable', sample_id=manifest['id']), plugin='gamer-video')
        assert copied['manifest'] == manifest
        return dict(frames=len(manifest['frames']), end_us=end_us, original_capture_base_pts_us=90000000,
                    archive_sha256=sha(exported), clip_archive_sha256=sha(clip_export),
                    recording_removed_before_portable_read=True)

    def samples(self):
        self.bundles = {}
        for name, popup, shifted, bad_end in [('sample-a', True, False, False), ('sample-b', False, False, False), ('sample-c', True, True, False), ('sample-negative', True, False, True)]:
            manifest, files = fixture(name, popup, shifted, bad_end)
            body = archive(manifest, files)
            self.bundles[name] = (manifest, files, body)
            (self.work/f'{name}.gamersample').write_bytes(body)
            self.http.request('PUT', self.resource('gamer-video', f'samples/{name}.gamersample'), body, expect=(200, 201))
        first = self.bundles['sample-a']
        got = self.http.call('sample.read', dict(package_id=self.package, sample_id='sample-a'), plugin='gamer-video')
        assert got['manifest'] == first[0], got
        exported = self.http.request('GET', self.resource('gamer-video', 'samples/sample-a.gamersample'), raw=True)
        assert exported == first[2]
        self.http.request('PUT', self.resource('gamer-video', 'samples/sample-a.gamersample', 'portable'), exported, expect=(200, 201))
        copied = self.http.call('sample.read', dict(package_id='portable', sample_id='sample-a'), plugin='gamer-video')
        assert copied == got
        files = dict(first[1])
        files['frames/f0.png'] += b'tampered'
        self.http.request('PUT', self.resource('gamer-video', 'samples/sample-a.gamersample', 'other-context'), archive(first[0], files), expect=(400, 409, 422))
        self.http.request('PUT', self.resource('gamer-video', 'samples/renamed.gamersample', 'other-context'), first[2], expect=(400, 409, 422))
        self.http.request('PUT', self.resource('gamer-video', 'samples/sample-a.gamersample', 'other-context'), dict(content='not a portable archive'), expect=(400, 409, 422))
        self.http.request('GET', self.resource('gamer-video', 'samples/sample-a.gamersample', 'other-context'), expect=(404,))
        return dict(samples=4, portable_sha256=sha(exported), original_media_directory='never created')

    def create(self, name, yaml=YAML, templates=None, samples=None, ai=False, limits=None):
        values = dict(package_id=self.package, name=name+'.yaml', goal='done pattern visible', samples=samples or SELECTED,
                      yaml=yaml, templates=CROPS if templates is None else templates)
        if limits:
            values['limits'] = limits
        return self.http.call('generation.start' if ai else 'generation.create', values)['candidate']

    def ccall(self, action, candidate, **kwargs):
        return self.http.call('generation.'+action, dict(package_id=candidate['package_id'], candidate_id=candidate['id'], **kwargs))

    def wait(self, candidate, timeout=30):
        deadline = time.monotonic()+timeout
        while time.monotonic() < deadline:
            result = self.ccall('get', candidate)['candidate']
            if result['state'] not in ('generating', 'validating'):
                return result
            time.sleep(.04)
        raise AssertionError('candidate did not reach terminal state within bounded HTTP test')

    def validate(self, candidate):
        self.ccall('validate', candidate)
        return self.wait(candidate)

    def no_ai(self):
        ready = self.http.call('generation.readiness')
        assert ready['ready'] is False, ready
        self.good = self.validate(self.create('manual-good'))
        assert self.good['state'] == 'passed', self.good
        report = self.good['report']
        assert [s['consumed_actions'] for s in report['samples']] == [2, 1, 2], report
        assert all(any(e.get('ev') == 'replay_vision' for e in s['events']) for s in report['samples'])
        again = self.validate(self.good)
        assert again['report'] == report, 'replay must be deterministic'
        self.good = again
        self.http.call('generation.start', dict(package_id=self.package, name='no-model.yaml', goal='done', samples=SELECTED), expect=(400, 409, 422))
        source = self.http.call('automation.validate_source', dict(package_id=self.package, yaml=YAML))
        assert source['valid'] is True, source
        return dict(status=report['status'], consumed_actions=[s['consumed_actions'] for s in report['samples']], fingerprint=report['candidate_sha256'])

    def clip_replay(self):
        """Actual archive video frames, production import/extraction and runtime replay."""
        ffmpeg = shutil.which('ffmpeg')
        assert ffmpeg, 'ffmpeg required for clip-backed replay acceptance'
        directory = self.work/'clip-replay-source'
        directory.mkdir()
        for i in range(21):
            seed = 1 if i < 4 else 2 if i < 9 else 3
            (directory/f'{i:03d}.png').write_bytes(screen(seed))
        clip = directory/'demonstration.mp4'
        subprocess.run([ffmpeg, '-nostdin', '-y', '-v', 'error', '-framerate', '20',
                        '-i', str(directory/'%03d.png'), '-c:v', 'libx264rgb', '-crf', '0',
                        '-preset', 'ultrafast', '-pix_fmt', 'rgb24', str(clip)], check=True,
                       timeout=30, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        clip_bytes = clip.read_bytes()
        imported = self.http.request('POST', '/api/media/import?name=clip-replay.mp4', clip_bytes, expect=(201,))
        media_id = imported['id']
        table = self.http.request('GET', f'/api/media/{media_id}/frames?pts_us=0')
        assert table['frame_count'] == 21 and table['last_pts_us'] == 1000000, table
        assert imported['sha256'] == sha(clip_bytes), imported
        manifest, _ = fixture('clip-sample')
        files = {}
        for frame in manifest['frames']:
            index = frame['timeline_us'] // 50000
            position = self.http.request('GET', f'/api/media/{media_id}/frames?pts_us={frame["timeline_us"]}')['current']
            assert position == dict(index=index, pts_us=frame['timeline_us']), position
            image = self.http.request('GET', f'/api/media/{media_id}/frame?index={index}', raw=True)
            frame.update(media_id=media_id, media_sha256=imported['sha256'], frame_index=index, sha256=sha(image))
            files[frame['path']] = image
        clip_path = f'clips/{media_id}.mp4'
        files[clip_path] = clip_bytes
        manifest['segments'] = [dict(media_id=media_id, start_us=0, duration_us=imported['duration_us'],
                                     base_pts_us=90000000, clip_path=clip_path, clip_sha256=sha(clip_bytes))]
        manifest['files'] = [dict(path=p, size=len(b), sha256=sha(b)) for p, b in sorted(files.items())]
        manifest['content_sha256'] = ''
        manifest['content_sha256'] = sha(encoded(manifest))
        bundle = archive(manifest, files)
        (self.work/'clip-sample.gamersample').write_bytes(bundle)
        self.http.request('PUT', self.resource('gamer-video', 'samples/clip-sample.gamersample'), bundle, expect=(200, 201))
        # Remove external source media and all fixture source frames. The archive
        # must supply both selected PNGs and frames extracted during replay.
        shutil.rmtree(self.work/'data'/'media'/media_id)
        shutil.rmtree(directory)
        crops = [{**crop, 'sample_id': 'clip-sample'} for crop in CROPS]
        candidate = self.validate(self.create('clip-replay', templates=crops,
                                  samples=[dict(sample_id='clip-sample', plugin_id='gamer-video')]))
        assert candidate['state'] == 'passed', candidate
        sample = candidate['report']['samples'][0]
        clip_events = [e for e in sample['events'] if e.get('ev') == 'replay_vision' and
                       e.get('image_frame_id', '').startswith('clip:')]
        assert clip_events, 'No runtime frame came from archived video; PNG-only replay is insufficient for this test'
        anchors = {f['timeline_us'] for f in manifest['frames']}
        assert any(e['timeline_us'] not in anchors for e in clip_events), clip_events
        assert all(e['image_frame_id'].startswith(f'clip:{media_id}:') for e in clip_events), clip_events
        assert sample['consumed_actions'] == 2, sample
        again = self.validate(candidate)
        assert again['report'] == candidate['report'], 'clip replay must be deterministic'
        contradiction = copy.deepcopy(manifest)
        contradiction['id'] = 'clip-contradiction'
        contradictory_files = dict(files)
        first = contradiction['frames'][0]
        contradictory_files[first['path']] = files[contradiction['frames'][2]['path']]
        first['sha256'] = sha(contradictory_files[first['path']])
        contradiction['files'] = [dict(path=p, size=len(b), sha256=sha(b)) for p, b in sorted(contradictory_files.items())]
        contradiction['content_sha256'] = ''
        contradiction['content_sha256'] = sha(encoded(contradiction))
        self.http.request('PUT', self.resource('gamer-video', 'samples/clip-contradiction.gamersample'),
                          archive(contradiction, contradictory_files), expect=(200, 201))
        bad_crops = [{**crop, 'sample_id': 'clip-contradiction'} for crop in CROPS]
        self.negative('clip-contradiction', templates=bad_crops,
                      samples=[dict(sample_id='clip-contradiction', plugin_id='gamer-video')],
                      expected=['insufficient_evidence'], message_contains='EVIDENCE_ANCHOR_PIXELS')
        return dict(state=candidate['state'], consumed_actions=sample['consumed_actions'],
                    contradictory_png_rejected=True,
                    archived_video_sha256=sha(clip_bytes), runtime_clip_observations=len(clip_events),
                    runtime_clip_pts_us=sorted({e['timeline_us'] for e in clip_events}),
                    original_media_and_frames_removed=True)

    def without_video(self):
        self.http.request('POST', '/api/extensions/gamer-video/disable')
        try:
            candidate = self.validate(self.create('without-video'))
            assert candidate['state'] == 'passed', candidate
            return dict(state=candidate['state'], video_running=False)
        finally:
            restored = self.http.request('POST', '/api/extensions/gamer-video/enable')
            assert restored['state'] == 'running', restored

    def negative(self, name, yaml=YAML, templates=None, samples=None, expected=None, message_contains=None):
        candidate = self.validate(self.create(name, yaml, templates, samples))
        assert candidate['state'] == 'failed' and candidate['report']['status'] != 'passed', candidate
        report = candidate['report']
        assert len(report['samples']) == len(samples or SELECTED), report
        if expected:
            assert [s['status'] for s in report['samples']] == expected, report
        if message_contains:
            assert any(message_contains in d['message'] for sample in report['samples'] for d in sample['diagnostics']), report
        self.http.call('generation.save', dict(package_id=self.package, candidate_id=candidate['id'], expected_revision=candidate['revision'],
                                              expected_version=candidate['base_version'], mode='validated'), expect=(400, 409, 422))
        self.ccall('save', candidate, expected_revision=candidate['revision'], mode='draft')
        self.http.request('GET', self.resource('gamer-yaml', 'automations/'+name+'.yaml'), expect=(404,))
        return dict(status=report['status'], samples=[dict(sample=s['sample_id'], status=s['status'], diagnostics=s['diagnostics']) for s in report['samples']])

    def revisions(self):
        candidate = self.good
        self.http.call('generation.edit', dict(package_id=self.package, candidate_id=candidate['id'], expected_revision=0, yaml=YAML), expect=(400, 409, 422))
        edited = self.ccall('edit', candidate, expected_revision=candidate['revision'], yaml=YAML+'# changed\n')['candidate']
        assert edited['report'] is None and edited['revision'] > candidate['revision']
        self.http.call('generation.save', dict(package_id=self.package, candidate_id=edited['id'], expected_revision=edited['revision'], expected_version=edited['base_version'], mode='validated'), expect=(400, 409, 422))
        valid = self.validate(edited)
        assert valid['state'] == 'passed', valid
        changed_settings = {**self.replay_settings, 'after_click_ms': self.replay_settings['after_click_ms'] + 1}
        self.http.call('settings.save', dict(settings=changed_settings, expected=self.replay_settings))
        self.http.call('generation.save', dict(package_id=self.package, candidate_id=valid['id'],
                        expected_revision=valid['revision'], expected_version=valid['base_version'], mode='validated'), expect=(400, 409, 422))
        self.http.call('settings.save', dict(settings=self.replay_settings, expected=changed_settings))
        valid = self.validate(valid)
        assert valid['state'] == 'passed', valid
        saved = self.ccall('save', valid, expected_revision=valid['revision'], expected_version=valid['base_version'], mode='validated')
        assert saved['candidate']['state'] == 'saved'
        self.http.request('GET', self.resource('gamer-yaml', 'automations/manual-good.yaml'))
        history = self.http.call('generation.history', dict(package_id=self.package))
        assert any(r['id'] == saved['revision']['id'] for r in history['revisions'])
        rollback = self.http.call('generation.rollback', dict(package_id=self.package, revision_id=saved['revision']['id'], expected_version=history['version']))
        assert rollback['version'] == saved['revision']['previous_version'], rollback
        self.http.request('GET', self.resource('gamer-yaml', 'automations/manual-good.yaml'), expect=(404,))
        self.http.request('GET', self.resource('gamer-yaml', 'templates/claim.png'), expect=(404,))
        # A contemporaneous ordinary resource edit invalidates the staged report.
        conflict = self.validate(self.create('conflict'))
        self.http.request('PUT', self.resource('gamer-yaml', 'automations/concurrent.yaml'), dict(content=YAML), expect=(200, 201))
        conflict = self.ccall('get', conflict)['candidate']
        assert conflict['state'] == 'draft' and conflict['report'] is None, conflict
        self.http.call('generation.save', dict(package_id=self.package, candidate_id=conflict['id'], expected_revision=conflict['revision'], expected_version=conflict['base_version'], mode='validated'), expect=(400, 409, 422))
        return dict(commit=saved['revision']['id'], rollback=rollback['revision']['id'], conflict_rejected=True, production_settings_change_rejected=True)

    def model_setup(self):
        self.stub = Stub()
        self.install('gamer-ai')
        settings = self.http.call('settings.get', plugin='gamer-ai')
        self.settings = self.http.call('settings.save', dict(expected_version=settings.get('version'), base_url=self.stub.url,
                                  model='local-deterministic-protocol-stub', protocol='responses', request_timeout_secs=30,
                                  api_key=self.secrets[1]), plugin='gamer-ai')
        assert self.http.call('generation.readiness')['ready'] is False, 'unprobed settings must not be ready'
        self.settings = self.http.call('connection.probe', plugin='gamer-ai')
        assert self.settings['probe']['ok'] is True, self.settings
        assert self.http.call('generation.readiness')['ready'] is True
        return dict(provider='deterministic loopback protocol stub; NOT real model quality', checks=self.settings['probe']['checks'])

    def generation(self):
        result = self.wait(self.create('generated', yaml='', templates=[], ai=True))
        assert result['state'] == 'passed' and result['attempts'] == 1, result
        assert result['known_tokens'] == 16 and not result['unknown_usage'], result
        self.generated_candidate = result
        clip_status = 'explicitly skipped'
        if not self.args.skip_clip_replay:
            clip = self.wait(self.create('generated-clip', yaml='', templates=[], ai=True,
                             samples=[dict(sample_id='clip-sample', plugin_id='gamer-video')]), timeout=60)
            assert clip['state'] == 'passed' and clip['attempts'] == 1, clip
            assert any(e.get('image_frame_id', '').startswith('clip:') for e in clip['report']['samples'][0]['events'])
            clip_status = clip['state']
        self.stub.mode = 'bad_action'
        failed = self.wait(self.create('bounded-retry', yaml='', templates=[], ai=True,
                                     limits=dict(max_attempts=2, max_seconds=30, max_tokens=200000)))
        assert failed['state'] == 'failed' and failed['attempts'] == 2 and failed['known_tokens'] == 32, failed
        assert failed['reason'] in ('no_progress', 'attempt_budget'), failed
        before_budget_check = len(self.stub.requests)
        budget = self.wait(self.create('tiny-token-budget', yaml='', templates=[], ai=True,
                           limits=dict(max_attempts=3, max_seconds=30, max_tokens=2048)))
        assert budget['state'] == 'failed' and 'generation_token_budget' in (budget['reason'] or ''), budget
        assert budget['known_tokens'] == 0 and len(self.stub.requests) == before_budget_check, budget
        self.stub.mode = 'drop_sample'
        immutable = self.wait(self.create('reject-sample-deletion', yaml='', templates=[], ai=True))
        assert immutable['state'] == 'failed' and immutable['report'] is None, immutable
        assert immutable['sample_ids'] == ['sample-a', 'sample-b', 'sample-c'], immutable
        self.stub.mode = 'valid'
        self.generated_candidate = result
        return dict(generation=result['state'], clip_generation=clip_status, retry_attempts=failed['attempts'],
                    bounded_reason=failed['reason'], token_budget_stopped_before_request=True, attempted_sample_deletion_rejected=True)

    def held_out(self):
        """Separate synthetic D; no model request and no production schema change."""
        original = self.generated_candidate
        model_requests_before = len(self.stub.requests)
        manifest, files = fixture('sample-d', popup=False, origin=(8, 29))
        body = archive(manifest, files)
        (self.work/'sample-d.gamersample').write_bytes(body)
        self.http.request('PUT', self.resource('gamer-video', 'samples/sample-d.gamersample'), body, expect=(200, 201))
        # Existing crop API requires its source sample to remain selected. Reuse
        # that immutable source for byte-identical crops; report D separately.
        crop_sources = sorted({crop['sample_id'] for crop in original['templates']})
        assert crop_sources and 'sample-d' not in crop_sources
        samples = [dict(sample_id=name, plugin_id='gamer-video') for name in crop_sources + ['sample-d']]
        held = self.create('held-out-d', yaml=original['yaml'], templates=original['templates'], samples=samples)
        hashes = {}
        for crop in original['templates']:
            first = self.ccall('template', original, name=crop['name'])
            second = self.ccall('template', held, name=crop['name'])
            assert first['base64'] == second['base64'], 'held-out validation must reuse exact generated template bytes'
            hashes[crop['name']] = sha(base64.b64decode(first['base64'], validate=True))
        held = self.validate(held)
        assert held['state'] == 'passed' and held['yaml'] == original['yaml'], held
        report = next(sample for sample in held['report']['samples'] if sample['sample_id'] == 'sample-d')
        assert report['status'] == 'passed' and report['consumed_actions'] == 1, report
        assert len(self.stub.requests) == model_requests_before, 'held-out evaluation must not call the provider'
        assert all('sample-d' not in request.get('sample_ids', []) for request in self.stub.requests)
        assert self.ccall('get', original)['candidate']['sample_ids'] == ['sample-a', 'sample-b', 'sample-c']
        return dict(held_out_sample='sample-d', status=report['status'], consumed_actions=report['consumed_actions'],
                    sent_to_provider=False, generated_selection_unchanged=True, crop_source_samples=crop_sources,
                    exact_template_sha256s=hashes, interpretation='synthetic replay coverage only; not real-model generalization')

    def context_change(self):
        self.stub.hold()
        candidate = self.create('context-held', yaml='', templates=[], ai=True)
        assert self.stub.started.wait(5)
        viewed = self.http.call('generation.list', dict(package_id='other-context'))
        assert viewed['candidates'] == []
        self.stub.release.set()
        result = self.wait(candidate)
        assert result['state'] == 'passed' and result['package_id'] == self.package, result
        assert self.http.call('generation.list', dict(package_id='other-context'))['candidates'] == []
        self.http.request('GET', self.resource('gamer-yaml', 'automations/context-held.yaml', 'other-context'), expect=(404,))
        return dict(result_package=result['package_id'], selected_other_package_unmodified=True)

    def cancellation(self):
        self.stub.hold()
        candidate = self.create('cancel-held', yaml='', templates=[], ai=True)
        assert self.stub.started.wait(5), 'generation never reached local provider'
        cancelled = self.ccall('cancel', candidate)['candidate']
        assert cancelled['state'] == 'cancelled'
        self.stub.release.set()
        time.sleep(.3)
        after = self.ccall('get', candidate)['candidate']
        assert after['state'] == 'cancelled' and after['report'] is None and after['yaml'] == '', after
        assert after['unknown_usage'] is True, 'cancellation after provider submission must not imply zero unreported cost'
        requests_before = len(self.stub.requests)
        drain_deadline = time.monotonic() + 5
        while True:
            retry = self.http.call('generation.retry', dict(package_id=self.package, candidate_id=after['id']), expect=(200, 400, 409, 422))
            if 'candidate_busy' not in str(retry) or time.monotonic() >= drain_deadline:
                break
            time.sleep(.05)  # Cancellation publishes intent before bounded worker drain.
        if 'candidate' in retry:
            blocked = self.wait(after)
            assert blocked['state'] == 'failed' and 'usage_unknown' in (blocked['reason'] or ''), blocked
        else:
            assert 'usage_unknown' in str(retry), retry
            blocked = self.ccall('get', after)['candidate']
        assert len(self.stub.requests) == requests_before, 'unknown-cost retry must not send another provider request'
        manual = self.ccall('edit', blocked, expected_revision=blocked['revision'], yaml=YAML, templates=CROPS)['candidate']
        manual = self.validate(manual)
        assert manual['state'] == 'passed', manual
        other = self.http.call('generation.list', dict(package_id='other-context'))
        assert other['candidates'] == [], 'candidate leaked across package context'
        return dict(state=after['state'], cross_package_candidates=len(other['candidates']),
                    unknown_usage=True, automatic_retry_blocked_without_request=True, manual_validation=manual['state'])

    def config_change(self):
        self.stub.hold()
        candidate = self.create('changed-model', yaml='', templates=[], ai=True)
        assert self.stub.started.wait(5)
        current = self.http.call('settings.get', plugin='gamer-ai')
        self.http.call('settings.save', dict(expected_version=current['version'], base_url=self.stub.url,
                        model='changed-local-protocol-stub', protocol='responses', request_timeout_secs=30), plugin='gamer-ai')
        assert self.http.call('generation.readiness')['ready'] is False
        self.stub.release.set()
        result = self.wait(candidate)
        assert result['state'] == 'failed' and result['report'] is None and result['yaml'] == '', result
        return dict(state=result['state'], reason=result['reason'], re_probe_required=True)

    def disabled_ai(self):
        self.http.call('connection.probe', plugin='gamer-ai')
        assert self.http.call('generation.readiness')['ready'] is True
        self.stub.hold()
        candidate = self.create('disabled-model', yaml='', templates=[], ai=True)
        assert self.stub.started.wait(5)
        self.http.request('POST', '/api/extensions/gamer-ai/disable')
        assert self.http.call('generation.readiness')['ready'] is False
        self.stub.release.set()
        result = self.wait(candidate)
        assert result['state'] in ('failed', 'cancelled') and result['report'] is None and result['yaml'] == '', result
        manual = self.validate(self.create('after-ai-disabled'))
        assert manual['state'] == 'passed', manual
        return dict(late_result_state=result['state'], ordinary_validation=manual['state'])

    def trace_routes(self):
        no_auth = HTTP(self.http.base)
        no_auth.request('GET', '/api/runs/not-a-run/trace', expect=(401,))
        self.http.request('GET', '/api/runs/not-a-run/trace', expect=(404,))
        self.http.request('GET', '/api/runs/not-a-run/trace/images/not-an-image', expect=(400, 404, 410))
        self.http.request('POST', '/api/runs/not-a-run/trace/retain', expect=(409,))
        return 'auth and absent-evidence behavior only; real-target image capture deferred'

    def close(self):
        if self.stub:
            self.stub.close()
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if self.log:
            self.log.close()
            path = self.work/'server.log'
            path.write_text(self.redact(path.read_text(encoding='utf-8', errors='replace')), encoding='utf-8')


def main():
    if not __debug__:
        raise SystemExit('Run without -O/PYTHONOPTIMIZE: acceptance assertions must be enabled')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--build-label', default='prebuilt binary supplied by caller; source identity not inferred', help='describe exact build snapshot')
    parser.add_argument('--server', type=Path, required=True, help='already-built gamer-server executable')
    parser.add_argument('--plugin-archive', action='append', default=[], metavar='ID=PATH', help='fresh archive; manifest must exactly match source')
    parser.add_argument('--output', type=Path, help='new, empty output directory; defaults to a temporary directory')
    parser.add_argument('--yaml-component', type=Path, help='freshly built YAML component; package current manifests/UI locally')
    parser.add_argument('--serve-seconds', type=int, default=0, help='keep isolated server for browser review, at most 1800s; browser.done stops early')
    parser.add_argument('--skip-clip-replay', action='store_true', help='explicitly skip clip-backed runtime replay (partial acceptance)')
    parser.add_argument('--skip-model-stub', action='store_true', help='explicitly skip local protocol-provider cases')
    args = parser.parse_args()
    args.repo, args.server = args.repo.resolve(), args.server.resolve()
    assert 0 <= args.serve_seconds <= 1800, '--serve-seconds must be 0..1800'
    assert args.server.is_file(), f'build server first: {args.server}'
    if args.output:
        work = args.output.resolve()
        work.mkdir(parents=True, exist_ok=True)
        assert not any(work.iterdir()), '--output must be empty to protect existing data'
    else:
        work = Path(tempfile.mkdtemp(prefix='gamer-ai-acceptance-'))
    os.chmod(work, 0o700)
    harness = Harness(args, work)
    try:
        if not harness.check('isolated server, real authentication, no devices', harness.start):
            return 1
        if not harness.check('install current YAML plugin', lambda: harness.install('gamer-yaml')):
            return 1
        if not harness.check('install current video plugin', lambda: harness.install('gamer-video')):
            return 1
        harness.check('real MP4 extraction and portable sample.create', harness.create_recording_sample)
        if not harness.check('portable samples, production import parser, tamper rejection', harness.samples):
            return 1
        manual = harness.check('no-AI editor and A/C-confirm B-absent real replay', harness.no_ai)
        if manual:
            if not args.skip_clip_replay:
                harness.check('archived clip supplies unlisted runtime frames with no source media', harness.clip_replay)
            harness.check('portable replay works while video plugin is disabled', harness.without_video)
            harness.check('wrong action cannot pass or save', lambda: harness.negative('wrong-action', YAML.replace('tap: claim', 'tap: [0.9, 0.9]')))
            harness.check('missing template cannot pass or save', lambda: harness.negative('missing-template', templates=CROPS[1:]))
            wrong = copy.deepcopy(CROPS)
            wrong[2]['frame_id'] = 'f0'
            harness.check('wrong goal template cannot pass or save', lambda: harness.negative('wrong-template', templates=wrong))
            harness.check('missing finish cannot pass or save', lambda: harness.negative('missing-finish', YAML.replace('  - finish: done', '  - return: true')))
            required = YAML.replace('  - optional: {find: confirm, timeout: 250ms, then: [{tap: confirm}]}', '  - wait: confirm\n    timeout: 250ms\n    then: [{tap: confirm}]')
            harness.check('all-sample gate retains B failure while A/C pass', lambda: harness.negative('required-confirm', required, expected=['passed', 'failed', 'passed']))
            negative = SELECTED + [dict(sample_id='sample-negative', plugin_id='gamer-video')]
            harness.check('negative END cannot be hidden by earlier success', lambda: harness.negative('negative-end', YAML.replace('  - finish: done', '  - finish: done\n    interval: 50ms'), samples=negative, expected=['passed', 'passed', 'passed', 'failed'], message_contains='GOAL_END_MISMATCH'))
            harness.check('revision stale report, atomic save and joint rollback', harness.revisions)
        harness.check('trace HTTP authorization and absent evidence', harness.trace_routes)
        if not args.skip_model_stub and manual:
            if harness.check('local-only AI protocol probe and readiness', harness.model_setup):
                harness.check('AI generation and bounded correction use real validation', harness.generation)
                harness.check('held-out D uses identical generated script and templates without provider exposure', harness.held_out)
                harness.check('context change keeps asynchronous result in original package', harness.context_change)
                harness.check('cancelled late result cannot write or cross packages', harness.cancellation)
                harness.check('configuration change rejects late model result', harness.config_change)
                harness.check('AI disable rejects late result; ordinary validation still works', harness.disabled_ai)
        if args.serve_seconds:
            # Local-only handoff file is never included in report or console.
            access = work/'browser-access.json'
            fd = os.open(access, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, 'wb') as stream:
                stream.write(encoded(dict(base_url=harness.http.base, username='admin', password=harness.secrets[0])))
            (work/'checks-before-browser.json').write_text(harness.redact(json.dumps(harness.results, ensure_ascii=False, indent=2)), encoding='utf-8')
            print(f'Browser review server ready; private local access file: {access}; create {work / "browser.done"} to stop', flush=True)
            deadline = time.monotonic() + args.serve_seconds
            try:
                while time.monotonic() < deadline and not (work/'browser.done').exists():
                    assert harness.process.poll() is None, 'browser review server exited'
                    time.sleep(.5)
            finally:
                access.unlink(missing_ok=True)
    finally:
        harness.close()
        report = dict(schema_version=1, generated_at=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
                      scope='isolated local HTTP integration; real server/router/auth/plugins/parser/interpreter/NCC; synthetic evidence and deterministic protocol provider',
                      results=harness.results, model_stub_skipped=args.skip_model_stub, clip_replay_skipped=args.skip_clip_replay,
                      model_stub_requests=harness.stub.requests if harness.stub else [],
                      deferred=['real Android/CDP recording and coordinate alignment', 'actual game input and lifecycle', 'real model image understanding, generation quality and cost', 'live-target runtime Trace image capture', 'browser/UI acceptance (separate supported browser session)'])
        (work/'report.json').write_text(harness.redact(json.dumps(report, ensure_ascii=False, indent=2))+'\n', encoding='utf-8')
        passed = sum(r['status'] == 'passed' for r in harness.results)
        print(f'\n{passed}/{len(harness.results)} checks passed; report: {work / "report.json"}', flush=True)
    return 0 if all(r['status'] == 'passed' for r in harness.results) else 1


if __name__ == '__main__':
    raise SystemExit(main())
