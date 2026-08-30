import { afterEach, describe, expect, it, vi } from 'vitest'
import { api } from './api'
import { useRunArgsFlow } from './composables/useRunArgsFlow'
import { readFileSync } from 'node:fs'

/**
 * 阶段 5 运行参数链路（mock fetch）：
 * - api.runScript / api.runFunction 请求体断言（稀疏 args、start_index、URL 整体编码）；
 * - useRunArgsFlow 状态机：无参数直跑 / 有参数弹表单 / 400 invalid_args 回填字段 /
 *   409 交还宿主 / 覆盖建议缓存写入；
 * - Console / ScriptEditor 接线静态断言（视图含 WebRTC/设备依赖，按仓库惯例不整体挂载）。
 */

// ---- fetch stub：按「METHOD 前缀匹配」注册响应，记录全部调用 ----

function stubFetch(routes = []) {
  const calls = []
  const fn = vi.fn(async (url, opt = {}) => {
    const method = opt.method || 'GET'
    const body = opt.body ? JSON.parse(opt.body) : null
    calls.push({ url: String(url), method, body })
    const hit = routes.find(r => method === r.method && String(url).startsWith(r.url))
    if (!hit) throw new Error(`unexpected fetch: ${method} ${url}`)
    const status = hit.status || 200
    return {
      ok: status < 400,
      status,
      headers: { get: (k) => (String(k).toLowerCase() === 'content-type' ? 'application/json' : null) },
      json: async () => (hit.body === undefined ? {} : (typeof hit.body === 'function' ? hit.body(calls.length) : hit.body)),
      blob: async () => new Blob(),
    }
  })
  vi.stubGlobal('fetch', fn)
  return calls
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('api.runScript / api.runFunction 请求体（阶段 5 契约）', () => {
  it('runScript：body = {device_id, start_index, args}；空 args 不携带', async () => {
    const calls = stubFetch([
      { method: 'POST', url: '/api/scripts/com.demo%2Fmain.yaml/run', body: { run_id: 'r1', state: 'starting' } },
    ])
    await api.runScript('com.demo/main.yaml', 'dev1', 2, { timeout: '10s', pos: [0.1, 0.2] })
    expect(calls[0]).toEqual({
      url: '/api/scripts/com.demo%2Fmain.yaml/run',
      method: 'POST',
      body: { device_id: 'dev1', start_index: 2, args: { timeout: '10s', pos: [0.1, 0.2] } },
    })
    await api.runScript('com.demo/main.yaml', 'dev1', 0, {})
    expect(calls[1].body).toEqual({ device_id: 'dev1', start_index: 0 }) // 稀疏空映射 → 省略
  })

  it('runFunction：URL 整体编码文件 id；body = {device_id, function?, start_index?, args?}', async () => {
    const calls = stubFetch([
      { method: 'POST', url: '/api/functions/com.demo%2Fcommon.yaml/run', body: { run_id: 'r2', state: 'starting' } },
    ])
    await api.runFunction('com.demo/common.yaml', 'dev1', {
      function: 'login', start_index: 1, args: { account: 'a.png' },
    })
    expect(calls[0].body).toEqual({
      device_id: 'dev1', function: 'login', start_index: 1, args: { account: 'a.png' },
    })
    await api.runFunction('com.demo/common.yaml', 'dev2', {})
    expect(calls[1].body).toEqual({ device_id: 'dev2' }) // 全省略 → 文件第一个函数从头跑
  })

  it('runScript 400 invalid_args：err.status/err.data.diagnostics 可取', async () => {
    stubFetch([
      {
        method: 'POST', url: '/api/scripts/x/run', status: 400,
        body: { error: 'invalid_args', diagnostics: [{ code: 'param.args.missing_required', message: '缺少 account', field: 'account' }] },
      },
    ])
    const err = await api.runScript('x', 'dev1', 0, {}).then(() => null, e => e)
    expect(err.status).toBe(400)
    expect(err.data.error).toBe('invalid_args')
    expect(err.data.diagnostics[0].field).toBe('account')
  })
})

// ---- useRunArgsFlow 状态机 ----

const SCRIPT_WITH_PARAMS = [
  'params:',
  "  - 'tmpl:account:账号模板'",
  "  - 'time:timeout:最长等待:30s'",
  'steps:',
  '  - log: hi',
].join('\n')

const PLAIN_SCRIPT = 'steps:\n  - log: hi\n'

function memoryStorage() {
  const m = new Map()
  return {
    getItem: k => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => m.set(k, v),
    removeItem: k => m.delete(k),
  }
}

describe('useRunArgsFlow', () => {
  it('无参数声明：跳过表单直接 exec（args 省略），notify 带摘要（无参数为空）', async () => {
    const exec = vi.fn().mockResolvedValue({ run_id: 'r1', state: 'starting' })
    const notes = []
    const flow = useRunArgsFlow({ exec, notify: n => notes.push(n) })
    const r = await flow.begin({ id: 's1', name: 'main.yaml', yaml: PLAIN_SCRIPT })
    expect(r).toEqual({ form: false })
    expect(exec).toHaveBeenCalledWith(expect.objectContaining({ id: 's1', args: undefined, startIndex: 0 }))
    expect(flow.modal.open).toBe(false)
    expect(notes[0].summary).toBe('')
  })

  it('有参数声明：打开表单；confirm 提交稀疏 args 并写入建议缓存', async () => {
    const exec = vi.fn().mockResolvedValue({ run_id: 'r2', state: 'starting', resolved_args: { account: 'a.png', timeout: '10s' } })
    const storage = memoryStorage()
    const notes = []
    const flow = useRunArgsFlow({ exec, notify: n => notes.push(n), storage })
    const r = await flow.begin({ id: 's1', yaml: SCRIPT_WITH_PARAMS, startIndex: 1 })
    expect(r).toEqual({ form: true })
    expect(flow.modal.open).toBe(true)
    expect(flow.modal.params.map(p => p.name)).toEqual(['account', 'timeout'])
    expect(flow.modal.suggestions).toEqual({}) // 建议缓存初读
    const done = await flow.confirm({ timeout: '10s' })
    expect(done.ok).toBe(true)
    expect(exec).toHaveBeenCalledWith(expect.objectContaining({ args: { timeout: '10s' }, startIndex: 1 }))
    expect(storage.getItem('gb_run_args:s1')).toBe(JSON.stringify({ timeout: '10s' }))
    expect(notes[0].summary).toContain('timeout=10s（覆盖）')
    expect(notes[0].summary).toContain('account=a.png（必填）')
    expect(flow.modal.open).toBe(false)
  })

  it('再次 begin 复读建议缓存（仅作覆盖态预填来源）', async () => {
    const exec = vi.fn().mockResolvedValue({ run_id: 'r3' })
    const storage = memoryStorage()
    const flow = useRunArgsFlow({ exec, notify: () => {}, storage })
    await flow.begin({ id: 's1', yaml: SCRIPT_WITH_PARAMS })
    await flow.confirm({ timeout: '10s' })
    await flow.begin({ id: 's1', yaml: SCRIPT_WITH_PARAMS })
    expect(flow.modal.open).toBe(true)
    expect(flow.modal.suggestions).toEqual({ timeout: '10s' })
  })

  it('400 invalid_args：诊断映射到字段回填表单标红，弹窗保持打开', async () => {
    const exec = vi.fn().mockRejectedValue(Object.assign(new Error('HTTP 400'), {
      status: 400,
      data: {
        error: 'invalid_args',
        diagnostics: [{ code: 'param.args.missing_required', message: '缺少必填参数 account', field: 'account' }],
      },
    }))
    const flow = useRunArgsFlow({ exec, notify: () => {} })
    await flow.begin({ id: 's1', yaml: SCRIPT_WITH_PARAMS })
    const r = await flow.confirm({})
    expect(r.ok).toBe(false)
    expect(r.reason).toBe('invalid_args')
    expect(flow.modal.open).toBe(true) // 表单保持打开供修正
    expect(flow.modal.fieldErrors).toEqual({ account: ['缺少必填参数 account'] })
    expect(flow.modal.submitting).toBe(false)
  })

  it('400 invalid_args 但表单未打开（无参数直跑）：原样上抛交宿主提示，不静默吞没', async () => {
    const exec = vi.fn().mockRejectedValue(Object.assign(new Error('HTTP 400'), {
      status: 400,
      data: {
        error: 'invalid_args',
        diagnostics: [{ code: 'param.args.missing_required', message: '缺少必填参数 account', field: 'account' }],
      },
    }))
    const flow = useRunArgsFlow({ exec, notify: () => {} })
    await expect(flow.begin({ id: 's1', yaml: PLAIN_SCRIPT })).rejects.toMatchObject({ status: 400 })
    expect(flow.modal.open).toBe(false)
    expect(flow.modal.fieldErrors).toEqual({}) // 无处展示的诊断不再写入，由宿主 toast/日志呈现
  })

  it('409 设备占用等其他错误：关闭表单并原样抛回宿主', async () => {
    const exec = vi.fn().mockRejectedValue(Object.assign(new Error('HTTP 409'), {
      status: 409,
      data: { error: 'device_busy', script_id: 'other', source: 'scheduled' },
    }))
    const flow = useRunArgsFlow({ exec, notify: () => {} })
    await flow.begin({ id: 's1', yaml: SCRIPT_WITH_PARAMS })
    await expect(flow.confirm({})).rejects.toMatchObject({ status: 409 })
    expect(flow.modal.open).toBe(false)
  })

  it('表单打开/提交中忽略重复 begin（防重复弹窗与双击）', async () => {
    const exec = vi.fn().mockResolvedValue({ run_id: 'r4' })
    const flow = useRunArgsFlow({ exec, notify: () => {} })
    await flow.begin({ id: 's1', yaml: SCRIPT_WITH_PARAMS })
    const second = await flow.begin({ id: 's1', yaml: SCRIPT_WITH_PARAMS })
    expect(second).toEqual({ form: false, busy: true })
    expect(flow.modal.open).toBe(true)
  })
})

// ---- Console / ScriptEditor / ScriptRunner 接线静态断言（视图不整体挂载，仓库惯例） ----

describe('Console / ScriptEditor 运行参数接线', () => {
  const read = p => readFileSync(new URL(p, import.meta.url), 'utf8')
  const consoleSrc = read('./views/Console.vue')
  const editorSrc = read('./views/ScriptEditor.vue')

  it('Console：运行入口走参数流程（弹窗 + 稀疏 args + 409 冲突 + 摘要日志）', () => {
    expect(consoleSrc).toContain("import { useRunArgsFlow } from '../composables/useRunArgsFlow'")
    expect(consoleSrc).toContain("import RunParamsModal from '../components/RunParamsModal.vue'")
    expect(consoleSrc).toContain('<RunParamsModal')
    expect(consoleSrc).toContain('api.runScript(id, store.deviceId, startIndex, args)')
    expect(consoleSrc).toContain('function onRunArgsSubmit(')
    expect(consoleSrc).toContain('runArgsFlow.confirm(args).catch(handleRunStartError)')
    expect(consoleSrc).toContain('function handleRunStartError(')
    expect(consoleSrc).toContain('isDeviceBusyConflict(e)')
    // resolved_args 摘要进运行日志区（「默认继承/显式覆盖」来源标注）
    expect(consoleSrc).toContain('if (summary) pushLog(\'info\', summary)')
    // 上下文透传（弹窗在 Console 根部渲染，绑定 runArgsFlow.modal）
    expect(consoleSrc).toContain('runArgsFlow, onRunArgsSubmit,')
    expect(consoleSrc).toContain(':field-errors="runArgsFlow.modal.fieldErrors"')
    expect(consoleSrc).toContain('@submit="onRunArgsSubmit"')
  })

  it('ScriptEditor：脚本运行与函数测试均走参数流程；函数测试用 functions run 接口', () => {
    expect(editorSrc).toContain("import { useRunArgsFlow } from '../composables/useRunArgsFlow'")
    expect(editorSrc).toContain('<RunParamsModal')
    expect(editorSrc).toContain('api.runFunction(id, store.deviceId, {')
    expect(editorSrc).toContain('function: fnName')
    // 「从此步骤测试」：画布 test-from → startIndexOf → start_index
    expect(editorSrc).toContain(':test-from="tab === \'func\'"')
    expect(editorSrc).toContain('function onTestFrom(')
    expect(editorSrc).toContain('startIndexOf(shell.model, uuid)')
    expect(editorSrc).toContain('function beginTestFn(')
    expect(editorSrc).toContain('onTestArgsSubmit')
  })
})
