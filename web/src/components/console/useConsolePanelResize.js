import { computed, onMounted, onUnmounted, ref } from 'vue'

// 右侧功能区宽度：五个页签共用同一宽度，避免切换页签时布局突然跳变；拖拽条支持手动调整。
const PANEL_STORAGE_KEY = 'gamer.console.panel-ratio'

export function useConsolePanelResize({ consoleEl, videoWrap }) {
  const totalWidth = ref(1200)
  const ratio = ref(0.32)
  try { const saved = Number(localStorage.getItem(PANEL_STORAGE_KEY)); if (saved > 0 && saved <= .5) ratio.value = saved } catch {}
  const PANEL_MAX_WIDTH = computed(() => Math.round(totalWidth.value * .5))
  // 画面可能是竖屏或任意浏览器比例；不以固定 16:9 推导面板最小宽度。
  const PANEL_MIN_WIDTH = computed(() => Math.min(PANEL_MAX_WIDTH.value, 320))
  const panelWidth = computed({ get: () => clampPanelWidth(totalWidth.value * ratio.value), set: width => { ratio.value = clampPanelWidth(width) / totalWidth.value } })
  const panelResizing = ref(false)
  let panelResizeState = null
  function clampPanelWidth(value) { return Math.round(Math.max(PANEL_MIN_WIDTH.value, Math.min(PANEL_MAX_WIDTH.value, value))) }
  function savePanelWidth() { try { localStorage.setItem(PANEL_STORAGE_KEY, String(ratio.value)) } catch {} }

  function startPanelResize(e) {
    if (e.button !== undefined && e.button !== 0) return
    e.currentTarget?.focus?.()
    panelResizing.value = true
    panelResizeState = { startX: e.clientX, startWidth: panelWidth.value, pointerId: e.pointerId }
    e.currentTarget?.setPointerCapture?.(e.pointerId)
    window.addEventListener('pointermove', onPanelResize)
    window.addEventListener('pointerup', stopPanelResize)
    window.addEventListener('pointercancel', stopPanelResize)
    e.preventDefault()
  }

  function onPanelResize(e) {
    if (!panelResizeState) return
    // 分隔条向左移动 = 右侧面板变宽，向右移动 = 右侧面板变窄。
    panelWidth.value = clampPanelWidth(panelResizeState.startWidth - (e.clientX - panelResizeState.startX))
  }

  function stopPanelResize(e) {
    if (!panelResizeState) return
    if (e?.pointerId !== undefined && panelResizeState.pointerId !== undefined && e.pointerId !== panelResizeState.pointerId) return
    panelResizeState = null
    panelResizing.value = false
    savePanelWidth()
    window.removeEventListener('pointermove', onPanelResize)
    window.removeEventListener('pointerup', stopPanelResize)
    window.removeEventListener('pointercancel', stopPanelResize)
  }

  function onPanelResizeKeydown(e) {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return
    // 键盘方向与拖动方向一致：左键增大右侧面板，右键减小。
    panelWidth.value = clampPanelWidth(panelWidth.value + (e.key === 'ArrowLeft' ? 20 : -20))
    savePanelWidth()
    e.preventDefault()
  }

  function clampPanelToViewport() {
    totalWidth.value = Math.max(1, (consoleEl.value?.clientWidth || window.innerWidth) - 6)
  }
  let observer

  onMounted(() => {
    clampPanelToViewport()
    if (typeof ResizeObserver !== 'undefined' && consoleEl.value) {
      observer = new ResizeObserver(clampPanelToViewport)
      observer.observe(consoleEl.value)
      if (videoWrap?.value) observer.observe(videoWrap.value)
    }
    window.addEventListener('resize', clampPanelToViewport)
    document.addEventListener('fullscreenchange', clampPanelToViewport)
  })

  onUnmounted(() => {
    stopPanelResize()
    observer?.disconnect()
    window.removeEventListener('resize', clampPanelToViewport)
    document.removeEventListener('fullscreenchange', clampPanelToViewport)
  })

  return {
    panelWidth,
    panelResizing,
    PANEL_MIN_WIDTH,
    PANEL_MAX_WIDTH,
    startPanelResize,
    onPanelResize,
    stopPanelResize,
    onPanelResizeKeydown,
  }
}
