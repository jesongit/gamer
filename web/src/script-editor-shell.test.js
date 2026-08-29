import { describe, expect, it, vi } from 'vitest'
import { useScriptEditorShell } from './composables/useScriptEditorShell'
import { parseFunctionLibrary, serialize } from './script-editor/codec'
import { createStep } from './script-editor/factories'
import { lit } from './script-editor/model'
import { stripUuids } from './script-editor/__tests__/helpers'

/**
 * 编辑器外壳（阶段 4）：加载 → 编辑（命令栈）→ dirty → 保存（expected_version）→
 * 409 version_conflict 冲突状态 → 重载/覆盖；运行起点映射；函数库编辑往返。
 * 页面（Console / ScriptEditor）只消费 shell 状态，SaveConflictModal 的打开与
 * 重载/覆盖回调即本文件断言的 conflict/reload/overwrite 契约。
 */

const SCRIPT_YAML = `steps:
  - log: 第一
  - tap: [0.5, 0.5]
  - find: a.png
    then:
      - log: 命中
`

const FN_YAML = `login:
  params:
    - 'tmpl:account:账号模板:account.png'
  steps:
    - return: true
`

/** 可变磁盘 + 可编程保存：expected_version 不符 → 409 {code:"version_conflict"}。 */
function makeApi({ scriptConflict = true, fnConflict = false } = {}) {
  const calls = { saveScript: [], updateFunction: [] }
  let script = { id: 'com.demo/main.yaml', name: 'main.yaml', package: 'com.demo', version: 'v1', content: SCRIPT_YAML }
  let fn = { id: 'com.demo/common.yaml', pkg: 'com.demo', file: 'common', version: 'f1', content: FN_YAML, functions: ['login'] }
  const conflict = (resource) => {
    const e = new Error('version conflict')
    e.status = 409
    e.data = { code: 'version_conflict', resource, message: '资源已被其他页面修改' }
    return e
  }
  const api = {
    getScript: vi.fn(async (id) => ({ ...script, id })),
    saveScript: vi.fn(async (payload) => {
      calls.saveScript.push(payload)
      if (scriptConflict && payload.expected_version) throw conflict(payload.id || payload.name)
      script = { ...script, content: payload.content, name: payload.name, version: 'v2' }
      return { id: script.id, name: script.name, package: script.package, version: script.version }
    }),
    getFunction: vi.fn(async () => ({ ...fn })),
    updateFunction: vi.fn(async (id, payload) => {
      calls.updateFunction.push({ id, payload })
      if (fnConflict && payload.expected_version) throw conflict(id)
      fn = { ...fn, content: payload.content, version: 'f2' }
      return { id: fn.id, file: fn.file, pkg: fn.pkg, version: fn.version }
    }),
  }
  return { api, calls, getScript: () => script }
}

describe('useScriptEditorShell：从步骤运行映射（uuid → start_index）', () => {
  it('顶层步骤给序号，嵌套分支步骤返回 null', async () => {
    const { api } = makeApi()
    const shell = useScriptEditorShell({ api })
    await shell.loadScript('com.demo/main.yaml')
    const steps = shell.model.steps
    expect(shell.runStartIndexOf(steps[0].uuid)).toBe(0)
    expect(shell.runStartIndexOf(steps[1].uuid)).toBe(1)
    expect(shell.runStartIndexOf(steps[2].then[0].uuid)).toBeNull()
    expect(shell.runStartIndexOf('no-such-uuid')).toBeNull()
  })
})

describe('useScriptEditorShell：保存 409 冲突（SaveConflictModal 契约）', () => {
  async function loadAndEdit(api) {
    const shell = useScriptEditorShell({ api })
    await shell.loadScript('com.demo/main.yaml')
    shell.insertStep(createStep('wait', { duration: lit('2s') }), '插入等待')
    expect(shell.dirty).toBe(true)
    return shell
  }

  it('保存带 expected_version；409 → conflict 置位，弹窗数据可取', async () => {
    const { api, calls } = makeApi()
    const shell = await loadAndEdit(api)
    const r = await shell.save()
    expect(r.reason).toBe('conflict')
    expect(calls.saveScript[0].expected_version).toBe('v1')
    expect(shell.conflict).toEqual({ resource: 'com.demo/main.yaml', message: '资源已被其他页面修改' })
  })

  it('「重新加载」= reload()：放弃本地修改恢复磁盘版本并清掉冲突', async () => {
    const { api } = makeApi()
    const shell = await loadAndEdit(api)
    await shell.save()
    const r = await shell.reload()
    expect(r.ok).toBe(true)
    expect(shell.conflict).toBeNull()
    expect(shell.dirty).toBe(false)
    expect(shell.model.steps).toHaveLength(3)
    expect(shell.version).toBe('v1')
  })

  it('「强制覆盖」= overwrite()：不带 expected_version 重存，成功后 version/dirty 对齐磁盘', async () => {
    const { api, calls } = makeApi()
    const shell = await loadAndEdit(api)
    await shell.save()
    const r = await shell.overwrite()
    expect(r.ok).toBe(true)
    expect(calls.saveScript[1].expected_version).toBeUndefined()
    expect(shell.conflict).toBeNull()
    expect(shell.dirty).toBe(false)
    expect(shell.version).toBe('v2')
  })

  it('dismissConflict 仅清弹窗状态，保留未保存修改', async () => {
    const { api } = makeApi()
    const shell = await loadAndEdit(api)
    await shell.save()
    shell.dismissConflict()
    expect(shell.conflict).toBeNull()
    expect(shell.dirty).toBe(true)
  })

  it('无冲突保存成功：返回 id/version，dirty 复位', async () => {
    const { api, calls } = makeApi({ scriptConflict: false })
    const shell = await loadAndEdit(api)
    const r = await shell.save()
    expect(r.ok).toBe(true)
    expect(calls.saveScript[0].expected_version).toBe('v1')
    expect(shell.resourceId).toBe('com.demo/main.yaml')
    expect(shell.version).toBe('v2')
    expect(shell.dirty).toBe(false)
  })
})

describe('useScriptEditorShell：函数库编辑（文件 → FunctionLibraryModel → 往返）', () => {
  it('函数级 params（functionParams 容器）+ 函数体插步 → serialize → parse ≡ 当前模型', async () => {
    const { api } = makeApi()
    const shell = useScriptEditorShell({ api })
    await shell.loadFunctionFile('com.demo/common.yaml')
    expect(shell.kind).toBe('function_library')
    expect(shell.editorContext).toBe('function')

    // 函数级 params：insert_param 携带 ['functions', 'login', 'params']
    const path = ['functions', 'login', 'params']
    expect(shell.stack.apply({ type: 'insert_param', path, index: 1, decl: { type: 'text', name: 'tag', remark: '', default: null } }, '添加函数参数')).toBe(true)
    // 函数体插步
    expect(shell.stack.apply({ type: 'insert_step', path: ['functions', 'login', 'steps'], index: 1, step: createStep('log', { message: lit('完成') }) }, '插入日志')).toBe(true)

    const yaml = serialize(shell.model)
    const reparsed = parseFunctionLibrary(yaml, { file: 'common' })
    expect(reparsed.diagnostics).toEqual([])
    expect(stripUuids(JSON.parse(JSON.stringify(reparsed.model)))).toEqual(stripUuids(JSON.parse(JSON.stringify(shell.model))))
    expect(yaml).toContain('text:tag:')
  })

  it('函数库保存走 updateFunction（覆盖更新，带 expected_version）', async () => {
    const { api, calls } = makeApi({ fnConflict: false })
    const shell = useScriptEditorShell({ api })
    await shell.loadFunctionFile('com.demo/common.yaml')
    shell.stack.apply({ type: 'insert_param', path: ['functions', 'login', 'params'], index: 0, decl: { type: 'bool', name: 'dry', remark: '', default: false } }, '添加函数参数')
    const r = await shell.save()
    expect(r.ok).toBe(true)
    expect(calls.updateFunction).toHaveLength(1)
    expect(calls.updateFunction[0].payload.expected_version).toBe('f1')
    expect(shell.version).toBe('f2')
  })
})

describe('useScriptEditorShell：跳转栈（call/func 结构化跳转）', () => {
  it('jumpToScript 压栈当前资源，jumpBack 恢复并还原选中', async () => {
    const { api } = makeApi()
    const shell = useScriptEditorShell({ api })
    await shell.loadScript('com.demo/main.yaml')
    shell.select(shell.model.steps[1].uuid)

    await shell.jumpToScript('com.demo/sub.yaml')
    expect(shell.canJumpBack).toBe(true)
    expect(shell.jumpBackLabel).toBe('com.demo/main.yaml')
    expect(shell.resourceId).toBe('com.demo/sub.yaml')

    await shell.jumpBack()
    expect(shell.resourceId).toBe('com.demo/main.yaml')
    expect(shell.selectedUuid).not.toBeNull()
    expect(shell.canJumpBack).toBe(false)
  })
})
