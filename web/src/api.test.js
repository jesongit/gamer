import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from './api'

function jsonRes(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: name => (/^content-type$/i.test(name) ? 'application/json' : null) },
    json: async () => body,
  }
}

function bodyOf(call = 0) {
  return JSON.parse(fetch.mock.calls[call][1].body)
}

beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn())
})

afterEach(() => vi.unstubAllGlobals())

describe('唯一资源 API surface', () => {
  it('只公开当前 create/update/replace 资源方法，不保留旧 save/upload/脚本级运行方法', () => {
    expect(api).toHaveProperty('createScript')
    expect(api).toHaveProperty('updateScript')
    expect(api).toHaveProperty('createFunction')
    expect(api).toHaveProperty('updateFunction')
    expect(api).toHaveProperty('createTemplate')
    expect(api).toHaveProperty('replaceTemplateImage')
    for (const name of ['saveScript', 'saveFunction', 'uploadTemplate', 'uploadTemplateRegion', 'stopScript', 'scriptStatus']) {
      expect(api).not.toHaveProperty(name)
    }
  })

  it('invalid_yaml 优先展示服务端首条结构化诊断，而不是只显示错误码', async () => {
    fetch.mockResolvedValueOnce(jsonRes(400, {
      error: 'invalid_yaml',
      diagnostics: [{
        code: 'step.field.missing',
        message: 'loop 缺少 steps',
        step_path: 'steps[2]',
        field: 'steps',
      }],
    }))

    await expect(api.createScript({ pkg: 'com.demo', name: 'main.yaml', content: 'steps: []\n' }))
      .rejects.toMatchObject({
        code: 'invalid_yaml',
        message: 'loop 缺少 steps（steps[2].steps）',
        details: [{ code: 'step.field.missing' }],
      })
  })

  it('诊断路径已包含 field 时不重复追加字段名', async () => {
    fetch.mockResolvedValueOnce(jsonRes(400, {
      error: 'invalid_yaml',
      diagnostics: [{
        code: 'step.field.type',
        message: '步骤必须是列表',
        step_path: '登录.steps[7].candidates[1].steps',
        field: 'steps',
      }],
    }))

    await expect(api.createScript({ pkg: 'com.demo', name: 'main.yaml', content: 'steps: []\n' }))
      .rejects.toMatchObject({
        code: 'invalid_yaml',
        message: '步骤必须是列表（登录.steps[7].candidates[1].steps）',
      })
  })

  it('脚本创建 POST 只发送当前字段；更新 PUT 整体编码 id 并携带 expected_version', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/main.yaml' }))
    await api.createScript({ pkg: 'com.demo', name: 'main.yaml', content: 'steps: []\n', id: 'old-id' })
    expect(fetch.mock.calls[0][0]).toBe('/api/apps/com.demo/resources/scripts')
    expect(fetch.mock.calls[0][1].method).toBe('POST')
    expect(bodyOf()).toEqual({ name: 'main.yaml', content: 'steps: []\n' })

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/main.yaml', version: 'v2' }))
    await api.updateScript('com.demo/main.yaml', {
      name: 'renamed.yaml', content: 'steps: []\n', expected_version: 'v1',
    })
    expect(fetch.mock.calls[1][0]).toBe('/api/apps/-/resources/scripts/com.demo%2Fmain.yaml')
    expect(fetch.mock.calls[1][1].method).toBe('PUT')
    expect(bodyOf(1)).toEqual({ content: 'steps: []\n', name: 'renamed.yaml', expected_version: 'v1' })
  })

  it('函数创建 POST 与更新 PUT 分离；缺版本不发请求，force:true 才跳过版本门禁', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/common.yaml' }))
    await api.createFunction({ pkg: 'com.demo', name: 'common', content: 'login:\n  steps: []\n' })
    expect(fetch.mock.calls[0][0]).toBe('/api/apps/com.demo/resources/functions')
    expect(fetch.mock.calls[0][1].method).toBe('POST')
    expect(bodyOf()).toEqual({ name: 'common', content: 'login:\n  steps: []\n' })

    await expect(api.updateFunction('com.demo/common.yaml', { content: 'login:\n  steps: []\n' }))
      .rejects.toMatchObject({ status: 409, code: 'version_required' })
    expect(fetch).toHaveBeenCalledTimes(1)

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/common.yaml', version: 'v3' }))
    await api.updateFunction('com.demo/common.yaml', { content: 'login:\n  steps: []\n', force: true })
    expect(fetch.mock.calls[1][0]).toBe('/api/apps/-/resources/functions/com.demo%2Fcommon.yaml')
    expect(fetch.mock.calls[1][1].method).toBe('PUT')
    expect(bodyOf(1)).toEqual({ content: 'login:\n  steps: []\n', force: true })
  })

  it('模板创建与图片替换：通用资源 API 原始字节 body，客户端组合完整文件名', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true, name: 'shot#100_200_800_900.png' }))
    await api.createTemplate('shot.png', 'QUJD', 'com.demo', [0.1, 0.2, 0.8, 0.9])
    expect(fetch.mock.calls[0][0]).toBe(
      '/api/apps/com.demo/resources/templates?name=shot%23100_200_800_900.png',
    )
    expect(fetch.mock.calls[0][1].method).toBe('POST')
    expect(fetch.mock.calls[0][1].headers['Content-Type']).toBe('image/png')
    expect(new TextDecoder().decode(fetch.mock.calls[0][1].body)).toBe('ABC')

    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true, name: 'color#100_200_800_900#1.png' }))
    await api.createTemplate('color.png', 'QUJD', 'com.demo', [0.1, 0.2, 0.8, 0.9], true)
    expect(fetch.mock.calls[1][0]).toBe(
      '/api/apps/com.demo/resources/templates?name=color%23100_200_800_900%231.png',
    )

    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true, name: 'shot.png' }))
    await api.replaceTemplateImage('shot#100_200_800_900.png', 'REVG', 'com.demo')
    expect(fetch.mock.calls[2][0]).toBe('/api/apps/com.demo/resources/templates/shot%23100_200_800_900.png')
    expect(fetch.mock.calls[2][1].method).toBe('PUT')
    expect(new TextDecoder().decode(fetch.mock.calls[2][1].body)).toBe('DEF')
  })

  it('按键映射详情/更新使用整体编码资源 id，并保留版本门禁', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/combat.yaml' }))
    await api.getKeymap('com.demo/combat.yaml', 'com.demo')
    expect(fetch.mock.calls[0][0]).toBe('/api/apps/com.demo/resources/keymaps/com.demo%2Fcombat.yaml')

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/combat.yaml' }))
    await api.updateKeymap('combat.yaml', 'com.demo', {
      content: 'version: 1\nname: combat\nbindings: []\n',
      expected_version: 'abc123',
    })
    expect(fetch.mock.calls[1][0]).toBe('/api/apps/com.demo/resources/keymaps/com.demo%2Fcombat.yaml')
    expect(bodyOf(1)).toEqual({
      content: 'version: 1\nname: combat\nbindings: []\n',
      expected_version: 'abc123',
    })
  })
})

describe('统一任务 API（P11.1 ADR-12）', () => {
  it('CRUD：GET/POST /api/tasks，PUT/DELETE /api/tasks/:id；启停走 enable/disable', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, []))
    await api.listTasks()
    expect(fetch.mock.calls[0][0]).toBe('/api/tasks')
    expect(fetch.mock.calls[0][1].method).toBe('GET')

    fetch.mockResolvedValueOnce(jsonRes(201, {}))
    const body = { name: 't', runner: { runner_id: 'gamer.yaml', entrypoint: 'p/a.yaml', payload: {} }, schedule: { provider_id: 'cron', config: { expression: '0 8 * * *' } } }
    await api.saveTask(body)
    expect(fetch.mock.calls[1][0]).toBe('/api/tasks')
    expect(fetch.mock.calls[1][1].method).toBe('POST')
    expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual(body)

    fetch.mockResolvedValueOnce(jsonRes(200, {}))
    await api.updateTask('t1', body)
    expect(fetch.mock.calls[2][0]).toBe('/api/tasks/t1')
    expect(fetch.mock.calls[2][1].method).toBe('PUT')

    fetch.mockResolvedValueOnce(jsonRes(204, ''))
    await api.deleteTask('t1')
    expect(fetch.mock.calls[3][0]).toBe('/api/tasks/t1')
    expect(fetch.mock.calls[3][1].method).toBe('DELETE')

    fetch.mockResolvedValue(jsonRes(200, {}))
    await api.enableTask('t1')
    await api.disableTask('t1')
    const urls = fetch.mock.calls.slice(4).map(c => c[0])
    expect(urls).toEqual(['/api/tasks/t1/enable', '/api/tasks/t1/disable'])
  })

  it('runTaskNow 要求 202 带 run_id；UI 支撑端点列 runner 与 schedule provider', async () => {
    fetch.mockResolvedValueOnce(jsonRes(202, { run_id: 'run-9' }))
    await expect(api.runTaskNow('t1')).resolves.toMatchObject({ run_id: 'run-9' })
    expect(fetch.mock.calls[0][0]).toBe('/api/tasks/t1/run')

    fetch.mockResolvedValueOnce(jsonRes(202, { ok: true }))
    await expect(api.runTaskNow('t1')).rejects.toMatchObject({ code: 'invalid_response' })

    fetch.mockResolvedValueOnce(jsonRes(200, [{ runner_id: 'gamer.yaml' }]))
    await expect(api.listRunners()).resolves.toEqual([{ runner_id: 'gamer.yaml' }])
    expect(fetch.mock.calls[2][0]).toBe('/api/runners')

    fetch.mockResolvedValueOnce(jsonRes(200, [{ provider_id: 'cron' }]))
    await expect(api.listScheduleProviders()).resolves.toEqual([{ provider_id: 'cron' }])
    expect(fetch.mock.calls[3][0]).toBe('/api/schedule-providers')
  })
})

describe('游戏包（App Package）与本地编辑区 API', () => {
  it('列表/安装：安装发送原始字节 + application/zip + X-Expected-Sha256 校验头', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { packages: [] }))
    await api.listAppPackages()
    expect(fetch.mock.calls[0][0]).toBe('/api/app-packages')
    expect(fetch.mock.calls[0][1].method).toBe('GET')

    fetch.mockResolvedValueOnce(jsonRes(201, { id: 'pkg.demo', active_version: '1.0.0' }))
    const bytes = new TextEncoder().encode('archive-bytes').buffer
    await api.installAppPackage(bytes, 'a'.repeat(64))
    const [url, opt] = fetch.mock.calls[1]
    expect(url).toBe('/api/app-packages/install')
    expect(opt.method).toBe('POST')
    expect(opt.headers['Content-Type']).toBe('application/zip')
    expect(opt.headers['X-Expected-Sha256']).toBe('a'.repeat(64))
    expect(opt.body).toBe(bytes)

    // 未提供 sha 时不带头
    fetch.mockResolvedValueOnce(jsonRes(201, { id: 'pkg.demo' }))
    await api.installAppPackage(bytes)
    expect(fetch.mock.calls[2][1].headers['X-Expected-Sha256']).toBeUndefined()
  })

  it('导出：POST android_package，返回 blob + 响应头解析的文件名与 SHA-256', async () => {
    const blob = new Blob(['archive'])
    fetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      blob: async () => blob,
      headers: {
        get: name => ({
          'content-type': 'application/octet-stream',
          'content-disposition': 'attachment; filename="pkg.demo-1.0.0.gamerpkg"',
          'x-content-sha256': 'b'.repeat(64),
        })[String(name).toLowerCase()] || null,
      },
    })
    const rep = await api.exportAppPackage('com.demo')
    const [url, opt] = fetch.mock.calls[0]
    expect(url).toBe('/api/app-packages/export')
    expect(opt.method).toBe('POST')
    expect(bodyOf()).toEqual({ android_package: 'com.demo' })
    expect(rep.blob).toBe(blob)
    expect(rep.filename).toBe('pkg.demo-1.0.0.gamerpkg')
    expect(rep.sha256).toBe('b'.repeat(64))
  })

  it('编辑：POST /api/app-packages/:id/:version/edit（URL 编码）+ android_package body', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { android_package: 'com.demo', replaced: {} }))
    await api.editAppPackage('pkg.demo', '1.0.0', 'com.demo')
    expect(fetch.mock.calls[0][0]).toBe('/api/app-packages/pkg.demo/1.0.0/edit')
    expect(fetch.mock.calls[0][1].method).toBe('POST')
    expect(bodyOf()).toEqual({ android_package: 'com.demo' })
  })

  it('工作区：GET/PUT /api/workspace/:android_package（整体编码），PUT 只发给定字段', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { metadata: null, stats: {} }))
    await api.getWorkspace('com.demo')
    expect(fetch.mock.calls[0][0]).toBe('/api/workspace/com.demo')
    expect(fetch.mock.calls[0][1].method).toBe('GET')

    fetch.mockResolvedValueOnce(jsonRes(200, { metadata: { id: 'pkg.demo' } }))
    await api.saveWorkspace('com.demo', { id: 'pkg.demo', version: '1.0.0', android_packages: ['com.demo'] })
    expect(fetch.mock.calls[1][0]).toBe('/api/workspace/com.demo')
    expect(fetch.mock.calls[1][1].method).toBe('PUT')
    expect(bodyOf(1)).toEqual({ id: 'pkg.demo', version: '1.0.0', android_packages: ['com.demo'] })
  })
})
