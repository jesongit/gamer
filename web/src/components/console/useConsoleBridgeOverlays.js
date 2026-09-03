import { computed, onUnmounted, ref } from 'vue'

/**
 * Host-owned bridge overlays（sandbox UI 经 UI Bridge 申请的画面叠加框）：
 * entries 是纯数据，由 ConsoleVideoStage 以 pointer-events 关闭的方式渲染。
 * 自 Console.vue 原样拆出，行为零变化。
 */
export function useConsoleBridgeOverlays({ videoElement, deviceRectStyle }) {
  const bridgeOverlays = ref([])
  let bridgeOverlaySerial = 0

  function overlayRect(overlay) {
    const item = overlay && typeof overlay === 'object' ? overlay : {}
    const rect = item.rect && typeof item.rect === 'object' ? item.rect : item
    let x = Number(rect.x || 0)
    let y = Number(rect.y || 0)
    let w = Number(rect.w ?? rect.width ?? 0)
    let h = Number(rect.h ?? rect.height ?? 0)
    const vw = videoElement.value?.videoWidth || 1920
    const vh = videoElement.value?.videoHeight || 1080
    if (item.normalized || rect.normalized) {
      x *= vw; y *= vh; w *= vw; h *= vh
    }
    return { x, y, w, h }
  }

  function showBridgeOverlay(payload, meta = {}) {
    const item = payload && typeof payload === 'object' ? payload : {}
    const owner = `${meta.pluginId || 'anonymous'}:${meta.panelId || ''}`
    const requestedId = String(item.id || `bridge-${++bridgeOverlaySerial}`)
    const existing = bridgeOverlays.value.find(entry => entry.id === requestedId)
    const id = existing && existing.owner !== owner ? `${owner}/${requestedId}` : requestedId
    const next = { ...item, ...overlayRect(item), id, owner, label: String(item.label || '') }
    bridgeOverlays.value = [...bridgeOverlays.value.filter(entry => entry.id !== id), next]
    return id
  }

  function clearBridgeOverlay(payload, meta = {}) {
    const id = typeof payload === 'object' && payload !== null ? payload.id : payload
    const owner = `${meta.pluginId || 'anonymous'}:${meta.panelId || ''}`
    if (id == null || id === '') {
      bridgeOverlays.value = bridgeOverlays.value.filter(entry => entry.owner !== owner)
      return true
    }
    const current = bridgeOverlays.value.find(entry => entry.id === String(id))
    if (current && current.owner !== owner) return false
    bridgeOverlays.value = bridgeOverlays.value.filter(entry => entry.id !== String(id))
    return true
  }

  const bridgeOverlayView = computed(() => bridgeOverlays.value.map(item => ({
    ...item,
    style: bridgeOverlayStyle(item),
  })))

  function bridgeOverlayStyle(item) {
    const style = deviceRectStyle(item.x, item.y, item.w, item.h)
    if (item.kind === 'point') {
      delete style.width
      delete style.height
    }
    return style
  }

  onUnmounted(() => {
    bridgeOverlays.value = []
  })

  return {
    bridgeOverlays,
    bridgeOverlayView,
    showBridgeOverlay,
    clearBridgeOverlay,
  }
}
