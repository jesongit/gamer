import { ref } from 'vue'

function makeNoop() {}

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
  const reconnectTimer = ref(null)
  const reconnectAttempts = ref(0)

  let ws = null
  let pc = null
  let controlChannel = null
  let connectLock = false
  let forceTakeover = false
  let closedByCleanup = false

  function cancelReconnect() {
    if (reconnectTimer.value) {
      clearTimeout(reconnectTimer.value)
      reconnectTimer.value = null
    }
  }

  function scheduleReconnect({ superseded } = {}) {
    if (reconnectTimer.value || !deviceIdRef?.value) return false
    if (superseded?.value) {
      if (errorMsgRef) errorMsgRef.value = '连接已被其他页面接管'
      return false
    }
    const delay = [3000, 6000, 12000][Math.min(reconnectAttempts.value, 2)]
    const attemptNo = reconnectAttempts.value + 1
    reconnectAttempts.value = attemptNo
    // 长时间停机（服务端重建等）会连续重试：仅前两次弹 toast，之后静默重试
    // 并把状态写进错误栏，避免每 12s 一条提示刷屏
    if (attemptNo <= 2) {
      toast(`连接已断开，${delay / 1000} 秒后自动重连…`, 'warn')
    } else if (errorMsgRef) {
      errorMsgRef.value = `连接已断开，自动重连中…（第 ${attemptNo} 次）`
    }
    reconnectTimer.value = setTimeout(() => {
      reconnectTimer.value = null
      if (superseded?.value) {
        if (errorMsgRef) errorMsgRef.value = '连接已被其他页面接管'
        return
      }
      connect(false)
    }, delay)
    return true
  }

  function stopPeer({ manual = false, preserveReconnect = false } = {}) {
    const currentPc = pc
    const currentWs = ws
    pc = null
    ws = null
    controlChannel = null
    if (!preserveReconnect) cancelReconnect()
    if (currentPc) {
      manualCloseRef.value = manual
      try { currentPc.close() } catch (e) {}
      onPeerDisposed({ pc: currentPc, manual })
    }
    if (currentWs) {
      try { currentWs.close() } catch (e) {}
    }
  }

  function cleanup(manual = false) {
    closedByCleanup = true
    cancelReconnect()
    reconnectAttempts.value = 0
    stopPeer({ manual, preserveReconnect: true })
    connectedRef.value = false
    connectingRef.value = false
    if (!manual) manualCloseRef.value = false
    onDisconnect({ manual })
  }

  function bindControlChannel(channel) {
    if (!channel) return null
    controlChannel = channel
    channel.onopen = () => {
      onChannelOpen({ controlChannel: channel })
      reconnectAttempts.value = 0
    }
    channel.onclose = () => {
      onChannelClose({ controlChannel: channel })
      if (!manualCloseRef.value && !supersededRef.value && !closedByCleanup) {
        scheduleReconnect({ superseded: supersededRef })
      }
      manualCloseRef.value = false
    }
    return channel
  }

  async function doConnect() {
    if (!deviceIdRef?.value) return toast('请先选择设备（设备页签下拉框）', 'error')
    if (pc) stopPeer({ manual: true })
    supersededRef.value = false
    errorMsgRef.value = ''
    connectingRef.value = true
    onConnectStart()

    try {
      await api.connectDevice(deviceIdRef.value)
    } catch (e) {
      connectingRef.value = false
      errorMsgRef.value = '设备连接失败：' + e.message
      onConnectFinish({ ok: false, error: e })
      return false
    }

    try {
      const wsProto = location.protocol === 'https:' ? 'wss:' : 'ws:'
      ws = new WebSocket(`${wsProto}//${location.host}/ws/device/${deviceIdRef.value}`)
      await new Promise((resolve, reject) => {
        ws.onopen = () => {
          onSignalOpen({ ws })
          resolve()
        }
        ws.onerror = () => reject(new Error('信令连接失败'))
      })

      // 不配 STUN：浏览器↔服务端同机/局域网直连走 host 候选即可（跨网不在
      // 支持范围）；Google STUN 国内不可达，白等收集超时反而拖慢建连
      pc = new RTCPeerConnection()
      onPeerCreated({ pc })
      pc.addTransceiver('video', { direction: 'recvonly' })
      pc.addTransceiver('audio', { direction: 'recvonly' })

      const originalChannel = bindControlChannel(pc.createDataChannel('control'))
      if (originalChannel) originalChannel.onmessage = ev => onControlMessage(ev)

      pc.ontrack = e => onRemoteTrack({ event: e, pc })
      pc.ondatachannel = e => {
        const channel = bindControlChannel(e.channel)
        if (channel) channel.onmessage = ev => onControlMessage(ev)
      }

      const offer = await pc.createOffer()
      await pc.setLocalDescription(offer)
      // createOffer 返回的 SDP 不含任何 a=candidate（候选由 setLocalDescription
      // 后的 localDescription 携带）。直接把 createOffer 的结果发给服务端
      // （webrtc-rs 非 trickle，不收后续 candidate 消息）= 服务端零远端候选 →
      // ICE 零 pair，容器部署下只剩「浏览器对 answer 候选的 prflx 回路」一条
      // 通路可走。等收集完成再发 localDescription；受限网络收集可能不结束，
      // 2000ms 兜底按现状发送（Chrome 官方 demo waitIceGatheringComplete 同口径）
      await new Promise((resolve) => {
        if (pc.iceGatheringState === 'complete') return resolve()
        // 最小实现（单测 FakePeer 等）无候选收集 API：直接放行不等待
        if (typeof pc.addEventListener !== 'function' || typeof pc.removeEventListener !== 'function') {
          return resolve()
        }
        let settle = false
        const done = () => {
          if (settle) return
          settle = true
          clearTimeout(timer)
          pc.removeEventListener('icegatheringstatechange', onGather)
          resolve()
        }
        const timer = setTimeout(done, 2000)
        const onGather = () => {
          if (pc.iceGatheringState === 'complete') done()
        }
        pc.addEventListener('icegatheringstatechange', onGather)
      })
      const offerToSend = pc.localDescription ?? offer
      const answer = await new Promise((resolve, reject) => {
        ws.onmessage = evt => {
          try {
            const msg = JSON.parse(evt.data)
            onSignalMessage({ type: 'signal', message: msg, ws })
            if (msg.type === 'answer') resolve(msg.sdp)
            else if (msg.type === 'conflict') reject({ conflict: true })
            else if (msg.type === 'error') reject(new Error(msg.error || '信令错误'))
          } catch (err) {
            reject(err)
          }
        }
        ws.send(JSON.stringify({ type: 'offer', sdp: offerToSend, force: forceTakeover }))
        setTimeout(() => reject(new Error('信令超时')), 10000)
      })
      onOfferAnswer({ offer, answer })
      await pc.setRemoteDescription(new RTCSessionDescription(answer))

      ws.onmessage = evt => {
        try {
          const msg = JSON.parse(evt.data)
          onSignalMessage({ type: 'signal', message: msg, ws })
          if (msg.type === 'taken_over') {
            supersededRef.value = true
            toast('连接已被其他页面接管', 'warn')
          }
        } catch (e) {}
      }

      connectedRef.value = true
      connectingRef.value = false
      closedByCleanup = false
      reconnectAttempts.value = 0 // 连接成功即重置退避计数（不依赖调用方回调）
      onConnectSuccess({ pc, ws })
      onConnectFinish({ ok: true })
      return true
    } catch (e) {
      connectingRef.value = false
      if (e && e.conflict) {
        stopPeer({ manual: true, preserveReconnect: true })
        onConnectFinish({ ok: false, conflict: true })
        throw e
      }
      errorMsgRef.value = e.message
      stopPeer({ manual: true, preserveReconnect: true })
      onConnectFinish({ ok: false, error: e })
      return false
    }
  }

  async function connect(manual = false) {
    if (connectLock || connectingRef.value || connectedRef.value) {
      console.warn('[webrtc] connect ignored (lock/connecting/connected)')
      return
    }
    connectLock = true
    forceTakeover = false
    try {
      const ok = await doConnect()
      // 自动重连链路必须续链：服务端停机/重启窗口内 connectDevice、信令 ws
      // 都会失败，doConnect 走失败分支 return false——这里不补排下一次重试，
      // 重连就永久停摆，页面定格成死图只能手动刷新（实测构建停机 4 分钟即复现）
      if (ok === false && !manual) scheduleReconnect({ superseded: supersededRef })
    } catch (e) {
      if (e && e.conflict) {
        if (manual) {
          const confirmed = confirm(`设备 ${deviceIdRef.value} 正在其他页面投屏。\n\n确认接管连接？对方页面将断开且不会自动重连。`)
          if (confirmed) {
            forceTakeover = true
            try {
              await doConnect()
            } finally {
              forceTakeover = false
            }
          } else {
            connectingRef.value = false
            errorMsgRef.value = '设备正在其他页面使用'
          }
        } else {
          connectingRef.value = false
          errorMsgRef.value = '设备已在其他页面连接，本页已停止重连'
          toast(errorMsgRef.value, 'warn')
        }
      }
    } finally {
      connectLock = false
    }
  }

  function getControlChannel() {
    return controlChannel
  }

  function getPeerConnection() {
    return pc
  }

  function hasActivePeer() {
    return !!pc
  }

  return {
    reconnectTimer,
    reconnectAttempts,
    connect,
    cleanup,
    cancelReconnect,
    scheduleReconnect,
    getControlChannel,
    getPeerConnection,
    hasActivePeer,
  }
}
