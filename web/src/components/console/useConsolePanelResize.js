import { onMounted, onUnmounted, ref } from 'vue'

// 右侧功能区宽度：五个页签共用同一宽度，避免切换页签时布局突然跳变；拖拽条支持手动调整。
const PANEL_STORAGE_KEY = 'gb_console_panel_width'
const PANEL_DEFAULT_WIDTH = 340
const PANEL_MIN_WIDTH = 280
const PANEL_MAX_WIDTH = 560
const MIN_STAGE_WIDTH = 360

/** 左右分区拖拽：Console 右侧面板宽度状态与拖拽/键盘调整（自 Console.vue 原样拆出）。 */
export function useConsolePanelResize({ consoleEl }) {
  const panelWidth = ref(readPanelWidth())
  const panelResizing = ref(false)
  let panelResizeState = null

  function readPanelWidth() {
    try {
      const saved = Number(localStorage.getItem(PANEL_STORAGE_KEY))
      return Number.isFinite(saved) && saved > 0
        ? Math.min(PANEL_MAX_WIDTH, Math.max(PANEL_MIN_WIDTH, Math.round(saved)))
        : PANEL_DEFAULT_WIDTH
    } catch {
      return PANEL_DEFAULT_WIDTH
    }
  }

  function panelMaxWidth() {
    const total = consoleEl.value?.clientWidth || window.innerWidth || 0
    if (!total) return PANEL_MAX_WIDTH
    // 保留最小画面区，并扣除 Console 的左右内边距、间距和拖拽条占位。
    return Math.max(PANEL_MIN_WIDTH, Math.min(PANEL_MAX_WIDTH, total - MIN_STAGE_WIDTH - 50))
  }

  function clampPanelWidth(value) {
    return Math.round(Math.max(PANEL_MIN_WIDTH, Math.min(panelMaxWidth(), Number(value) || PANEL_DEFAULT_WIDTH)))
  }

  function savePanelWidth() {
    try { localStorage.setItem(PANEL_STORAGE_KEY, String(panelWidth.value)) } catch { /* 忽略不可用的存储 */ }
  }

  function startPanelResize(e) {
    if (e.button !== undefined && e.button !== 0) return
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
    const next = clampPanelWidth(panelWidth.value)
    if (next === panelWidth.value) return
    panelWidth.value = next
    savePanelWidth()
  }

  onMounted(() => {
    clampPanelToViewport()
    window.addEventListener('resize', clampPanelToViewport)
  })

  onUnmounted(() => {
    stopPanelResize()
    window.removeEventListener('resize', clampPanelToViewport)
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
