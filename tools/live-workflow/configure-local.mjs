// Save reviewed local workflow resources through the versioned public APIs.
// Does not connect a device, run a script, send a notification or start output.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const base = process.env.GAMER_WORKFLOW_BASE || 'http://127.0.0.1:8443';
const baseUrl = new URL(base);
assert(baseUrl.protocol === 'http:' && baseUrl.hostname === '127.0.0.1' && baseUrl.pathname === '/' && !baseUrl.username && !baseUrl.password && !baseUrl.search && !baseUrl.hash, 'Only a plain loopback HTTP base URL is allowed');
const token = process.env.GAMER_ADMIN_TOKEN?.trim() || (await readFile(process.env.GAMER_WORKFLOW_TOKEN_FILE || join(process.env.LOCALAPPDATA, 'gamer/admin-token'), 'utf8')).trim();
const work = resolve(process.env.GAMER_WORKFLOW_DIR || join(repo, 'server/data/live-workflow-20261015'));
assert(work.startsWith(repo + '\\') || work.startsWith(repo + '/'), 'Workflow inputs must stay inside this workspace');
const dryRun = process.env.GAMER_WORKFLOW_DRY_RUN === '1';
const localData = resolve(process.env.GAMER_WORKFLOW_DATA_DIR || join(repo, 'server/data'));
assert(localData.startsWith(repo + '\\') || localData.startsWith(repo + '/'), 'Private draft inspection must stay inside this workspace');
if (dryRun && baseUrl.port !== '8443') assert(process.env.GAMER_WORKFLOW_DATA_DIR, 'Read-only QA requires its explicit local data directory');
const backup = resolve(process.env.GAMER_WORKFLOW_BACKUP_DIR || join(work, 'applications', `${new Date().toISOString().replaceAll(':', '-')}-${randomUUID()}`));
assert(backup.startsWith(work + '\\') || backup.startsWith(work + '/'), 'Backup must stay inside this workflow directory');
const digest = value => createHash('sha256').update(value).digest('hex');
const report = { resources: [], real_game_run: false, stream_started: false, notifications_sent: false };
async function request(path, options = {}) {
  const { allowMissing, ...fetchOptions } = options;
  const response = await fetch(base + path, { method: options.body ? 'POST' : 'GET', ...fetchOptions, headers: { 'X-Admin-Token': token, 'Content-Type': 'application/json', ...options.headers }, signal: AbortSignal.timeout(60000) });
  if (allowMissing && response.status === 404) return null;
  const data = await response.json();
  if (!response.ok) throw new Error(`${path}: HTTP ${response.status} ${JSON.stringify(data)}`);
  return data;
}
const call = (id, action, values = {}) => request(`/api/extensions/${id}/call`, { body: JSON.stringify({ action, values }) });
const scriptSpecs = [
  ['云游戏日常.yaml', 'daily-notify.yaml', '新增成功/失败通知开关，默认均true，保留原日常步骤'],
  ['直播日常测试.yaml', 'daily-test.yaml', '短日常替代：重置输入、按参数通知、支持模拟失败'],
  ['直播云游戏.yaml', 'live-cloud.yaml', '直播扫码、过期刷新、调用日常、退出账号并恢复扫码'],
];
async function inspectScript([scriptId, file, instruction]) {
  const yaml = await readFile(join(work, 'staging', file), 'utf8');
  const formal = await request(`/api/packages/hsr/plugins/gamer-yaml/resources/automations/${encodeURIComponent(scriptId)}`, { allowMissing: true });
  let doc;
  try {
    doc=JSON.parse(await readFile(join(localData,'extension-data/gamer-yaml/authoring/hsr',digest(scriptId)+'.json'),'utf8'));
    assert.equal(doc.script_id,scriptId,'Private draft identity does not match its file');
    doc.head_version=doc.versions.at(-1)?.version||'';
  } catch(error) { if(error.code!=='ENOENT') throw error;doc=null; }
  assert(!['generating', 'running'].includes(doc?.job?.state), `${scriptId} has unfinished authoring work`);
  if (formal?.content === yaml) return { scriptId, yaml, instruction, doc, formal, unchanged: true };
  if (scriptId === '云游戏日常.yaml') {
    const original = await readFile(join(work, 'backup', '云游戏日常.before-notify.yaml'), 'utf8');
    assert.equal(formal && digest(formal.content), digest(original), 'Daily changed since preparation; rebase the parameter edit before applying');
  } else assert.equal(formal, null, `${scriptId} already exists with different content; preserve it and review the conflict`);
  const head = doc?.versions.at(-1);
  const ownedHead = head?.yaml === yaml && head?.source === 'manual' && head?.instruction === instruction;
  if (head && !ownedHead) {
    assert(formal && head.yaml === formal.content && (head.published || head.source === 'library'), `${scriptId} has an unpublished user draft; preserve it and resolve the conflict first`);
  }
  return { scriptId, yaml, instruction, doc, formal, ownedHead, unchanged: false };
}
async function publish(plan) {
  const { scriptId, yaml, instruction, formal } = plan;
  if (plan.unchanged) {
    report.resources.push({ script_id: scriptId, unchanged: true, sha256: digest(yaml) });
    return;
  }
  let doc = plan.doc;
  if (!doc) doc = (await call('gamer-yaml', formal ? 'authoring.import' : 'authoring.create', { package_id: 'hsr', script_id: scriptId })).script;
  await writeFile(join(backup, `${scriptId}.authoring-before.json`), JSON.stringify(doc), { flag: 'wx' });
  let saved = doc;
  if (!plan.ownedHead) saved = (await call('gamer-yaml', 'authoring.save', { package_id: 'hsr', script_id: scriptId, expected_version: doc.head_version, yaml, instruction })).script;
  assert.deepEqual(saved.versions.at(-1).diagnostics, [], 'Source and references must pass before publication');
  const published = (await call('gamer-yaml', 'authoring.publish', { package_id: 'hsr', script_id: scriptId, expected_version: saved.head_version, version: saved.head_version })).script;
  const resource = await request(`/api/packages/hsr/plugins/gamer-yaml/resources/automations/${encodeURIComponent(scriptId)}`);
  assert.equal(resource.content, yaml);
  report.resources.push({ script_id: scriptId, head_version: published.head_version, published_version: published.published_version, sha256: digest(yaml) });
}

async function configureWorkflow() {
const status = await call('gamer-live', 'live.status');
assert(['stopped','failed'].includes(status.automatic.state), 'Automatic session must be idle');
assert.equal(status.stream, null);
assert.equal(status.automatic.queue.current, null);
assert.equal(status.automatic.queue.waiting.length, 0);
const extensions = (await request('/api/extensions')).extensions;
for (const [id, version] of [['gamer-yaml','0.3.18'], ['gamer-live','0.2.9'], ['gamer-notify','0.1.1']]) {
  const extension = extensions.find(extension => extension.id === id);
  if (!dryRun) assert.equal(extension?.active_version, version, `${id} requires the paired local host/plugin upgrade`);
  assert.equal(extension.state, 'running');
  assert.equal(extension.last_error, null);
}
const catalog = (await request('/api/runners/gamer-yaml/functions')).functions;
if (!dryRun) {
  assert(catalog.some(fn => fn.name === 'hover'));
  assert(catalog.find(fn => fn.name === 'wait_find').params.some(param => param.name === 'forever'));
}
const plans = [];
for (const spec of scriptSpecs) plans.push(await inspectScript(spec));
const oldRules = await call('gamer-live', 'rules.read', { package_id: 'hsr' });
const proposed = JSON.parse(await readFile(join(repo, 'server/target/live-daily-qa/rules.example.json'), 'utf8'));
const replaced = new Set(proposed.rules.map(rule => rule.id));
const priorRules = Array.isArray(oldRules.rules) ? oldRules.rules : [];
let notifications = await call('gamer-notify', 'runs.read');
const channels = await call('gamer-notify', 'channels.read');
const existingDailyRule = notifications.rules.find(rule => rule.package_id === 'hsr' && rule.entrypoint === 'hsr/云游戏日常.yaml' && rule.enabled);
assert(existingDailyRule?.channels.length, 'Keep the existing selected daily notification channels');
for (const id of existingDailyRule.channels) assert(channels.channels.some(channel => channel.id === id && channel.enabled && channel.has_key), `Daily notification channel ${id} is unavailable`);
for (const scriptId of ['直播云游戏.yaml','直播日常测试.yaml']) {
  const existing = notifications.rules.find(rule => rule.package_id === 'hsr' && rule.entrypoint === `hsr/${scriptId}`);
  if (!existing) continue;
  assert(existing.enabled && existing.runner_ids?.some(id => ['gamer-yaml','gamer-yaml-version'].includes(id)) && existing.channels?.length, `${scriptId} has an inactive or incompatible existing notification rule; preserve it and resolve the conflict`);
  for (const id of existing.channels) assert(channels.channels.some(channel => channel.id === id && channel.enabled && channel.has_key), `${scriptId} selected notification channel ${id} is unavailable`);
}
const automaticBefore = await call('gamer-live', 'automatic.status');
const preview = await call('gamer-live', 'automatic.preview', { video_folder: join(repo, 'live-videos') });
assert(preview.files.length >= 2);
const targets = await request('/api/browser-targets');
assert(targets.some(target => target.id === 'browser-multiview-blank'), 'Configured live browser target is missing');
if (dryRun) {
  console.log(JSON.stringify({ passed: true, dry_run: true, resources: plans.map(plan => ({ script_id: plan.scriptId, unchanged: plan.unchanged, sha256: digest(plan.yaml) })), selected_channels: existingDailyRule.channels, default_channel: channels.default_channel, backup_directory: backup }, null, 2));
  return;
}
await mkdir(backup, { recursive: true });
await writeFile(join(backup, 'live-rules-before.json'), JSON.stringify(oldRules), { flag: 'wx' });
await writeFile(join(backup, 'automatic-before.json'), JSON.stringify(automaticBefore), { flag: 'wx' });
await writeFile(join(backup, 'notification-rules-before.json'), JSON.stringify(notifications), { flag: 'wx' });
await writeFile(join(backup, 'notification-channels-before.json'), JSON.stringify(channels), { flag: 'wx' });
for (const plan of plans) await publish(plan);
const mergedRules = [...proposed.rules, ...priorRules.filter(rule => !replaced.has(rule.id))];
const normalizeRule = rule => ({ ...rule, args: Object.fromEntries(Object.entries(rule.args || {}).map(([name, binding]) => [name, { source: binding.source, value: binding.value ?? null, field: binding.field ?? '' }])) });
const savedRules = isDeepStrictEqual(mergedRules.map(normalizeRule), priorRules.map(normalizeRule)) ? oldRules : await call('gamer-live', 'rules.save', { package_id: 'hsr', expected_version: oldRules.version, ruleset: { schema_version: 1, rules: mergedRules } });
if (automaticBefore.queue.target?.device_id !== 'browser-multiview-blank' || automaticBefore.queue.target?.package_id !== 'hsr' || resolve(automaticBefore.video_folder || '.') !== join(repo, 'live-videos')) {
  await call('gamer-live', 'queue.configure', { device_id: 'browser-multiview-blank', package_id: 'hsr', video_folder: join(repo, 'live-videos') });
}

for (const scriptId of ['直播云游戏.yaml', '直播日常测试.yaml']) {
  if (notifications.rules.some(rule => rule.package_id === 'hsr' && rule.entrypoint === `hsr/${scriptId}`)) continue;
  notifications = await call('gamer-notify', 'runs.save', { expected_version: notifications.version, rule: { package_id: 'hsr', entrypoint: `hsr/${scriptId}`, runner_ids: ['gamer-yaml','gamer-yaml-version'], channels: existingDailyRule.channels, enabled: true } });
}
const automatic = await call('gamer-live', 'automatic.status');
assert(['stopped','failed'].includes(automatic.state), 'Automatic session must remain idle');
assert.equal(resolve(automatic.video_folder), join(repo, 'live-videos'));
assert.equal(automatic.queue.target.device_id, 'browser-multiview-blank');
assert.equal(automatic.queue.target.package_id, 'hsr');
const folder = await call('gamer-live', 'automatic.preview', { video_folder: automatic.video_folder });
assert(folder.files.length >= 2);
report.rules_version = savedRules.version;
report.target = { device_id: automatic.queue.target.device_id, package_id: automatic.queue.target.package_id };
report.video_folder = automatic.video_folder;
report.video_count = folder.files.length;
report.notification_rules_version = notifications.version;
assert.equal((await call('gamer-notify', 'channels.read')).default_channel, channels.default_channel, 'Preserve the global default channel');
for (const plan of plans) assert.equal((await request(`/api/packages/hsr/plugins/gamer-yaml/resources/automations/${encodeURIComponent(plan.scriptId)}`)).content, plan.yaml, 'Final workflow resource changed during configuration');
report.backup_directory = backup;
report.passed = true;
await writeFile(join(backup, 'configured-report.json'), JSON.stringify(report, null, 2), { flag: 'wx' });
await writeFile(join(work, 'configured-report.json'), JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
}
await configureWorkflow().catch(error => { console.error(error.message || String(error)); process.exitCode = 1; });
