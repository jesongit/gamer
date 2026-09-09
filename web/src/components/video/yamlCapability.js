import { computed, ref } from 'vue'
import { videoApi } from './videoApi'
import { GAMER_YAML_PLUGIN_ID } from '../../gamer-plugin-ids'
/**
 * gamer.yaml 依赖能力探测（简化计划 Phase 4/5）：视频工作台的模板创建/模板
 * 离线测试/草稿生成与保存依赖 gamer.yaml 的公开动作（template.create_from_frame
 * 等，清单由 gamer.yaml actions.rs 集中声明，经 POST /api/extensions/gamer.yaml/call
 * 分发）。依赖判定走**运行时能力发现**（GET /api/extensions/gamer.yaml/capabilities：
 * 目标 Running + 动作在公开清单内），manifest `[[dependencies]]` 只表达插件
 * 关系、不代替运行时检查。gamer.yaml 缺失/停用时：视频导入、录制、播放、
 * 标记、项目、校准**不受影响**，仅 YAML 相关制作入口禁用并给出依赖提示。
 */

/** 扩展快照列表 → gamer.yaml 是否 Running（快照形态的兜底判定，保留给无
 * capabilities 端点的旧服务端场景）。 */
export function isGamerYamlRunning(extensions) {
  const list = Array.isArray(extensions) ? extensions : []
  const snapshot = list.find(item => item?.id === GAMER_YAML_PLUGIN_ID)
  return snapshot?.state === 'running'
}

/**
 * 视频面板用的依赖状态组合式：`ready` = gamer.yaml Running 且能力清单可达；
 * `hasAction(action)` = 该公开动作当前可调用。轮询间隔 10s（扩展状态变化
 * 低频；插件中心操作后由面板切换/手动刷新收敛）。`fetchCapabilities` 可注入
 * （测试用），缺省走 videoApi.gamerYamlCapabilities。
 */
export function useYamlCapability({ pollMs = 10000, fetchCapabilities } = {}) {
  const ready = ref(false)
  const actions = ref([]) // [{action, version, surface, summary}]
  const checking = ref(false)
  const load = fetchCapabilities || ((...args) => videoApi.gamerYamlCapabilities(...args))
  let timer = null

  const actionNames = computed(() => new Set(actions.value.map(item => item?.action).filter(Boolean)))

  function hasAction(action) {
    return actionNames.value.has(String(action || ''))
  }

  async function refresh() {
    checking.value = true
    try {
      const rep = await load()
      actions.value = Array.isArray(rep?.actions) ? rep.actions : []
      ready.value = rep?.running === true
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

  return { ready, actions, hasAction, checking, refresh, start, stop }
}
