import { reactive, onUnmounted, watch } from 'vue'
import { api } from '../../api'

export function useBrowserPreview({ deviceId, connected, connecting, errorMsg, toast, onEvent }) {
  const view = reactive({ src: '', stamp: null, pending: null })
  let socket = null
  let generation = 0
  let lastMove = 0
  let pendingConnect = false
  function close() {
    generation++
    if (socket || view.src || pendingConnect) { connected.value = false; connecting.value = false }
    pendingConnect = false
    socket?.close()
    socket = null
    view.src = ''
    view.stamp = null
    view.pending = null
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
      ws.onmessage = event => {
        if (seq !== generation || id !== deviceId()) return
        const data = JSON.parse(event.data)
        if (data.type === 'frame') {
          view.stamp = null
          view.pending = data.stamp
          const src = `data:image/png;base64,${data.png}`
          if (view.src === src) view.stamp = data.stamp
          else view.src = src
        } else if (data.type === 'se') { onEvent?.(data)
        } else if (data.error) {
          view.stamp = null
          errorMsg.value = data.error
        }
      }
      ws.onclose = () => {
        if (seq !== generation) return
        connected.value = false
        connecting.value = false
        view.stamp = null
        view.pending = null
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
    if (socket?.readyState !== WebSocket.OPEN || !view.pending || event.target.src !== view.src) return
    view.stamp = view.pending
    connected.value = true
    connecting.value = false
    errorMsg.value = ''
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
  return { view, connect, close, send, loaded, release }
}
