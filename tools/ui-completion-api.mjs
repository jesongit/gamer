// Isolated acceptance only. Does not connect ADB or write daily service data.
import assert from 'node:assert/strict'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { createServer } from '../web/node_modules/vite/dist/node/index.js'
const base = 'http://127.0.0.1:18443', qa = new URL('../server/target/yaml-acceptance/', import.meta.url)
const originalFetch = globalThis.fetch
const login = await originalFetch(`${base}/api/login`, { method: 'POST', headers: { 'Content-Type':'application/json' }, body: JSON.stringify({ username:'admin', password:(await readFile(new URL('password.txt',qa),'utf8')).trim() }) })
assert.equal(login.status,200)
const cookie = login.headers.get('set-cookie').split(';')[0]
globalThis.fetch = (url, options={}) => originalFetch(new URL(url,base), { ...options, headers:{...options.headers,Cookie:cookie} })
const vite = await createServer({root:new URL('../web/',import.meta.url).pathname.replace(/^\/(\w:)/,'$1'),server:{middlewareMode:true},appType:'custom'})
const evidence=[]
async function check(name, fn) { await fn(); evidence.push({name,passed:true}); console.log('PASS '+name) }
try {
  const {api}=await vite.ssrLoadModule('/src/api.js')
  const archive=await readFile(new URL('../web/public/plugins/gamer-yaml-3.1.2.gplugin',import.meta.url))
  await check('实际 YAML guest 更新到 3.1.2',async()=>{
    const installed=(await api.listExtensions()).extensions || []
    const yaml = installed.find(e=>e.id==='gamer-yaml')
    if(yaml?.version === '3.1.2') assert.equal(yaml.version, '3.1.2')
    else if(yaml) await api.updateExtension('gamer-yaml',archive,{permissionConfirmed:true})
    else await api.installExtension(archive,{permissionConfirmed:true})
  })
  const pkg='ui-completion-'+Date.now()
  await api.createPackage({id:pkg,name:'界面收口验收'})
  const file=await api.createFunction({pkg,name:'_function_extra.yaml',content:'functions:\n  测试函数:\n    run:\n      - log: hello\n'})
  const caller=await api.createScript({pkg,name:'调用测试.yaml',content:'run:\n  - 测试函数: {}\n'})
  await check('拆分库真实写回与乐观并发',async()=>{
    const changed=await api.updateFunction(file.id,{content:'functions:\n  测试函数:\n    run:\n      - log: changed\n',expected_version:file.version})
    assert.notEqual(changed.version,file.version)
    await assert.rejects(api.updateFunction(file.id,{content:file.content,expected_version:file.version}),e=>e.status===409)
  })
  await check('跨文件同名拒绝，引用未修复时禁止删除/改名',async()=>{
    await assert.rejects(api.createFunction({pkg,name:'_function.yaml',content:file.content}),e=>e.status===400)
    const latest=await api.getFunction(file.id)
    await assert.rejects(api.updateFunction(file.id,{content:'functions: {}\n',expected_version:latest.version}),e=>e.status===400)
  })
  await check('删除最后一个函数按版本保留空库',async()=>{
    await api.deleteScript(caller.id)
    const latest=await api.getFunction(file.id)
    await api.updateFunction(file.id,{content:'functions: {}\n',expected_version:latest.version})
    assert.equal((await api.getFunction(file.id)).content,'functions: {}\n')
  })
  const empty=await api.getFunction(file.id)
  await api.updateFunction(file.id,{content:file.content,expected_version:empty.version})
  await check('准备 100/500 步性能验收资源',async()=>{
    for(const count of [100,500]) await api.createScript({pkg,name:`性能-${count}.yaml`,content:'run:\n'+Array.from({length:count},(_,i)=>`  - log: "步骤 ${i+1}"\n`).join('')})
  })
  await check('真实录制历史 API 包含无素材的取消会话',async()=>{
    const root=new URL('data/media/.recording-history/qa-ui-history/',qa)
    await mkdir(root,{recursive:true})
    await writeFile(new URL('session.json',root),JSON.stringify({id:'qa-ui-history',device_id:'验收设备（模拟记录）',state:'cancelled',started_at:'2026-09-19T00:00:00Z',ended_at:'2026-09-19T00:00:01Z',segments:[],event_count:0,error:null}))
    await writeFile(new URL('event-source.txt',root),'qa-empty-media')
    const response=await fetch('/api/recording')
    assert.equal(response.status,200)
    assert.ok((await response.json()).sessions.some(s=>s.id==='qa-ui-history'&&s.state==='cancelled'))
  })
  await writeFile(new URL('completion-results.json',qa),JSON.stringify({pkg,evidence},null,2))
  console.log('PACKAGE '+pkg)
} finally { await vite.close(); globalThis.fetch=originalFetch }
