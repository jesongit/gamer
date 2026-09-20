// 仅连接 start-acceptance.ps1 启动的隔离实例；复用真实前端 API、编辑器与 Rust REST。
import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { createServer } from '../../web/node_modules/vite/dist/node/index.js'

const root = fileURLToPath(new URL('../../', import.meta.url))
const qa = new URL('../../server/target/yaml-acceptance/', import.meta.url)
const base = 'http://127.0.0.1:18443'
const originalFetch = globalThis.fetch
const login = await originalFetch(`${base}/api/login`, {
  method: 'POST', headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ username: 'admin', password: (await readFile(new URL('password.txt', qa), 'utf8')).trim() }),
})
assert.equal(login.status, 200, '隔离实例登录失败')
const cookie = login.headers.get('set-cookie').split(';')[0]
globalThis.fetch = (url, options = {}) => originalFetch(new URL(url, base), {
  ...options, headers: { ...options.headers, Cookie: cookie },
})
const vite = await createServer({ root: `${root}/web`, server: { middlewareMode: true }, appType: 'custom' })
const evidence = []
async function check(name, work) {
  await work()
  evidence.push({ name, passed: true })
  console.log(`PASS ${name}`)
}
try {
  const { api } = await vite.ssrLoadModule('/src/api.js')
  const { createEditorShellApi } = await vite.ssrLoadModule('/src/components/console/current-api-adapters.js')
  const { useScriptEditorShell } = await vite.ssrLoadModule('/src/composables/useScriptEditorShell.js')
  const { createCall } = await vite.ssrLoadModule('/src/script-editor/factories.ts')
  const { serialize } = await vite.ssrLoadModule('/src/script-editor/codec.ts')
  const { validateSource } = await vite.ssrLoadModule('/src/script-editor/validation.ts')
  const shell = () => useScriptEditorShell({ api: createEditorShellApi(api) })
  const pkg = `yaml-qa-${Date.now()}`
  await api.createPackage({ id: pkg, name: 'YAML 全链路验收' })
  await check('安装实际 YAML 插件并获取全部 18 个函数 Schema', async () => {
    const extensions = await api.listExtensions()
    if (!(extensions.extensions || []).some(e => e.id === 'gamer-yaml')) {
      const archive = await readFile(`${root}/web/public/plugins/gamer-yaml-3.1.1.gplugin`)
      await api.installExtension(archive, { permissionConfirmed: true, source: 'official' })
    }
    const catalog = await api.getRunnerFunctions('gamer-yaml')
    assert.equal(catalog.functions.length, 18)
    await writeFile(new URL('native-functions.json', qa), JSON.stringify(catalog.functions, null, 2))
  })
  const catalog = await api.getRunnerFunctions('gamer-yaml')
  const knownFunctions = new Set([...catalog.functions.map(f => f.name), 'echo_value', 'nested_echo', 'early_return', '每日任务跳转'])
  const templateBytes = await readFile(`${root}/server/testdata/perf/templates/perf_corner_menu#dr.png`)
  await api.importTemplateBytes('button.png', templateBytes, pkg)
  for (const file of ['_function.yaml', 'flow.yaml', 'native.yaml']) {
    await check(`真实资源保存、读取、列表与编辑器往返：${file}`, async () => {
      const source = await readFile(new URL(file, import.meta.url), 'utf8')
      const kind = file.startsWith('_function') ? 'function_library' : 'script'
      const parsed = validateSource(source, kind, { knownFunctions, resolveTemplate: name => name === 'button.png', resolveParams: name => catalog.functions.find(f => f.name === name)?.params })
      assert.deepEqual(parsed.diagnostics, [])
      const created = kind === 'script'
        ? await api.createScript({ pkg, name: file, content: source })
        : await api.createFunction({ pkg, name: file, content: source })
      assert.equal(created.id, `${pkg}/${file}`)
      const edit = shell()
      if (kind === 'script') await edit.loadScript(created.id)
      else await edit.loadFunctionFile(created.id)
      assert.equal(edit.resourceId, created.id)
      assert.equal(edit.name, file)
      assert.equal(edit.dirty, false)
      assert.equal((await edit.save()).ok, true)
      const reopened = kind === 'script' ? await api.getScript(created.id) : await api.getFunction(created.id)
      assert.equal(reopened.content, serialize(edit.model))
      if (kind === 'script') {
        const list = await api.listScripts(pkg)
        assert.ok(list.some(entry => entry.id === created.id && entry.package === pkg && entry.name === file && entry.content === reopened.content), JSON.stringify(list))
      }
      const descriptor = await api.getEntrypointParams('gamer-yaml', kind === 'script' ? created.id : `${pkg}#echo_value`)
      assert.equal(descriptor.format, 'yaml-params-v1')
      if (file === 'flow.yaml') assert.equal(descriptor.schema.length, 11)
    })
  }
  await check('新建脚本 → 连续自动保存 → 仅改名 → 重新打开', async () => {
    const edit = shell()
    edit.newScript({ pkg, name: '新建测试.yaml' })
    edit.insertStep(createCall('log', { kind: 'value', cell: { lit: 'first' } }))
    assert.equal((await edit.save({ suppressConflict: true })).ok, true)
    assert.equal(edit.resourceId, `${pkg}/新建测试.yaml`)
    edit.insertStep(createCall('log', { kind: 'value', cell: { lit: 'second' } }))
    assert.equal((await edit.save({ suppressConflict: true })).ok, true)
    edit.scriptDisplayName = '改名测试'
    assert.equal(edit.dirty, true)
    assert.equal((await edit.save()).ok, true)
    assert.equal(edit.resourceId, `${pkg}/改名测试.yaml`)
    const reopened = shell()
    await reopened.loadScript(edit.resourceId)
    assert.equal(reopened.scriptDisplayName, '改名测试')
    assert.equal(reopened.model.run.length, 2)
    await assert.rejects(api.getScript(`${pkg}/新建测试.yaml`), error => error.status === 404)
  })
  await check('中文函数名称保存、重开、目录发现、入口参数查询与脚本调用', async () => {
    const edit = shell()
    await edit.loadFunctionFile(`${pkg}/_function.yaml`)
    assert.equal((await edit.save()).ok, true)
    const reopened = shell()
    await reopened.loadFunctionFile(edit.resourceId)
    assert.ok(reopened.model.functions.some(f => f.name === '每日任务跳转'))
    const entries = await api.listFunctions(pkg)
    assert.ok(entries.find(f => f.id === edit.resourceId).functions.includes('每日任务跳转'))
    const descriptor = await api.getEntrypointParams('gamer-yaml', `${pkg}#每日任务跳转`)
    assert.equal(descriptor.schema[0].name, 'value')
    const script = shell()
    script.newScript({ pkg, name: '中文函数调用.yaml' })
    script.insertStep(createCall('每日任务跳转', { kind: 'map', entries: { value: { lit: '已跳转' } } }))
    assert.equal((await script.save()).ok, true)
    await script.loadScript(script.resourceId)
    assert.equal(script.model.run[0].fn, '每日任务跳转')
  })
  await check('新建函数库带 .yaml → 内置调用自动保存 → 再次保存', async () => {
    const edit = shell()
    edit.newFunctionFile({ pkg, file: '_function_new.yaml', functionName: 'wait_button' })
    edit.stack.apply({ type: 'insert_step', path: ['functions', 'wait_button', 'run'], index: 0,
      step: createCall('wait_find', { kind: 'value', cell: { lit: 'button.png' } }, 'hit') })
    assert.equal((await edit.save({ suppressConflict: true })).ok, true)
    assert.equal(edit.resourceId, `${pkg}/_function_new.yaml`)
    assert.equal((await edit.save()).ok, true)
    const entries = await api.listFunctions(pkg)
    assert.deepEqual(entries.find(f => f.id === edit.resourceId).functions, ['wait_button'])
  })
  await check('重复名称和版本冲突不会覆写目标；重命名失败后可以继续保存', async () => {
    const edit = shell()
    await edit.loadScript(`${pkg}/改名测试.yaml`)
    edit.scriptDisplayName = 'flow'
    assert.equal((await edit.save()).ok, false)
    assert.equal(edit.resourceId, `${pkg}/改名测试.yaml`)
    edit.scriptDisplayName = '改名测试'
    edit.insertStep(createCall('log', { kind: 'value', cell: { lit: 'after-conflict' } }))
    assert.equal((await edit.save()).ok, true)
    const stale = shell()
    await stale.loadScript(edit.resourceId)
    edit.insertStep(createCall('log', { kind: 'value', cell: { lit: 'new-version' } }))
    assert.equal((await edit.save()).ok, true)
    stale.insertStep(createCall('log', { kind: 'value', cell: { lit: 'stale' } }))
    assert.equal((await stale.save()).reason, 'conflict')
  })
  await check('服务端拒绝旧 YAML 和非法默认值', async () => {
    for (const source of ['version: 3\nrun: []\n', 'steps: []\n', 'params:\n  count: {type: integer, default: nope}\nrun: []\n']) {
      await assert.rejects(api.createScript({ pkg, name: 'invalid.yaml', content: source }), error => error.status === 400 && error.data.diagnostics.length > 0)
    }
  })
  await check('模板上传、列表、重命名同步改写脚本与函数引用', async () => {
    assert.ok((await api.listTemplates(pkg)).some(t => t.name === 'button.png'))
    await api.renameTemplate('button.png', 'renamed.png', pkg)
    assert.match((await api.getScript(`${pkg}/native.yaml`)).content, /renamed\.png/)
    assert.match((await api.getFunction(`${pkg}/_function_new.yaml`)).content, /renamed\.png/)
    assert.doesNotMatch((await api.getScript(`${pkg}/native.yaml`)).content, /button\.png/)
    await api.renameTemplate('renamed.png', 'button.png', pkg)
    await assert.rejects(api.importTemplateBytes('invalid.png', Buffer.from('invalid'), pkg), error => error.status === 400)
  })
  await check('定时任务的 YAML 入口与参数保存、回读、删除', async () => {
    const created = await api.saveTask({ name: 'YAML 验收任务',
      app: { device_id: 'qa-offline', android_package: 'com.example.qa', content_package: pkg },
      runner: { runner_id: 'gamer-yaml', entrypoint: `${pkg}/flow.yaml`, payload: { args: { count: 3 } } },
      schedule: { provider_id: 'cron', config: { expression: '0 8 * * *' } }, enabled: false,
    })
    const task = await api.getTask(created.id)
    assert.equal(task.runner.entrypoint, `${pkg}/flow.yaml`)
    assert.equal(task.runner.payload.args.count, 3)
    await api.deleteTask(created.id)
  })
  await writeFile(new URL('live-api-report.json', qa), JSON.stringify({ pkg, checks: evidence }, null, 2))
  console.log(`Package retained for browser acceptance: ${pkg}; ${evidence.length} checks passed.`)
} finally {
  globalThis.fetch = originalFetch
  await vite.close()
}
