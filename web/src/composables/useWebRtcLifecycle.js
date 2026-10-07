import { useConfirmDialog } from '../components/ui/useConfirmDialog'
import { getCurrentScope, onScopeDispose, ref, shallowRef, watch } from 'vue'

function makeNoop() {}

// 被接管（服务端仲裁顶替）的持久文案：落在错误栏常驻显示（区别于一闪而过的
// toast），用户手动重连（onConnectStart 清空 errorMsg）前不消失
const TAKEN_OVER_MSG = '本页投屏已被其它页面接管，可手动重新连接'

export function useWebRtcLifecycle({
  api,
  deviceIdRef,
  connectedRef,
  connectingRef,
  errorMsgRef,
  supersededRef,
  manualCloseRef,
  toast = makeNoop,
  onPeerReset = makeNoop,
  onConnectStart = makeNoop,
  onConnectSuccess = makeNoop,
  onConnectFinish = makeNoop,
  onDisconnect = makeNoop,
  onChannelOpen = makeNoop,
  onChannelClose = makeNoop,
  onOfferAnswer = makeNoop,
  onSignalMessage = makeNoop,
  onRemoteTrack = makeNoop,
  onControlMessage = makeNoop,
  onPeerCreated = makeNoop,
  onPeerDisposed = makeNoop,
  onSignalOpen = makeNoop,
  onSignalClose = makeNoop,
} = {}) {
  const confirmDialog = useConfirmDialog()
  const reconnectTimer = shallowRef(null)
  const reconnectAttempts = ref(0)

  // Each connect owns an immutable target and its own resources. Neither a late
  // promise nor a queued browser callback may act on a replacement connection.
  const CANCELLED = Symbol('cancelled connection')
  let generation = 0
  let active = null
  let connectLock = null
  let closedByCleanup = false

  const current = attempt => active === attempt && attempt.generation === generation
    && attempt.deviceId === deviceIdRef?.value
  const live = (attempt, link) => current(attempt) && attempt.link === link && !link.disposed
  const details = (attempt, extra = {}) => ({ deviceId: attempt.deviceId, ...extra })

  function cancelReconnect() {
    if (reconnectTimer.value !== null) {
      clearTimeout(reconnectTimer.value)
      reconnectTimer.value = null
    }
  }

  function scheduleReconnect({ superseded = supersededRef } = {}) {
    if (closedByCleanup || reconnectTimer.value !== null || !deviceIdRef?.value) return false
    if (superseded?.value) {
      if (errorMsgRef) errorMsgRef.value = TAKEN_OVER_MSG
      return false
    }
    const target = deviceIdRef.value
    const epoch = generation
    const delay = [3000, 6000, 12000][Math.min(reconnectAttempts.value, 2)]
    const attemptNo = ++reconnectAttempts.value
    if (attemptNo <= 2) toast(`连接已断开，${delay / 1000} 秒后自动重连…`, 'warn')
    else if (errorMsgRef) errorMsgRef.value = `连接已断开，自动重连中…（第 ${attemptNo} 次）`
    const timer = setTimeout(() => {
      if (reconnectTimer.value !== timer) return
      reconnectTimer.value = null
      if (epoch !== generation || target !== deviceIdRef.value || closedByCleanup) return
      if (superseded?.value) {
        if (errorMsgRef) errorMsgRef.value = TAKEN_OVER_MSG
        return
      }
      connect(false)
    }, delay)
    reconnectTimer.value = timer
    return true
  }

  function disposeLink(attempt, manual = false) {
    const link = attempt?.link
    if (!link || link.disposed) return
    link.disposed = true
    link.cancel()
    for (const cancel of [...link.pending]) cancel()
    for (const channel of link.channels) {
      channel.onopen = channel.onclose = channel.onmessage = null
      try { channel.close() } catch {}
    }
    if (link.pc) {
      link.pc.ontrack = link.pc.ondatachannel = null
      try { link.pc.close() } catch {}
      onPeerDisposed(details(attempt, { pc: link.pc, manual }))
    }
    if (link.ws) {
      link.ws.onopen = link.ws.onmessage = link.ws.onerror = link.ws.onclose = null
      try { link.ws.close() } catch {}
    }
    link.controlChannel = null
  }

  function cleanup(manual = false) {
    const previous = active
    const hadWork = !!previous || reconnectTimer.value !== null
    active = null
    connectLock = null
    generation++
    closedByCleanup = true
    confirmDialog.cancel()
    cancelReconnect()
    reconnectAttempts.value = 0
    previous?.cancel()
    disposeLink(previous, manual)
    connectedRef.value = false
    connectingRef.value = false
    manualCloseRef.value = manual
    if (hadWork) onDisconnect({ manual, deviceId: previous?.deviceId })
  }

  // Cancel API/SDP waits immediately as well as ignoring their eventual result.
  async function waitFor(attempt, link, promise) {
    const result = await Promise.race([promise, attempt.cancelled, link.cancelled])
    if (result === CANCELLED || !live(attempt, link)) throw CANCELLED
    return result
  }

  function waitEvent(attempt, link, setup, timeout, message) {
    return waitFor(attempt, link, new Promise((resolve, reject) => {
      let settled = false
      let dispose = makeNoop
      let timer
      const finish = (value, error = false) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        link.pending.delete(cancel)
        dispose()
        if (error) reject(value)
        else resolve(value)
      }
      const cancel = () => finish(CANCELLED)
      link.pending.add(cancel)
      timer = setTimeout(() => finish(new Error(message), true), timeout)
      dispose = setup(value => finish(value), error => finish(error, true)) || makeNoop
      if (settled) dispose()
    }))
  }

  function bindControlChannel(attempt, link, channel) {
    if (!channel) return
    link.channels.add(channel)
    link.controlChannel = channel
    const valid = () => live(attempt, link) && link.controlChannel === channel
    channel.onopen = () => {
      if (!valid()) return
      reconnectAttempts.value = 0
      onChannelOpen(details(attempt, { controlChannel: channel }))
    }
    channel.onclose = () => {
      if (!valid()) return
      connectedRef.value = false
      onChannelClose(details(attempt, { controlChannel: channel }))
      if (!valid()) return
      connectingRef.value = false
      disposeLink(attempt, false)
      if (current(attempt) && !manualCloseRef.value && !supersededRef.value) scheduleReconnect()
    }
    channel.onmessage = event => { if (valid()) onControlMessage(event) }
  }

  async function doConnect(attempt, force = false) {
    const link = { pc: null, ws: null, controlChannel: null, channels: new Set(), pending: new Set(), disposed: false }
    link.cancelled = new Promise(resolve => { link.cancel = () => resolve(CANCELLED) })
    attempt.link = link
    connectingRef.value = true
    manualCloseRef.value = false
    supersededRef.value = false
    errorMsgRef.value = ''
    onConnectStart(details(attempt))
    let deviceConnected = false
    try {
      if (!live(attempt, link)) throw CANCELLED
      await waitFor(attempt, link, api.connectDevice(attempt.deviceId))
      deviceConnected = true
      const wsProto = location.protocol === 'https:' ? 'wss:' : 'ws:'
      const ws = link.ws = new WebSocket(`${wsProto}//${location.host}/ws/device/${attempt.deviceId}`)
      await waitEvent(attempt, link, (resolve, reject) => {
        ws.onopen = () => {
          if (!live(attempt, link)) return
          onSignalOpen(details(attempt, { ws }))
          resolve()
        }
        ws.onerror = () => reject(new Error('信令连接失败'))
        ws.onclose = () => reject(new Error('信令连接已关闭'))
      }, 10000, '信令连接超时')

      ws.onclose = () => {
        if (!live(attempt, link)) return
        connectedRef.value = false
        connectingRef.value = false
        onSignalClose(details(attempt, { ws }))
        if (!live(attempt, link)) return
        disposeLink(attempt, false)
        if (current(attempt) && !manualCloseRef.value && !supersededRef.value) scheduleReconnect()
      }

      const pc = link.pc = new RTCPeerConnection()
      onPeerCreated(details(attempt, { pc }))
      if (!live(attempt, link)) throw CANCELLED
      pc.addTransceiver('video', { direction: 'recvonly' })
      pc.addTransceiver('audio', { direction: 'recvonly' })
      bindControlChannel(attempt, link, pc.createDataChannel('control'))
      pc.ontrack = event => { if (live(attempt, link)) onRemoteTrack(details(attempt, { event, pc })) }
      pc.ondatachannel = event => {
        if (live(attempt, link)) bindControlChannel(attempt, link, event.channel)
        else { try { event.channel.close() } catch {} }
      }
      const offer = await waitFor(attempt, link, pc.createOffer())
      await waitFor(attempt, link, pc.setLocalDescription(offer))
      // The server is non-trickle: send localDescription after gathering host
      // candidates, bounded to two seconds for networks that never finish ICE.
      if (pc.iceGatheringState !== 'complete' && typeof pc.addEventListener === 'function'
        && typeof pc.removeEventListener === 'function') {
        await waitEvent(attempt, link, resolve => {
          const onGather = () => { if (pc.iceGatheringState === 'complete') resolve() }
          pc.addEventListener('icegatheringstatechange', onGather)
          const timer = setTimeout(resolve, 2000)
          return () => { clearTimeout(timer); pc.removeEventListener('icegatheringstatechange', onGather) }
        }, 2100, 'ICE 收集超时')
      }
      let answerPending = true
      const answer = await waitEvent(attempt, link, (resolve, reject) => {
        ws.onerror = () => reject(new Error('信令连接失败'))
        ws.onmessage = event => {
          if (!live(attempt, link)) return
          try {
            const message = JSON.parse(event.data)
            onSignalMessage(details(attempt, { type: 'signal', message, ws }))
            if (!live(attempt, link)) return
            if (message.type === 'taken_over') {
              supersededRef.value = true
              errorMsgRef.value = TAKEN_OVER_MSG
              toast('连接已被其他页面接管', 'warn')
              cancelReconnect()
              connectedRef.value = false
              connectingRef.value = false
              onChannelClose(details(attempt, { controlChannel: link.controlChannel }))
              disposeLink(attempt, true)
            } else if (answerPending) {
              if (message.type === 'answer') resolve(message.sdp)
              else if (message.type === 'conflict') reject({ conflict: true })
              else if (message.type === 'error') reject(new Error(message.error || '信令错误'))
            }
          } catch (error) { if (answerPending) reject(error) }
        }
        ws.send(JSON.stringify({ type: 'offer', sdp: pc.localDescription ?? offer, force }))
      }, 10000, '信令超时')
      answerPending = false
      onOfferAnswer(details(attempt, { offer, answer }))
      if (!live(attempt, link)) throw CANCELLED
      await waitFor(attempt, link, pc.setRemoteDescription(new RTCSessionDescription(answer)))
      connectedRef.value = true
      connectingRef.value = false
      reconnectAttempts.value = 0
      onConnectSuccess(details(attempt, { pc, ws }))
      if (live(attempt, link)) onConnectFinish(details(attempt, { ok: true }))
      return true
    } catch (error) {
      if (error === CANCELLED || !live(attempt, link)) return
      connectingRef.value = false
      connectedRef.value = false
      disposeLink(attempt, true)
      if (!current(attempt)) return
      if (error?.conflict) {
        onConnectFinish(details(attempt, { ok: false, conflict: true }))
        throw error
      }
      errorMsgRef.value = (deviceConnected ? '' : '设备连接失败：') + error.message
      onConnectFinish(details(attempt, { ok: false, error }))
      return false
    }
  }

  async function connect(manual = false) {
    if (connectLock || connectingRef.value || connectedRef.value) return
    if (!deviceIdRef?.value) return toast('请先选择设备（设备页签下拉框）', 'error')
    if (!manual && supersededRef.value) return
    cancelReconnect()
    const previous = active
    active = null
    previous?.cancel()
    disposeLink(previous, true)
    const attempt = { deviceId: deviceIdRef.value, generation: ++generation, link: null }
    attempt.cancelled = new Promise(resolve => { attempt.cancel = () => resolve(CANCELLED) })
    active = connectLock = attempt
    closedByCleanup = false
    try {
      const ok = await doConnect(attempt)
      if (current(attempt) && ok === false && !manual) scheduleReconnect()
    } catch (error) {
      if (!current(attempt) || !error?.conflict) return
      if (manual) {
        const confirmed = await Promise.race([
          confirmDialog(`设备 ${attempt.deviceId} 正在其他页面投屏。\n\n确认接管连接？对方页面将断开且不会自动重连。`, { title: '接管投屏连接', confirmText: '接管连接' }),
          attempt.cancelled,
        ])
        if (!current(attempt)) return
        if (confirmed === true) await doConnect(attempt, true).catch(error => {
          if (current(attempt) && error?.conflict) errorMsgRef.value = '设备正在其他页面使用'
        })
        else errorMsgRef.value = '设备正在其他页面使用'
      } else {
        supersededRef.value = true
        errorMsgRef.value = '设备已在其他页面连接，本页已停止重连'
        toast(errorMsgRef.value, 'warn')
      }
    } finally {
      if (connectLock === attempt) connectLock = null
    }
  }

  // Synchronous invalidation also covers a target changing away and back before
  // the old REST request/offer resolves. Vue owns this watcher in component scope.
  watch(() => deviceIdRef?.value, () => cleanup(true), { flush: 'sync' })
  if (getCurrentScope()) onScopeDispose(() => cleanup(true))

  const getControlChannel = () => active?.link?.disposed ? null : active?.link?.controlChannel ?? null
  const getPeerConnection = () => active?.link?.disposed ? null : active?.link?.pc ?? null
  return {
    reconnectTimer, reconnectAttempts, connect, cleanup, cancelReconnect, scheduleReconnect,
    getControlChannel, getPeerConnection, hasActivePeer: () => !!getPeerConnection(),
  }
}
