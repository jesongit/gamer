// @vitest-environment happy-dom
// Real per-target component + browser/WebRTC lifecycle, with only network/media primitives faked.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { api } from './api'
import TargetPreviewSession from './components/console/TargetPreviewSession.vue'

vi.mock('./api', () => ({ api: { connectBrowser: vi.fn(async () => ({})), connectDevice: vi.fn(async () => ({})), cancelRun: vi.fn() } }))
class Socket {
  static OPEN = 1
  static instances = []
  readyState = 1; bufferedAmount = 0; sent = []; closed = false
  constructor(url) { this.url = url; Socket.instances.push(this); queueMicrotask(() => this.onopen?.()) }
  send(raw) {
    const message = JSON.parse(raw); this.sent.push(message)
    if (message.type === 'offer') queueMicrotask(() => this.onmessage?.({ data: JSON.stringify({ type: 'answer', sdp: { type: 'answer', sdp: 'answer' } }) }))
  }
  close() { this.closed = true; this.readyState = 3; this.onclose?.() }
  receive(message) { this.onmessage?.({ data: JSON.stringify(message) }) }
}
class Peer {
  static instances = []
  closed = false
  channel = { readyState: 'open', sent: [], send(raw) { this.sent.push(JSON.parse(raw)) }, close() { this.readyState = 'closed' } }
  constructor() { Peer.instances.push(this) }
  addTransceiver() {}
  createDataChannel() { return this.channel }
  async createOffer() { return { type: 'offer', sdp: 'offer' } }
  async setLocalDescription(value) { this.localDescription = value }
  async setRemoteDescription(value) { this.remoteDescription = value }
  close() { this.closed = true; this.channel.close() }
}
const mounted = []
function mountTarget(targetId, selected = false) {
  let session
  const wrapper = mount(TargetPreviewSession, {
    props: { targetId, selected, onRegister: (id, value) => { if (value) session = value } },
    attrs: { onMouseDown: () => {}, onMouseMove: () => {}, onMouseUp: () => {}, onWheel: () => {}, onVideoMouseLeave: () => {}, scriptFx: { tap: {}, swipe: {}, hit: {} }, loupe: {}, keymapOverlay: [], bridgeOverlays: [] },
  })
  Object.defineProperty(wrapper.find('video').element, 'srcObject', { configurable: true, writable: true, value: null })
  mounted.push(wrapper)
  return { wrapper, session }
}
async function showFrame(wrapper, socket, id, jpeg = 'AAAA') {
  socket.receive({ type: 'frame', id: 1, jpeg, stamp: { target: id, epoch: 'epoch', revision: 1 } })
  await flushPromises()
  const image = wrapper.find('img[alt="浏览器目标画面"]')
  Object.defineProperty(image.element, 'naturalWidth', { configurable: true, value: 1920 })
  Object.defineProperty(image.element, 'naturalHeight', { configurable: true, value: 1080 })
  await image.trigger('load')
  return image
}
beforeEach(() => {
  vi.useFakeTimers(); vi.clearAllMocks(); Socket.instances = []; Peer.instances = []
  vi.stubGlobal('WebSocket', Socket); vi.stubGlobal('RTCPeerConnection', Peer)
  vi.stubGlobal('RTCSessionDescription', function (value) { return value })
})
afterEach(() => {
  for (const wrapper of mounted.splice(0)) if (wrapper.exists()) wrapper.unmount()
  vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals()
})

describe('independent target preview sessions', () => {
  it('holds two browser transports with distinct frame identity and closes only the requested preview', async () => {
    const a = mountTarget('browser-a', true), b = mountTarget('browser-b')
    await Promise.all([a.session.connect(), b.session.connect()])
    const [socketA, socketB] = Socket.instances
    expect(socketA.url).toContain('/ws/browser/browser-a')
    expect(socketB.url).toContain('/ws/browser/browser-b')
    await showFrame(a.wrapper, socketA, 'browser-a')
    await showFrame(b.wrapper, socketB, 'browser-b', 'BBBB')
    expect(a.session.connected.value).toBe(true)
    expect(b.session.connected.value).toBe(true)
    expect(a.session.browser.view.width).toBe(1920)
    expect(b.session.browser.view.width).toBe(1920)
    a.session.browser.send({ type: 'pointer', action: 'down', x: 900, y: 800 })
    expect(socketA.sent.at(-1)).toMatchObject({ stamp: { target: 'browser-a' }, x: 900, y: 800 })
    expect(socketB.sent.some(message => message.type === 'pointer')).toBe(false)
    a.session.close()
    expect(socketA.closed).toBe(true)
    expect(socketA.sent.at(-1)).toEqual({ type: 'release' })
    expect(socketB.closed).toBe(false)
    expect(b.session.connected.value).toBe(true)
    expect(b.session.browser.view.stamp.target).toBe('browser-b')
    expect(api.cancelRun).not.toHaveBeenCalled()
  })

  it('cancelled asynchronous browser connection cannot resurrect a closed tile or affect its neighbour', async () => {
    let finish
    api.connectBrowser.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const a = mountTarget('browser-a', true), b = mountTarget('browser-b')
    const pending = a.session.connect()
    await b.session.connect()
    a.wrapper.unmount()
    finish({})
    await pending
    expect(Socket.instances).toHaveLength(1)
    expect(Socket.instances[0].url).toContain('browser-b')
    expect(Socket.instances[0].closed).toBe(false)
    expect(api.cancelRun).not.toHaveBeenCalled()
  })

  it('Android audio starts muted, selection loss disables track and channel, and only selected target can unmute', async () => {
    const a = mountTarget('android-a', true), b = mountTarget('android-b')
    await a.session.connect(); await b.session.connect()
    const [peerA, peerB] = Peer.instances
    peerA.channel.onopen(); peerB.channel.onopen()
    const audioA = { kind: 'audio', enabled: true }, audioB = { kind: 'audio', enabled: true }
    const streamA = { getTracks: () => [audioA], getAudioTracks: () => [audioA] }
    const streamB = { getTracks: () => [audioB], getAudioTracks: () => [audioB] }
    peerA.ontrack({ target: peerA, streams: [streamA], track: audioA })
    peerB.ontrack({ target: peerB, streams: [streamB], track: audioB })
    expect(audioA.enabled).toBe(false); expect(audioB.enabled).toBe(false)
    a.session.setMuted(false)
    b.session.setMuted(false)
    expect(audioA.enabled).toBe(true); expect(audioB.enabled).toBe(false)
    await a.wrapper.setProps({ selected: false })
    await b.wrapper.setProps({ selected: true })
    expect(audioA.enabled).toBe(false)
    expect(peerA.channel.sent.at(-1)).toEqual({ type: 'audio', on: false })
    b.session.setMuted(false)
    expect(audioB.enabled).toBe(true)
    expect(a.wrapper.find('video').element.muted).toBe(true)
    expect(b.wrapper.find('video').element.muted).toBe(false)
    a.wrapper.unmount()
    expect(peerA.closed).toBe(true); expect(peerB.closed).toBe(false)
    expect(audioB.enabled).toBe(true)
    expect(api.cancelRun).not.toHaveBeenCalled()
  })
})
