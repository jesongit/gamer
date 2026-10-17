// Read-only management fixtures. Never contact the production backend or send notifications.
import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';
import { readFile, mkdtemp, writeFile, rm, mkdir } from 'node:fs/promises';
import { resolve, join, sep, basename } from 'node:path';

const repo = resolve(import.meta.dirname, '../..');
const work = join(repo, 'server/data/live-workflow-20261015');
const names = ['云游戏日常.yaml','直播日常测试.yaml','直播云游戏.yaml'];
const staging = await Promise.all(['daily-notify.yaml','daily-test.yaml','live-cloud.yaml'].map(file => readFile(join(work,'staging',file),'utf8')));
const original = await readFile(join(work,'backup/云游戏日常.before-notify.yaml'),'utf8');
const readonly = new Set(['live.status','authoring.list','authoring.get','rules.read','runs.read','channels.read','automatic.status','automatic.preview']);

async function fixture({ imported = false, draftConflict = false, formalConflict = false, disabledChannel = false, failed = false, activeOutput = false, disabledRule = false } = {}) {
  const calls=[];
  const root=await mkdtemp(join(repo,'server/target/gamer-workflow-readonly-'));
  await writeFile(join(root,'token'),'fixture-only-admin-token');
  if(draftConflict) {
    const dir=join(root,'data/extension-data/gamer-yaml/authoring/hsr');await mkdir(dir,{recursive:true});
    await writeFile(join(dir,createHash('sha256').update(names[0]).digest('hex')+'.json'),JSON.stringify({script_id:names[0],versions:[{version:'2.1',yaml:'run: [{log: unpublished user edit}]\n',published:false,source:'manual',instruction:'user edit'}]}));
  }
  const queue={current:null,waiting:[],target:{device_id:'browser-multiview-blank',package_id:'hsr'},automatic_video_folder:join(repo,'live-videos')};
  const automatic={state:failed?'failed':'stopped',stream:activeOutput?{state:'playing'}:null,video_folder:join(repo,'live-videos'),queue};
  const fakeFetch=async (url,req) => {
    try {
      assert.equal(req.headers['X-Admin-Token'],'fixture-only-admin-token');
      const path=decodeURIComponent(new URL(url).pathname);
      let value;
      if (req.method==='GET' && path==='/api/extensions') value={extensions:[['gamer-yaml','0.3.15'],['gamer-live','0.2.7'],['gamer-notify','0.1.0']].map(([id,active_version])=>({id,active_version,state:'running',last_error:null}))};
      else if (req.method==='GET' && path==='/api/runners/gamer-yaml/functions') value={functions:[]};
      else if (req.method==='GET' && path==='/api/browser-targets') value=[{id:'browser-multiview-blank'}];
      else if (req.method==='GET' && path.startsWith('/api/packages/hsr/plugins/gamer-yaml/resources/automations/')) {
        const index=names.indexOf(path.split('/').at(-1)); assert(index>=0);
        if (imported && index>0 && !(formalConflict && index===2)) return new Response('{}',{status:404});
        value={content:formalConflict && index===2 ? 'run: [{log: existing user source}]\n' : imported && index===0 ? original : staging[index]};
      } else {
        const {action,values}=JSON.parse(req.body);calls.push(action);assert(readonly.has(action),`unexpected mutating action ${action}`);
        if(action==='live.status') value={stream:automatic.stream,automatic};
        else if(action==='automatic.status') value=automatic;
        else if(action==='automatic.preview') value={files:['one.mp4','two.mp4']};
        else if(action==='authoring.list') value={scripts:draftConflict?[{script_id:names[0]}]:[]};
        else if(action==='authoring.get') { assert.equal(values.script_id,names[0]);value={script:{head_version:'2.1',versions:[{version:'2.1',yaml:'run: [{log: unpublished user edit}]\n',published:false,source:'manual',instruction:'user edit'}]}}; }
        else if(action==='rules.read') value={version:'rules-before',rules:[]};
        else if(action==='channels.read') value={default_channel:null,channels:[{id:'selected',enabled:!disabledChannel,has_key:true}]};
        else if(action==='runs.read') value={version:'notify-before',rules:[{package_id:'hsr',entrypoint:`hsr/${names[0]}`,runner_ids:['gamer-yaml','gamer-yaml-version'],channels:['selected'],enabled:true},...(disabledRule?[{package_id:'hsr',entrypoint:`hsr/${names[1]}`,runner_ids:['gamer-yaml'],channels:['selected'],enabled:false}]:[])]};
      }
      return new Response(JSON.stringify(value),{status:200});
    } catch(error) { return new Response(JSON.stringify({error:error.message}),{status:500}); }
  };
  const previous={fetch:globalThis.fetch,log:console.log,error:console.error,exitCode:process.exitCode,environment:{...process.env}};
  let stdout='',stderr='',exit=0;
  try {
    Object.assign(process.env,{GAMER_ADMIN_TOKEN:'',GAMER_WORKFLOW_BASE:'http://127.0.0.1:19555',GAMER_WORKFLOW_TOKEN_FILE:join(root,'token'),GAMER_WORKFLOW_DRY_RUN:'1',GAMER_WORKFLOW_BACKUP_DIR:'',GAMER_WORKFLOW_DATA_DIR:join(root,'data')});
    globalThis.fetch=fakeFetch;console.log=value=>stdout+=value;console.error=value=>stderr+=value;process.exitCode=0;
    await import(pathToFileURL(join(repo,'tools/live-workflow/configure-local.mjs')).href+'?fixture='+randomUUID());
    exit=process.exitCode||0;
  } catch(error) { exit=1;stderr=String(error); }
  finally { globalThis.fetch=previous.fetch;console.log=previous.log;console.error=previous.error;process.exitCode=previous.exitCode;for(const key of Object.keys(process.env)) if(!(key in previous.environment)) delete process.env[key];Object.assign(process.env,previous.environment);assert(resolve(root).startsWith(join(repo,'server/target')+sep)&&basename(root).startsWith('gamer-workflow-readonly-'));await rm(root,{recursive:true}); }
  return {exit,stdout,stderr,calls};
}
test('already-published candidate is idempotent and reads the real automatic.queue shape',async()=>{
  const result=await fixture();assert.equal(result.exit,0,result.stderr);
  const report=JSON.parse(result.stdout);assert(report.dry_run);assert(report.resources.every(resource=>resource.unchanged));assert.equal(report.default_channel,null);assert.deepEqual(report.selected_channels,['selected']);
});
test('formal script without private draft is recognized for import without mutating APIs',async()=>{
  const result=await fixture({imported:true});assert.equal(result.exit,0,result.stderr);assert(JSON.parse(result.stdout).resources.every(resource=>!resource.unchanged));
});
test('unpublished user draft blocks before any source or rule write',async()=>{
  const result=await fixture({imported:true,draftConflict:true});assert.equal(result.exit,1);assert.match(result.stderr,/unpublished user draft/);
});
test('different existing workflow source remains a conflict',async()=>{
  const result=await fixture({imported:true,formalConflict:true});assert.equal(result.exit,1);assert.match(result.stderr,/already exists with different content/);
});
test('unavailable selected daily channel blocks rather than changing the global default',async()=>{
  const result=await fixture({disabledChannel:true});assert.equal(result.exit,1);assert.match(result.stderr,/notification channel selected is unavailable/);
});
test('failed automatic session with no output or queue work is idle',async()=>{
  const result=await fixture({failed:true});assert.equal(result.exit,0,result.stderr);
});
test('failed automatic session retaining output is refused',async()=>{
  const result=await fixture({failed:true,activeOutput:true});assert.equal(result.exit,1);
});
test('existing disabled workflow notification rule is preserved as a blocking conflict',async()=>{
  const result=await fixture({disabledRule:true});assert.equal(result.exit,1);assert.match(result.stderr,/inactive or incompatible existing notification rule/);
});

test('complete mock import/save/publish preserves history and second application performs no writes',async()=>{
  const root=await mkdtemp(join(repo,'server/target/gamer-workflow-fullmock-'));
  const inputRoot=join(root,'inputs'), dataRoot=join(root,'data');
  await mkdir(join(inputRoot,'staging'),{recursive:true});await mkdir(join(inputRoot,'backup'),{recursive:true});
  await Promise.all(['daily-notify.yaml','daily-test.yaml','live-cloud.yaml'].map((name,index)=>writeFile(join(inputRoot,'staging',name),staging[index])));
  await writeFile(join(inputRoot,'backup/云游戏日常.before-notify.yaml'),original);await writeFile(join(root,'token'),'fixture-only-admin-token');
  const docs=new Map(), formal=new Map([[names[0],original]]), calls=[];
  let rules={version:'rules-0',rules:[]}, notifications={version:'notifications-0',rules:[{package_id:'hsr',entrypoint:`hsr/${names[0]}`,runner_ids:['gamer-yaml','gamer-yaml-version'],channels:['selected'],enabled:true}]};
  const queue={current:null,waiting:[],target:null,automatic_video_folder:''};
  const automatic={state:'stopped',stream:null,video_folder:'',queue};
  async function persist(doc) {
    const directory=join(dataRoot,'extension-data/gamer-yaml/authoring/hsr');await mkdir(directory,{recursive:true});
    await writeFile(join(directory,createHash('sha256').update(doc.script_id).digest('hex')+'.json'),JSON.stringify(doc));
  }
  const fakeFetch=async(url,req)=>{
    try {
      assert.equal(req.headers['X-Admin-Token'],'fixture-only-admin-token');const path=decodeURIComponent(new URL(url).pathname);let value;
      if(req.method==='GET' && path==='/api/extensions') value={extensions:[['gamer-yaml','0.3.18'],['gamer-live','0.2.9'],['gamer-notify','0.1.1']].map(([id,active_version])=>({id,active_version,state:'running',last_error:null}))};
      else if(req.method==='GET' && path==='/api/runners/gamer-yaml/functions') value={functions:[{name:'hover'},{name:'wait_find',params:[{name:'forever'}]}]};
      else if(req.method==='GET' && path==='/api/browser-targets') value=[{id:'browser-multiview-blank'}];
      else if(req.method==='GET' && path.startsWith('/api/packages/hsr/plugins/gamer-yaml/resources/automations/')) {
        const id=path.split('/').at(-1);if(!formal.has(id))return new Response('{}',{status:404});value={content:formal.get(id)};
      } else {
        const {action,values}=JSON.parse(req.body);calls.push(action);
        if(action==='live.status') value={stream:null,automatic};
        else if(action==='automatic.status') value=automatic;
        else if(action==='automatic.preview') value={files:['one.mp4','two.mp4']};
        else if(action==='channels.read') value={default_channel:null,channels:[{id:'selected',enabled:true,has_key:true}]};
        else if(action==='runs.read') value=notifications;
        else if(action==='rules.read') value=rules;
        else if(action==='authoring.import' || action==='authoring.create') {
          assert(!docs.has(values.script_id));assert.equal(formal.has(values.script_id),action==='authoring.import');
          const versions=formal.has(values.script_id)?[{version:'1.0',yaml:formal.get(values.script_id),source:'library',instruction:'',published:true,diagnostics:[]}]:[];
          const doc={script_id:values.script_id,versions,published_version:versions.length?'1.0':null,head_version:versions.at(-1)?.version||''};docs.set(values.script_id,doc);await persist(doc);value={script:doc};
        } else if(action==='authoring.save') {
          const doc=docs.get(values.script_id);assert.equal(values.expected_version,doc.head_version);
          const version=`${doc.published_version?'1':'0'}.${doc.versions.length+1}`;
          doc.versions.push({version,yaml:values.yaml,instruction:values.instruction,source:'manual',published:false,diagnostics:[]});doc.head_version=version;await persist(doc);value={script:doc};
        } else if(action==='authoring.publish') {
          const doc=docs.get(values.script_id);assert.equal(values.expected_version,doc.head_version);assert.equal(values.version,doc.head_version);
          const source=doc.versions.at(-1), version=doc.published_version?'2.0':'1.0';doc.versions.push({...source,version,source:'publish',published:true});doc.published_version=version;doc.head_version=version;formal.set(values.script_id,source.yaml);await persist(doc);value={script:doc};
        } else if(action==='rules.save') {assert.equal(values.expected_version,rules.version);rules={version:'rules-1',rules:values.ruleset.rules};value={version:rules.version};}
        else if(action==='queue.configure') {queue.target={device_id:values.device_id,package_id:values.package_id};queue.automatic_video_folder=values.video_folder;automatic.video_folder=values.video_folder;value={ok:true};}
        else if(action==='runs.save') {assert.equal(values.expected_version,notifications.version);notifications={version:`notifications-${notifications.rules.length}`,rules:[...notifications.rules,values.rule]};value=notifications;}
        else throw new Error(`Unexpected API/action: ${path} ${action}`);
      }
      return new Response(JSON.stringify(value),{status:200});
    }catch(error){return new Response(JSON.stringify({error:error.message}),{status:500});}
  };
  const previous={fetch:globalThis.fetch,log:console.log,error:console.error,exitCode:process.exitCode,environment:{...process.env}};
  let errors='';
  try {
    Object.assign(process.env,{GAMER_ADMIN_TOKEN:'',GAMER_WORKFLOW_BASE:'http://127.0.0.1:19555',GAMER_WORKFLOW_TOKEN_FILE:join(root,'token'),GAMER_WORKFLOW_DRY_RUN:'0',GAMER_WORKFLOW_BACKUP_DIR:'',GAMER_WORKFLOW_DIR:inputRoot,GAMER_WORKFLOW_DATA_DIR:dataRoot});
    globalThis.fetch=fakeFetch;console.log=()=>{};console.error=value=>errors+=value;process.exitCode=0;
    await import(pathToFileURL(join(repo,'tools/live-workflow/configure-local.mjs')).href+'?fullmock='+randomUUID());
    assert.equal(process.exitCode,0,errors);
    assert.equal(calls.filter(action=>action==='authoring.import').length,1);
    assert.equal(calls.filter(action=>action==='authoring.create').length,2);
    assert.equal(calls.filter(action=>action==='authoring.save').length,3);
    assert.equal(calls.filter(action=>action==='authoring.publish').length,3);
    assert(calls.includes('rules.save'));assert(calls.includes('queue.configure'));assert.equal(calls.filter(action=>action==='runs.save').length,2);
    assert.equal(docs.get(names[0]).versions[0].yaml,original);assert.equal(docs.get(names[0]).versions[0].published,true);
    for(const [index,name] of names.entries()) assert.equal(formal.get(name),staging[index]);
    const before=JSON.stringify([...docs.values()]), writes=calls.filter(action=>!readonly.has(action)).length;
    await import(pathToFileURL(join(repo,'tools/live-workflow/configure-local.mjs')).href+'?fullmock='+randomUUID());
    assert.equal(process.exitCode,0,errors);assert.equal(JSON.stringify([...docs.values()]),before);assert.equal(calls.filter(action=>!readonly.has(action)).length,writes);
    assert(notifications.rules.every(rule=>rule.channels[0]==='selected'));
  } finally {
    globalThis.fetch=previous.fetch;console.log=previous.log;console.error=previous.error;process.exitCode=previous.exitCode;for(const key of Object.keys(process.env))if(!(key in previous.environment))delete process.env[key];Object.assign(process.env,previous.environment);
    assert(resolve(root).startsWith(join(repo,'server/target')+sep)&&basename(root).startsWith('gamer-workflow-fullmock-'));await rm(root,{recursive:true});
  }
});
