import { reactive, onUnmounted, watch } from 'vue'
import { api } from '../../api'

function loadFrameImage(src, signal) {
  return new Promise((resolve, reject) => {
    const image = new Image()
    const aborted = () => { image.src = ''; finish(null, new Error('画面捕获已取消')) }
    const finish = (value, error) => {
      signal?.removeEventListener('abort', aborted)
      image.onload = image.onerror = null
      if (error) reject(error)
      else resolve(value)
    }
    image.onload = () => finish(image)
    image.onerror = () => finish(null, new Error('浏览器画面解码失败'))
    signal?.addEventListener('abort', aborted, { once: true })
    if (signal?.aborted) aborted()
    else image.src = src
  })
}

function jpegBlob(value) {
  const bytes = atob(value)
  const buffer = new Uint8Array(bytes.length)
  for (let i = 0; i < bytes.length; i++) buffer[i] = bytes.charCodeAt(i)
  return new Blob([buffer], { type: 'image/jpeg' })
}

export function useBrowserPreview({ deviceId, connected, connecting, errorMsg, toast, onEvent, onConnected, onDisconnected,
  previewMode = () => 'single', loadImage = loadFrameImage }) {
  const view = reactive({ src: '', stamp: null, pending: null, width: 0, height: 0, previewWidth: 0, previewHeight: 0, fps: 0 })
  let socket = null
  let generation = 0
  let lastMove = 0
  let notifiedConnected = false
  let lastInputError = ''
  let pendingFrame = null
  let fpsTimer = null
  let displayedFrames = 0
  let fpsSince = 0
  let captureOperation = null
  const captures = new Set()
  const mode = () => ['grid4', 'grid9'].includes(previewMode()) ? previewMode() : 'single'
  function sameStamp(a, b) { return a && b && a.target === b.target && a.epoch === b.epoch && a.revision === b.revision }
  function cancelCaptures() { for (const controller of captures) controller.abort(); captures.clear(); captureOperation = null }
  function clearImage() {
    if (view.src) URL.revokeObjectURL(view.src)
    view.src = ''
    view.stamp = view.pending = null
    pendingFrame = null
    view.width = view.height = view.previewWidth = view.previewHeight = 0
  }
  function disconnected() {
    if (!notifiedConnected) return
    notifiedConnected = false
    onDisconnected?.()
  }
  function close() {
    generation++
    connected.value = false
    connecting.value = false
    const previous = socket
    socket = null
    previous?.close()
    clearImage()
    cancelCaptures()
    clearInterval(fpsTimer)
    fpsTimer = null
    view.fps = 0
    displayedFrames = 0
    lastInputError = ''
    disconnected()
  }
  async function connect() {
    close()
    const seq = generation
    const id = deviceId()
    connecting.value = true
    errorMsg.value = ''
    try {
      await api.connectBrowser(id)
      if (seq !== generation || id !== deviceId()) return
      const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/ws/browser/${encodeURIComponent(id)}?mode=${mode()}`)
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
        let data
        try { data = JSON.parse(event.data) } catch { return }
        if (data.type === 'frame') {
          // Coordinates use the full viewport, irrespective of thumbnail size.
          // Only the latest Blob URL survives; no JPEG base64 is kept in state.
          if (!sameStamp(view.stamp, data.stamp)) { view.stamp = null; cancelCaptures() }
          const previous = view.src
          view.pending = data.stamp
          pendingFrame = { id: data.id, width: data.width, height: data.height,
            previewWidth: data.preview_width, previewHeight: data.preview_height }
          view.src = URL.createObjectURL(jpegBlob(data.jpeg))
          if (previous) URL.revokeObjectURL(previous)
        } else if (data.type === 'invalidated') {
          view.stamp = view.pending = null
          pendingFrame = null
          cancelCaptures()
        } else if (data.type === 'se') { onEvent?.(data)
        } else if (data.error) {
          view.stamp = view.pending = null
          // An input rejection does not cancel an image already in flight.
          // It still needs an ACK after loading, or the producer remains
          // blocked. Keep its identity, but do not restore its input stamp;
          // a newly received valid frame will enable input again.
          if (data.source !== 'input') pendingFrame = null
          cancelCaptures()
          errorMsg.value = data.error
          if (data.source === 'input' && data.error !== lastInputError) {
            lastInputError = data.error
            toast(`操作未执行：${data.error}`, 'error')
          }
        }
      }
      ws.onclose = () => {
        if (seq !== generation || socket !== ws) return
        close()
        errorMsg.value ||= '浏览器预览已断开；请确认没有其他投屏页面占用，再重新连接'
      }
      ws.onerror = () => { if (seq === generation) errorMsg.value = '无法打开浏览器预览' }
    } catch (e) {
      if (seq !== generation) return
      connecting.value = false
      errorMsg.value = e.message
      toast(e.message, 'error')
    }
  }
  function loaded(event) {
    if (socket?.readyState !== WebSocket.OPEN || event.target.src !== view.src || !pendingFrame) return
    view.width = pendingFrame.width || 0
    view.height = pendingFrame.height || 0
    view.previewWidth = pendingFrame.previewWidth || event.target.naturalWidth || 0
    view.previewHeight = pendingFrame.previewHeight || event.target.naturalHeight || 0
    view.stamp = view.pending
    socket.send(JSON.stringify({ type: 'frame_ack', id: pendingFrame.id }))
    pendingFrame = null
    view.pending = null
    displayedFrames++
    connected.value = true
    connecting.value = false
    if (view.stamp) errorMsg.value = ''
    if (!notifiedConnected) { notifiedConnected = true; onConnected?.() }
  }
  function failed(event) {
    if (socket?.readyState !== WebSocket.OPEN || event.target.src !== view.src || !pendingFrame) return
    // The image was consumed but could not be displayed. Release the network
    // slot without granting input against pixels that were never shown.
    socket.send(JSON.stringify({ type: 'frame_ack', id: pendingFrame.id, displayed: false }))
    pendingFrame = null
    view.stamp = view.pending = null
    cancelCaptures()
    errorMsg.value = '浏览器画面解码失败，正在等待新画面'
  }
  // Template tools always use an original PNG. A reduced JPEG is neither a
  // template source nor evidence of the pixels used by automation.
  async function captureOriginalFrame() {
    const stamp = view.stamp
    const seq = generation, id = deviceId()
    if (!view.width || !view.height || !stamp || socket?.readyState !== WebSocket.OPEN) return null
    const controller = new AbortController()
    captures.add(controller)
    let src = ''
    const current = () => !controller.signal.aborted && seq === generation && id === deviceId()
      && sameStamp(stamp, view.stamp) && socket?.readyState === WebSocket.OPEN
    try {
      const frame = await api.screenshotFrame(id, { signal: controller.signal })
      if (!current() || !sameStamp(stamp, frame.stamp)) return null
      src = URL.createObjectURL(frame.blob)
      const source = await loadImage(src, controller.signal)
      if (!current() || source?.naturalWidth !== view.width || source?.naturalHeight !== view.height) return null
      return { source, width: source.naturalWidth, height: source.naturalHeight, stamp: frame.stamp, label: '浏览器原始 PNG 当前帧' }
    } catch (e) {
      if (controller.signal.aborted) return null
      throw e
    } finally {
      if (src) URL.revokeObjectURL(src)
      captures.delete(controller)
    }
  }
  function captureFrame() {
    // A mouse hover may ask for many loupes while one original PNG is loading.
    // Share that request rather than piling up screenshots and image decoders.
    if (captureOperation) return captureOperation
    const operation = captureOriginalFrame()
    captureOperation = operation
    const finished = () => { if (captureOperation === operation) captureOperation = null }
    operation.then(finished, finished)
    return operation
  }
  function send(value) {
    if (mode() !== 'single') return false
    if (socket?.readyState !== WebSocket.OPEN) return false
    if (!view.stamp) { if (value.action === 'up') release(); return false }
    if ((value.type === 'pointer' && value.action === 'move') || (value.type === 'input_event' && value.event?.type === 'mouse_move')) {
      const now = performance.now()
      if (now - lastMove < 50 || socket.bufferedAmount > 32 * 1024) return false
      lastMove = now
    }
    socket.send(JSON.stringify({ ...value, stamp: view.stamp }))
    return true
  }
  function release() { if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: 'release' })) }
  watch(previewMode, () => { if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: 'preview', mode: mode() })) }, { flush: 'sync' })
  watch(deviceId, close, { flush: 'sync' })
  onUnmounted(close)
  return { view, connect, close, send, loaded, failed, release, captureFrame }
}
