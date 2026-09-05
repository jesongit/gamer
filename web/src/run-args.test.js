import { afterEach, describe, expect, it, vi } from 'vitest'
import { api } from './api'
import { runYamlFunction, runYamlScript } from './gamer-yaml-runner'
import { useRunArgsFlow } from './composables/useRunArgsFlow'
import { readFileSync } from 'node:fs'

/**
 * 运行参数链路（mock fetch / 注入 loadParams）：
 * - runYamlScript / runYamlFunction 请求体断言（稀疏 args、start_index、URL 整体编码）；
 * - useRunArgsFlow 状态机（P12.3 起参数声明经服务端 entrypoint schema API）：
 *   无参数直跑 / 有参数弹表单 / 400 invalid_args 回填字段 / 409 交还宿主 /
 *   覆盖建议缓存写入 / schema 加载失败（404/400）结构化错误上抛；
 * - Console 接线静态断言（视图含 WebRTC/设备依赖，按仓库惯例不整体挂载）。
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

describe('runYamlScript / runYamlFunction 请求体（gamer.yaml 经 api.run 统一执行入口）', () => {
  it('runYamlScript：POST /api/runs {runner_id, entrypoint, device_id, payload}；空 args 不携带', async () => {
    const calls = stubFetch([
      { method: 'POST', url: '/api/runs', body: { run_id: 'r1', state: 'starting' } },
    ])
    await runYamlScript('com.demo/main.yaml', 'dev1', 2, { timeout: '10s', pos: [0.1, 0.2] })
    expect(calls[0]).toEqual({
      url: '/api/runs',
      method: 'POST',
      body: {
        runner_id: 'gamer.yaml',
        entrypoint: 'com.demo/main.yaml',
        device_id: 'dev1',
        payload: { start_index: 2, args: { timeout: '10s', pos: [0.1, 0.2] } },
      },
    })
    await runYamlScript('com.demo/main.yaml', 'dev1', 0, {})
    expect(calls[1].body.payload).toEqual({}) // 稀疏空映射 → 省略
  })

  it('runYamlFunction：entrypoint = "<file id>#<函数名>"；payload = {start_index?, args?}', async () => {
    const calls = stubFetch([
      { method: 'POST', url: '/api/runs', body: { run_id: 'r2', state: 'starting' } },
    ])
    await runYamlFunction('com.demo/common.yaml', 'dev1', {
      function: 'login', start_index: 1, args: { account: 'a.png' },
    })
    expect(calls[0].body).toEqual({
      runner_id: 'gamer.yaml',
      entrypoint: 'com.demo/common.yaml#login',
      device_id: 'dev1',
      payload: { start_index: 1, args: { account: 'a.png' } },
    })
    await runYamlFunction('com.demo/common.yaml', 'dev2', {})
    expect(calls[1].body).toEqual({
      runner_id: 'gamer.yaml',
      entrypoint: 'com.demo/common.yaml',
      device_id: 'dev2',
      payload: {},
    }) // function/start_index/args 全省略 → 文件第一个函数从头跑
  })

  it('api.run 对 runner 无知；缺 runner_id/entrypoint 客户端即拒', async () => {
    stubFetch([
      { method: 'POST', url: '/api/runs', body: { run_id: 'r3', state: 'starting' } },
    ])
    await api.run({ runner_id: 'thirdparty.macro', entrypoint: 'macro://boot', device_id: 'dev9', payload: { steps: 3 } })
    expect(true).toBe(true) // 未知 runner 原样透传不报错（Core 不认识具体 runner）
    await expect(api.run({ runner_id: '', entrypoint: 'x' })).rejects.toMatchObject({ code: 'invalid_argument' })
    await expect(api.run({ runner_id: 'a', entrypoint: '' })).rejects.toMatchObject({ code: 'invalid_argument' })
  })

  it('runYamlScript 400 invalid_args：err.status/err.data.diagnostics 可取', async () => {
    stubFetch([
      {
        method: 'POST', url: '/api/runs', status: 400,
        body: { error: 'invalid_args', diagnostics: [{ code: 'param.args.missing_required', message: '缺少 account', field: 'account' }] },
      },
    ])
    const err = await runYamlScript('x', 'dev1', 0, {}).then(() => null, e => e)
    expect(err.status).toBe(400)
    expect(err.data.error).toBe('invalid_args')
    expect(err.data.diagnostics[0].field).toBe('account')
  })
})

// ---- useRunArgsFlow 状态机（P12.3：参数声明经服务端 entrypoint schema API 获取） ----

// 契约 §7 descriptor：account（tmpl，必填）+ timeout（time，默认 30s，带说明）
const DESCRIPTOR_WITH_PARAMS = {
  runner_id: 'gamer.yaml',
  entrypoint: 'com.demo/main.yaml',
  kind: 'script',
  format: 'yaml-params-v1',
  schema: {
    type: 'object',
    properties: {
      account: { type: 'string', param_type: 'tmpl' },
      timeout: { type: 'string', default: '30s', description: '最长等待', param_type: 'time' },
    },
    required: ['account'],
  },
  signature: 'psig1|tmpl,account,0,|time,timeout,0,30s',
}

const EMPTY_DESCRIPTOR = {
  kind: 'script',
  format: 'yaml-params-v1',
  schema: { type: 'object', properties: {}, required: [] },
  signature: 'psig1|',
}

const BEGIN_OPTS = { id: 's1', runnerId: 'gamer.yaml', entrypoint: 'com.demo/main.yaml' }

function memoryStorage() {
  const m = new Map()
  return {
    getItem: k => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => m.set(k, v),
    removeItem: k => m.delete(k),
  }
}

describe('useRunArgsFlow', () => {
  it('默认 loadParams 走 api.getEntrypointParams：GET entrypoint descriptor（整体编码），schema 适配进弹窗', async () => {
    const calls = stubFetch([
      { method: 'GET', url: '/api/runners/gamer.yaml/entrypoint', body: DESCRIPTOR_WITH_PARAMS },
      { method: 'POST', url: '/api/runs', body: { run_id: 'r9' } },
    ])
    const exec = vi.fn().mockResolvedValue({ run_id: 'r9', state: 'starting' })
    const flow = useRunArgsFlow({ exec, notify: () => {} })
    const r = await flow.begin({ ...BEGIN_OPTS, startIndex: 1 })
    expect(r).toEqual({ form: true })
    expect(calls[0].url).toBe('/api/runners/gamer.yaml/entrypoint?entrypoint=com.demo%2Fmain.yaml')
    expect(flow.modal.params.map(p => [p.name, p.type, p.default])).toEqual([
      ['account', 'tmpl', null],
      ['timeout', 'time', '30s'],
    ])
  })

  it('无参数声明：跳过表单直接 exec（args 省略），notify 带摘要（无参数为空）', async () => {
    const exec = vi.fn().mockResolvedValue({ run_id: 'r1', state: 'starting' })
    const loadParams = vi.fn().mockResolvedValue(EMPTY_DESCRIPTOR)
    const notes = []
    const flow = useRunArgsFlow({ exec, notify: n => notes.push(n), loadParams })
    const r = await flow.begin({ ...BEGIN_OPTS, name: 'main.yaml' })
    expect(r).toEqual({ form: false })
    expect(loadParams).toHaveBeenCalledWith({ runnerId: 'gamer.yaml', entrypoint: 'com.demo/main.yaml' })
    expect(exec).toHaveBeenCalledWith(expect.objectContaining({ id: 's1', args: undefined, startIndex: 0 }))
    expect(flow.modal.open).toBe(false)
    expect(notes[0].summary).toBe('')
  })

  it('有参数声明：打开表单；confirm 提交稀疏 args 并写入建议缓存', async () => {
    const exec = vi.fn().mockResolvedValue({ run_id: 'r2', state: 'starting', resolved_args: { account: 'a.png', timeout: '10s' } })
    const loadParams = vi.fn().mockResolvedValue(DESCRIPTOR_WITH_PARAMS)
    const storage = memoryStorage()
    const notes = []
    const flow = useRunArgsFlow({ exec, notify: n => notes.push(n), storage, loadParams })
    const r = await flow.begin({ ...BEGIN_OPTS, startIndex: 1 })
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
    const loadParams = vi.fn().mockResolvedValue(DESCRIPTOR_WITH_PARAMS)
    const storage = memoryStorage()
    const flow = useRunArgsFlow({ exec, notify: () => {}, storage, loadParams })
    await flow.begin(BEGIN_OPTS)
    await flow.confirm({ timeout: '10s' })
    await flow.begin(BEGIN_OPTS)
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
    const flow = useRunArgsFlow({ exec, notify: () => {}, loadParams: vi.fn().mockResolvedValue(DESCRIPTOR_WITH_PARAMS) })
    await flow.begin(BEGIN_OPTS)
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
    const flow = useRunArgsFlow({ exec, notify: () => {}, loadParams: vi.fn().mockResolvedValue(EMPTY_DESCRIPTOR) })
    await expect(flow.begin(BEGIN_OPTS)).rejects.toMatchObject({ status: 400 })
    expect(flow.modal.open).toBe(false)
    expect(flow.modal.fieldErrors).toEqual({}) // 无处展示的诊断不再写入，由宿主 toast/日志呈现
  })

  it('409 设备占用等其他错误：关闭表单并原样抛回宿主', async () => {
    const exec = vi.fn().mockRejectedValue(Object.assign(new Error('HTTP 409'), {
      status: 409,
      data: { error: 'device_busy', script_id: 'other', source: 'scheduled' },
    }))
    const flow = useRunArgsFlow({ exec, notify: () => {}, loadParams: vi.fn().mockResolvedValue(DESCRIPTOR_WITH_PARAMS) })
    await flow.begin(BEGIN_OPTS)
    await expect(flow.confirm({})).rejects.toMatchObject({ status: 409 })
    expect(flow.modal.open).toBe(false)
  })

  it('表单打开/提交中忽略重复 begin（防重复弹窗与双击）', async () => {
    const exec = vi.fn().mockResolvedValue({ run_id: 'r4' })
    const flow = useRunArgsFlow({ exec, notify: () => {}, loadParams: vi.fn().mockResolvedValue(DESCRIPTOR_WITH_PARAMS) })
    await flow.begin(BEGIN_OPTS)
    const second = await flow.begin(BEGIN_OPTS)
    expect(second).toEqual({ form: false, busy: true })
    expect(flow.modal.open).toBe(true)
  })

  it('schema 加载中忽略重复 begin（descriptor 只取一次，防双击并发）', async () => {
    let resolveLoad
    const loadParams = vi.fn().mockImplementation(
      () => new Promise((resolve) => { resolveLoad = resolve }),
    )
    const flow = useRunArgsFlow({ exec: vi.fn(), notify: () => {}, loadParams })
    const first = flow.begin(BEGIN_OPTS)
    expect(await flow.begin(BEGIN_OPTS)).toEqual({ form: false, busy: true })
    expect(loadParams).toHaveBeenCalledTimes(1)
    resolveLoad(DESCRIPTOR_WITH_PARAMS)
    expect(await first).toEqual({ form: true })
  })
})

describe('useRunArgsFlow schema 加载失败（P12.3：不弹参数框，结构化错误上抛）', () => {
  const base = (data, status) => Object.assign(new Error(`HTTP ${status}`), { status, data })

  it('404 not_found：报「运行目标不存在」并携带资源 id；弹窗保持关闭', async () => {
    const flow = useRunArgsFlow({
      exec: vi.fn(),
      notify: () => {},
      loadParams: vi.fn().mockRejectedValue(base({ error: 'not_found', resource: 'com.demo/gone.yaml' }, 404)),
    })
    await expect(flow.begin(BEGIN_OPTS)).rejects.toMatchObject({
      code: 'not_found',
      message: expect.stringContaining('com.demo/gone.yaml'),
    })
    expect(flow.modal.open).toBe(false)
  })

  it('400 invalid_script：报「参数声明无法解析」，diagnostics 随错误携带、消息含首条诊断', async () => {
    const diagnostics = [
      { code: 'yaml.v3.step.type', message: '步骤类型 unknown 不可识别', step_path: 'steps[2]' },
    ]
    const flow = useRunArgsFlow({
      exec: vi.fn(),
      notify: () => {},
      loadParams: vi.fn().mockRejectedValue(base({ error: 'invalid_script', resource: 'com.demo/main.yaml', diagnostics }, 400)),
    })
    await expect(flow.begin(BEGIN_OPTS)).rejects.toMatchObject({
      code: 'invalid_script',
      message: expect.stringContaining('步骤类型 unknown 不可识别'),
      diagnostics,
    })
    expect(flow.modal.open).toBe(false)
  })

  it('404 runner_not_found：报「执行器未注册」', async () => {
    const flow = useRunArgsFlow({
      exec: vi.fn(),
      notify: () => {},
      loadParams: vi.fn().mockRejectedValue(base({ error: 'runner_not_found', runner_id: 'gamer.yaml' }, 404)),
    })
    await expect(flow.begin(BEGIN_OPTS)).rejects.toMatchObject({
      code: 'runner_not_found',
      message: expect.stringContaining('gamer.yaml'),
    })
  })

  it('网络错误等其他失败：原样上抛（不套壳）', async () => {
    const raw = base({ code: 'network_error' }, 0)
    const flow = useRunArgsFlow({ exec: vi.fn(), notify: () => {}, loadParams: vi.fn().mockRejectedValue(raw) })
    await expect(flow.begin(BEGIN_OPTS)).rejects.toBe(raw)
  })
})

// ---- Console / ScriptRunner 接线静态断言（视图不整体挂载，仓库惯例） ----

describe('Console 运行参数接线', () => {
  const read = p => readFileSync(new URL(p, import.meta.url), 'utf8')
  const consoleSrc = read('./views/Console.vue')
  // Console 拆分后：运行参数流程实现移入脚本运行 composable
  const runnerSrc = read('./components/console/useConsoleScriptRunner.js')

  it('Console：运行入口走参数流程（弹窗 + 稀疏 args + 409 冲突 + 摘要日志）', () => {
    expect(runnerSrc).toContain("import { useRunArgsFlow } from '../../composables/useRunArgsFlow'")
    expect(consoleSrc).toContain("import RunParamsModal from '../components/RunParamsModal.vue'")
    expect(consoleSrc).toContain('<RunParamsModal')
    expect(runnerSrc).toContain('runYamlScript(id, store.deviceId, startIndex, args)')
    expect(runnerSrc).toContain("import { GAMER_YAML_RUNNER_ID, runYamlFunction, runYamlScript } from '../../gamer-yaml-runner'")
    expect(runnerSrc).toContain('function onRunArgsSubmit(')
    expect(runnerSrc).toContain('runArgsFlow.confirm(args).catch(handleRunStartError)')
    expect(runnerSrc).toContain('function handleRunStartError(')
    expect(runnerSrc).toContain('isDeviceBusyConflict(e)')
    // resolved_args 摘要进运行日志区（「默认继承/显式覆盖」来源标注）
    expect(runnerSrc).toContain('if (summary) pushLog(\'info\', summary)')
    // 上下文透传（弹窗在 Console 根部渲染，绑定 runArgsFlow.modal）
    expect(runnerSrc).toContain('runArgsFlow, onRunArgsSubmit,')
    expect(consoleSrc).toContain(':field-errors="runArgsFlow.modal.fieldErrors"')
    expect(consoleSrc).toContain('@submit="onRunArgsSubmit"')
  })

  it('P12.3：begin 改传 {runnerId, entrypoint}（schema 经服务端获取，不再传 yaml 源码）', () => {
    expect(runnerSrc).not.toMatch(/begin\(\{[^}]*yaml:/s)
    // 脚本：entrypoint = 脚本资源 id；函数：与 runYamlFunction 拼装同形态（<file>#<函数名>）
    expect(runnerSrc).toContain('runnerId: GAMER_YAML_RUNNER_ID')
    expect(runnerSrc).toContain('entrypoint: s.id')
    expect(runnerSrc).toContain("entrypoint: `${f.id}#${fnName}`")
  })

  it('api.js：提供 runner 无关的 entrypoint schema 读取（URL 整体编码）', () => {
    expect(read('./api.js')).toContain('getEntrypointParams: (runnerId, entrypoint)')
  })
})
