// Isolated host acceptance. Never run this against the user's live server.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { resolve, join } from 'node:path';

const qa = resolve(process.env.GAMER_AUDIENCE_QA_DIR || 'server/target/live-shared-address-20261015');
const base = process.env.GAMER_AUDIENCE_QA_BASE || 'http://127.0.0.1:18551';
const url = new URL(base);
assert.equal(url.hostname, '127.0.0.1');
assert(url.port && !['8443', '5173'].includes(url.port), 'Use a separate local test host');
const token = (await readFile(join(qa, 'admin-token'), 'utf8')).trim();
const archive = await readFile(join(qa, 'plugins/gamer-live-0.2.11.gplugin'));
const sha = value => createHash('sha256').update(value).digest('hex');
async function checkGeneratedPage(address, expectedText) {
  const require = createRequire(import.meta.url);
  const { chromium } = require('C:/Users/Jeson/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright');
  const browser = await chromium.launch({ executablePath: 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true, args: ['--disable-background-networking', '--disable-component-update', '--no-first-run'] });
  try {
    const page = await browser.newPage({ viewport: { width: 360, height: 540 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/*', route => {
      const requested = new URL(route.request().url());
      return requested.origin === new URL(address).origin ? route.continue() : route.abort();
    });
    await page.goto(address);
    await page.waitForFunction(expected => document.querySelector('#custom-text')?.textContent === expected, expectedText, { timeout: 8000 });
    assert.equal(await page.locator('#activity').textContent(), '空闲中');
    assert.equal(new URL(page.url()).hash, '');
    assert.equal(await page.locator('img').count(), 0);
    assert.deepEqual(errors, [], 'The actual CSP-protected page must run without script errors');
    await page.screenshot({ path: join(qa, 'actual-generated-page.png') });
  } finally { await browser.close(); }
}
async function api(path, body, options = {}) {
  const response = await fetch(base + path, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'X-Admin-Token': token, 'Content-Type': 'application/json', ...options.headers },
    body: body === undefined ? undefined : options.bytes || JSON.stringify(body),
    signal: AbortSignal.timeout(15000),
  });
  const value = await response.json();
  if (!options.allowFailure) assert(response.ok, `${path}: ${response.status}: ${JSON.stringify(value)}`);
  return { response, value };
}
const call = async (action, values = {}, options) => (await api('/api/extensions/gamer-live/call', { action, values }, options)).value;
const devices = (await api('/api/devices')).value;
const browsers = (await api('/api/browser-targets')).value;
assert.deepEqual(devices, [], 'No real execution targets in the isolated host');
assert.deepEqual(browsers, [], 'No unrelated browser sessions in the isolated host');
const extensions = (await api('/api/extensions')).value.extensions;
assert.equal(extensions.length, 0, 'Use fresh test data');
await api('/api/extensions', {}, { bytes: archive, headers: { 'Content-Type': 'application/octet-stream', 'X-Gamer-Permission-Confirm': 'true', 'X-Expected-Sha256': sha(archive) } });
const enabled = (await api('/api/extensions/gamer-live/enable', {})).value;
assert.equal(enabled.state, 'running');
assert.equal(enabled.active_version, '0.2.11');
const initial = await call('audience.read');
assert.equal(initial.enabled, false);
assert.equal(initial.text, '');
assert.equal(initial.version, null);
assert.equal(new URL(initial.url).origin, url.origin);
const localSources = (await call('live.status')).local_sources;
assert.equal(localSources.audience_url, initial.url);
assert.equal(new URL(localSources.stream_url).origin, url.origin);
assert.equal((await fetch(initial.url)).status, 403, 'A disabled page is not readable');
const text = '弹幕包含“日常”即可排队。\n请等待当前任务完成。';
const saved = await call('audience.save', { text, expected_version: initial.version });
assert.equal(saved.text, text);
assert(saved.version);
const opened = await call('audience.open');
assert(opened.enabled && opened.url);
const link = new URL(opened.url);
assert.equal(link.hostname, '127.0.0.1');
assert.equal(link.origin, url.origin, 'The audience page reuses the main HTTP listener');
assert.equal(link.hash, '');
const readToken = link.pathname.split('/')[4];
assert(readToken);
const stateUrl = new URL('state', link);
assert.equal((await call('audience.open')).url, opened.url, 'Enabling twice preserves the active address');
assert.equal((await call('audience.read')).url, opened.url, 'Panel remount restores the address');
const page = await fetch(link, { signal: AbortSignal.timeout(5000) });
assert.equal(page.status, 200);
assert.equal(page.headers.get('cache-control'), 'no-store');
const html = await page.text();
assert(html.includes('空闲中') && html.includes('任务中'));
assert(!html.includes('URLSearchParams'), 'Removed layout/font/row configuration');
const invalidUrl = new URL(stateUrl);
invalidUrl.pathname = invalidUrl.pathname.replace(readToken, 'invalid-read-key');
assert([403, 404].includes((await fetch(invalidUrl)).status));
assert.equal((await fetch(stateUrl, { headers: { Origin: 'https://example.com' } })).status, 403);
assert.equal((await fetch(stateUrl, { headers: { 'X-Forwarded-For': '127.0.0.1' } })).status, 403);
assert.equal((await fetch(stateUrl, { method: 'POST' })).status, 405);
const snapshot = await (await fetch(stateUrl)).json();
assert.equal(snapshot.text, text);
assert.equal(snapshot.current, null);
assert.equal(snapshot.featured, null);
assert.deepEqual(snapshot.waiting, []);
for (const forbidden of ['entrypoint', 'args', 'run_id', 'actor_id', 'device_id', 'secret', readToken]) {
  assert(!JSON.stringify(snapshot).includes(forbidden), `Public snapshot leaked ${forbidden}`);
}
const changedText = '欢迎排队\n<img src=x onerror=alert(1)> 按文本显示';
const changed = await call('audience.save', { text: changedText, expected_version: saved.version });
assert.equal((await (await fetch(stateUrl)).json()).text, changedText, 'Saved copy is live without a new address');
assert.equal(changed.url, opened.url);
await checkGeneratedPage(opened.url, changedText);
const conflict = await api('/api/extensions/gamer-live/call', { action: 'audience.save', values: { text: '不能覆盖', expected_version: saved.version } }, { allowFailure: true });
assert(!conflict.response.ok);
assert.equal((await call('audience.read')).text, changedText);
const oversize = await api('/api/extensions/gamer-live/call', { action: 'audience.save', values: { text: '字'.repeat(1001), expected_version: changed.version } }, { allowFailure: true });
assert(!oversize.response.ok);
const closed = await call('audience.close');
assert.equal(closed.enabled, false);
assert.equal(closed.url, opened.url, 'The fixed address remains available for copying while off');
assert.equal(closed.text, changedText);
let revoked = false;
try { revoked = !(await fetch(stateUrl, { signal: AbortSignal.timeout(3000) })).ok; } catch { revoked = true; }
assert(revoked, 'Closing revokes the old page');
const reopened = await call('audience.open');
assert.equal(reopened.url, opened.url, 'Re-enabling restores exactly the same saved URL');
assert.equal((await fetch(stateUrl)).status, 200);

// A generated video exercises the real FFmpeg publisher without devices.
const videoDirectory = join(qa, 'videos');
await (await import('node:fs/promises')).mkdir(videoDirectory, { recursive: true });
execFileSync('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=10', '-t', '6', '-c:v', 'libx264', '-preset', 'ultrafast', '-pix_fmt', 'yuv420p', join(videoDirectory, 'local-test.mp4')], { windowsHide: true });
const sleep = ms => new Promise(resolveSleep => setTimeout(resolveSleep, ms));
async function readManifest() {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    const response = await fetch(localSources.stream_url);
    if (response.ok) {
      const manifest = await response.text();
      if (manifest.includes('#EXTINF:')) return manifest;
    }
    await sleep(150);
  }
  throw new Error('The fixed HLS endpoint never became readable');
}
const streamChecks = [];
const pid = Number((await readFile(join(qa, 'server.pid'), 'utf8')).trim());
assert(Number.isInteger(pid) && pid > 0);
function candidateListeners() {
  const data = execFileSync('powershell.exe', ['-NoProfile', '-Command', `Get-NetTCPConnection -OwningProcess ${pid} -State Listen | Select-Object -ExpandProperty LocalPort | ConvertTo-Json -Compress`], { windowsHide: true, encoding: 'utf8' }).trim();
  const parsed = JSON.parse(data);
  return [...new Set(Array.isArray(parsed) ? parsed : [parsed])];
}
for (let iteration = 0; iteration < 2; iteration++) {
  const stream = await call('stream.start', { video_folder: videoDirectory, mode: 'local', audio: false, fps: 10, bitrate_kbps: 500 });
  assert.equal(stream.source_url, localSources.stream_url);
  const manifest = await readManifest();
  const segmentName = manifest.split(/\r?\n/).find(line => line.endsWith('.ts'));
  assert(segmentName);
  const segment = await fetch(new URL(segmentName, stream.source_url));
  assert.equal(segment.status, 200);
  assert.equal(segment.headers.get('content-type'), 'video/mp2t');
  assert((await segment.arrayBuffer()).byteLength > 188);
  assert.deepEqual(candidateListeners(), [Number(url.port)], 'Active video and feedback use only the main HTTP listener');
  streamChecks.push({ id: stream.id, first_segment: segmentName });
  await call('stream.stop');
  assert.equal((await fetch(localSources.stream_url)).status, 503);
  const segmentUnavailable = await fetch(new URL(segmentName, stream.source_url));
  assert(!segmentUnavailable.ok, 'A stopped output cannot serve stale media');
}
assert.notEqual(streamChecks[0].id, streamChecks[1].id);
assert.notEqual(streamChecks[0].first_segment, streamChecks[1].first_segment, 'Epoch segment names avoid stale media across restarts');
const listenerPorts = candidateListeners();
assert.deepEqual(listenerPorts, [Number(url.port)], 'Media and audience do not open additional listeners');
await api('/api/extensions/gamer-live/disable', {});
assert.equal((await fetch(link)).status, 503, 'A disabled plugin cannot serve its page');
assert.equal((await fetch(stateUrl)).status, 503, 'A disabled plugin cannot serve public state');
await api('/api/extensions/gamer-live/enable', {});
const restored = await call('audience.read');
assert.equal(restored.enabled, false);
assert.equal(restored.text, changedText);
assert.equal(restored.version, changed.version);
assert.equal(restored.url, opened.url);
assert.deepEqual((await call('live.status')).local_sources, localSources);
assert.equal((await call('audience.open')).url, opened.url);
assert.equal((await fetch(stateUrl)).status, 200);
await call('audience.close');
const result = {
  passed: true, kind: 'isolated_installed_plugin_api', base, plugin_version: enabled.active_version,
  archive_sha256: sha(archive), loopback_read_only_page: true, stable_address_while_enabled: true,
  panel_remount_recovers_state: true, live_copy_update: true, version_conflicts_preserve_copy: true,
  close_blocks_reading: true, fixed_address_survives_reenable: true, disable_closes_page: true,
  actual_generated_page_in_chrome: true,
  persisted_copy_survives_plugin_restart: true, real_platform: false, real_devices: false,
  fixed_hls_survives_output_restart: true, hls_real_segments: streamChecks, listener_ports: listenerPorts,
  both_sources_share_main_port: true, no_extra_listeners: true,
  notes: 'Public projections with active/terminal/waiting fixtures are checked by queue tests and browser visual QA.',
};
await writeFile(join(qa, 'api-result.json'), JSON.stringify(result, null, 2));
console.log(JSON.stringify(result, null, 2));
