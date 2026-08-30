// 当前 API 认证契约：受保护端点 401 → Cookie 会话失效处理；调用方拿到稳定 ApiError。
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

function lsStub(initial = {}) {
  const m = new Map(Object.entries(initial))
  return {
    getItem: k => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => { m.set(k, String(v)) },
    removeItem: k => { m.delete(k) },
    clear: () => { m.clear() },
    _m: m,
  }
}

function res(status, body, ct = 'application/json') {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: h => (/^content-type$/i.test(h) ? ct : null) },
    json: async () => body,
  }
}

let api

beforeEach(async () => {
  vi.resetModules()
  vi.stubGlobal('fetch', vi.fn())
  vi.stubGlobal('localStorage', lsStub())
  vi.stubGlobal('location', { hash: '#/console' })
  ;({ api } = await import('../src/api'))
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('api 层统一 401 拦截', () => {
  it('业务端点 401 → 清本地态、跳 #/login 保留回跳参数，并向调用方抛错', async () => {
    localStorage.setItem('gb_device_id', 'dev-1')
    fetch.mockResolvedValueOnce(res(401, { error: 'unauthorized' }))
    await expect(api.listDevices()).rejects.toMatchObject({
      name: 'ApiError', status: 401, code: 'unauthorized', data: { error: 'unauthorized' },
    })
    expect(localStorage.getItem('gb_device_id')).toBeNull()
    expect(location.hash).toBe(`#/login?redirect=${encodeURIComponent('/console')}`)
  })

  it('非 401 错误不触发跳转，错误原样透传', async () => {
    fetch.mockResolvedValueOnce(res(500, { error: 'internal' }))
    await expect(api.listDevices()).rejects.toMatchObject({ status: 500, code: 'internal' })
    expect(location.hash).toBe('#/console')
    // 本地态不被误清
    localStorage.setItem('gb_device_id', 'dev-1')
    fetch.mockResolvedValueOnce(res(503, {}))
    await expect(api.listTasks()).rejects.toMatchObject({ status: 503, code: 'http_503', data: {} })
    expect(localStorage.getItem('gb_device_id')).toBe('dev-1')
  })

  it('raw fetch 路径（分区导出 zip）同样拦截 401', async () => {
    const exportRes = res(401, { error: 'unauthorized' })
    exportRes.blob = async () => new Blob()
    fetch.mockResolvedValueOnce(exportRes)
    await expect(api.exportPartition('hkrpg')).rejects.toMatchObject({ status: 401, code: 'unauthorized' })
    expect(location.hash).toBe(`#/login?redirect=${encodeURIComponent('/console')}`)
  })

  it('登录成功路径不受影响：POST /api/scripts 照常解析 JSON', async () => {
    fetch.mockResolvedValueOnce(res(200, [{ id: 'a/b.yaml' }]))
    await expect(api.listScripts()).resolves.toEqual([{ id: 'a/b.yaml' }])
    expect(location.hash).toBe('#/console')
  })

  it('网络异常也转换为稳定 ApiError，不触发任何旧 endpoint 探测', async () => {
    fetch.mockRejectedValueOnce(new TypeError('offline'))
    await expect(api.listDevices()).rejects.toMatchObject({
      name: 'ApiError', status: 0, code: 'network_error', data: null,
    })
    expect(fetch).toHaveBeenCalledTimes(1)
  })
})
