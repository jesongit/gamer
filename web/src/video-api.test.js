/**
 * videoApi（视频工作台 REST 封装）端点形态测试：逐字锁定实施合同
 * docs/plans/gamer_video_workbench_contracts.md §1（媒体）/§2（录制）/§5（草稿 call 通路）
 * 的 URL / 方法 / body / 查询参数与错误形态（带 status 的 Error）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ptsFromTime, videoApi } from './components/video/videoApi'

/** 够用的 Response 桩：request() 只读 ok/status/headers(content-type)/json。 */
function jsonResponse(status, body, contentType = 'application/json') {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name) => (String(name).toLowerCase() === 'content-type' ? contentType : null) },
    json: async () => body,
  }
}

let fetchStub

beforeEach(() => {
  fetchStub = vi.fn()
  vi.stubGlobal('fetch', fetchStub)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('videoApi 媒体端点（合同 §1）', () => {
  it('listMedia → GET /api/media，解包 media 数组', async () => {
    const meta = { id: 'm1', name: 'a.mp4' }
    fetchStub.mockResolvedValue(jsonResponse(200, { media: [meta] }))
    await expect(videoApi.listMedia()).resolves.toEqual([meta])
    expect(fetchStub).toHaveBeenCalledWith('/api/media', expect.objectContaining({ method: 'GET' }))
  })

  it('getMedia → GET /api/media/:id', async () => {
    fetchStub.mockResolvedValue(jsonResponse(200, { id: 'm 1', name: 'a.mp4' }))
    await videoApi.getMedia('m 1')
    expect(fetchStub).toHaveBeenCalledWith('/api/media/m%201', expect.objectContaining({ method: 'GET' }))
  })

  it('importMedia → POST /api/media/import?name=<urlencoded>，raw 字节 body + octet-stream', async () => {
    const meta = { id: 'm2', name: 'clip.mp4' }
    fetchStub.mockResolvedValue(jsonResponse(201, meta))
    const bytes = new Uint8Array([1, 2, 3])
    await expect(videoApi.importMedia(bytes, '我的 clip.mp4')).resolves.toEqual(meta)
    const [url, options] = fetchStub.mock.calls[0]
    expect(url).toBe(`/api/media/import?name=${encodeURIComponent('我的 clip.mp4')}`)
    expect(options.method).toBe('POST')
    expect(options.headers['Content-Type']).toBe('application/octet-stream')
    expect(options.body).toBe(bytes)
  })

  it('deleteMedia → DELETE /api/media/:id，204 → null', async () => {
    fetchStub.mockResolvedValue(jsonResponse(204, null))
    await expect(videoApi.deleteMedia('m1')).resolves.toBeNull()
    expect(fetchStub).toHaveBeenCalledWith('/api/media/m1', expect.objectContaining({ method: 'DELETE' }))
  })

  it('mediaFileUrl / mediaFrameUrl 只拼 URL 不发请求；frame 查询参数 = pts_us|index|max_width', () => {
    expect(videoApi.mediaFileUrl('m1')).toBe('/api/media/m1/file')
    expect(videoApi.mediaFrameUrl('m1', { ptsUs: 1234.6, maxWidth: 640 }))
      .toBe('/api/media/m1/frame?pts_us=1235&max_width=640')
    expect(videoApi.mediaFrameUrl('m1', { index: 5 })).toBe('/api/media/m1/frame?index=5')
    // pts_us 与 index 同时给出：pts_us 优先（顺序在前）
    expect(videoApi.mediaFrameUrl('m1', { ptsUs: 10, index: 5 })).toBe('/api/media/m1/frame?pts_us=10&index=5')
    expect(videoApi.mediaFrameUrl('m1')).toBe('/api/media/m1/frame')
    expect(fetchStub).not.toHaveBeenCalled()
  })

  it('删除被引用素材：409 媒体引用错误原样映射为 status/code', async () => {
    fetchStub.mockResolvedValue(jsonResponse(409, { error: 'media_referenced' }))
    await expect(videoApi.deleteMedia('m1')).rejects.toMatchObject({
      status: 409,
      code: 'media_referenced',
      name: 'VideoApiError',
    })
  })
})

describe('videoApi 录制端点（合同 §2）', () => {
  it('recordingStart → POST /api/recording/start，body {device_id}', async () => {
    const session = { id: 'r1', device_id: 'dev-a', state: 'recording' }
    fetchStub.mockResolvedValue(jsonResponse(202, session))
    await expect(videoApi.recordingStart('dev-a')).resolves.toEqual(session)
    const [url, options] = fetchStub.mock.calls[0]
    expect(url).toBe('/api/recording/start')
    expect(options.method).toBe('POST')
    expect(JSON.parse(options.body)).toEqual({ device_id: 'dev-a' })
  })

  it('recordingStop / recordingCancel / recordingStatus / recordingEvents 端点形态', async () => {
    fetchStub.mockResolvedValue(jsonResponse(200, { id: 'r-1', state: 'completed' }))
    await videoApi.recordingStop('r-1')
    expect(fetchStub.mock.calls[0][0]).toBe('/api/recording/r-1/stop')
    expect(fetchStub.mock.calls[0][1].method).toBe('POST')

    await videoApi.recordingCancel('r-1')
    expect(fetchStub.mock.calls[1][0]).toBe('/api/recording/r-1/cancel')
    expect(fetchStub.mock.calls[1][1].method).toBe('POST')

    await videoApi.recordingStatus('r-1')
    expect(fetchStub.mock.calls[2][0]).toBe('/api/recording/r-1')
    expect(fetchStub.mock.calls[2][1].method).toBe('GET')

    fetchStub.mockResolvedValue(jsonResponse(200, { schema_version: 1, events: [{ event_id: 'e1' }] }))
    await expect(videoApi.recordingEvents('r-1')).resolves.toEqual([{ event_id: 'e1' }])
    expect(fetchStub.mock.calls[3][0]).toBe('/api/recording/r-1/events')
  })

  it('activeRecording：200 → session；404（无活动会话，轮询常态）→ null 不抛错', async () => {
    fetchStub.mockResolvedValue(jsonResponse(200, { id: 'r1', state: 'recording' }))
    await expect(videoApi.activeRecording('dev-a')).resolves.toEqual({ id: 'r1', state: 'recording' })
    expect(fetchStub.mock.calls[0][0]).toBe('/api/recording/active?device_id=dev-a')

    fetchStub.mockResolvedValue(jsonResponse(404, { error: 'recording_not_found' }))
    await expect(videoApi.activeRecording('dev-a')).resolves.toBeNull()
  })
})

describe('videoApi 草稿（合同 §5 call 通路）与工具函数', () => {
  it('createVideoDraft → POST /api/extensions/gamer.yaml/call action=automation.create_draft，取 data 返回', async () => {
    const data = { yaml: 'version: 3\nsteps: []', diagnostics: [] }
    fetchStub.mockResolvedValue(jsonResponse(200, { ok: true, data }))
    await expect(videoApi.createVideoDraft('r-1', ['e1', 'e2'])).resolves.toBe(data)
    const [url, options] = fetchStub.mock.calls[0]
    expect(url).toBe('/api/extensions/gamer.yaml/call')
    expect(options.method).toBe('POST')
    expect(JSON.parse(options.body)).toEqual({
      action: 'automation.create_draft',
      values: { recording_id: 'r-1', event_ids: ['e1', 'e2'] },
    })
  })

  it('createVideoDraft：结果未包 data 信封时按原结果兜底', async () => {
    const bare = { yaml: 'version: 3\nsteps: []', diagnostics: [{ event_id: 'e9', reason: 'multi_touch' }] }
    fetchStub.mockResolvedValue(jsonResponse(200, bare))
    await expect(videoApi.createVideoDraft('r-1', [])).resolves.toEqual(bare)
  })

  it('错误统一带 status：404 recording_not_found / 网络失败 status=0', async () => {
    fetchStub.mockResolvedValue(jsonResponse(404, { error: 'recording_not_found' }))
    await expect(videoApi.recordingEvents('nope')).rejects.toMatchObject({ status: 404, code: 'recording_not_found' })

    fetchStub.mockRejectedValue(new TypeError('boom'))
    await expect(videoApi.listMedia()).rejects.toMatchObject({ status: 0, code: 'network_error' })
  })

  it('mediaFrames：GET /api/media/:id/frames（可选 pts_us → current 解析）', async () => {
    const info = { frame_count: 40, first_pts_us: 0, last_pts_us: 1332000 }
    fetchStub.mockResolvedValue(jsonResponse(200, info))
    await expect(videoApi.mediaFrames('m1')).resolves.toEqual(info)
    expect(fetchStub.mock.calls[0][0]).toBe('/api/media/m1/frames')

    fetchStub.mockResolvedValue(jsonResponse(200, { ...info, current: { index: 15, pts_us: 500000 } }))
    await expect(videoApi.mediaFrames('m1', { ptsUs: 499999.4 })).resolves.toMatchObject({ current: { index: 15 } })
    expect(fetchStub.mock.calls[1][0]).toBe('/api/media/m1/frames?pts_us=499999')
  })

  it('mediaFrameNeighbors：GET /api/media/:id/frames/:index；越界 404 原样上抛', async () => {
    fetchStub.mockResolvedValue(jsonResponse(200, {
      index: 5, pts_us: 166666, prev: { index: 4, pts_us: 133333 }, next: { index: 6, pts_us: 199999 },
    }))
    await expect(videoApi.mediaFrameNeighbors('m1', 5)).resolves.toMatchObject({ index: 5, prev: { index: 4 } })
    expect(fetchStub.mock.calls[0][0]).toBe('/api/media/m1/frames/5')

    fetchStub.mockResolvedValue(jsonResponse(404, { error: 'frame_not_found' }))
    await expect(videoApi.mediaFrameNeighbors('m1', 999)).rejects.toMatchObject({
      status: 404, code: 'frame_not_found',
    })
    // 非法索引归一为 0（客户端防御），不发非法 URL
    fetchStub.mockResolvedValue(jsonResponse(200, { index: 0, prev: null, next: null }))
    await expect(videoApi.mediaFrameNeighbors('m1', Number.NaN)).resolves.toMatchObject({ index: 0 })
    expect(fetchStub.mock.calls[2][0]).toBe('/api/media/m1/frames/0')
  })

  it('ptsFromTime：秒 → 微秒取整；非有限数/负值 → 0（仅预览粗定位，无步长假设）', () => {
    expect(ptsFromTime(0.5)).toBe(500000)
    expect(ptsFromTime(1.2345678)).toBe(1234568)
    expect(ptsFromTime(-3)).toBe(0)
    expect(ptsFromTime(Number.NaN)).toBe(0)
  })

  it('空 id/名称参数在客户端即拒绝：async 方法走 rejected promise（不发请求、非同步 throw）', async () => {
    await expect(videoApi.getMedia('  ')).rejects.toMatchObject({ status: 0, code: 'invalid_argument' })
    await expect(videoApi.getMedia('')).rejects.toMatchObject({ code: 'invalid_argument' })
    await expect(videoApi.deleteMedia(' ')).rejects.toMatchObject({ code: 'invalid_argument' })
    await expect(videoApi.importMedia(new Uint8Array([1]), ' ')).rejects.toMatchObject({ code: 'invalid_argument' })
    await expect(videoApi.recordingStart('')).rejects.toMatchObject({ code: 'invalid_argument' })
    await expect(videoApi.recordingStatus('  ')).rejects.toMatchObject({ code: 'invalid_argument' })
    await expect(videoApi.createVideoDraft(' ', [])).rejects.toMatchObject({ code: 'invalid_argument' })
    expect(fetchStub).not.toHaveBeenCalled()
  })
})
