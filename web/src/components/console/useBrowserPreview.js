import { reactive, onUnmounted, watch } from 'vue'
import { api } from '../../api'

function loadFrameImage(src) {
  return new Promise((resolve, reject) => {
    const image = new Image()
    image.onload = () => resolve(image)
    image.onerror = () => reject(new Error('浏览器画面解码失败'))
    image.src = src
  })
}

export function useBrowserPreview({ deviceId, connected, connecting, errorMsg, toast, onEvent, onConnected, onDisconnected, loadImage = loadFrameImage }) {
  const view = reactive({ src: '', stamp: null, pending: null, width: 0, height: 0, fps: 0 })
  let socket = null
  let generation = 0
  let lastMove = 0
  let pendingConnect = false
  let displayedSrc = ''
  let notifiedConnected = false
  let lastInputError = ''
  let pendingId = null
  let fpsTimer = null
  let displayedFrames = 0
  let fpsSince = 0
  let displayedFrame = null
  function disconnected() {
    if (!notifiedConnected) return
    notifiedConnected = false
    onDisconnected?.()
  }
  function close() {
    generation++
    if (socket || view.src || pendingConnect) { connected.value = false; connecting.value = false }
    pendingConnect = false
    socket?.close()
    socket = null
    view.src = ''
    view.stamp = null
    view.pending = null
    view.width = 0
    view.height = 0
    view.fps = 0
    pendingId = null
    clearInterval(fpsTimer)
    fpsTimer = null
    displayedFrames = 0
    displayedSrc = ''
    displayedFrame = null
    lastInputError = ''
    disconnected()
  }
  async function connect() {
    close()
    const seq = generation
    const id = deviceId()
    pendingConnect = true
    connecting.value = true
    errorMsg.value = ''
    try {
      await api.connectBrowser(id)
      if (seq !== generation || id !== deviceId()) return
      pendingConnect = false
      const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/ws/browser/${encodeURIComponent(id)}`)
      socket = ws
      fpsSince = performance.now()
      fpsTimer = setInterval(() => {
        const now = performance.now()
        view.fps = Math.round(displayedFrames * 1000 / Math.max(1, now - fpsSince))
        displayedFrames = 0
        fpsSince = now
      }, 1000)
      ws.onmessage = event => {
        if (seq !== generation || id !== deviceId() || ws.readyState !== WebSocket.OPEN) return
        const data = JSON.parse(event.data)
        if (data.type === 'frame') {
          // Same target/mapping: keep controls usable while the next image
          // decodes. Navigation/resize still invalidates before any new input.
          if (!sameStamp(view.stamp, data.stamp)) view.stamp = null
          view.pending = data.stamp
          pendingId = data.id
          const src = `data:image/jpeg;base64,${data.jpeg}`
          const alreadyDisplayed = view.src === src && displayedSrc === src
          view.src = src
          if (alreadyDisplayed) acceptFrame()
          else displayedSrc = ''
        } else if (data.type === 'invalidated') {
          view.stamp = null
          view.pending = null
          pendingId = null
        } else if (data.type === 'se') { onEvent?.(data)
        } else if (data.error) {
          view.stamp = null
          view.pending = null
          errorMsg.value = data.error
          if (data.source === 'input' && data.error !== lastInputError) {
            lastInputError = data.error
            toast(`操作未执行：${data.error}`, 'error')
          }
        }
      }
      ws.onclose = () => {
        if (seq !== generation) return
        connected.value = false
        connecting.value = false
        view.stamp = null
        view.pending = null
        clearInterval(fpsTimer)
        view.fps = 0
        disconnected()
        errorMsg.value ||= '浏览器预览已断开；请确认没有其他投屏页面占用，再重新连接'
      }
      ws.onerror = () => { if (seq === generation) errorMsg.value = '无法打开浏览器预览' }
    } catch (e) {
      if (seq !== generation) return
      pendingConnect = false
      connecting.value = false
      errorMsg.value = e.message
      toast(e.message, 'error')
    }
  }
  function loaded(event) {
    if (socket?.readyState !== WebSocket.OPEN || event.target.src !== view.src) return
    displayedSrc = view.src
    view.width = event.target.naturalWidth || 0
    view.height = event.target.naturalHeight || 0
    if (view.pending) acceptFrame()
    else acknowledgeFrame()
  }
  function sameStamp(a, b) { return a && b && a.target === b.target && a.epoch === b.epoch && a.revision === b.revision }
  function acknowledgeFrame() {
    if (pendingId == null || socket?.readyState !== WebSocket.OPEN) return
    socket.send(JSON.stringify({ type: 'frame_ack', id: pendingId }))
    pendingId = null
    displayedFrames++
  }
  function acceptFrame() {
    view.stamp = view.pending
    displayedFrame = { src: view.src, stamp: view.stamp, width: view.width, height: view.height }
    acknowledgeFrame()
    connected.value = true
    connecting.value = false
    errorMsg.value = ''
    if (!notifiedConnected) {
      notifiedConnected = true
      onConnected?.()
    }
  }
  // Decode a fixed copy of the last displayed frame only when requested. The
  // live img may already be loading its next JPEG and have zero naturalWidth.
  async function captureFrame() {
    const frame = displayedFrame
    const seq = generation
    if (!frame?.width || !frame.height || !sameStamp(frame.stamp, view.stamp)) return null
    const source = await loadImage(frame.src)
    if (seq !== generation || !sameStamp(frame.stamp, view.stamp) || socket?.readyState !== WebSocket.OPEN) return null
    return { source, width: frame.width, height: frame.height, label: '浏览器画面当前帧' }
  }
  function send(value) {
    if (socket?.readyState !== WebSocket.OPEN) return false
    if (!view.stamp) { if (value.action === 'up') release(); return false }
    if (value.type === 'pointer' && value.action === 'move') {
      const now = performance.now()
      if (now - lastMove < 50 || socket.bufferedAmount > 32 * 1024) return false
      lastMove = now
    }
    socket.send(JSON.stringify({ ...value, stamp: view.stamp }))
    return true
  }
  function release() { if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: 'release' })) }
  watch(deviceId, close, { flush: 'sync' })
  onUnmounted(close)
  return { view, connect, close, send, loaded, release, captureFrame }
}
