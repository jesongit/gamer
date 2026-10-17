// Versioned HSR resource update only; never submits a run or sends device input.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { dirname, resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const work = join(repo, 'server/data/live-workflow-20261016/reward-boundary');
const qa = join(repo, 'server/target/live-reward-boundary-20261016');
const mode = process.argv[2];
assert(['prepare', 'save', 'publish'].includes(mode), 'Use prepare, save or publish');
const sha = data => createHash('sha256').update(data).digest('hex');
const token = (await readFile(join(process.env.LOCALAPPDATA, 'gamer/admin-token'), 'utf8')).trim();
const ids = ['云游戏日常.yaml', '直播云游戏.yaml'];
const instruction = '直播日常以最后500奖励金色勾选实际核验为成功边界；直播领取后直接返回，独立收尾负责重置和退出；普通模式保留原关窗流程';
async function request(path, body) {
  const r = await fetch('http://127.0.0.1:8443' + path, { method: body ? 'POST' : 'GET', headers: { 'X-Admin-Token': token, 'Content-Type': 'application/json' }, body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(45000) });
  const value = await r.json();
  assert(r.ok, `HTTP ${r.status} ${path}: ${JSON.stringify(value)}`);
  return value;
}
const call = (id, action, values = {}) => request(`/api/extensions/${id}/call`, { action, values });
const resource = id => request('/api/packages/hsr/plugins/gamer-yaml/resources/automations/' + encodeURIComponent(id));
const authoring = (action, id, values = {}) => call('gamer-yaml', 'authoring.' + action, { package_id: 'hsr', script_id: id, ...values });
function replaceOne(source, before, after) {
  assert.equal(source.split(before).length, 2, 'Expected unique source fragment: ' + before);
  return source.replace(before, after);
}
await mkdir(work, { recursive: true });
await mkdir(qa, { recursive: true });
if (mode === 'prepare') {
  const snapshots = [];
  for (const id of ids) {
    const [formal, draft] = await Promise.all([resource(id), authoring('get', id)]);
    assert.equal(draft.script.head_version, draft.script.published_version, 'Preserve unpublished user drafts');
    assert.equal(draft.script.versions.at(-1).yaml, formal.content);
    snapshots.push({ id, formal, draft: draft.script });
  }
  const rules = await call('gamer-live', 'rules.read', { package_id: 'hsr' });
  const cleanup = await resource('直播云游戏收尾.yaml');
  let daily = snapshots[0].formal.content.replaceAll('\r\n', '\n');
  daily = replaceOne(daily, '  notify_failure: {type: boolean, default: true, desc: 失败是否通知}', '  notify_failure: {type: boolean, default: true, desc: 失败是否通知}\n  stop_after_rewards: {type: boolean, default: false, desc: 直播模式在最后奖励核验成功后立即返回，由独立收尾重置并退出}');
  daily = replaceOne(daily, 'vars:\n  comm_done: false', 'vars:\n  comm_done: false\n  daily_completed: false');
  daily = replaceOne(daily, '- observe: {template: daily_500_claimed_check.png, region: [0.8, 0.22, 0.09, 0.15], timeout: 15s}', '- checkpoint: {id: hsr_daily_final_reward, template: daily_500_claimed_check.png, region: [0.8, 0.22, 0.09, 0.15], timeout: 15s}');
  const successLog = "                  - log: {message: '已确认500节点金色勾选，全部活跃奖励领取完成，任务界面流程结束', level: info}";
  daily = replaceOne(daily, successLog, successLog + '\n                  - return: true');
  daily = replaceOne(daily, '              - break: {}\n            else: []\n          # 情况1：领取', '            else: []\n          # 情况1：领取');
  daily = replaceOne(daily, '\n  close_challenge:\n', '\n      # 无金勾确认的提前退出或8轮耗尽，返回未完成；直播主入口必须拒绝成功。\n      - return: false\n\n  close_challenge:\n');
  let calls = 0;
  daily = daily.replace(/^(\s*)- (daily_tasks|phone_resume_confirm): \{\}$/gm, (line, spaces) => { calls++; return line + '\n' + spaces + '  as: daily_completed'; });
  assert.equal(calls, 11, 'All ten main branches and the resume helper must propagate their result');
  daily = replaceOne(daily, '      - daily_tasks: {}\n        as: daily_completed\n\nrun:', '      - daily_tasks: {}\n        as: daily_completed\n      - return: $daily_completed\n\nrun:');
  const tail = '  # 最后步骤：关闭指南与手机菜单';
  assert.equal(daily.split(tail).length, 2);
  daily = daily.replace(tail, '  # 直播成功只认最后奖励核验结果；此处之后的关窗与主界面检测不参与直播成功判定。\n  - if: $stop_after_rewards\n    then:\n      - eq: {a: $daily_completed, b: true}\n        as: rewards_verified\n      - if: $rewards_verified\n        then:\n          - return: true\n        else:\n          - fail: {code: ACTIVE_REWARD_UNCONFIRMED, message: 最后活跃奖励尚未核验成功，直播日常不能记为成功}\n' + tail);
  let parent = snapshots[1].formal.content.replaceAll('\r\n', '\n');
  parent = replaceOne(parent, '云游戏日常.yaml: {notify_success: false, notify_failure: true}', '云游戏日常.yaml: {notify_success: false, notify_failure: true, stop_after_rewards: true}');
  const candidates = [daily, parent];
  await mkdir(join(work, 'backup'), { recursive: true });
  for (let i = 0; i < ids.length; i++) {
    await writeFile(join(work, 'backup', ids[i]), snapshots[i].formal.content, { flag: 'wx' });
    await writeFile(join(work, 'backup', ids[i] + '.authoring.json'), JSON.stringify(snapshots[i].draft, null, 2), { flag: 'wx' });
    await writeFile(join(work, ids[i]), candidates[i], { flag: 'wx' });
  }
  const metadata = { scripts: snapshots.map((s, i) => ({ id: s.id, expected_version: s.formal.version, expected_head: s.draft.head_version, before_sha256: sha(s.formal.content), candidate_sha256: sha(candidates[i]) })), preserved_cleanup_version: cleanup.version, preserved_rules: rules };
  await writeFile(join(work, 'preparation.json'), JSON.stringify(metadata, null, 2), { flag: 'wx' });
  console.log(JSON.stringify({ prepared: true, work, scripts: metadata.scripts }, null, 2));
} else {
  const preparation = JSON.parse(await readFile(join(work, 'preparation.json'), 'utf8'));
  const plans = [];
  for (const spec of preparation.scripts) {
    const source = await readFile(join(work, spec.id), 'utf8');
    assert.equal(sha(source), spec.candidate_sha256);
    const [formal, result] = await Promise.all([resource(spec.id), authoring('get', spec.id)]);
    const doc = result.script;
    const head = doc.versions.at(-1);
    const published = formal.content === source;
    assert(published || formal.version === spec.expected_version, 'Formal script changed; preserve and rebase');
    const owned = head.yaml === source && head.instruction === instruction && head.source === 'manual';
    assert(published || owned || (doc.head_version === spec.expected_head && head.yaml === formal.content), 'Preserve an unrelated user draft');
    plans.push({ ...spec, source, doc, published, owned });
  }
  assert.equal((await resource('直播云游戏收尾.yaml')).version, preparation.preserved_cleanup_version);
  assert.deepEqual(await call('gamer-live', 'rules.read', { package_id: 'hsr' }), preparation.preserved_rules);
  if (mode === 'save') {
    const results = [];
    for (const plan of plans) {
      const result = plan.owned || plan.published ? { script: plan.doc } : await authoring('save', plan.id, { expected_version: plan.doc.head_version, yaml: plan.source, instruction });
      assert.deepEqual(result.script.versions.at(-1).diagnostics, []);
      results.push({ id: plan.id, draft_version: result.script.head_version, sha256: plan.candidate_sha256, diagnostics: [] });
    }
    await writeFile(join(work, 'source-validation.json'), JSON.stringify({ passed: true, scripts: results }, null, 2));
    console.log(JSON.stringify({ saved: true, scripts: results }, null, 2));
  } else {
    const probe = JSON.parse(await readFile(join(qa, 'interpreter-report.json'), 'utf8'));
    assert(probe.passed && probe.candidate_sha256 === plans[0].candidate_sha256 && probe.parent_sha256 === plans[1].candidate_sha256, 'Run the current interpreter against both exact candidates first');
    const status = await call('gamer-live', 'live.status');
    assert(['stopped', 'failed'].includes(status.automatic.state) && !status.stream && !status.automatic.queue.current && !status.automatic.queue.waiting.length && !status.automatic.queue.device_run, 'Wait for live/output/queue/device work to be idle');
    const catalog = await request('/api/runners/gamer-yaml/functions');
    assert(catalog.functions.some(fn => fn.name === 'checkpoint'), 'Current host must support native visual checkpoints');
    const published = [];
    // Add the optional daily parameter before the parent passes it.
    for (const plan of plans) {
      assert(plan.owned || plan.published, 'Save and validate the exact candidate first');
      if (!plan.published) await authoring('publish', plan.id, { expected_version: plan.doc.head_version, version: plan.doc.head_version });
      const formal = await resource(plan.id);
      assert.equal(formal.content, plan.source);
      const doc = (await authoring('get', plan.id)).script;
      published.push({ id: plan.id, published_version: doc.published_version, resource_version: formal.version, sha256: sha(formal.content) });
    }
    const report = { passed: true, published, preserved_cleanup_version: preparation.preserved_cleanup_version, rules_unchanged: true, runs_started: false, device_input: false, host_rebuild_required: false, published_at: new Date().toISOString() };
    await writeFile(join(work, 'published.json'), JSON.stringify(report, null, 2));
    console.log(JSON.stringify(report, null, 2));
  }
}
