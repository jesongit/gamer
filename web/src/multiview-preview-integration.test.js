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
  socket.receive({ type: 'frame', id: 1, jpeg, width: 1920, height: 1080,
    preview_width: 640, preview_height: 360, stamp: { target: id, epoch: 'epoch', revision: 1 } })
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
  it('forwards image decode failures through the actual stage and recovers without reconnecting', async () => {
    const a = mountTarget('browser-a', true)
    await a.session.connect()
    const socket = Socket.instances[0]
    await showFrame(a.wrapper, socket, 'browser-a')
    socket.receive({ type: 'frame', id: 2, jpeg: 'AA==', width: 1920, height: 1080,
      preview_width: 640, preview_height: 360, stamp: { target: 'browser-a', epoch: 'epoch', revision: 1 } })
    await flushPromises()
    await a.wrapper.find('img[alt="浏览器目标画面"]').trigger('error')
    expect(socket.sent.at(-1)).toEqual({ type: 'frame_ack', id: 2, displayed: false })
    expect(a.session.browser.view.stamp).toBeNull()
    await showFrame(a.wrapper, socket, 'browser-a')
    expect(a.session.browser.view.stamp).toEqual({ target: 'browser-a', epoch: 'epoch', revision: 1 })
    expect(a.session.errorMsg.value).toBe('')
    expect(Socket.instances).toHaveLength(1)
  })

  it('unexpected browser socket closure drops the image reference and dimensions while preserving preview intent', async () => {
    const a = mountTarget('browser-a', true)
    await a.session.connect()
    const socket = Socket.instances[0]
    const image = await showFrame(a.wrapper, socket, 'browser-a')
    expect(a.session.videoElement.value).toBe(image.element)
    expect(a.session.resolution.value).toBe('1920×1080')
    socket.close(); await flushPromises()
    expect(a.session.videoElement.value).toBeNull()
    expect(image.element.hasAttribute('src')).toBe(false)
    expect(a.session.resolution.value).toBe('—')
    expect(a.session.browser.view).toMatchObject({ src: '', stamp: null, width: 0, height: 0, previewWidth: 0, previewHeight: 0 })
    expect(a.session.previewWanted.value).toBe(true)
    expect(a.wrapper.find('img[alt="浏览器目标画面"]').exists()).toBe(false)
    await a.wrapper.setProps({ active: false }); await a.wrapper.setProps({ active: true }); await flushPromises()
    expect(Socket.instances).toHaveLength(2)
    expect(Socket.instances[1].closed).toBe(false)
    expect(api.cancelRun).not.toHaveBeenCalled()
  })

  it('hiding browser tiles releases frames and sockets, restores only requested previews and keeps manual close closed', async () => {
    const a = mountTarget('browser-a', true), b = mountTarget('browser-b')
    await a.session.connect(); await b.session.connect()
    const [socketA, socketB] = Socket.instances
    await showFrame(a.wrapper, socketA, 'browser-a')
    await showFrame(b.wrapper, socketB, 'browser-b')
    b.session.close()
    await a.wrapper.setProps({ active: false }); await b.wrapper.setProps({ active: false })
    expect(socketA.closed).toBe(true); expect(socketB.closed).toBe(true)
    expect(a.session.browser.view).toMatchObject({ src: '', width: 0, height: 0, stamp: null })
    expect(a.session.videoElement.value).toBeNull()
    expect(a.session.suspended.value).toBe(true)
    expect(b.session.suspended.value).toBe(false)
    await a.wrapper.setProps({ active: true, previewMode: 'grid9' })
    await b.wrapper.setProps({ active: true }); await flushPromises()
    expect(Socket.instances).toHaveLength(3)
    expect(Socket.instances[2].url).toContain('browser-a?mode=grid9')
    expect(api.cancelRun).not.toHaveBeenCalled()
  })

  it('a hidden browser target defers startup, and a stale startup cannot reopen it after another hide', async () => {
    const a = mountTarget('browser-a', true)
    await a.wrapper.setProps({ active: false })
    await a.session.connect()
    expect(api.connectBrowser).not.toHaveBeenCalled()
    let finish
    api.connectBrowser.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    await a.wrapper.setProps({ active: true })
    expect(api.connectBrowser).toHaveBeenCalledOnce()
    await a.wrapper.setProps({ active: false })
    finish({}); await flushPromises()
    expect(Socket.instances).toHaveLength(0)
    await a.wrapper.setProps({ active: true }); await flushPromises()
    expect(Socket.instances).toHaveLength(1)
  })

  it('hidden Android previews close the peer and clear the decoder without disconnecting the device or cancelling its run', async () => {
    const a = mountTarget('android-a', true)
    await a.session.connect()
    const firstPeer = Peer.instances[0], stop = vi.fn(), video = { kind: 'video', stop }
    const stream = { getTracks: () => [video], getAudioTracks: () => [] }
    firstPeer.ontrack({ target: firstPeer, streams: [stream], track: video })
    expect(a.wrapper.find('video').element.srcObject).toBe(stream)
    await a.wrapper.setProps({ active: false })
    expect(firstPeer.closed).toBe(true)
    expect(a.wrapper.find('video').element.srcObject).toBeNull()
    expect(stop).toHaveBeenCalledOnce()
    expect(a.session.previewWanted.value).toBe(true)
    await a.wrapper.setProps({ active: true }); await flushPromises()
    expect(Peer.instances).toHaveLength(2)
    expect(api.connectDevice.mock.calls).toEqual([['android-a'], ['android-a']])
    expect(api.cancelRun).not.toHaveBeenCalled()
    a.session.close()
    await a.wrapper.setProps({ active: false }); await a.wrapper.setProps({ active: true })
    expect(Peer.instances).toHaveLength(2)
  })
  it('grid mute disables every audio output and prevents selection or late tracks from unmuting it', async () => {
    const a = mountTarget('android-a', true)
    await a.session.connect()
    const peer = Peer.instances[0]
    peer.channel.onopen()
    const audio = { kind: 'audio', enabled: true }, stream = { getTracks: () => [audio], getAudioTracks: () => [audio] }
    peer.ontrack({ target: peer, streams: [stream], track: audio })
    a.session.setMuted(false)
    expect(audio.enabled).toBe(true)
    await a.wrapper.setProps({ forceMuted: true })
    expect(audio.enabled).toBe(false)
    expect(a.wrapper.find('video').element.muted).toBe(true)
    expect(peer.channel.sent.at(-1)).toEqual({ type: 'audio', on: false })
    a.session.setMuted(false)
    expect(audio.enabled).toBe(false)
    await a.wrapper.setProps({ selected: false }); await a.wrapper.setProps({ selected: true })
    const lateAudio = { kind: 'audio', enabled: true }
    peer.ontrack({ target: peer, streams: [{ getTracks: () => [lateAudio], getAudioTracks: () => [lateAudio] }], track: lateAudio })
    expect(lateAudio.enabled).toBe(false)
    await a.wrapper.setProps({ forceMuted: false })
    expect(lateAudio.enabled).toBe(false)
    a.session.setMuted(false)
    expect(lateAudio.enabled).toBe(true)
    expect(peer.closed).toBe(false)
    expect(api.cancelRun).not.toHaveBeenCalled()
  })
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
