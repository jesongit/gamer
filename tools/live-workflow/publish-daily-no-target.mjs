// Resource-only update. Never starts a run, controls a device or restarts a host.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { basename, join } from 'node:path';

const mode = process.argv[2];
assert(['templates', 'save', 'publish'].includes(mode), 'Use templates, save or publish');
const root = 'D:/code/gamer';
const work = join(root, 'server/data/live-workflow-20261015/no-target-20261015');
const material = join(root, 'server/data/hsr-supervision/no-cultivation-target-20261015');
const qa = join(root, 'server/target/live-daily-no-target-20261015');
const preparation = JSON.parse(await readFile(join(work, 'preparation.json'), 'utf8'));
const scriptId = preparation.script_id;
const sha = value => createHash('sha256').update(value).digest('hex');
const token = (await readFile(join(process.env.LOCALAPPDATA, 'gamer/admin-token'), 'utf8')).trim();
async function request(path, { body, bytes, method, raw = false, missing = false } = {}) {
  const response = await fetch('http://127.0.0.1:8443' + path, {
    method: method || (body === undefined && bytes === undefined ? 'GET' : 'POST'),
    headers: { 'X-Admin-Token': token, 'Content-Type': bytes ? 'image/png' : 'application/json' },
    body: bytes || (body === undefined ? undefined : JSON.stringify(body)), signal: AbortSignal.timeout(45000),
  });
  if (response.status === 404 && missing) return null;
  if (raw && response.ok) return Buffer.from(await response.arrayBuffer());
  const value = await response.json();
  assert(response.ok, `${path}: HTTP ${response.status}: ${JSON.stringify(value)}`);
  return value;
}
const call = (action, values = {}) => request('/api/extensions/gamer-yaml/call', { body: { action, values } });
const resource = path => '/api/packages/hsr/plugins/gamer-yaml/resources/' + path.split('/').map(encodeURIComponent).join('/');
const yaml = await readFile(join(work, '云游戏日常.candidate.yaml'), 'utf8');
const candidateHash = sha(yaml);
const outer = await request(resource('automations/直播云游戏.yaml'));
assert.equal(outer.version, preparation.preserved_outer_version, 'Preserve later edits to the live wrapper');

if (mode === 'templates') {
  const specs = JSON.parse(await readFile(join(material, 'template-specs.json'), 'utf8')).templates;
  const sources = specs.map(spec => ({ name: spec.full_name, source: spec.file }));
  sources.push({ name: preparation.click_enter_obstacle, source: join(work, preparation.click_enter_obstacle) });
  const installed = [];
  let previous;
  try { previous = JSON.parse(await readFile(join(work, 'template-installation.json'), 'utf8')); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  await mkdir(join(work, 'templates'), { recursive: true });
  for (const source of sources) {
    const bytes = await readFile(source.source);
    const endpoint = resource('templates/' + source.name);
    const existing = await request(endpoint, { raw: true, missing: true });
    if (existing) {
      const known = previous?.templates.find(template => template.name === source.name);
      assert(known && known.stored_sha256 === sha(existing) && known.source_sha256 === sha(bytes), 'Preserve an existing template: ' + source.name);
    } else await request(endpoint, { method: 'PUT', bytes });
    const stored = await request(endpoint, { raw: true });
    await writeFile(join(work, 'templates', basename(source.name)), stored);
    installed.push({ name: source.name, source: source.source, source_sha256: sha(bytes), stored_sha256: sha(stored) });
  }
  const report = { passed: true, templates: installed, runs_started: false, device_input: false };
  await writeFile(join(work, 'template-installation.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
} else {
  const current = await request(resource('automations/' + scriptId));
  const doc = (await call('authoring.get', { package_id: 'hsr', script_id: scriptId })).script;
  if (current.content === yaml && doc.published_version === doc.head_version) {
    console.log(JSON.stringify({ passed: true, unchanged: true, resource_version: current.version, published_version: doc.published_version }));
    process.exit(0);
  }
  assert.equal(current.version, preparation.expected_resource_version, 'Formal script changed; preserve that edit');
  assert(!['running', 'generating', 'pending'].includes(doc.job?.state), 'Do not replace in-progress authoring');
  const head = doc.versions.at(-1);
  const instruction = '直播共用日常：等待手机时重复清理点击进入障碍；按添加培养目标判据选择侵蚀隧洞收容舱段，按同一行进入后复用原战斗；素材88cac741dbd540b9bf3764bd35fca71f';
  const owned = head.yaml === yaml && head.instruction === instruction && head.source === 'manual';
  if (mode === 'save') {
    let previousValidation;
    try { previousValidation = JSON.parse(await readFile(join(work, 'source-validation.json'), 'utf8')); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    const previousOwned = head.source === 'manual' && head.instruction === instruction && previousValidation?.draft_version === doc.head_version && previousValidation?.candidate_sha256 === sha(head.yaml);
    assert(owned || previousOwned || (doc.head_version === preparation.expected_head_version && head.yaml === current.content), 'Preserve an unpublished user draft');
    const saved = owned ? doc : (await call('authoring.save', { package_id: 'hsr', script_id: scriptId, expected_version: doc.head_version, yaml, instruction })).script;
    assert.deepEqual(saved.versions.at(-1).diagnostics, [], 'Syntax, function binding and template references must pass');
    // authoring.save runs the host's actual syntax/function/reference validator.
    // The historical draft also references long recordings. Its full material
    // scan is separate from the targeted new-template image checks below.
    const validation = { scope: 'syntax_and_references_from_authoring_save', diagnostics: saved.versions.at(-1).diagnostics };
    const report = { passed: true, candidate_sha256: candidateHash, draft_version: saved.head_version, validation, real_game: false, runs_started: false };
    await writeFile(join(work, 'source-validation.json'), JSON.stringify(report, null, 2));
    console.log(JSON.stringify(report, null, 2));
  } else {
    assert(owned, 'Publish only our exact validated draft');
    const syntax = JSON.parse(await readFile(join(work, 'source-validation.json'), 'utf8'));
    const scope = JSON.parse(await readFile(join(work, 'scope-validation.json'), 'utf8'));
    const matches = JSON.parse(await readFile(join(material, 'template-validation.json'), 'utf8'));
    const alias = JSON.parse(await readFile(join(material, 'click-enter-alias-validation.json'), 'utf8'));
    const probe = JSON.parse(await readFile(join(qa, 'interpreter-report.json'), 'utf8'));
    assert(syntax.passed && syntax.candidate_sha256 === candidateHash && syntax.draft_version === doc.head_version);
    assert(scope.passed && scope.candidate_sha256 === candidateHash, 'Keep the original common combat flow');
    assert(matches.passed && alias.passed, 'Recorded-frame template checks must pass');
    assert(probe.passed && probe.candidate_sha256 === candidateHash, 'Run the current interpreter on exactly this candidate');
    const installed = JSON.parse(await readFile(join(work, 'template-installation.json'), 'utf8'));
    for (const template of installed.templates) {
      assert.equal(sha(await request(resource('templates/' + template.name), { raw: true })), template.stored_sha256, 'Template changed after validation: ' + template.name);
    }
    const published = (await call('authoring.publish', { package_id: 'hsr', script_id: scriptId, expected_version: doc.head_version, version: doc.head_version })).script;
    const after = await request(resource('automations/' + scriptId));
    assert.equal(after.content, yaml);
    assert.equal((await request(resource('automations/直播云游戏.yaml'))).version, preparation.preserved_outer_version);
    const report = { passed: true, resource_version: after.version, published_version: published.published_version, candidate_sha256: candidateHash, sample_id: '88cac741dbd540b9bf3764bd35fca71f', source_validation: join(work, 'source-validation.json'), material_validation: join(material, 'template-validation.json'), interpreter_validation: join(qa, 'interpreter-report.json'), backup: join(work, 'backup'), runs_started: false, real_game: false, device_input: false };
    await writeFile(join(work, 'published-update.json'), JSON.stringify(report, null, 2), { flag: 'wx' });
    console.log(JSON.stringify(report, null, 2));
  }
}
