// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { defineComponent, ref, nextTick } from 'vue'
import { useBrowserPreview } from './components/console/useBrowserPreview'
import { api } from './api'
vi.mock('./api', () => ({ api: { connectBrowser: vi.fn().mockResolvedValue({}), screenshotFrame: vi.fn() } }))
let wrapper, createUrl, revokeUrl
class Socket {
  static OPEN = 1
  static instances = []
  readyState = 1; bufferedAmount = 0; sent = []
  constructor(url) { this.url = url; Socket.instances.push(this) }
  send(s) { this.sent.push(JSON.parse(s)) }
  close() { this.readyState = 3; this.onclose?.() }
  receive(value) { this.onmessage?.({ data: JSON.stringify(value) }) }
}
const stamp = { target: 'browser-a', epoch: 'one', revision: 1 }
const frame = (id = 1, overrides = {}) => ({ type: 'frame', id, jpeg: 'AA==', stamp,
  width: 1920, height: 1080, preview_width: 640, preview_height: 360, ...overrides })
function setup(hooks = {}) {
  Socket.instances = []
  vi.stubGlobal('WebSocket', Socket)
  const id = ref('browser-a'), connected = ref(false), connecting = ref(false), errorMsg = ref(''), mode = ref('single')
  let preview
  wrapper = mount(defineComponent({ setup() { preview = useBrowserPreview({ deviceId: () => id.value,
    previewMode: () => mode.value, connected, connecting, errorMsg, toast: vi.fn(), ...hooks }); return () => null } }))
  return { id, preview, connected, connecting, errorMsg, mode }
}
function loaded(preview, src = preview.view.src) { preview.loaded({ target: { src, naturalWidth: 640, naturalHeight: 360 } }) }
beforeEach(() => {
  let counter = 0
  createUrl = vi.spyOn(URL, 'createObjectURL').mockImplementation(() => `blob:frame-${++counter}`)
  revokeUrl = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {})
})
afterEach(() => { wrapper?.unmount(); vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.clearAllMocks() })
describe('browser preview resources and original-frame guards', () => {
  it('uses thumbnails for display while preserving full-resolution input coordinates', async () => {
    const { preview, mode } = setup()
    mode.value = 'grid4'
    await preview.connect()
    const socket = Socket.instances[0]
    expect(socket.url).toContain('?mode=grid4')
    socket.receive(frame())
    expect(preview.send({ type: 'tap', x: 1800, y: 900 })).toBe(false)
    loaded(preview)
    expect(preview.view).toMatchObject({ width: 1920, height: 1080, previewWidth: 640, previewHeight: 360 })
    for (const value of [{ type: 'tap', x: 1800, y: 900 }, { type: 'pointer', action: 'down', x: 100, y: 100 },
      { type: 'scroll', x: 100, y: 100, delta_y: 20 }, { type: 'key', key: 'w', action: 'down' },
      { type: 'input_event', event: { type: 'mouse_down', x: 100, y: 100 } }]) {
      expect(preview.send(value)).toBe(false)
    }
    mode.value = 'single'; await nextTick()
    expect(preview.send({ type: 'tap', x: 1800, y: 900 })).toBe(true)
    expect(socket.sent.at(-1)).toMatchObject({ x: 1800, y: 900, stamp })
    expect(socket.sent[0]).toEqual({ type: 'frame_ack', id: 1 })
  })
  it('updates the preview mode on the same socket without invalidating target input', async () => {
    const { preview, mode } = setup()
    await preview.connect()
    const socket = Socket.instances[0]
    socket.receive(frame()); loaded(preview)
    mode.value = 'grid9'; await nextTick()
    mode.value = 'single'; await nextTick()
    expect(socket.sent.slice(-2)).toEqual([{ type: 'preview', mode: 'grid9' }, { type: 'preview', mode: 'single' }])
    expect(Socket.instances).toHaveLength(1)
    expect(preview.send({ type: 'key', key: 'w', action: 'up' })).toBe(true)
  })
  it('keeps one JPEG blob, revokes superseded frames and rejects an earlier image load', async () => {
    const { preview } = setup()
    await preview.connect()
    const socket = Socket.instances[0]
    socket.receive(frame()); const first = preview.view.src
    loaded(preview)
    socket.receive(frame(2, { jpeg: 'BB==' })); const second = preview.view.src
    expect(revokeUrl).toHaveBeenCalledWith(first)
    loaded(preview, first)
    expect(socket.sent.filter(message => message.type === 'frame_ack')).toHaveLength(1)
    loaded(preview, second)
    expect(socket.sent.at(-1)).toEqual({ type: 'frame_ack', id: 2 })
    preview.close()
    expect(revokeUrl).toHaveBeenCalledWith(second)
    expect(preview.view).toMatchObject({ src: '', stamp: null, width: 0, height: 0, fps: 0 })
  })
  it('acknowledges repeated static frames only after their actual image load', async () => {
    const onConnected = vi.fn(), onDisconnected = vi.fn()
    const { preview, connected } = setup({ onConnected, onDisconnected })
    await preview.connect()
    const socket = Socket.instances[0]
    socket.receive(frame()); socket.receive(frame(2))
    expect(connected.value).toBe(false)
    expect(socket.sent).toEqual([])
    loaded(preview)
    expect(socket.sent).toEqual([{ type: 'frame_ack', id: 2 }])
    socket.receive(frame(3)); loaded(preview)
    expect(socket.sent.at(-1)).toEqual({ type: 'frame_ack', id: 3 })
    expect(onConnected).toHaveBeenCalledTimes(1)
    socket.close(); preview.close()
    expect(onDisconnected).toHaveBeenCalledTimes(1)
  })
  it('reads full PNG with server identity while the next thumbnail loads', async () => {
    let resolveImage
    const loadImage = vi.fn(() => new Promise(resolve => { resolveImage = resolve }))
    const { preview } = setup({ loadImage })
    await preview.connect()
    const socket = Socket.instances[0]
    socket.receive(frame()); loaded(preview)
    socket.receive(frame(2, { jpeg: 'BB==' }))
    const original = new Blob(['png'], { type: 'image/png' })
    api.screenshotFrame.mockResolvedValueOnce({ blob: original, stamp })
    const capture = preview.captureFrame(); await flushPromises()
    const concurrentCapture = preview.captureFrame()
    expect(concurrentCapture).toBe(capture)
    expect(api.screenshotFrame).toHaveBeenCalledOnce()
    expect(api.screenshotFrame).toHaveBeenCalledWith('browser-a', { signal: expect.any(AbortSignal) })
    expect(createUrl).toHaveBeenLastCalledWith(original)
    const image = { naturalWidth: 1920, naturalHeight: 1080 }
    resolveImage(image)
    expect(await capture).toMatchObject({ source: image, width: 1920, height: 1080, stamp })
    expect(revokeUrl).toHaveBeenCalledWith(loadImage.mock.calls[0][0])
  })
  it.each([null, { ...stamp, revision: 2 }, { ...stamp, epoch: 'old' }, { ...stamp, target: 'browser-b' }])('rejects a missing or stale original PNG stamp %j', async snapshotStamp => {
    const loadImage = vi.fn()
    const { preview } = setup({ loadImage })
    await preview.connect(); Socket.instances[0].receive(frame()); loaded(preview)
    api.screenshotFrame.mockResolvedValueOnce({ blob: new Blob(['png']), stamp: snapshotStamp })
    expect(await preview.captureFrame()).toBeNull()
    expect(loadImage).not.toHaveBeenCalled()
  })
  it('cancels a late original PNG decode after navigation or hiding the preview', async () => {
    let resolveImage
    const loadImage = vi.fn(() => new Promise(resolve => { resolveImage = resolve }))
    const { preview } = setup({ loadImage })
    await preview.connect()
    const socket = Socket.instances[0]
    socket.receive(frame()); loaded(preview)
    api.screenshotFrame.mockResolvedValue({ blob: new Blob(['png']), stamp })
    const capture = preview.captureFrame(); await flushPromises()
    socket.receive({ type: 'invalidated' })
    expect(loadImage.mock.calls[0][1].aborted).toBe(true)
    resolveImage({ naturalWidth: 1920, naturalHeight: 1080 })
    expect(await capture).toBeNull()
    expect(await preview.captureFrame()).toBeNull()
    socket.receive(frame(2)); loaded(preview)
    const hiddenCapture = preview.captureFrame(); await flushPromises()
    preview.close()
    resolveImage({ naturalWidth: 1920, naturalHeight: 1080 })
    expect(await hiddenCapture).toBeNull()
  })
  it('shows input rejection once, requires a new displayed frame, and keeps next-frame input available', async () => {
    const toast = vi.fn()
    const { preview, errorMsg } = setup({ toast })
    await preview.connect()
    const socket = Socket.instances[0]
    socket.receive(frame()); loaded(preview)
    socket.receive(frame(2))
    expect(preview.send({ type: 'key', key: 'w', action: 'up' })).toBe(true)
    const rejection = { type: 'error', source: 'input', error: '目标正在执行任务' }
    socket.receive(rejection); loaded(preview)
    expect(socket.sent.at(-1)).toEqual({ type: 'frame_ack', id: 2 })
    expect(preview.view.stamp).toBeNull()
    expect(errorMsg.value).toBe('目标正在执行任务')
    socket.receive(frame(3)); loaded(preview)
    expect(errorMsg.value).toBe('')
    socket.receive(rejection)
    expect(toast).toHaveBeenCalledTimes(1)
  })
  it('keeps the pending image ACK on coordinate rejection and recovers on the same socket', async () => {
    const { preview, connected } = setup()
    await preview.connect()
    const socket = Socket.instances[0]
    socket.receive(frame()); loaded(preview)
    socket.receive(frame(2))
    socket.receive({ type: 'error', source: 'input', error: '坐标超出画面' })
    expect(preview.send({ type: 'pointer', action: 'down', x: 20, y: 20 })).toBe(false)
    loaded(preview)
    expect(socket.sent.at(-1)).toEqual({ type: 'frame_ack', id: 2 })
    expect(preview.view.stamp).toBeNull()
    socket.receive(frame(3)); loaded(preview)
    expect(preview.send({ type: 'tap', x: 20, y: 20 })).toBe(true)
    expect(connected.value).toBe(true)
    expect(Socket.instances).toHaveLength(1)
  })
  it('discards a failed decode without granting input and ignores late image callbacks', async () => {
    const { preview, errorMsg } = setup()
    await preview.connect()
    const socket = Socket.instances[0]
    socket.receive(frame()); loaded(preview)
    socket.receive(frame(2)); const failedSrc = preview.view.src
    preview.failed({ target: { src: failedSrc } })
    expect(socket.sent.at(-1)).toEqual({ type: 'frame_ack', id: 2, displayed: false })
    expect(preview.view.stamp).toBeNull()
    expect(errorMsg.value).toContain('解码失败')
    socket.receive(frame(3))
    loaded(preview, failedSrc)
    preview.failed({ target: { src: failedSrc } })
    expect(socket.sent.at(-1).id).toBe(2)
    loaded(preview)
    expect(socket.sent.at(-1)).toEqual({ type: 'frame_ack', id: 3 })
    expect(preview.view.stamp).toEqual(stamp)
    expect(errorMsg.value).toBe('')
  })
  it('invalidates target changes and cancels a connection still waiting for REST', async () => {
    let finish
    api.connectBrowser.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const { preview, id } = setup()
    const connecting = preview.connect()
    id.value = 'browser-b'; await nextTick(); finish({}); await connecting
    expect(Socket.instances).toHaveLength(0)
    await preview.connect()
    const socket = Socket.instances[0]
    socket.receive(frame(1, { stamp: { ...stamp, target: 'browser-b' } })); loaded(preview)
    const oldSrc = preview.view.src
    id.value = 'browser-c'; await nextTick()
    socket.receive(frame(2)); loaded(preview, oldSrc)
    expect(preview.view.src).toBe('')
    expect(preview.send({ type: 'tap', x: 1, y: 1 })).toBe(false)
  })
})
