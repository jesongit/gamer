import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest'
const { dialogDecision } = vi.hoisted(() => ({ dialogDecision: vi.fn() }))
vi.mock('./components/ui/useConfirmDialog', () => ({ useConfirmDialog: () => Object.assign(dialogDecision, { cancel: vi.fn() }) }))
import { effectScope, ref } from 'vue'
import { useWebRtcLifecycle } from './composables/useWebRtcLifecycle'

class FakeChannel {
  constructor() {
    this.readyState = 'open'
    this.onopen = null
    this.onclose = null
    this.onmessage = null
    this.sent = []
  }

  send(payload) {
    this.sent.push(payload)
  }

  close() {
    // 真实 DataChannel 对已关闭通道再次 close 不会重复触发 onclose
    if (this.readyState === 'closed') return
    this.readyState = 'closed'
    this.onclose?.()
  }
}

class FakePeer {
  constructor() {
    this.ontrack = null
    this.ondatachannel = null
    this.closed = false
    this.localDescription = null
    this.remoteDescription = null
    this.channel = new FakeChannel()
    FakePeer.instances.push(this)
  }

  addTransceiver() {}
  createDataChannel() { return this.channel }
  async createOffer() { return { type: 'offer', sdp: 'offer-sdp' } }
  async setLocalDescription(desc) { this.localDescription = desc }
  async setRemoteDescription(desc) { this.remoteDescription = desc }
  getStats() { return Promise.resolve(new Map()) }
  close() {
    this.closed = true
    this.channel.close()
  }
}

FakePeer.instances = []

class FakeSocket {
  constructor(url) {
    this.url = url
    this.closed = false
    this.onopen = null
    this.onmessage = null
    this.onerror = null
    this.onclose = null
    this.sent = []
    FakeSocket.instances.push(this)
    queueMicrotask(() => this.onopen?.())
  }

  send(payload) {
    this.sent.push(payload)
    const msg = JSON.parse(payload)
    if (msg.type === 'offer' && FakeSocket.mode !== 'silent') {
      queueMicrotask(() => {
        if (FakeSocket.mode === 'conflict' && !msg.force) {
          this.onmessage?.({ data: JSON.stringify({ type: 'conflict' }) })
          return
        }
        if (msg.force) {
          this.onmessage?.({ data: JSON.stringify({ type: 'answer', sdp: { type: 'answer', sdp: 'answer-sdp-force' } }) })
          return
        } else {
          this.onmessage?.({ data: JSON.stringify({ type: 'answer', sdp: { type: 'answer', sdp: 'answer-sdp' } }) })
        }
      })
    }
  }

  close() {
    this.closed = true
    this.onclose?.()
  }
}
FakeSocket.instances = []
FakeSocket.mode = 'answer'

function makeLifecycle(overrides = {}) {
  return useWebRtcLifecycle({
    api: { connectDevice: vi.fn().mockResolvedValue() },
    deviceIdRef: ref('dev-a'),
    connectedRef: ref(false),
    connectingRef: ref(false),
    errorMsgRef: ref(''),
    supersededRef: ref(false),
    manualCloseRef: ref(false),
    toast: vi.fn(),
    onControlMessage: vi.fn(),
    onRemoteTrack: vi.fn(),
    onConnectSuccess: vi.fn(),
    onDisconnect: vi.fn(),
    ...overrides,
  })
}

describe('useWebRtcLifecycle', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    FakeSocket.instances = []
    FakePeer.instances = []
    FakeSocket.mode = 'answer'
    global.WebSocket = FakeSocket
    global.RTCPeerConnection = FakePeer
    global.RTCSessionDescription = function RTCSessionDescription(desc) { return desc }
    vi.stubGlobal('location', { protocol: 'http:', host: 'example.test' })
    dialogDecision.mockReset().mockResolvedValue(false)
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.clearAllTimers()
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('schedules reconnect and cancels it in cleanup', async () => {
    const lifecycle = makeLifecycle()
    expect(lifecycle.scheduleReconnect()).toBe(true)
    lifecycle.cleanup()
    expect(lifecycle.reconnectTimer.value).toBeNull()
    vi.advanceTimersByTime(3000)
    expect(lifecycle.reconnectAttempts.value).toBe(0)
  })

  it('connects, forwards taken_over, and keeps cleanup idempotent', async () => {
    const toast = vi.fn()
    const errorMsgRef = ref('')
    const supersededRef = ref(false)
    const lifecycle = makeLifecycle({ toast, errorMsgRef, supersededRef })

    await lifecycle.connect(false)
    expect(FakeSocket.instances).toHaveLength(1)
    const socket = FakeSocket.instances[0]
    socket.onmessage?.({ data: JSON.stringify({ type: 'taken_over' }) })
    expect(supersededRef.value).toBe(true)
    expect(toast).toHaveBeenCalledWith('连接已被其他页面接管', 'warn')
    // 被接管落持久横幅（错误栏常驻），不再停在通用「未连接设备」
    expect(errorMsgRef.value).toBe('本页投屏已被其它页面接管，可手动重新连接')
    // superseded 置位后不再自动重连（scheduleReconnect 直接拒绝）
    expect(lifecycle.scheduleReconnect({ superseded: supersededRef })).toBe(false)
    expect(lifecycle.reconnectTimer.value).toBeNull()

    lifecycle.cleanup(true)
    lifecycle.cleanup(true)
    // cleanup 幂等且不清横幅：接管文案保留到用户手动重连（onConnectStart 清空）
    expect(errorMsgRef.value).toBe('本页投屏已被其它页面接管，可手动重新连接')
    expect(lifecycle.reconnectTimer.value).toBeNull()
  })

  it('propagates conflict on manual takeover and closes stale peer before retry', async () => {
    const lifecycle = makeLifecycle()
    FakeSocket.mode = 'conflict'
    dialogDecision.mockResolvedValue(true)
    await lifecycle.connect(true)
    expect(dialogDecision).toHaveBeenCalled()
    expect(FakeSocket.instances.some(sock => sock.sent.some(s => JSON.parse(s).force === true))).toBe(true)
  })

  it('isolates concurrent viewers, immutable targets, resources and cleanup', async () => {
    const stateA = ref(false), stateB = ref(false)
    const apiA = { connectDevice: vi.fn().mockResolvedValue() }
    const apiB = { connectDevice: vi.fn().mockResolvedValue() }
    const messageA = vi.fn(), messageB = vi.fn()
    const a = makeLifecycle({ deviceIdRef: ref('dev-a'), connectedRef: stateA, api: apiA, onControlMessage: messageA })
    const b = makeLifecycle({ deviceIdRef: ref('dev-b'), connectedRef: stateB, api: apiB, onControlMessage: messageB })
    await Promise.all([a.connect(true), b.connect(true)])
    expect(apiA.connectDevice).toHaveBeenCalledWith('dev-a')
    expect(apiB.connectDevice).toHaveBeenCalledWith('dev-b')
    expect(FakeSocket.instances.map(socket => socket.url)).toEqual([
      'ws://example.test/ws/device/dev-a', 'ws://example.test/ws/device/dev-b',
    ])
    const peerA = a.getPeerConnection(), peerB = b.getPeerConnection()
    const channelB = b.getControlChannel()
    a.getControlChannel().onmessage({ data: 'a' })
    channelB.onmessage({ data: 'b' })
    expect(messageA).toHaveBeenCalledExactlyOnceWith({ data: 'a' })
    expect(messageB).toHaveBeenCalledExactlyOnceWith({ data: 'b' })
    a.cleanup(true)
    expect(peerA.closed).toBe(true)
    expect(peerB.closed).toBe(false)
    expect(FakeSocket.instances[0].closed).toBe(true)
    expect(FakeSocket.instances[1].closed).toBe(false)
    expect(stateA.value).toBe(false)
    expect(stateB.value).toBe(true)
    expect(b.getControlChannel()).toBe(channelB)
    expect(b.reconnectTimer.value).toBeNull()
    b.cleanup(true)
  })

  it.each(['resolve', 'reject'])('cancels an in-flight device request and ignores its late %s', async outcome => {
    const pending = deferred()
    const connectDevice = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue()
    const connectedRef = ref(false), connectingRef = ref(false), errorMsgRef = ref('')
    const success = vi.fn(), finish = vi.fn(), disposed = vi.fn(), disconnected = vi.fn()
    const lifecycle = makeLifecycle({ api: { connectDevice }, connectedRef, connectingRef, errorMsgRef,
      onConnectSuccess: success, onConnectFinish: finish, onPeerDisposed: disposed, onDisconnect: disconnected })
    const first = lifecycle.connect(false)
    lifecycle.cleanup(true)
    await first // Does not wait for the uncancellable HTTP request.
    await lifecycle.connect(true)
    const replacement = lifecycle.getPeerConnection()
    pending[outcome](outcome === 'reject' ? new Error('old failure') : undefined)
    await flushPromises()
    expect(connectDevice).toHaveBeenCalledTimes(2)
    expect(FakeSocket.instances).toHaveLength(1)
    expect(success).toHaveBeenCalledTimes(1)
    expect(finish).toHaveBeenCalledTimes(1)
    expect(connectedRef.value).toBe(true)
    expect(connectingRef.value).toBe(false)
    expect(errorMsgRef.value).toBe('')
    expect(replacement.closed).toBe(false)
    expect(lifecycle.reconnectTimer.value).toBeNull()
    lifecycle.cleanup(true)
    lifecycle.cleanup(true)
    expect(disposed).toHaveBeenCalledTimes(1)
    expect(disconnected).toHaveBeenCalledTimes(2)
  })

  it.each(['createOffer', 'setLocalDescription', 'setRemoteDescription'])('ignores a late %s result after cleanup and replacement', async method => {
    const pending = deferred()
    vi.spyOn(FakePeer.prototype, method).mockImplementationOnce(() => pending.promise)
    const connectedRef = ref(false), success = vi.fn(), finish = vi.fn()
    const lifecycle = makeLifecycle({ connectedRef, onConnectSuccess: success, onConnectFinish: finish })
    const first = lifecycle.connect(true)
    await flushPromises()
    expect(FakePeer.instances).toHaveLength(1)
    const original = FakePeer.instances[0]
    lifecycle.cleanup(true)
    await first
    await lifecycle.connect(true)
    const replacement = lifecycle.getPeerConnection()
    pending.resolve({ type: 'offer', sdp: 'late-sdp' })
    await flushPromises()
    expect(original.closed).toBe(true)
    expect(FakeSocket.instances[0].closed).toBe(true)
    expect(replacement.closed).toBe(false)
    expect(connectedRef.value).toBe(true)
    expect(success).toHaveBeenCalledTimes(1)
    expect(finish).toHaveBeenCalledTimes(1)
    if (method !== 'setRemoteDescription') expect(FakeSocket.instances[0].sent).toHaveLength(0)
    expect(vi.getTimerCount()).toBe(0)
    lifecycle.cleanup(true)
  })

  it('invalidates a changed target immediately, including changing away and back', async () => {
    const pending = deferred(), target = ref('dev-a')
    const connectDevice = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue()
    const connectedRef = ref(false), success = vi.fn()
    const lifecycle = makeLifecycle({ deviceIdRef: target, api: { connectDevice }, connectedRef, onConnectSuccess: success })
    const first = lifecycle.connect(true)
    target.value = 'dev-b'
    target.value = 'dev-a'
    await lifecycle.connect(true)
    pending.resolve()
    await first
    expect(connectDevice.mock.calls).toEqual([['dev-a'], ['dev-a']])
    expect(FakeSocket.instances).toHaveLength(1)
    expect(success).toHaveBeenCalledTimes(1)
    expect(connectedRef.value).toBe(true)
    target.value = 'dev-b'
    expect(connectedRef.value).toBe(false)
    expect(FakeSocket.instances[0].closed).toBe(true)
    await lifecycle.connect(true)
    expect(FakeSocket.instances[1].url.endsWith('/dev-b')).toBe(true)
    expect(success.mock.calls[1][0].deviceId).toBe('dev-b')
    lifecycle.cleanup(true)
  })

  it('queued old peer, channel and signaling callbacks cannot change a new connection', async () => {
    const connectedRef = ref(false), errorMsgRef = ref(''), supersededRef = ref(false)
    const track = vi.fn(), message = vi.fn(), opened = vi.fn(), closed = vi.fn(), signal = vi.fn()
    const lifecycle = makeLifecycle({ connectedRef, errorMsgRef, supersededRef, onRemoteTrack: track,
      onControlMessage: message, onChannelOpen: opened, onChannelClose: closed, onSignalMessage: signal })
    await lifecycle.connect(true)
    const pc = lifecycle.getPeerConnection(), channel = lifecycle.getControlChannel(), ws = FakeSocket.instances[0]
    const callbacks = { track: pc.ontrack, data: pc.ondatachannel, open: channel.onopen,
      close: channel.onclose, message: channel.onmessage, signal: ws.onmessage, socketClose: ws.onclose }
    lifecycle.cleanup(true)
    await lifecycle.connect(true)
    const currentPeer = lifecycle.getPeerConnection(), currentChannel = lifecycle.getControlChannel()
    signal.mockClear()
    const lateChannel = new FakeChannel()
    callbacks.track({ target: pc, streams: [] })
    callbacks.data({ channel: lateChannel })
    callbacks.open()
    callbacks.close()
    callbacks.message({ data: 'old' })
    callbacks.signal({ data: JSON.stringify({ type: 'taken_over' }) })
    callbacks.socketClose()
    expect(track).not.toHaveBeenCalled()
    expect(message).not.toHaveBeenCalled()
    expect(opened).not.toHaveBeenCalled()
    expect(closed).not.toHaveBeenCalled()
    expect(signal).not.toHaveBeenCalled()
    expect(lateChannel.readyState).toBe('closed')
    expect(currentPeer.closed).toBe(false)
    expect(lifecycle.getControlChannel()).toBe(currentChannel)
    expect(connectedRef.value).toBe(true)
    expect(errorMsgRef.value).toBe('')
    expect(supersededRef.value).toBe(false)
    expect(lifecycle.reconnectTimer.value).toBeNull()
    lifecycle.cleanup(true)
  })

  it('cancels an unopened socket and releases its lock without waiting for timeout', async () => {
    class UnopenedSocket extends FakeSocket {
      constructor(url) { super(url); this.onopen = null }
    }
    // Prevent the FakeSocket microtask from opening by capturing the event on a
    // setter. The saved callback simulates an already queued browser event.
    let lateOpen
    Object.defineProperty(UnopenedSocket.prototype, 'onopen', {
      set(callback) { if (callback) lateOpen = callback }, get() { return null },
    })
    global.WebSocket = UnopenedSocket
    const success = vi.fn(), lifecycle = makeLifecycle({ onConnectSuccess: success })
    const pending = lifecycle.connect(true)
    await flushPromises()
    lifecycle.cleanup(true)
    await pending
    lateOpen()
    expect(FakePeer.instances).toHaveLength(0)
    expect(FakeSocket.instances[0].closed).toBe(true)
    expect(vi.getTimerCount()).toBe(0)
    global.WebSocket = FakeSocket
    await lifecycle.connect(true)
    expect(success).toHaveBeenCalledTimes(1)
    lifecycle.cleanup(true)
  })

  it('cancels pending answers and ICE listeners without leaking timers', async () => {
    FakeSocket.mode = 'silent'
    const lifecycle = makeLifecycle()
    const pending = lifecycle.connect(true)
    await flushPromises()
    const socket = FakeSocket.instances[0], lateAnswer = socket.onmessage
    expect(socket.sent).toHaveLength(1)
    lifecycle.cleanup(true)
    await pending
    lateAnswer({ data: JSON.stringify({ type: 'answer', sdp: { type: 'answer', sdp: 'late' } }) })
    expect(FakePeer.instances[0].remoteDescription).toBeNull()
    expect(vi.getTimerCount()).toBe(0)
    const added = vi.fn(), removed = vi.fn()
    FakePeer.prototype.addEventListener = added
    FakePeer.prototype.removeEventListener = removed
    try {
      FakeSocket.mode = 'answer'
      const ice = lifecycle.connect(true)
      await flushPromises()
      expect(added).toHaveBeenCalledTimes(1)
      lifecycle.cleanup(true)
      await ice
      expect(removed).toHaveBeenCalledWith('icegatheringstatechange', added.mock.calls[0][1])
      expect(FakeSocket.instances[1].sent).toHaveLength(0)
      expect(vi.getTimerCount()).toBe(0)
    } finally {
      delete FakePeer.prototype.addEventListener
      delete FakePeer.prototype.removeEventListener
    }
  })

  it.each(['complete', 'timeout'])('sends gathered local SDP when ICE reaches %s and clears its listeners', async mode => {
    class GatheringPeer extends FakePeer {
      constructor() { super(); this.iceGatheringState = 'gathering'; this.listeners = new Map() }
      addEventListener(name, callback) { this.listeners.set(name, callback) }
      removeEventListener(name, callback) { if (this.listeners.get(name) === callback) this.listeners.delete(name) }
      async setLocalDescription(offer) { this.localDescription = { ...offer, sdp: 'sdp-with-host-candidate' } }
    }
    global.RTCPeerConnection = GatheringPeer
    const lifecycle = makeLifecycle()
    const connection = lifecycle.connect(true)
    await flushPromises()
    const peer = lifecycle.getPeerConnection()
    expect(FakeSocket.instances[0].sent).toHaveLength(0)
    if (mode === 'complete') {
      peer.iceGatheringState = 'complete'
      peer.listeners.get('icegatheringstatechange')()
    } else await vi.advanceTimersByTimeAsync(2000)
    await connection
    expect(JSON.parse(FakeSocket.instances[0].sent[0]).sdp.sdp).toBe('sdp-with-host-candidate')
    expect(peer.listeners.size).toBe(0)
    expect(vi.getTimerCount()).toBe(0)
    lifecycle.cleanup(true)
  })

  it('does not auto-take-over, and ignores a manual confirmation after cleanup on the same target', async () => {
    FakeSocket.mode = 'conflict'
    const supersededRef = ref(false), lifecycle = makeLifecycle({ supersededRef })
    await lifecycle.connect(false)
    expect(dialogDecision).not.toHaveBeenCalled()
    expect(supersededRef.value).toBe(true)
    expect(lifecycle.reconnectTimer.value).toBeNull()
    await lifecycle.connect(false)
    expect(FakeSocket.instances).toHaveLength(1)
    const decision = deferred()
    dialogDecision.mockReturnValueOnce(decision.promise)
    const manual = lifecycle.connect(true)
    await flushPromises()
    expect(dialogDecision).toHaveBeenCalledTimes(1)
    lifecycle.cleanup(true)
    await manual
    FakeSocket.mode = 'answer'
    await lifecycle.connect(true)
    const peer = lifecycle.getPeerConnection()
    decision.resolve(true)
    await flushPromises()
    expect(FakeSocket.instances).toHaveLength(3)
    expect(FakeSocket.instances.flatMap(socket => socket.sent).map(JSON.parse).every(offer => offer.force === false)).toBe(true)
    expect(peer.closed).toBe(false)
    lifecycle.cleanup(true)
  })

  it('blocks duplicate connects and cancels target-bound retry timers on target change', async () => {
    const pending = deferred(), target = ref('dev-a')
    const connectDevice = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue()
    const lifecycle = makeLifecycle({ deviceIdRef: target, api: { connectDevice } })
    const first = lifecycle.connect(false)
    await lifecycle.connect(true)
    expect(connectDevice).toHaveBeenCalledTimes(1)
    pending.reject(new Error('offline'))
    await first
    expect(lifecycle.reconnectTimer.value).not.toBeNull()
    target.value = 'dev-b'
    await vi.advanceTimersByTimeAsync(12000)
    expect(connectDevice).toHaveBeenCalledTimes(1)
    expect(lifecycle.reconnectTimer.value).toBeNull()
    await lifecycle.connect(true)
    await lifecycle.connect(true)
    expect(connectDevice).toHaveBeenCalledTimes(2)
    expect(connectDevice).toHaveBeenLastCalledWith('dev-b')
    lifecycle.cleanup(true)
  })

  it.each(['socket', 'channel', 'taken_over'])('does not resurrect a connection when %s closes it during remote SDP', async reason => {
    const pending = deferred()
    vi.spyOn(FakePeer.prototype, 'setRemoteDescription').mockImplementationOnce(() => pending.promise)
    const connectedRef = ref(false), connectingRef = ref(false), supersededRef = ref(false), success = vi.fn()
    const lifecycle = makeLifecycle({ connectedRef, connectingRef, supersededRef, onConnectSuccess: success })
    const first = lifecycle.connect(true)
    await flushPromises()
    const peer = lifecycle.getPeerConnection(), socket = FakeSocket.instances[0]
    expect(peer).not.toBeNull()
    if (reason === 'socket') socket.close()
    else if (reason === 'channel') lifecycle.getControlChannel().close()
    else socket.onmessage({ data: JSON.stringify({ type: 'taken_over' }) })
    await first
    expect(peer.closed).toBe(true)
    expect(connectedRef.value).toBe(false)
    expect(connectingRef.value).toBe(false)
    expect(success).not.toHaveBeenCalled()
    pending.resolve()
    await flushPromises()
    expect(success).not.toHaveBeenCalled()
    if (reason === 'taken_over') {
      expect(supersededRef.value).toBe(true)
      expect(lifecycle.reconnectTimer.value).toBeNull()
    } else {
      await vi.advanceTimersByTimeAsync(3000)
      expect(connectedRef.value).toBe(true)
      expect(success).toHaveBeenCalledTimes(1)
      expect(FakeSocket.instances).toHaveLength(2)
    }
    lifecycle.cleanup(true)
  })

  it('cancels an in-flight offer on socket closure and reconnects without waiting for it', async () => {
    const pending = deferred()
    vi.spyOn(FakePeer.prototype, 'createOffer').mockImplementationOnce(() => pending.promise)
    const success = vi.fn(), lifecycle = makeLifecycle({ onConnectSuccess: success })
    const first = lifecycle.connect(false)
    await flushPromises()
    const socket = FakeSocket.instances[0]
    socket.close()
    await first
    expect(FakePeer.instances[0].closed).toBe(true)
    expect(lifecycle.reconnectTimer.value).not.toBeNull()
    await vi.advanceTimersByTimeAsync(3000)
    expect(success).toHaveBeenCalledTimes(1)
    pending.resolve({ type: 'offer', sdp: 'late' })
    await flushPromises()
    expect(socket.sent).toHaveLength(0)
    expect(lifecycle.getPeerConnection().closed).toBe(false)
    lifecycle.cleanup(true)
  })

  it('cancels a manual takeover dialog on target change and never sends force to the replacement target', async () => {
    const decision = deferred(), deviceIdRef = ref('dev-a')
    dialogDecision.mockReturnValueOnce(decision.promise)
    FakeSocket.mode = 'conflict'
    const lifecycle = makeLifecycle({ deviceIdRef })
    const first = lifecycle.connect(true)
    await flushPromises()
    expect(dialogDecision).toHaveBeenCalledTimes(1)
    deviceIdRef.value = 'dev-b'
    await first
    FakeSocket.mode = 'answer'
    await lifecycle.connect(true)
    decision.resolve(true)
    await flushPromises()
    expect(FakeSocket.instances).toHaveLength(2)
    expect(FakeSocket.instances[1].url).toBe('ws://example.test/ws/device/dev-b')
    expect(FakeSocket.instances[1].sent.map(JSON.parse)).toEqual([
      { type: 'offer', sdp: { type: 'offer', sdp: 'offer-sdp' }, force: false },
    ])
    lifecycle.cleanup(true)
  })

  it('disposes component-scoped resources and pending connects on scope stop', async () => {
    const pending = deferred(), scope = effectScope(), success = vi.fn()
    let lifecycle
    scope.run(() => { lifecycle = makeLifecycle({ api: { connectDevice: () => pending.promise }, onConnectSuccess: success }) })
    const first = lifecycle.connect(true)
    scope.stop()
    await first
    pending.resolve()
    await flushPromises()
    expect(FakeSocket.instances).toHaveLength(0)
    expect(success).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
  })

  // 服务端停机/重启窗口内 connectDevice 与信令 ws 都会失败：自动重连必须续链
  // （否则一次失败后重连永久停摆，页面定格成死图只能手动刷新）
  it('auto reconnect keeps retrying after failures and recovers when server returns', async () => {
    const connectDevice = vi.fn()
      .mockRejectedValueOnce(new Error('server down'))
      .mockRejectedValueOnce(new Error('server down'))
      .mockResolvedValue()
    const connectedRef = ref(false)
    const lifecycle = makeLifecycle({ api: { connectDevice }, connectedRef })

    await lifecycle.connect(false)
    expect(connectDevice).toHaveBeenCalledTimes(1)
    expect(lifecycle.reconnectTimer.value).not.toBeNull()

    await vi.advanceTimersByTimeAsync(3000)
    expect(connectDevice).toHaveBeenCalledTimes(2)
    expect(lifecycle.reconnectTimer.value).not.toBeNull()

    await vi.advanceTimersByTimeAsync(6000)
    expect(connectDevice).toHaveBeenCalledTimes(3)
    expect(connectedRef.value).toBe(true)
    expect(lifecycle.reconnectTimer.value).toBeNull()
    expect(lifecycle.reconnectAttempts.value).toBe(0)
  })

  // 长时间停机降噪：仅前两次弹 toast，之后写错误栏静默重试
  it('reduces reconnect toast noise to the first two attempts', async () => {
    const toast = vi.fn()
    const errorMsgRef = ref('')
    const connectDevice = vi.fn().mockRejectedValue(new Error('server down'))
    const lifecycle = makeLifecycle({ api: { connectDevice }, toast, errorMsgRef })

    await lifecycle.connect(false)
    await vi.advanceTimersByTimeAsync(3000)
    await vi.advanceTimersByTimeAsync(6000)

    const disconnectToasts = toast.mock.calls.filter(([m]) => String(m).startsWith('连接已断开，'))
    expect(disconnectToasts).toHaveLength(2)
    expect(errorMsgRef.value).toContain('自动重连中')
  })

  it('manual connect failure does not auto-reschedule', async () => {
    const connectDevice = vi.fn().mockRejectedValue(new Error('server down'))
    const lifecycle = makeLifecycle({ api: { connectDevice } })
    await lifecycle.connect(true)
    expect(lifecycle.reconnectTimer.value).toBeNull()
  })

  // 自动重连建链必须复位 manualClose 标志：doConnect 开头对残留旧 pc 调
  // stopPeer({manual:true}) 置位后无人复位，下一次服务端踢连接（配置变更踢
  // viewer / watchdog 确死强拆）会被 onclose 守卫误判为手动关闭而跳过重连，
  // 页面定格最后一帧永不重试（G3 回归实测复现两次）
  it('auto reconnect resets manual flag so a second server kick reconnects again', async () => {
    const connectedRef = ref(false)
    const lifecycle = makeLifecycle({
      connectedRef,
      // 模拟 Console 接线：channel open/close 同步 connectedRef，
      // 否则 connect() 的 connectedRef 守卫会拦截重连
      onChannelOpen: () => { connectedRef.value = true },
      onChannelClose: () => { connectedRef.value = false },
    })

    await lifecycle.connect(false)
    const channel1 = lifecycle.getControlChannel()
    expect(channel1.readyState).toBe('open')

    // 第一次服务端踢连接 → 3s 后自动重连成功（新 channel）
    channel1.close()
    expect(lifecycle.reconnectTimer.value).not.toBeNull()
    await vi.advanceTimersByTimeAsync(3000)
    const channel2 = lifecycle.getControlChannel()
    expect(channel2).not.toBe(channel1)
    expect(channel2.readyState).toBe('open')

    // 第二次服务端踢连接必须再次触发自动重连
    // （修复前被 doConnect 遗留的 manualClose=true 拦截，定时器为空）
    channel2.close()
    expect(lifecycle.reconnectTimer.value).not.toBeNull()

    // 重连恢复后用户手动关闭（cleanup(true) → manual 标志）不自动重连：
    // 既有正确行为不因修复改变
    await vi.advanceTimersByTimeAsync(3000)
    const channel3 = lifecycle.getControlChannel()
    expect(channel3.readyState).toBe('open')
    lifecycle.cleanup(true)
    expect(lifecycle.reconnectTimer.value).toBeNull()
  })
})

function deferred() {
  let resolve, reject
  const promise = new Promise((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

async function flushPromises() {
  for (let i = 0; i < 30; i++) await Promise.resolve()
}
