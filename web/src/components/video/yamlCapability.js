import { ref } from 'vue'
import { api } from '../../api'
import { GAMER_YAML_PLUGIN_ID } from '../../gamer-plugin-ids'

/**
 * gamer.yaml 依赖能力探测（Phase 7 §10.1）：视频工作台的模板创建/模板离线
 * 测试/草稿生成与保存依赖 gamer.yaml 扩展处于 Running 状态（动作清单由
 * gamer.yaml 集中声明，经 POST /api/extensions/gamer.yaml/call 分发；服务端
 * 对非 Running 一律拒绝）。gamer.yaml 缺失/未运行时：视频导入、录制、播放、
 * 标记、项目、校准**不受影响**，仅制作入口禁用并给出依赖提示（计划 §10.1）。
 */

/** 扩展快照列表 → gamer.yaml 是否 Running（镜像 gamer-keymap-extension 的判定模式）。 */
export function isGamerYamlRunning(extensions) {
  const list = Array.isArray(extensions) ? extensions : []
  const snapshot = list.find(item => item?.id === GAMER_YAML_PLUGIN_ID)
  return snapshot?.state === 'running'
}

/**
 * 视频面板用的依赖状态组合式：`ready` = gamer.yaml Running。轮询间隔 10s
 * （扩展状态变化低频；插件中心操作后由面板切换/手动刷新收敛）。
 */
export function useYamlCapability({ pollMs = 10000 } = {}) {
  const ready = ref(false)
  const checking = ref(false)
  let timer = null

  async function refresh() {
    checking.value = true
    try {
      const response = await api.listExtensions()
      ready.value = isGamerYamlRunning(response?.extensions)
    } catch {
      // 探测失败保持上一次状态（瞬时网络抖动不应把可用入口闪断）
    } finally {
      checking.value = false
    }
  }

  function start() {
    if (timer) return
    void refresh()
    timer = setInterval(() => { void refresh() }, Math.max(2000, pollMs))
  }

  function stop() {
    if (timer) { clearInterval(timer); timer = null }
  }

  return { ready, checking, refresh, start, stop }
}
