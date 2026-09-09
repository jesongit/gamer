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

describe('Package 插件资源 API surface（plan §12-§14）', () => {
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
    // 旧 Installed/Editable 双层模型 API 不复存在
    for (const name of ['listAppPackages', 'installAppPackage', 'exportAppPackage', 'editAppPackage', 'getWorkspace', 'saveWorkspace']) {
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

  it('脚本创建/更新走 gamer.yaml automations/ PUT（资源 id 首段 = Package id）', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/main.yaml', version: 'v1' }))
    await api.createScript({ pkg: 'com.demo', name: 'main.yaml', content: 'steps: []\n', id: 'old-id' })
    expect(fetch.mock.calls[0][0]).toBe('/api/packages/com.demo/plugins/gamer.yaml/resources/automations%2Fmain.yaml')
    expect(fetch.mock.calls[0][1].method).toBe('PUT')
    expect(bodyOf()).toEqual({ content: 'steps: []\n' })

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/main.yaml', version: 'v2' }))
    await api.updateScript('com.demo/main.yaml', {
      content: 'steps: []\n', expected_version: 'v1',
    })
    expect(fetch.mock.calls[1][0]).toBe('/api/packages/com.demo/plugins/gamer.yaml/resources/automations%2Fmain.yaml')
    expect(fetch.mock.calls[1][1].method).toBe('PUT')
    expect(bodyOf(1)).toEqual({ content: 'steps: []\n', expected_version: 'v1' })
  })

  it('函数库创建与更新共用 PUT（automations/ 前缀识别）；缺版本不发请求，force:true 才跳过版本门禁', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/_function.yaml', version: 'v1' }))
    await api.createFunction({ pkg: 'com.demo', name: '_function.yaml', content: 'functions:\n  login:\n    run: []\n' })
    expect(fetch.mock.calls[0][0]).toBe('/api/packages/com.demo/plugins/gamer.yaml/resources/automations%2F_function.yaml')
    expect(fetch.mock.calls[0][1].method).toBe('PUT')
    expect(bodyOf()).toEqual({ content: 'functions:\n  login:\n    run: []\n' })

    await expect(api.updateFunction('com.demo/_function.yaml', { content: 'functions:\n  login:\n    run: []\n' }))
      .rejects.toMatchObject({ status: 409, code: 'version_required' })
    expect(fetch).toHaveBeenCalledTimes(1)

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/_function.yaml', version: 'v3' }))
    await api.updateFunction('com.demo/_function.yaml', { content: 'functions:\n  login:\n    run: []\n', force: true })
    expect(fetch.mock.calls[1][0]).toBe('/api/packages/com.demo/plugins/gamer.yaml/resources/automations%2F_function.yaml')
    expect(fetch.mock.calls[1][1].method).toBe('PUT')
    expect(bodyOf(1)).toEqual({ content: 'functions:\n  login:\n    run: []\n', force: true })
  })

  it('模板创建与图片替换：原始字节 PUT 到 templates/，客户端组合完整文件名', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true, name: 'shot#100_200_800_900.png' }))
    await api.createTemplate('shot.png', 'QUJD', 'com.demo', [0.1, 0.2, 0.8, 0.9])
    expect(fetch.mock.calls[0][0]).toBe(
      '/api/packages/com.demo/plugins/gamer.yaml/resources/templates%2Fshot%23100_200_800_900.png',
    )
    expect(fetch.mock.calls[0][1].method).toBe('PUT')
    expect(fetch.mock.calls[0][1].headers['Content-Type']).toBe('application/octet-stream')
    expect(fetch.mock.calls[0][1].headers['X-Force']).toBe('true')
    expect(new TextDecoder().decode(fetch.mock.calls[0][1].body)).toBe('ABC')

    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true, name: 'color#100_200_800_900#1.png' }))
    await api.createTemplate('color.png', 'QUJD', 'com.demo', [0.1, 0.2, 0.8, 0.9], true)
    expect(fetch.mock.calls[1][0]).toBe(
      '/api/packages/com.demo/plugins/gamer.yaml/resources/templates%2Fcolor%23100_200_800_900%231.png',
    )

    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true, name: 'shot.png' }))
    await api.replaceTemplateImage('shot#100_200_800_900.png', 'REVG', 'com.demo')
    expect(fetch.mock.calls[2][0]).toBe('/api/packages/com.demo/plugins/gamer.yaml/resources/templates%2Fshot%23100_200_800_900.png')
    expect(fetch.mock.calls[2][1].method).toBe('PUT')
    expect(new TextDecoder().decode(fetch.mock.calls[2][1].body)).toBe('DEF')
  })

  it('模板重命名走 rename 端点（服务端同步改写脚本/函数引用）', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true }))
    await api.renameTemplate('old.png', 'new.png', 'com.demo')
    expect(fetch.mock.calls[0][0]).toBe('/api/packages/com.demo/plugins/gamer.yaml/rename')
    expect(bodyOf()).toEqual({ path: 'templates/old.png', new_path: 'templates/new.png' })
  })

  it('按键映射详情/更新走 gamer.keymap mappings/，并保留版本门禁', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { content: 'version: 1\n', version: 'abc' }))
    await api.getKeymap('com.demo/combat.yaml', 'com.demo')
    expect(fetch.mock.calls[0][0]).toBe('/api/packages/com.demo/plugins/gamer.keymap/resources/mappings%2Fcombat.yaml')

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/combat.yaml', version: 'v2' }))
    await api.updateKeymap('combat.yaml', 'com.demo', {
      content: 'version: 1\nname: combat\nbindings: []\n',
      expected_version: 'abc123',
    })
    expect(fetch.mock.calls[1][0]).toBe('/api/packages/com.demo/plugins/gamer.keymap/resources/mappings%2Fcombat.yaml')
    expect(bodyOf(1)).toEqual({
      content: 'version: 1\nname: combat\nbindings: []\n',
      expected_version: 'abc123',
    })
  })

  it('vision 测试显式携带 pkg（Package id）+ plugin（gamer.yaml）三元组', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { found: true }))
    await api.testTemplate('shot.png', 'dev-1', 0.9, [0, 0, 1, 1], 'com.demo')
    expect(fetch.mock.calls[0][0]).toBe('/api/capabilities/vision/test')
    expect(bodyOf()).toMatchObject({
      device_id: 'dev-1', pkg: 'com.demo', plugin: 'gamer.yaml', name: 'shot.png',
    })
  })
})

describe('Package 生命周期 API（plan §7-§11）', () => {
  it('列表/详情/新建/编辑/删除/复制/兼容性', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { packages: [] }))
    await api.listPackages()
    expect(fetch.mock.calls[0][0]).toBe('/api/packages')

    fetch.mockResolvedValueOnce(jsonRes(201, { id: 'user.demo' }))
    await api.createPackage({ id: 'user.demo', targets: { android: { packages: ['com.Demo.App'] } } })
    expect(fetch.mock.calls[1][1].method).toBe('POST')
    expect(bodyOf(1)).toEqual({ id: 'user.demo', targets: { android: { packages: ['com.Demo.App'] } } })

    fetch.mockResolvedValueOnce(jsonRes(200, { package: { id: 'user.demo' } }))
    await api.getPackage('user.demo')
    expect(fetch.mock.calls[2][0]).toBe('/api/packages/user.demo')

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'user.demo' }))
    await api.updatePackage('user.demo', { name: 'Demo', expected_revision: 3 })
    expect(fetch.mock.calls[3][1].method).toBe('PUT')
    expect(bodyOf(3)).toEqual({ name: 'Demo', expected_revision: 3 })

    fetch.mockResolvedValue(jsonRes(204, ''))
    await api.deletePackage('user.demo')
    expect(fetch.mock.calls[4][0]).toBe('/api/packages/user.demo')
    expect(fetch.mock.calls[4][1].method).toBe('DELETE')

    fetch.mockResolvedValueOnce(jsonRes(201, { id: 'user.copy' }))
    await api.duplicatePackage('user.demo', 'user.copy')
    expect(fetch.mock.calls[5][0]).toBe('/api/packages/user.demo/duplicate')
    expect(bodyOf(5)).toEqual({ new_id: 'user.copy' })

    fetch.mockResolvedValueOnce(jsonRes(200, { compatible: true }))
    await api.packageCompatibility('user.demo', 'com.Demo.App')
    expect(fetch.mock.calls[6][0]).toBe('/api/packages/user.demo/compatibility?android_package=com.Demo.App')
  })

  it('导入：原始字节 + application/zip + X-Expected-Sha256；overwrite 走查询参数', async () => {
    const bytes = new TextEncoder().encode('archive-bytes').buffer
    fetch.mockResolvedValueOnce(jsonRes(201, { id: 'pkg.demo' }))
    await api.importPackageArchive(bytes, { expectedSha256: 'a'.repeat(64) })
    let [url, opt] = fetch.mock.calls[0]
    expect(url).toBe('/api/packages/import')
    expect(opt.method).toBe('POST')
    expect(opt.headers['Content-Type']).toBe('application/zip')
    expect(opt.headers['X-Expected-Sha256']).toBe('a'.repeat(64))
    expect(opt.body).toBe(bytes)

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'pkg.demo' }))
    await api.importPackageArchive(bytes, { overwrite: true })
    ;[url, opt] = fetch.mock.calls[1]
    expect(url).toBe('/api/packages/import?overwrite=true')
    expect(opt.headers['X-Expected-Sha256']).toBeUndefined()
  })

  it('导出：POST /api/packages/:pkg/export，返回 blob + 响应头解析的文件名与 SHA-256', async () => {
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
    const rep = await api.exportPackageArchive('pkg.demo')
    const [url, opt] = fetch.mock.calls[0]
    expect(url).toBe('/api/packages/pkg.demo/export')
    expect(opt.method).toBe('POST')
    expect(rep.blob).toBe(blob)
    expect(rep.filename).toBe('pkg.demo-1.0.0.gamerpkg')
    expect(rep.sha256).toBe('b'.repeat(64))

    // Phase 8 §11.1：includeMedia=true → ?include_media=true（携带素材字节）
    fetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      blob: async () => blob,
      headers: { get: () => null },
    })
    await api.exportPackageArchive('pkg.demo', { includeMedia: true })
    expect(fetch.mock.calls[1][0]).toBe('/api/packages/pkg.demo/export?include_media=true')
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
