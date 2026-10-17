// Resource/plugin update only. Never starts/stops a server, output, device or run.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';

async function main() {
const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const work = join(repo, 'server/data/live-workflow-20261015/cleanup-20261015');
const apply = process.argv.includes('--apply');
const stage = process.argv.includes('--stage');
assert(!(apply && stage), 'Choose --stage or --apply');
const base = 'http://127.0.0.1:8443';
const token = (await readFile(join(process.env.LOCALAPPDATA, 'gamer/admin-token'), 'utf8')).trim();
const sha = value => createHash('sha256').update(value).digest('hex');
const historicItem = '898757dd-f292-40ea-a1e7-6298d14cc659';
const instruction = '直播日常完成即成功；成功失败都由直播队列执行独立重置与退出收尾，收尾异常不覆盖日常结果';
const cleanupSpec = { entrypoint: 'hsr/直播云游戏收尾.yaml', args: {}, timeout_secs: 180 };
async function writeOnce(path, value) {
  try { await writeFile(path, value, { flag: 'wx' }); }
  catch (error) { if (error.code !== 'EEXIST') throw error; assert.equal(await readFile(path, 'utf8'), value, 'Preserve differing partial preparation: ' + path); }
}
function prepareRules(before) {
  const matches = before.rules.filter(r => r.entrypoint === 'hsr/直播云游戏.yaml');
  assert(matches.some(r => r.id === 'hsr-live-daily'), 'Expected daily interaction rule is missing');
  for (const rule of matches) assert(!rule.cleanup || isDeepStrictEqual(rule.cleanup, cleanupSpec), 'Preserve a different user-selected cleanup: ' + rule.id);
  return { matches, rules: before.rules.map(rule => rule.entrypoint === 'hsr/直播云游戏.yaml' ? { ...rule, cleanup: cleanupSpec } : rule) };
}
async function request(path, { body, bytes, missing = false, headers = {} } = {}) {
  const response = await fetch(base + path, {
    method: body === undefined && !bytes ? 'GET' : 'POST',
    headers: { 'X-Admin-Token': token, 'Content-Type': bytes ? 'application/octet-stream' : 'application/json', ...headers },
    body: bytes || (body === undefined ? undefined : JSON.stringify(body)), signal: AbortSignal.timeout(45000),
  });
  if (response.status === 404 && missing) return null;
  const data = await response.json();
  assert(response.ok, `${path}: HTTP ${response.status}: ${JSON.stringify(data)}`);
  return data;
}
const call = (id, action, values = {}) => request(`/api/extensions/${id}/call`, { body: { action, values } });
const formal = id => request('/api/packages/hsr/plugins/gamer-yaml/resources/automations/' + encodeURIComponent(id), { missing: true });
const privateDoc = async id => {
  try { return JSON.parse(await readFile(join(repo, 'server/data/extension-data/gamer-yaml/authoring/hsr', sha(id) + '.json'), 'utf8')); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
};
async function idle() {
  const status = await call('gamer-live', 'live.status');
  assert(['stopped', 'failed'].includes(status.automatic.state), 'Stop automatic live first; never interrupt an active session');
  assert.equal(status.stream, null, 'Output must already be stopped');
  assert.equal(status.automatic.queue.current, null, 'Wait for the active item to finish');
  assert.equal(status.automatic.queue.waiting.length, 0, 'Handle waiting items first');
  assert.equal(status.automatic.queue.device_run, null, 'Wait for other work on the execution device');
  return status;
}
await mkdir(work, { recursive: true });
let preparation;
try { preparation = JSON.parse(await readFile(join(work, 'preparation.json'), 'utf8')); }
catch (error) {
  if (error.code !== 'ENOENT') throw error;
  const [outer, daily, rules] = await Promise.all([formal('直播云游戏.yaml'), formal('云游戏日常.yaml'), call('gamer-live', 'rules.read', { package_id: 'hsr' })]);
  assert(outer && daily);
  const source = outer.content.replaceAll('\r\n', '\n');
  const marker = '  # 日常返回后刷新绑定页面';
  const tailIndex = source.indexOf(marker);
  assert(tailIndex > 0, 'Expected original wrapper with cleanup tail');
  const functionsIndex = source.indexOf('functions:\n');
  const runIndex = source.indexOf('\nrun:\n');
  assert(functionsIndex > 0 && runIndex > functionsIndex);
  const helpers = source.slice(functionsIndex, runIndex);
  let primary = source.slice(0, tailIndex);
  primary = primary.slice(0, functionsIndex) + primary.slice(runIndex + 1);
  primary = primary.replace('本脚本只执行一个观众的登录、日常和退出。', '本脚本只执行一个观众的登录和日常，日常返回即成功。')
    .replace('  # 登录或退出步骤失败也通知；', '  # 登录或日常步骤失败也通知；');
  primary += '  # 日常完成即自然成功；队列另行执行重置与退出，收尾异常不覆盖日常结果。\n';
  const cleanup = 'name: 直播云游戏收尾\n# 由直播队列在日常成功或失败后独立执行；第一步始终重置输入。\n' + helpers + '\nrun:\n' + source.slice(tailIndex).replace('本次直播流程结束', '本次直播收尾结束');
  await mkdir(join(work, 'backup'), { recursive: true });
  await writeOnce(join(work, 'backup/直播云游戏.before.yaml'), outer.content);
  await writeOnce(join(work, 'backup/rules-before.json'), JSON.stringify(rules, null, 2));
  await writeOnce(join(work, '直播云游戏.yaml'), primary);
  await writeOnce(join(work, '直播云游戏收尾.yaml'), cleanup);
  preparation = { original_version: outer.version, original_sha256: sha(outer.content), daily_version: daily.version, daily_sha256: sha(daily.content), primary_sha256: sha(primary), cleanup_sha256: sha(cleanup), historic_item: historicItem };
  await writeFile(join(work, 'preparation.json'), JSON.stringify(preparation, null, 2), { flag: 'wx' });
}
const specs = [];
for (const id of ['直播云游戏.yaml', '直播云游戏收尾.yaml']) {
  const yaml = await readFile(join(work, id), 'utf8');
  assert.equal(sha(yaml), id.includes('收尾') ? preparation.cleanup_sha256 : preparation.primary_sha256);
  const [resource, doc] = await Promise.all([formal(id), privateDoc(id)]);
  const head = doc?.versions.at(-1);
  assert(!['running', 'generating', 'pending'].includes(doc?.job?.state), 'Preserve active authoring work');
  const owned = head?.yaml === yaml && head.instruction === instruction && head.source === 'manual';
  const unchanged = resource?.content === yaml;
  if (!unchanged && id === '直播云游戏.yaml') assert.equal(resource?.version, preparation.original_version, 'Formal wrapper changed; rebase instead of overwriting');
  if (!unchanged && id.includes('收尾')) assert.equal(resource, null, 'Preserve a pre-existing cleanup script');
  if (head && !owned && !unchanged) assert.equal(head.yaml, resource?.content, 'Preserve an unpublished user draft');
  specs.push({ id, yaml, resource, doc, owned, unchanged });
}
assert.equal((await formal('云游戏日常.yaml')).version, preparation.daily_version, 'Daily changed since preparation');
if (stage) {
  const results = [];
  for (const plan of specs) {
    let doc = plan.doc ? (await call('gamer-yaml', 'authoring.get', { package_id: 'hsr', script_id: plan.id })).script : null;
    if (plan.doc) {
      try { await writeFile(join(work, 'backup', plan.id + '.authoring-before.json'), JSON.stringify(plan.doc, null, 2), { flag: 'wx' }); }
      catch (error) { if (error.code !== 'EEXIST') throw error; }
    }
    if (!doc) doc = (await call('gamer-yaml', plan.resource ? 'authoring.import' : 'authoring.create', { package_id: 'hsr', script_id: plan.id })).script;
    const saved = plan.owned ? doc : (await call('gamer-yaml', 'authoring.save', { package_id: 'hsr', script_id: plan.id, expected_version: doc.head_version, yaml: plan.yaml, instruction })).script;
    assert.deepEqual(saved.versions.at(-1).diagnostics, []);
    results.push({ id: plan.id, draft_version: saved.head_version, sha256: sha(plan.yaml), diagnostics: [] });
  }
  await writeFile(join(work, 'source-validation.json'), JSON.stringify({ passed: true, scripts: results, formal_resources_changed: false }, null, 2));
  console.log(JSON.stringify({ staged: true, scripts: results, formal_resources_changed: false }, null, 2));
} else {
  const status = await call('gamer-live', 'live.status');
  if (!apply) {
    console.log(JSON.stringify({ requires_backend_rebuild: status.features?.workflow_cleanup !== true, stage_directory: work, scripts: specs.map(p => ({ id: p.id, validated_draft: p.owned, published: p.unchanged })), runs_started: false }, null, 2));
  } else {
    assert.equal(status.features?.workflow_cleanup, true, 'Rebuild the Gamer backend first; preserve the original formal wrapper until cleanup is supported');
    await idle();
    prepareRules(await call('gamer-live', 'rules.read', { package_id: 'hsr' }));
    const validation = JSON.parse(await readFile(join(work, 'source-validation.json'), 'utf8'));
    assert(validation.passed && validation.scripts.length === 2, 'Run --stage before applying');
    const scope = JSON.parse(await readFile(join(work, 'scope-validation.json'), 'utf8'));
    assert(scope.passed && scope.primary_sha256 === preparation.primary_sha256 && scope.cleanup_sha256 === preparation.cleanup_sha256, 'Run the scope check before applying');
    for (const plan of specs) assert(plan.owned || plan.unchanged, 'Only publish the exact validated draft');
    // Inspect/update the paired UI payload after the host advertises support.
    const archive = join(repo, 'server/target/live-cleanup-20261015/plugins/gamer-live-0.2.13.gplugin');
    const bytes = await readFile(archive);
    const headers = { 'X-Expected-Sha256': sha(bytes) };
    const installed = (await request('/api/extensions')).extensions.find(e => e.id === 'gamer-live');
    if (installed.active_version !== '0.2.13') {
      const inspected = await request('/api/extensions/inspect', { bytes, headers });
      assert.equal(inspected.id, 'gamer-live');
      assert.equal(inspected.version, '0.2.13');
      assert.equal(inspected.permission_diff.added.length, 0);
      await request('/api/extensions/gamer-live/update', { bytes, headers });
    }
    await idle();
    const publish = async plan => {
      if (plan.unchanged) return;
      const doc = (await call('gamer-yaml', 'authoring.get', { package_id: 'hsr', script_id: plan.id })).script;
      assert.equal(doc.versions.at(-1).yaml, plan.yaml);
      assert.deepEqual(doc.versions.at(-1).diagnostics, []);
      await call('gamer-yaml', 'authoring.publish', { package_id: 'hsr', script_id: plan.id, expected_version: doc.head_version, version: doc.head_version });
      assert.equal((await formal(plan.id)).content, plan.yaml);
    };
    await publish(specs[1]);
    const before = await call('gamer-live', 'rules.read', { package_id: 'hsr' });
    const { matches, rules } = prepareRules(before);
    if (!isDeepStrictEqual(before.rules, rules)) await call('gamer-live', 'rules.save', { package_id: 'hsr', expected_version: before.version, ruleset: { schema_version: 1, rules } });
    await idle();
    await publish(specs[0]);
    // Server verifies the trusted direct-child call_end and released run slot.
    const corrected = await call('gamer-live', 'queue.reconcile_success', { item_id: historicItem, completed_entrypoint: 'hsr/云游戏日常.yaml' });
    assert.equal((await formal('云游戏日常.yaml')).version, preparation.daily_version);
    const result = { passed: true, rules: matches.map(r => r.id), cleanup: cleanupSpec, corrected, applied_at: new Date().toISOString(), runs_started: false, device_input: false, core_history_rewritten: false };
    await writeFile(join(work, 'applied.json'), JSON.stringify(result, null, 2));
    console.log(JSON.stringify(result, null, 2));
  }
}
}
await main().catch(error => {
  console.error(error.message);
  process.exitCode = 1;
});
