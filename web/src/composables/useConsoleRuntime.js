import { ref } from 'vue'

export function useConsoleRuntime({ api, devicesData, scriptsData, templatesData, toast, connect, deviceIdRef }) {
  const scanning = ref(false)
  const logTimer = ref(null)
  const reconnectTimer = ref(null)
  const reconnectAttempts = ref(0)

  async function loadData() {
    try {
      devicesData.value = await api.listDevices()
    } catch (e) {
      console.warn('load devices:', e.message)
    }
    try {
      scriptsData.value = await api.listScripts()
    } catch (e) {
      console.warn('load scripts:', e.message)
    }
    try {
      templatesData.value = await api.listTemplates()
    } catch (e) {
      console.warn('load templates:', e.message)
    }
  }

  async function refreshDevices() {
    if (scanning.value) return { ok: false, busy: true }
    scanning.value = true
    try {
      const r = await api.scanDevices()
      const list = r.devices && Array.isArray(r.devices) ? r.devices : await api.listDevices()
      devicesData.value = list
      if (!list.some(x => x.id === deviceIdRef.value)) {
        deviceIdRef.value = list[0]?.id || null
      }
      return { ok: true, added: Number(r.added) || 0, list }
    } finally {
      scanning.value = false
    }
  }

  async function refreshLogs() {
    if (!deviceIdRef.value) return []
    return api.listLogs(deviceIdRef.value, null, 50)
  }

  function stopLogPolling() {
    if (logTimer.value) {
      clearInterval(logTimer.value)
      logTimer.value = null
    }
  }

  function startLogPolling(onTick) {
    stopLogPolling()
    if (typeof onTick === 'function') onTick()
    logTimer.value = setInterval(() => {
      if (typeof onTick === 'function') onTick()
    }, 1000)
  }

  function cancelReconnect() {
    if (reconnectTimer.value) {
      clearTimeout(reconnectTimer.value)
      reconnectTimer.value = null
    }
  }

  function scheduleReconnect({ superseded, errorMsgRef }) {
    if (reconnectTimer.value || !deviceIdRef.value) return false
    if (superseded?.value) {
      if (errorMsgRef) errorMsgRef.value = '连接已被其他页面接管'
      return false
    }
    const delay = [3000, 6000, 12000][Math.min(reconnectAttempts.value, 2)]
    reconnectAttempts.value++
    toast(`连接已断开，${delay / 1000} 秒后自动重连…`, 'warn')
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

  function onChannelOpen({ connectedRef, connectingRef, videoConnectTsRef, audioMutedRef, sendControl }) {
    connectedRef.value = true
    connectingRef.value = false
    reconnectAttempts.value = 0
    videoConnectTsRef.value = Date.now()
    sendControl({ type: 'audio', on: !audioMutedRef.value })
    toast('WebRTC 连接建立', 'success')
  }

  function onChannelClose({ connectedRef, manualCloseRef, supersededRef }) {
    connectedRef.value = false
    if (!manualCloseRef.value && !supersededRef.value) {
      scheduleReconnect({ superseded: supersededRef })
    }
    manualCloseRef.value = false
  }

  function cleanup() {
    cancelReconnect()
    stopLogPolling()
    reconnectAttempts.value = 0
  }

  return {
    scanning,
    logTimer,
    reconnectTimer,
    reconnectAttempts,
    loadData,
    refreshDevices,
    refreshLogs,
    startLogPolling,
    stopLogPolling,
    cancelReconnect,
    scheduleReconnect,
    onChannelOpen,
    onChannelClose,
    cleanup,
  }
}
