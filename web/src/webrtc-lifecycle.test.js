import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
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

class FakeSocket {
  constructor() {
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
    if (msg.type === 'offer') {
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
    FakeSocket.mode = 'answer'
    global.WebSocket = FakeSocket
    global.RTCPeerConnection = FakePeer
    global.RTCSessionDescription = function RTCSessionDescription(desc) { return desc }
    vi.stubGlobal('location', { protocol: 'http:', host: 'example.test' })
    vi.stubGlobal('confirm', vi.fn(() => false))
  })

  afterEach(() => {
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
    global.confirm = vi.fn(() => true)
    await lifecycle.connect(true)
    expect(global.confirm).toHaveBeenCalled()
    expect(FakeSocket.instances.some(sock => sock.sent.some(s => JSON.parse(s).force === true))).toBe(true)
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
})
