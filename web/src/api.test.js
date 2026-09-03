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
    expect(fetch.mock.calls[0][0]).toBe('/api/scripts')
    expect(fetch.mock.calls[0][1].method).toBe('POST')
    expect(bodyOf()).toEqual({ pkg: 'com.demo', name: 'main.yaml', content: 'steps: []\n' })

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/main.yaml', version: 'v2' }))
    await api.updateScript('com.demo/main.yaml', {
      name: 'renamed.yaml', content: 'steps: []\n', expected_version: 'v1',
    })
    expect(fetch.mock.calls[1][0]).toBe('/api/scripts/com.demo%2Fmain.yaml')
    expect(fetch.mock.calls[1][1].method).toBe('PUT')
    expect(bodyOf(1)).toEqual({ content: 'steps: []\n', name: 'renamed.yaml', expected_version: 'v1' })
  })

  it('函数创建 POST 与更新 PUT 分离；缺版本不发请求，force:true 才跳过版本门禁', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/common.yaml' }))
    await api.createFunction({ pkg: 'com.demo', name: 'common', content: 'login:\n  steps: []\n' })
    expect(fetch.mock.calls[0][0]).toBe('/api/functions')
    expect(fetch.mock.calls[0][1].method).toBe('POST')
    expect(bodyOf()).toEqual({ pkg: 'com.demo', name: 'common', content: 'login:\n  steps: []\n' })

    await expect(api.updateFunction('com.demo/common.yaml', { content: 'login:\n  steps: []\n' }))
      .rejects.toMatchObject({ status: 409, code: 'version_required' })
    expect(fetch).toHaveBeenCalledTimes(1)

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/common.yaml', version: 'v3' }))
    await api.updateFunction('com.demo/common.yaml', { content: 'login:\n  steps: []\n', force: true })
    expect(fetch.mock.calls[1][0]).toBe('/api/functions/com.demo%2Fcommon.yaml')
    expect(fetch.mock.calls[1][1].method).toBe('PUT')
    expect(bodyOf(1)).toEqual({ content: 'login:\n  steps: []\n', force: true })
  })

  it('模板创建与图片替换使用不同 endpoint/body，替换不伪装成创建', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true, name: 'shot#100_200_800_900.png' }))
    await api.createTemplate('shot.png', 'QUJD', 'com.demo', [0.1, 0.2, 0.8, 0.9])
    expect(fetch.mock.calls[0][0]).toBe('/api/templates')
    expect(fetch.mock.calls[0][1].method).toBe('POST')
    expect(bodyOf()).toEqual({
      short_name: 'shot.png', region: [0.1, 0.2, 0.8, 0.9], data_b64: 'QUJD', pkg: 'com.demo',
    })

    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true, name: 'color#100_200_800_900#1.png' }))
    await api.createTemplate('color.png', 'QUJD', 'com.demo', [0.1, 0.2, 0.8, 0.9], true)
    expect(bodyOf(1)).toEqual({
      short_name: 'color.png', region: [0.1, 0.2, 0.8, 0.9], grayscale_only: false,
      data_b64: 'QUJD', pkg: 'com.demo',
    })

    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true, name: 'shot.png' }))
    await api.replaceTemplateImage('shot#100_200_800_900.png', 'REVG', 'com.demo')
    expect(fetch.mock.calls[2][0]).toBe('/api/templates/shot%23100_200_800_900.png/image?pkg=com.demo')
    expect(fetch.mock.calls[2][1].method).toBe('PUT')
    expect(bodyOf(2)).toEqual({ data_b64: 'REVG' })
  })

  it('按键映射详情/更新使用整体编码资源 id，并保留版本门禁', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/combat.yaml' }))
    await api.getKeymap('com.demo/combat.yaml', 'com.demo')
    expect(fetch.mock.calls[0][0]).toBe('/api/keymaps/com.demo%2Fcombat.yaml')

    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'com.demo/combat.yaml' }))
    await api.updateKeymap('combat.yaml', 'com.demo', {
      content: 'version: 1\nname: combat\nbindings: []\n',
      expected_version: 'abc123',
    })
    expect(fetch.mock.calls[1][0]).toBe('/api/keymaps/com.demo%2Fcombat.yaml')
    expect(bodyOf(1)).toEqual({
      content: 'version: 1\nname: combat\nbindings: []\n',
      expected_version: 'abc123',
    })
  })
})
