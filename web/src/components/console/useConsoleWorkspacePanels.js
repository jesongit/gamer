import { onUnmounted, watch } from 'vue'
import { DEFAULT_PANEL_KEY } from '../../workspace/registry'
import { isRemoteKeymapRunning } from '../../gamer-keymap-extension'

/**
 * Console 右侧 Workspace 接线：URL panel 同步（hash 路由 query）、
 * 服务端扩展 UI 贡献轮询、远端 keymap 运行态与手柄输入轮询。
 * 面板注册完全由服务端 ui_contributions 驱动（runtime=core 面板挂宿主组件），
 * 本模块不再做任何本地回退注册；keymap 扩展 id 知识收敛在
 * gamer-keymap-extension.js（唯一前端配置点），此处只消费其运行态判定。
 */
export function useConsoleWorkspacePanels({
  route,
  router,
  panelRegistry,
  serverUiAdapter,
  remoteKeymapRunning,
  keymap,
  connected,
  activePanelKey,
}) {
  let extensionUiPollTimer = null
  let gamepadPollTimer = null
  const gamepadSnapshot = new Map()

  async function refreshServerExtensions() {
    try {
      const response = await serverUiAdapter.refresh()
      remoteKeymapRunning.value = isRemoteKeymapRunning(response?.extensions)
    } catch (error) {
      // Extension discovery is additive; a transient failure must not tear down
      // the already mounted core panels or the currently active input route.
    }
  }

  function pollRemoteGamepads() {
    if (!remoteKeymapRunning.value || !connected.value || !navigator.getGamepads) {
      gamepadSnapshot.clear()
      return
    }
    for (const gamepad of navigator.getGamepads()) {
      if (!gamepad) continue
      gamepad.buttons.forEach((button, buttonIndex) => {
        const key = `${gamepad.index}:button:${buttonIndex}`
        const next = { pressed: !!button.pressed, value: Number(button.value) || 0 }
        const previous = gamepadSnapshot.get(key)
        if (previous && (previous.pressed !== next.pressed || Math.abs(previous.value - next.value) > 0.02)) {
          keymap.handleInputEvent({ kind: 'gamepad_button', index: buttonIndex, ...next })
        }
        gamepadSnapshot.set(key, next)
      })
      gamepad.axes.forEach((value, axisIndex) => {
        const key = `${gamepad.index}:axis:${axisIndex}`
        const next = Number(value) || 0
        const previous = gamepadSnapshot.get(key)
        if (previous !== undefined && Math.abs(previous - next) > 0.02) {
          keymap.handleInputEvent({ kind: 'gamepad_axis', index: axisIndex, value: next })
        }
        gamepadSnapshot.set(key, next)
      })
    }
  }

  /** 由 Console 的 onMounted 在首刷扩展后调用（保持原有轮询启动时序）。 */
  function startExtensionPolling() {
    if (extensionUiPollTimer) return
    extensionUiPollTimer = window.setInterval(refreshServerExtensions, 2000)
    gamepadPollTimer = window.setInterval(pollRemoteGamepads, 16)
  }

  onUnmounted(() => {
    if (extensionUiPollTimer) { clearInterval(extensionUiPollTimer); extensionUiPollTimer = null }
    if (gamepadPollTimer) { clearInterval(gamepadPollTimer); gamepadPollTimer = null }
    gamepadSnapshot.clear()
    serverUiAdapter.dispose()
  })

  function routePanelValue(value = route.query.panel) {
    return Array.isArray(value) ? value[0] : value
  }

  function syncPanelFromRoute({ replaceInvalid = true } = {}) {
    const requested = String(routePanelValue() || '')
    const selected = panelRegistry.resolve(requested) || panelRegistry.defaultPanel()
    const key = selected?.key || DEFAULT_PANEL_KEY
    activePanelKey.value = key
    if (replaceInvalid && requested !== key) {
      router.replace({ path: route.path, query: { ...route.query, panel: key } })
    }
  }

  function openPanel(panel, { replace = false } = {}) {
    const selected = panelRegistry.resolve(panel) || panelRegistry.defaultPanel()
    if (!selected) return null
    if (activePanelKey.value === selected.key && String(routePanelValue() || '') === selected.key) return selected.key
    activePanelKey.value = selected.key
    const query = { ...route.query, panel: selected.key }
    return router[replace ? 'replace' : 'push']({ path: route.path, query }).then(() => selected.key)
  }

  function fallbackPanel(key) {
    if (panelRegistry.resolve(key)) openPanel(key, { replace: true })
    else syncPanelFromRoute()
  }

  // 注册完核心贡献后立即同步一次 URL panel（保持原有 immediate 时序与位置）
  watch(() => route.query.panel, () => syncPanelFromRoute(), { immediate: true })

  return {
    refreshServerExtensions,
    startExtensionPolling,
    syncPanelFromRoute,
    openPanel,
    fallbackPanel,
  }
}
