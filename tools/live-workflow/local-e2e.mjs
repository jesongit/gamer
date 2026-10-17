// Full local workflow integration using recorded visual states and the actual
// installed YAML WASM runner. This is synthetic browser/queue evidence only.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir, readdir } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { dirname, join, resolve, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRecordedLoginFixture } from './recorded-login-fixture.mjs';

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const qa = resolve(process.env.GAMER_QA_DIR || join(repo, 'server/target/live-daily-qa'));
const base = (process.env.GAMER_QA_BASE || 'http://127.0.0.1:18544').replace(/\/$/, '');
const baseUrl = new URL(base);
assert(['127.0.0.1', 'localhost', '[::1]'].includes(baseUrl.hostname), 'QA requires loopback');
assert(baseUrl.protocol === 'http:' && baseUrl.port && !['8443', '5173'].includes(baseUrl.port), 'QA requires an isolated HTTP port');
const token = (await readFile(process.env.GAMER_QA_TOKEN_FILE || join(qa, 'admin-token'), 'utf8')).trim();
assert(token, 'Missing isolated admin token');
const headers = { 'X-Admin-Token': token };
const marker = randomUUID().slice(0, 8);
const targetId = `browser-live-daily-qa-${marker}`;
const evidenceDirectory = join(repo, 'server/data/hsr-supervision/live-materials-20261015');
const popupDirectory = join(repo, 'server/data/live-workflow-20261015/popup-later');
const staging = join(repo, 'server/data/live-workflow-20261015/staging');
const videoFolder = process.env.GAMER_QA_VIDEOS || join(qa, 'videos');
const onlyNotifications = process.env.GAMER_QA_ONLY_NOTIFICATIONS === '1';
const evidence = { kind: 'isolated_full_workflow_integration', target: 'synthetic_browser_with_recorded_png_states', events: 'simulated_danmaku_queue.test', interpreter: 'real_installed_yaml_wasm', real_game: false, real_platform: false, external_notifications: false, target_id: targetId };
let fixture;
let browserCreated = false;
let automaticAttempted = false;
let failure;
let monitoringStopped = false;
let monitoringTask;
const outputChanges = [];
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

async function request(path, { method = 'GET', json, bytes, extraHeaders = {}, allowFailure = false, timeout = 45000 } = {}) {
  const response = await fetch(base + path, { method, headers: { ...headers, ...(json !== undefined ? { 'Content-Type': 'application/json' } : {}), ...extraHeaders }, body: json !== undefined ? JSON.stringify(json) : bytes, signal: AbortSignal.timeout(timeout) });
  const text = await response.text();
  let data; try { data = text ? JSON.parse(text) : null; } catch { data = text; }
  if (!response.ok && !allowFailure) throw new Error(`${method} ${path}: HTTP ${response.status} ${JSON.stringify(data)}`);
  return { status: response.status, ok: response.ok, data };
}
async function callPlugin(id, action, values = {}, options = {}) {
  return (await request(`/api/extensions/${id}/call`, { method: 'POST', json: { action, values }, ...options })).data;
}
const call = (action, values = {}, options = {}) => callPlugin('gamer-live', action, values, options);
const status = () => call('automatic.status');
async function until(get, predicate, label, timeout = 45000, allowBlocked = false) {
  const deadline = Date.now() + timeout; let last;
  do {
    last = await get();
    if (predicate(last)) return last;
    if (!allowBlocked && (last?.error || last?.queue?.blocked)) throw new Error(`${label}: ${last.error || last.queue.blocked}`);
    await sleep(100);
  } while (Date.now() < deadline);
  throw new Error(`${label} timed out: ${JSON.stringify(last)}`);
}
function streamIdentity(stream) {
  return Object.fromEntries(['id', 'source_url', 'mode', 'audio', 'fps', 'bitrate_kbps', 'width', 'height', 'reconnects'].map(key => [key, stream[key]]));
}
function monitorOutput() {
  monitoringTask = (async () => {
    let previous = '';
    while (!monitoringStopped) {
      try {
        const state = await status();
        const entry = { automatic: state.state, automatic_error: state.error, stream_id: state.stream?.id, source_kind: state.stream?.source_kind, stream_state: state.stream?.state, stream_error: state.stream?.error, reconnects: state.stream?.reconnects, current_run: state.queue?.current?.run_id };
        const signature = JSON.stringify(entry);
        if (signature !== previous) {
          previous = signature;
          outputChanges.push({ at: new Date().toISOString(), ...entry });
          await writeFile(join(qa, 'output-status-changes.json'), JSON.stringify(outputChanges, null, 2));
        }
      } catch (error) { outputChanges.push({ at: new Date().toISOString(), monitor_error: error.message }); }
      await sleep(30);
    }
  })();
}
async function install(id, override) {
  const manifest = await readFile(join(repo, 'plugins', id, 'manifest.toml'), 'utf8');
  const version = manifest.match(/^version\s*=\s*"([^"]+)"/m)?.[1]; assert(version);
  const archive = override || join(repo, 'web/public/plugins', `${id}-${version}.gplugin`);
  const bytes = await readFile(archive); const sha = createHash('sha256').update(bytes).digest('hex');
  const installed = (await request('/api/extensions')).data.extensions;
  const current = installed.find(item => item.id === id);
  if (!current) {
    await request('/api/extensions', { method: 'POST', bytes, extraHeaders: { 'Content-Type': 'application/octet-stream', 'X-Gamer-Permission-Confirm': 'true', 'X-Expected-Sha256': sha } });
  } else assert.equal(current.active_version, version, 'Use fresh isolated data for a changed plugin version');
  const enabled = (await request(`/api/extensions/${id}/enable`, { method: 'POST', json: {} })).data;
  assert.equal(enabled.state, 'running'); assert.equal(enabled.last_error, null);
  assert.equal(enabled.active_version, version, `QA archive for ${id} must match the current manifest version`);
  assert.equal(enabled.execution.kind, id === 'gamer-yaml' ? 'wasm' : 'builtin');
  return { id, version, archive: basename(archive), sha256: sha, execution_kind: enabled.execution.kind };
}
async function putText(path, content) {
  const current = await request(path, { allowFailure: true }); assert(current.ok || current.status === 404);
  return (await request(path, { method: 'PUT', json: { content, ...(current.ok ? { expected_version: current.data.version } : {}) } })).data;
}
async function run(entrypoint, args) {
  return (await request('/api/runs', { method: 'POST', json: { runner_id: 'gamer-yaml', entrypoint, device_id: targetId, content_package: 'hsr', payload: { args } } })).data.run_id;
}
async function terminal(runId) {
  const result = await until(async () => (await request(`/api/runs/${runId}`)).data, record => ['success', 'failed', 'cancelled'].includes(record.state), `terminal ${runId}`);
  await until(async () => (await request(`/api/devices/${targetId}/input-control`)).data, control => control.phase === 'idle', 'input release');
  return result;
}
async function events(runId) {
  const all = []; let after = 0;
  for (;;) {
    const page = (await request(`/api/runs/${runId}/events?after=${after}`)).data;
    all.push(...page.events); if (!page.has_more) return all;
    after = page.events.at(-1).id;
  }
}
async function recordsFor(runId) {
  return (await callPlugin('gamer-notify', 'records.read')).records.filter(item => item.source === 'script' && item.source_id?.startsWith(runId + ':'));
}

try {
  await mkdir(qa, { recursive: true });
  const physical = (await request('/api/devices')).data;
  assert(Array.isArray(physical) && physical.every(target => target.kind === 'browser' && target.id.startsWith('browser-live-daily-qa-')), 'Isolated host must have no Android targets or unrelated browsers');
  evidence.plugins = [await install('gamer-yaml', process.env.GAMER_QA_YAML_ARCHIVE), await install('gamer-live', process.env.GAMER_QA_LIVE_ARCHIVE), await install('gamer-notify', process.env.GAMER_QA_NOTIFY_ARCHIVE)];
  const channels = await callPlugin('gamer-notify', 'channels.read');
  assert.equal(channels.channels.length, 0, 'QA must not have notification credentials');
  const packageRead = await request('/api/packages/hsr', { allowFailure: true });
  if (packageRead.status === 404) await request('/api/packages', { method: 'POST', json: { id: 'hsr', name: '直播日常隔离合成测试', targets: { web: { url_prefixes: ['http://127.0.0.1'] } } } });
  else assert(packageRead.ok);
  const liveYaml = await readFile(join(staging, 'live-cloud.yaml'), 'utf8');
  const dailyYaml = await readFile(join(staging, 'daily-test.yaml'), 'utf8');
  evidence.source_sha256 = { live: createHash('sha256').update(liveYaml).digest('hex'), short_daily: createHash('sha256').update(dailyYaml).digest('hex') };
  assert(!/^\s*-\s*finish:/m.test(liveYaml) && !/^\s*-\s*finish:/m.test(dailyYaml), 'Workflow must naturally complete every step');
  const formalDailyPath = '/api/packages/hsr/plugins/gamer-yaml/resources/automations/' + encodeURIComponent('云游戏日常.yaml');
  const formalDaily = await request(formalDailyPath, { allowFailure: true });
  assert(formalDaily.ok || formalDaily.status === 404);
  if (!formalDaily.ok) {
    // Satisfy static call-surface resolution while making an accidental real
    // branch impossible in this isolated fixture. Existing package data stays.
    await putText(formalDailyPath, 'params:\n  notify_success: {type: boolean, default: true}\n  notify_failure: {type: boolean, default: true}\nrun:\n  - fail: {message: 隔离测试禁止执行正式日常, code: QA_FORMAL_DAILY_FORBIDDEN}\n');
    evidence.formal_daily_guard = true;
  }
  evidence.resources = [
    await putText('/api/packages/hsr/plugins/gamer-yaml/resources/automations/' + encodeURIComponent('直播日常测试.yaml'), dailyYaml),
    await putText('/api/packages/hsr/plugins/gamer-yaml/resources/automations/' + encodeURIComponent('直播云游戏.yaml'), liveYaml),
  ].map(({ path, version }) => ({ path, version }));
  const specs = JSON.parse(await readFile(join(evidenceDirectory, 'template-specs.json'), 'utf8'));
  const popupValidation = JSON.parse(await readFile(join(popupDirectory, 'template-validation.json'), 'utf8'));
  const templateImports = [
    ...specs.map(spec => ({ ...spec, file: join(evidenceDirectory, 'crops', spec.full_name) })),
    { full_name: popupValidation.full_name, file: join(popupDirectory, 'later-button-gray.png') },
  ];
  evidence.templates = [];
  for (const spec of templateImports) {
    const bytes = await readFile(spec.file);
    const path = '/api/packages/hsr/plugins/gamer-yaml/resources/templates/' + encodeURIComponent(spec.full_name);
    const existing = await request(path, { allowFailure: true });
    assert(existing.ok || existing.status === 404);
    await request(path, { method: 'PUT', bytes, extraHeaders: { 'Content-Type': 'image/png', ...(existing.ok ? { 'X-Force': 'true' } : {}) } });
    evidence.templates.push({ full_name: spec.full_name, source: spec.file, sha256: createHash('sha256').update(bytes).digest('hex') });
  }
  // The real popup obscures inputs but leaves background controls visually
  // matchable. This is the production failure mode, checked offline first.
  const popupPng = await readFile(join(popupDirectory, 'current.png'));
  evidence.popup_visual_preconditions = [];
  for (const name of ['live_cloud_add_desktop_later_20261015.png', 'live_cloud_enter_game_20261015.png', 'live_cloud_more_20261015.png']) {
    const result = (await request('/api/capabilities/vision/test', { method: 'POST', json: { pkg: 'hsr', plugin: 'gamer-yaml', name, threshold: 0.90, image_png: popupPng.toString('base64') } })).data;
    evidence.popup_visual_preconditions.push({ name, result });
    assert.equal(result.hit, true, `${name} must remain visually matchable beneath the real desktop popup`);
  }
  assert((await readdir(videoFolder)).some(file => /\.mp4$/i.test(file)), 'Generate local QA videos before running');
  fixture = await createRecordedLoginFixture(evidenceDirectory, join(popupDirectory, 'current.png'));
  await request('/api/browser-targets', { method: 'POST', json: { id: targetId, name: '直播日常隔离浏览器', url: fixture.url, profile_id: targetId, width: 1920, height: 1080 } });
  browserCreated = true;
  const connected = await request(`/api/browser-targets/${targetId}/connect`, { method: 'POST', json: {}, allowFailure: true });
  if (!connected.ok) { await sleep(300); await request(`/api/browser-targets/${targetId}/connect`, { method: 'POST', json: {} }); }
  await until(async () => fixture.snapshot(), snap => snap.page?.phase === 'scan', 'fixture initial SMS login');
  await sleep(2100);
  assert.equal(fixture.snapshot().page.phase, 'scan', 'SMS login must never open QR or log in before the script clicks its icon');
  assert.equal(fixture.snapshot().page.scanSwitches, 0);
  evidence.initial_login_phase = 'scan';
  if (!onlyNotifications) {
  const oldRules = await call('rules.read', { package_id: 'hsr' });
  await call('rules.save', { package_id: 'hsr', expected_version: oldRules.version, ruleset: { schema_version: 1, rules: [{ id: 'live-daily-e2e', name: '直播日常', public_name: '日常', enabled: true, kind: 'message', operator: 'contains', value: '日常', entrypoint: 'hsr/直播云游戏.yaml', args: { test_mode: { source: 'fixed', value: true } }, cooldown_secs: 0, timeout_secs: 0 }] } });
  automaticAttempted = true;
  await call('automatic.start', { device_id: targetId, package_id: 'hsr', video_folder: videoFolder, mode: 'local', audio: false, fps: 10, bitrate_kbps: 500 });
  monitorOutput();
  const initial = await until(status, state => state.state === 'video' && state.stream?.state === 'streaming' && state.stream.source_kind === 'video_folder', 'initial video');
  const identity = streamIdentity(initial.stream); evidence.initial_output = identity;
  const ignored = await call('queue.test', { kind: 'message', payload: { text: '其他弹幕' }, request_id: `ignored-${marker}` });
  assert(!ignored.item_id, 'Unmatched danmaku must not enqueue a run');
  const firstRequest = `daily-first-${marker}`; const secondRequest = `daily-second-${marker}`;
  const firstId = (await call('queue.test', { kind: 'message', payload: { text: '帮我做日常' }, request_id: firstRequest })).item_id;
  const secondId = (await call('queue.test', { kind: 'message', payload: { text: '第二位的日常' }, request_id: secondRequest })).item_id;
  assert(firstId && secondId && firstId !== secondId);
  const first = await until(status, state => state.queue.current?.id === firstId && state.queue.current.state === 'running' && state.stream?.source_kind === 'device', 'first workflow');
  assert(first.queue.waiting.some(item => item.id === secondId)); assert.deepEqual(streamIdentity(first.stream), identity);
  evidence.first_running = { run_id: first.queue.current.run_id, waiting: first.queue.waiting.length, source_kind: first.stream.source_kind };
  console.log('First viewer workflow running; second matching message is queued.');
  const second = await until(status, state => {
    const failed = state.queue.history.find(item => [firstId, secondId].includes(item.id) && ['failed', 'cancelled', 'review'].includes(item.state));
    if (failed) throw new Error(`Workflow failed: ${failed.state}: ${failed.error}`);
    return state.queue.current?.id === secondId && state.queue.current.state === 'running' && state.queue.history.some(item => item.id === firstId && item.state === 'success');
  }, 'first completed and second running', 120000);
  assert.deepEqual(streamIdentity(second.stream), identity); assert.equal(second.stream.source_kind, 'device');
  const afterFirst = fixture.snapshot();
  assert(afterFirst.events.some(event => event.reason === 'logout_confirm' && event.phase === 'scan'), 'First workflow must leave the default SMS window after confirming logout');
  assert.equal(afterFirst.page.logoutConfirmations, 1); assert.equal(afterFirst.page.refreshes, 1);
  assert.equal(afterFirst.page.popupDismissals, 1, 'First viewer must dismiss the real desktop popup after post-daily reset');
  assert(afterFirst.events.some(event => event.reason === 'desktop_popup_shown' && event.trigger === 'post_daily_reset'));
  assert(afterFirst.page.reloads >= 3, 'Daily reset and post-daily reset must both reload while preserving state');
  assert(afterFirst.events.some(event => event.reason === 'more_hover'), 'More menu must open through real CDP hover');
  evidence.first_flow = afterFirst.events;
  const resumed = await until(status, state => {
    const failed = state.queue.history.find(item => [firstId, secondId].includes(item.id) && ['failed', 'cancelled', 'review'].includes(item.state));
    if (failed) throw new Error(`Workflow failed: ${failed.state}: ${failed.error}`);
    return state.state === 'video' && state.stream?.state === 'streaming' && state.stream.source_kind === 'video_folder' && !state.queue.current && !state.queue.waiting.length && [firstId, secondId].every(id => state.queue.history.some(item => item.id === id && item.state === 'success'));
  }, 'both completed and video restored', 120000);
  assert.deepEqual(streamIdentity(resumed.stream), identity); assert.equal(resumed.queue.device_run, null);
  const completed = [firstId, secondId].map(id => resumed.queue.history.find(item => item.id === id));
  assert(completed[1].started_at >= completed[0].finished_at, 'FIFO cannot start a second task before first cleanup');
  const finishPage = fixture.snapshot().page;
  assert.equal(finishPage.phase, 'scan'); assert.equal(finishPage.logoutConfirmations, 2); assert.equal(finishPage.scanSwitches, 2);
  assert.equal(finishPage.popupDismissals, 2, 'Each queued workflow must click the popup left button before logout');
  assert.equal(finishPage.popupAddClicks, 0, 'The yellow Add button must never be clicked');
  assert.equal(finishPage.popup, false);
  assert(finishPage.popupBlockedInputs >= 1, 'A popup appearing at More hover must block that background input');
  assert(fixture.snapshot().events.some(event => event.reason === 'desktop_popup_shown' && event.trigger === 'more_hover_race'), 'Second viewer must cover a popup racing the More hover');
  assert(fixture.snapshot().events.some(event => event.reason === 'more_click' && event.requireMoreClick === true), 'After the late popup, a fresh More click must open a menu requiring click');
  evidence.desktop_popup_regression = { source: join(popupDirectory, 'current.png'), source_sha256: createHash('sha256').update(popupPng).digest('hex'), dismissals: finishPage.popupDismissals, blocked_inputs: finishPage.popupBlockedInputs, add_button_clicks: finishPage.popupAddClicks, fresh_more_click_fallback: true, events: fixture.snapshot().events.filter(event => event.reason.startsWith('desktop_popup_') || event.reason.startsWith('more_')) };
  const normalTransitions = fixture.snapshot().events.filter(event => ['scan_switch', 'expired_refresh', 'synthetic_login', 'logout_confirm'].includes(event.reason));
  assert.deepEqual(normalTransitions.map(({ reason, phase }) => ({ reason, phase })), [
    { reason: 'scan_switch', phase: 'expired' },
    { reason: 'expired_refresh', phase: 'qr' },
    { reason: 'synthetic_login', phase: 'enter' },
    { reason: 'logout_confirm', phase: 'scan' },
    { reason: 'scan_switch', phase: 'qr' },
    { reason: 'synthetic_login', phase: 'enter' },
    { reason: 'logout_confirm', phase: 'scan' },
  ], 'Each workflow must itself open QR at the beginning, then end at SMS with no tail scan click');
  evidence.normal_login_transitions = normalTransitions;
  assert(finishPage.reloads >= 5, 'Both workflows must run both resets without losing logout/login state');
  evidence.completed = [];
  for (const item of completed) {
    const record = (await request(`/api/runs/${item.run_id}`)).data; assert.equal(record.state, 'success');
    const timeline = await events(item.run_id); const policies = timeline.filter(event => event.name === 'notification_policy').map(event => event.data);
    const taps = timeline.filter(event => event.ev === 'tap');
    assert(taps[0]?.x >= 740 && taps[0].x <= 790 && taps[0].y >= 295 && taps[0].y <= 350, 'First actual input of each viewer run must click the SMS scan icon');
    assert(taps.at(-1)?.x >= 970 && taps.at(-1).x <= 1137 && taps.at(-1).y >= 507 && taps.at(-1).y <= 557, 'Last input must confirm logout, with no tail QR input');
    assert.equal(taps.filter(event => event.x >= 740 && event.x <= 790 && event.y >= 295 && event.y <= 350).length, 1, 'One scan-icon click belongs at the beginning of each normal workflow');
    assert.equal(taps.filter(event => event.x >= 784 && event.x <= 952 && event.y >= 521 && event.y <= 570).length, 1, 'Actual WASM input must dismiss the real desktop popup once per viewer');
    assert(policies.some(policy => policy.when === 'success' && policy.enabled === false), 'Live caller must suppress daily success');
    assert(policies.some(policy => policy.when === 'failure' && policy.enabled === true));
    assert(timeline.some(event => event.ev === 'call_start' && event.target?.includes('直播日常测试.yaml')), 'Workflow must call the short daily script in the real interpreter');
    assert.equal((await recordsFor(item.run_id)).length, 0, 'Successful live workflow must be notification silent');
    evidence.completed.push({ item_id: item.id, run_id: item.run_id, state: record.state, policies, events: timeline, started_at: item.started_at, finished_at: item.finished_at });
  }
  const hls = await fetch(identity.source_url, { signal: AbortSignal.timeout(5000) });
  assert(hls.ok && (await hls.text()).includes('#EXTINF:'), 'Restored HLS output remains readable');
  evidence.resumed = { same_output: true, stream: streamIdentity(resumed.stream), source_kind: resumed.stream.source_kind, empty_queue: true, page: finishPage };
  evidence.output_reconnects = { initial: initial.stream.reconnects, resumed: resumed.stream.reconnects };
  assert(!outputChanges.some(entry => entry.stream_error || ['reconnecting', 'failed'].includes(entry.stream_state)), 'Encoder must remain continuous across browser reload and source switching');
  console.log('Two real WASM workflows opened QR, dismissed post-reset and late More-hover desktop popups, logged out to SMS, and resumed the same output.');

  // Cancellation is tested while observation waits forever with no successful
  // login. Stop must cancel only the owned run and discard pending admission.
  await fixture.configure({ phase: 'qr', autoLogin: false, expireFirstScan: false });
  await until(async () => fixture.snapshot(), snap => snap.page?.phase === 'qr' && snap.page.epoch === snap.control.epoch, 'hold login fixture');
  const cancelId = (await call('queue.test', { kind: 'message', payload: { text: '日常，先一直等扫码' }, request_id: `daily-cancel-${marker}` })).item_id;
  const waitingId = (await call('queue.test', { kind: 'message', payload: { text: '日常，等待队列清理' }, request_id: `daily-never-start-${marker}` })).item_id;
  const held = await until(status, state => state.queue.current?.id === cancelId && state.queue.current.state === 'running' && state.queue.waiting.some(item => item.id === waitingId), 'forever login wait');
  await until(async () => events(held.queue.current.run_id), timeline => timeline.some(event => event.name === 'effective_args' && event.data?.args?.forever === true), 'optional QR entry skipped and permanent wait reached');
  assert.equal(fixture.snapshot().page.phase, 'qr'); assert.equal(fixture.snapshot().page.scanSwitches, 0);
  // A late SMS window must be handled by the permanent wait's scan obstacle,
  // after the initial optional entry has already completed without a match.
  await fixture.configure({ phase: 'scan', autoLogin: false, expireFirstScan: false });
  await until(async () => fixture.snapshot(), snap => snap.page?.epoch === snap.control.epoch && snap.page.phase === 'qr' && snap.page.scanSwitches === 1, 'late SMS obstacle opens QR without fixture input');
  await sleep(1300);
  const stillWaiting = await status(); assert.equal(stillWaiting.queue.current.id, cancelId); assert.equal(stillWaiting.queue.current.state, 'running');
  const stopped = await call('automatic.stop');
  const cancelled = stopped.queue.history.find(item => item.id === cancelId); const removed = stopped.queue.history.find(item => item.id === waitingId);
  assert.equal(cancelled.state, 'cancelled'); assert.equal(removed.state, 'removed'); assert.equal(removed.run_id, null);
  assert.equal(stopped.queue.current, null); assert.equal(stopped.queue.device_run, null); assert.equal(stopped.stream, null);
  assert.equal((await request(`/api/runs/${held.queue.current.run_id}`)).data.state, 'cancelled');
  const stoppedControl = (await request(`/api/devices/${targetId}/input-control`)).data;
  assert.equal(stoppedControl.phase, 'idle'); assert.equal(stoppedControl.manual_allowed, true);
  evidence.cancelled_permanent_wait = { run_id: cancelled.run_id, state: cancelled.state, waiting_state: removed.state, waiting_never_started: true, input_released: true, existing_qr_optional_skipped: true, permanent_wait_observed: true, late_sms_obstacle_clicked: true };

  // A failed daily remains in history. After releasing its Core slot, automatic
  // mode continues the next matching message and restores video when drained.
  await fixture.configure({ phase: 'scan', autoLogin: true, expireFirstScan: false });
  await until(async () => fixture.snapshot(), snap => snap.page?.phase === 'scan' && snap.page.epoch === snap.control.epoch, 'failure fixture starts at SMS');
  const beforeFailureRules = await call('rules.read', { package_id: 'hsr' });
  await call('rules.save', { package_id: 'hsr', expected_version: beforeFailureRules.version, ruleset: { schema_version: 1, rules: [
    { id: 'live-daily-failure-e2e', name: '直播日常失败测试', public_name: '日常', enabled: true, kind: 'message', operator: 'equals', value: '日常失败测试', entrypoint: 'hsr/直播云游戏.yaml', args: { test_mode: { source: 'fixed', value: true }, test_fail: { source: 'fixed', value: true } }, cooldown_secs: 0, timeout_secs: 0 },
    { id: 'live-daily-after-failure-e2e', name: '失败后继续日常', public_name: '日常', enabled: true, kind: 'message', operator: 'contains', value: '日常', entrypoint: 'hsr/直播云游戏.yaml', args: { test_mode: { source: 'fixed', value: true } }, cooldown_secs: 0, timeout_secs: 0 },
  ] } });
  await call('automatic.start', { device_id: targetId, package_id: 'hsr', video_folder: videoFolder, mode: 'local', audio: false, fps: 10, bitrate_kbps: 500 });
  const failureInitial = await until(status, state => state.state === 'video' && state.stream?.state === 'streaming', 'failure test video');
  const failedId = (await call('queue.test', { kind: 'message', payload: { text: '日常失败测试' }, request_id: `daily-failure-${marker}` })).item_id;
  const followingId = (await call('queue.test', { kind: 'message', payload: { text: '下一位正常日常，失败后继续' }, request_id: `daily-after-failure-${marker}` })).item_id;
  assert(failedId && followingId && failedId !== followingId);
  const following = await until(status, state => state.queue.current?.id === followingId && state.queue.current.state === 'running' && state.queue.history.some(item => item.id === failedId && item.state === 'failed'), 'failed task recorded and following task starts', 45000);
  assert.equal(following.queue.paused, false); assert.equal(following.queue.blocked, null);
  assert.equal(following.stream.source_kind, 'device'); assert.deepEqual(streamIdentity(following.stream), streamIdentity(failureInitial.stream));
  const failureResumed = await until(status, state => state.state === 'video' && state.stream?.source_kind === 'video_folder' && state.stream.state === 'streaming' && !state.queue.current && !state.queue.waiting.length && state.queue.history.some(item => item.id === failedId && item.state === 'failed') && state.queue.history.some(item => item.id === followingId && item.state === 'success'), 'following task completes and video restores', 90000);
  assert.equal(failureResumed.queue.paused, false); assert.equal(failureResumed.queue.blocked, null); assert.equal(failureResumed.error, null);
  assert.deepEqual(streamIdentity(failureResumed.stream), streamIdentity(failureInitial.stream)); assert.equal(failureResumed.queue.device_run, null);
  const failedItem = failureResumed.queue.history.find(item => item.id === failedId); const followingItem = failureResumed.queue.history.find(item => item.id === followingId);
  assert(failedItem.error, 'The original task error must remain in history');
  assert(failedItem.run_id && followingItem.run_id && failedItem.run_id !== followingItem.run_id);
  assert(followingItem.started_at >= failedItem.finished_at, 'Following FIFO run must wait for failed run cleanup');
  assert.equal((await request(`/api/runs/${failedItem.run_id}`)).data.state, 'failed');
  assert.equal((await request(`/api/runs/${followingItem.run_id}`)).data.state, 'success');
  const failureNotifications = await recordsFor(failedItem.run_id);
  assert.equal(failureNotifications.length, 1, 'Called daily failure and enclosing live failure must produce only one notification');
  assert.equal(failureNotifications[0].status, 'skipped'); assert.equal(failureNotifications[0].title, '直播日常隔离浏览器 · 直播日常测试失败');
  assert.equal((await recordsFor(followingItem.run_id)).length, 0, 'Following successful live daily remains notification silent');
  const afterFailureControl = (await request(`/api/devices/${targetId}/input-control`)).data;
  assert.equal(afterFailureControl.phase, 'idle'); assert.equal(afterFailureControl.manual_allowed, true);
  assert.equal(fixture.snapshot().page.phase, 'scan'); assert.equal(fixture.snapshot().page.scanSwitches, 1);
  const failureTransitions = fixture.snapshot().events.filter(event => event.epoch === fixture.snapshot().page.epoch && ['scan_switch', 'expired_refresh', 'synthetic_login', 'logout_confirm'].includes(event.reason));
  assert.deepEqual(failureTransitions.map(({ reason, phase }) => ({ reason, phase })), [
    { reason: 'scan_switch', phase: 'qr' },
    { reason: 'synthetic_login', phase: 'enter' },
    { reason: 'logout_confirm', phase: 'scan' },
  ], 'Following task must reuse the Enter state left by failure without fixture commands or a duplicate scan click');
  evidence.failed_daily_continues_queue = { run_id: failedItem.run_id, state: failedItem.state, error: failedItem.error, finished_at: failedItem.finished_at, following_item_id: followingId, following_run_id: followingItem.run_id, following_state: followingItem.state, following_started_at: followingItem.started_at, automatic_state: failureResumed.state, paused: failureResumed.queue.paused, blocked: failureResumed.queue.blocked, source_kind: failureResumed.stream.source_kind, same_output: true, reconnects: failureResumed.stream.reconnects, input_phase: afterFailureControl.phase, initial_phase: 'scan', final_phase: 'scan', scan_switches: 1, manual_recovery: false, transitions: failureTransitions, notifications: failureNotifications, failed_events: await events(failedItem.run_id), following_events: await events(followingItem.run_id) };
  await call('automatic.stop');
  console.log('Failed short daily remained in history with one notification admission; the next FIFO daily succeeded and video resumed.');
  }

  // No channel exists, so result policies can be checked through recorded
  // skipped admissions without making any external notification request.
  evidence.notification_cases = [];
  for (const test of [
    { name: 'default_success', args: {}, state: 'success', records: 1, title: '直播日常测试成功' },
    { name: 'success_disabled', args: { notify_success: false }, state: 'success', records: 0 },
    { name: 'default_failure', args: { simulate_failure: true }, state: 'failed', records: 1, title: '直播日常测试失败' },
    { name: 'failure_disabled', args: { simulate_failure: true, notify_failure: false }, state: 'failed', records: 0 },
  ]) {
    const runId = await run('hsr/直播日常测试.yaml', test.args); const record = await terminal(runId); assert.equal(record.state, test.state);
    const notifications = await recordsFor(runId); assert.equal(notifications.length, test.records, test.name);
    if (test.records) { assert.equal(notifications[0].status, 'skipped'); assert.equal(notifications[0].title, '直播日常隔离浏览器 · ' + test.title); }
    evidence.notification_cases.push({ name: test.name, run_id: runId, state: record.state, notifications, events: await events(runId) });
  }
  evidence.fixture = fixture.snapshot();
  if (!onlyNotifications) assert(!outputChanges.some(entry => entry.stream_error || ['reconnecting', 'failed'].includes(entry.stream_state)), 'Every tested reload must preserve the active encoder');
  if (onlyNotifications) evidence.kind = 'isolated_daily_notification_parameter_integration';
  evidence.passed = true;
  console.log(onlyNotifications ? 'All four daily notification parameter cases passed.' : 'Permanent login wait cancellation and all four daily notification parameter cases passed.');
} catch (error) {
  failure = error; evidence.passed = false; evidence.error = error.stack || error.message;
  if (fixture) evidence.fixture = fixture.snapshot();
  console.error(error.stack || error.message);
} finally {
  if (automaticAttempted) try { await call('automatic.stop'); } catch (error) { evidence.cleanup_error = error.message; }
  if (browserCreated) try { await request(`/api/browser-targets/${targetId}/disconnect`, { method: 'POST', json: {} }); } catch (error) { evidence.browser_cleanup_error = error.message; }
  if (fixture) await fixture.close();
  monitoringStopped = true;
  if (monitoringTask) await monitoringTask;
  evidence.output_status_changes = outputChanges;
  await mkdir(qa, { recursive: true });
  const resultFile = join(qa, onlyNotifications ? 'notification-result.json' : 'full-workflow-result.json');
  await writeFile(resultFile, JSON.stringify(evidence, null, 2));
  console.log(JSON.stringify({ passed: evidence.passed, evidence: resultFile, error: evidence.error || null }));
  if (failure) process.exitCode = 1;
}
