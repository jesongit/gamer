import { ref } from 'vue'

/**
 * Console 壳运行时编排：设备数据/扫描、日志轮询、重连计时器归属。
 * 壳只认识设备与日志（ADR-11 Core 知识边界）——脚本/模板等业务资源不在此
 * 预拉，由各自面板实现（useConsoleScriptRunner/useConsoleTemplates 等扩展
 * 面板侧 composable）自行加载进共享 store。
 *
 * WebRTC 连接/重连语义统一在 useWebRtcLifecycle（含 taken_over 处理），
 * 本模块只持有重连计时器/退避计数的复位与清理，不重复实现连接状态机。
 */
export function useConsoleRuntime({ api, devicesData, deviceIdRef }) {
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
    cleanup,
  }
}
