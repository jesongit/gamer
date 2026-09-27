// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { defineComponent, ref, nextTick } from 'vue'
import { useBrowserPreview } from './components/console/useBrowserPreview'
import { api } from './api'
vi.mock('./api', () => ({ api: { connectBrowser: vi.fn().mockResolvedValue({}) } }))
let wrapper
class Socket {
  static OPEN = 1
  static instances = []
  readyState = 1
  bufferedAmount = 0
  sent = []
  constructor() { Socket.instances.push(this) }
  send(s) { this.sent.push(JSON.parse(s)) }
  close() { this.readyState = 3; this.onclose?.() }
  receive(value) { this.onmessage?.({ data: JSON.stringify(value) }) }
}
function setup(hooks = {}) {
  Socket.instances = []
  vi.stubGlobal('WebSocket', Socket)
  const id = ref('browser-a'), connected = ref(false), connecting = ref(false), errorMsg = ref('')
  let preview
  wrapper = mount(defineComponent({ setup() { preview = useBrowserPreview({ deviceId: () => id.value, connected, connecting, errorMsg, toast: vi.fn(), ...hooks }); return () => null } }))
  return { id, preview, connected, connecting, errorMsg }
}
afterEach(() => { wrapper?.unmount(); vi.unstubAllGlobals(); vi.clearAllMocks() })
describe('browser preview target and displayed frame guards', () => {
  it('sends only after the displayed image loads, and invalidates immediately on target change', async () => {
    const { preview, id, connected } = setup()
    await preview.connect()
    const socket = Socket.instances[0]
    const stamp = { target: 'browser-a', epoch: 'one', revision: 1 }
    socket.receive({ type: 'frame', png: 'AA==', stamp })
    expect(preview.send({ type: 'tap', x: 1, y: 1 })).toBe(false)
    preview.loaded({ target: { src: preview.view.src } })
    expect(connected.value).toBe(true)
    expect(preview.send({ type: 'tap', x: 1, y: 1 })).toBe(true)
    expect(socket.sent[0].stamp).toEqual(stamp)
    id.value = 'browser-b'
    await nextTick()
    expect(preview.send({ type: 'tap', x: 1, y: 1 })).toBe(false)
    socket.receive({ type: 'frame', png: 'BB==', stamp })
    expect(preview.view.src).toBe('')
  })
  it('does not restore a closed connection when its last image finishes loading', async () => {
    const { preview, connected } = setup()
    await preview.connect()
    const socket = Socket.instances[0]
    socket.receive({ type: 'frame', png: 'AA==', stamp: { target: 'browser-a', epoch: 'one', revision: 1 } })
    socket.close()
    preview.loaded({ target: { src: preview.view.src } })
    expect(connected.value).toBe(false)
    expect(preview.view.stamp).toBeNull()
  })
  it('waits for actual display even when the same PNG arrives twice before load', async () => {
    const onConnected = vi.fn(), onDisconnected = vi.fn()
    const { preview, connected } = setup({ onConnected, onDisconnected })
    await preview.connect()
    const socket = Socket.instances[0]
    const frame = { type: 'frame', png: 'AA==', stamp: { target: 'browser-a', epoch: 'one', revision: 1 } }
    socket.receive(frame)
    socket.receive(frame)
    expect(preview.send({ type: 'tap', x: 1, y: 1 })).toBe(false)
    expect(connected.value).toBe(false)
    expect(onConnected).not.toHaveBeenCalled()
    preview.loaded({ target: { src: preview.view.src, naturalWidth: 640, naturalHeight: 480 } })
    socket.receive(frame)
    expect(preview.view.width).toBe(640)
    expect(preview.view.height).toBe(480)
    expect(onConnected).toHaveBeenCalledTimes(1)
    socket.close()
    preview.close()
    expect(onDisconnected).toHaveBeenCalledTimes(1)
  })
  it('rejects an image that finishes loading after a frame error, then recovers on a valid frame', async () => {
    const { preview, errorMsg } = setup()
    await preview.connect()
    const socket = Socket.instances[0]
    const frame = { type: 'frame', png: 'AA==', stamp: { target: 'browser-a', epoch: 'one', revision: 1 } }
    socket.receive(frame)
    socket.receive({ type: 'error', error: 'stale frame' })
    preview.loaded({ target: { src: preview.view.src } })
    expect(preview.view.stamp).toBeNull()
    expect(errorMsg.value).toBe('stale frame')
    socket.receive(frame)
    expect(preview.view.stamp).toEqual(frame.stamp)
    socket.receive({ type: 'error', error: 'stale frame' })
    socket.receive(frame)
    expect(errorMsg.value).toBe('')
    expect(preview.send({ type: 'tap', x: 1, y: 1 })).toBe(true)
  })
  it('waits for display when an earlier PNG returns while a different PNG is loading', async () => {
    const { preview } = setup()
    await preview.connect()
    const socket = Socket.instances[0]
    const frame = { type: 'frame', png: 'AA==', stamp: { target: 'browser-a', epoch: 'one', revision: 1 } }
    socket.receive(frame)
    preview.loaded({ target: { src: preview.view.src } })
    socket.receive({ ...frame, png: 'BB==' })
    socket.receive({ ...frame, stamp: { ...frame.stamp, revision: 2 } })
    expect(preview.view.src).toBe('data:image/png;base64,AA==')
    expect(preview.send({ type: 'tap', x: 1, y: 1 })).toBe(false)
    preview.loaded({ target: { src: preview.view.src } })
    expect(preview.send({ type: 'tap', x: 1, y: 1 })).toBe(true)
    expect(socket.sent.at(-1).stamp.revision).toBe(2)
  })
  it('does not create a stale connection after changing the target during startup', async () => {
    let resolve
    api.connectBrowser.mockImplementationOnce(() => new Promise(r => { resolve = r }))
    const { preview, id } = setup()
    const pending = preview.connect()
    id.value = 'browser-b'
    await nextTick()
    resolve({})
    await pending
    expect(Socket.instances).toHaveLength(0)
  })
})
