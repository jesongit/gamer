// Stages/reviews resources; apply is explicit and never starts a run or a service.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const repo=resolve(dirname(fileURLToPath(import.meta.url)),'../..');
const work=join(repo,'server/data/live-workflow-20261016/users-confirmation');
const mode=process.argv[2]; assert(['prepare','apply'].includes(mode),'Use prepare or apply');
const sha=value=>createHash('sha256').update(value).digest('hex');
const token=(await readFile(join(process.env.LOCALAPPDATA,'gamer/admin-token'),'utf8')).trim();
async function request(path,body){const r=await fetch('http://127.0.0.1:8443'+path,{method:body?'POST':'GET',headers:{'X-Admin-Token':token,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(45000)});const v=await r.json();assert(r.ok,`HTTP ${r.status}: ${path}`);return v;}
const call=(id,action,values={})=>request(`/api/extensions/${id}/call`,{action,values});
const resource=id=>request('/api/packages/hsr/plugins/gamer-yaml/resources/automations/'+encodeURIComponent(id));
const author=(action,id,values={})=>call('gamer-yaml','authoring.'+action,{package_id:'hsr',script_id:id,...values});
const ids=['直播云游戏收尾.yaml','直播云游戏.yaml'];
const instruction='直播扫码后等待本人确认；确认取消或超时只退出账号；最后领奖成功才扣次，普通结束仍重置输入并退出';
await mkdir(work,{recursive:true});
if(mode==='prepare'){
  const preparation=JSON.parse(await readFile(join(work,'preparation.json'),'utf8'));
  const scripts=[];
  for(const id of ids){const [formal,a]=await Promise.all([resource(id),author('get',id)]);const baseline=preparation.baseline.find(x=>x.script_id===id);const candidate=await readFile(join(work,id),'utf8');assert.equal(sha(formal.content),baseline.sha256,'Formal baseline changed');assert.equal(a.script.head_version,a.script.published_version,'Preserve unrelated drafts');assert.equal(a.script.versions.at(-1).yaml,formal.content);scripts.push({id,expected_version:formal.version,expected_head:a.script.head_version,before_sha256:sha(formal.content),candidate_sha256:sha(candidate)});}
  const rules=await call('gamer-live','rules.read',{package_id:'hsr'});
  const next=structuredClone(rules.rules);
  for(const id of ['hsr-live-daily','hsr-live-daily-test']){const rule=next.find(x=>x.id===id);assert(rule,'Missing expected live rule');rule.cost=1;rule.confirmation={timeout_secs:300,cancel_cleanup:{entrypoint:'hsr/直播云游戏收尾.yaml',args:{reset_before_logout:false},timeout_secs:60}};if(id==='hsr-live-daily')rule.completion={entrypoint:'hsr/云游戏日常.yaml',checkpoint_id:'hsr_daily_final_reward'};}
  const candidate={schema_version:rules.schema_version,rules:next};
  await writeFile(join(work,'rules.candidate.json'),JSON.stringify(candidate,null,2)+'\n');
  const plan={prepared_at:new Date().toISOString(),scripts,preserved_rules:rules,rules_sha256:sha(JSON.stringify(candidate)),daily_sha256:sha((await resource('云游戏日常.yaml')).content),applied:false};
  await writeFile(join(work,'deployment-plan.json'),JSON.stringify(plan,null,2)+'\n');
  console.log(JSON.stringify({prepared:true,api_writes:false,device_input:false,work,scripts},null,2));
}else{
  const plan=JSON.parse(await readFile(join(work,'deployment-plan.json'),'utf8'));
  const probe=JSON.parse(await readFile(join(work,'confirmation-probe.json'),'utf8'));
  assert(probe.passed,'Run the exact candidate control-flow probe first');
  assert.equal(probe.parent_sha256,plan.scripts.find(x=>x.id==='直播云游戏.yaml').candidate_sha256,'Parent probe is stale');
  assert.equal(probe.cleanup_sha256,plan.scripts.find(x=>x.id==='直播云游戏收尾.yaml').candidate_sha256,'Cleanup probe is stale');
  const status=await call('gamer-live','live.status');
  assert(status.features?.viewer_confirmation&&status.features?.user_quotas&&status.features?.danmaku_sender,'Rebuild/apply the new host and paired plugins first');
  assert(['disconnected','failed'].includes(status.connection.state),'Disconnect live interaction before publishing');
  assert(['stopped','failed'].includes(status.automatic.state)&&!status.stream&&!status.automatic.queue.current&&!status.automatic.queue.waiting.length&&!status.automatic.queue.device_run,'Live output, queue and device must be idle');
  const catalog=await request('/api/runners/gamer-yaml/functions');assert(catalog.functions.some(x=>x.name==='wait_live_confirmation'),'New YAML native function is unavailable');
  assert.equal(sha((await resource('云游戏日常.yaml')).content),plan.daily_sha256,'Daily script changed');
  const candidate=JSON.parse(await readFile(join(work,'rules.candidate.json'),'utf8'));assert.equal(sha(JSON.stringify(candidate)),plan.rules_sha256);
  const currentRules=await call('gamer-live','rules.read',{package_id:'hsr'});
  const rulesDone=isDeepStrictEqual(currentRules.rules,candidate.rules);
  if(!rulesDone)assert.equal(currentRules.version,plan.preserved_rules.version,'Rules changed; preserve and rebase');
  const changes=[];
  for(const spec of plan.scripts){const yaml=await readFile(join(work,spec.id),'utf8');assert.equal(sha(yaml),spec.candidate_sha256);const [formal,a]=await Promise.all([resource(spec.id),author('get',spec.id)]);const owned=a.script.versions.at(-1).yaml===yaml&&a.script.versions.at(-1).instruction===instruction;const published=formal.content===yaml;assert(published||formal.version===spec.expected_version,'Formal changed');assert(published||owned||(a.script.head_version===spec.expected_head&&a.script.versions.at(-1).yaml===formal.content),'Preserve unrelated draft');changes.push({spec,yaml,doc:a.script,owned,published});}
  for(const change of changes){if(change.published)continue;let doc=change.doc;if(!change.owned)doc=(await author('save',change.spec.id,{yaml:change.yaml,expected_version:doc.head_version,instruction})).script;assert.deepEqual(doc.versions.at(-1).diagnostics,[]);await author('publish',change.spec.id,{expected_version:doc.head_version,version:doc.head_version});assert.equal((await resource(change.spec.id)).content,change.yaml);}
  if(!rulesDone)await call('gamer-live','rules.save',{package_id:'hsr',ruleset:candidate,expected_version:currentRules.version});
  const report={applied:true,at:new Date().toISOString(),scripts:plan.scripts.map(x=>({id:x.id,sha256:x.candidate_sha256})),rules_sha256:plan.rules_sha256,device_input:false,runs_started:false,services_started:false};
  await writeFile(join(work,'deployment-result.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report,null,2));
}
