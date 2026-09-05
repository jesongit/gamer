import { reactive } from 'vue'

/**
 * v3 运行可视化事件（P12.6 / 契约 §6 / ADR-YAML-03 wire 契约）。
 *
 * 服务端经 control DataChannel 反向推送 `{"type":"se","ev":...}` 运行结构
 * 事件（run_start / run_end / step_start / step_end / call_start / vision /
 * budget），Console 壳的 onControlMessage 收到 se 消息后交给
 * pushRunEvent 分发：维护「运行事件 feed」列表与「当前步骤高亮」状态，
 * 由 ScriptRunner（feed 面板）与 ScriptSummary（步骤卡片高亮）消费。
 *
 * tap / swipe / hit / miss 是投屏标记事件（设备像素坐标），不属于运行结构
 * 事件——pushRunEvent 返回 false，由 Console 既有 overlay 逻辑继续处理。
 *
 * step.path 语法 `steps[0].then[1]`（脚本本地，与编辑器 commands 寻址一致）；
 * 嵌套路径高亮其顶层祖先卡片（runEventTopIndex）。call 被调方帧内的事件
 * path 仍是该脚本的 steps[0]…（call_start 已宣告帧切换），高亮映射不区分
 * 调用帧——已知近似，feed 文本保留 path 原样。
 */

const RUN_EVENT_KINDS = new Set([
  'run_start', 'run_end', 'step_start', 'step_end', 'call_start', 'vision', 'budget',
])

/** feed 容量：滚动保留最近 N 条（运行结束后的最后一段仍可回看）。 */
const MAX_EVENTS = 120

/** 模块级单例：一台设备一页，事件流全局共享（面板与投屏壳分离接线）。 */
const state = reactive({
  list: [],
  /** 当前执行中的 step path（step_start 置位 / step_end ok 复位）。 */
  activePath: '',
  /** 最近一次失败的 step path（下一次 step_start 清除）。 */
  errorPath: '',
  /** 是否有运行在进行（run_start → run_end）。 */
  running: false,
})

function timestamp() {
  const d = new Date()
  return d.toTimeString().slice(0, 8) + '.' + String(d.getMilliseconds()).padStart(3, '0')
}

/**
 * step path → 顶层卡片序号：`steps[2].then[1]` → 2（顶层卡片高亮其祖先）。
 * 无法解析（空 / 非法形态）返回 null。
 */
export function runEventTopIndex(path) {
  const m = /^steps\[(\d+)\]/.exec(String(path || ''))
  return m ? Number(m[1]) : null
}

/** 只读访问运行事件状态（reactive 单例）。 */
export function useRunEvents() {
  return state
}

/**
 * 消费一条已解析的 se 控制消息。返回 true 表示它是运行结构事件（已被
 * feed/高亮消费）；false = 投屏标记类（tap/swipe/hit/miss），调用方继续走
 * overlay 逻辑。未知 ev 一律按 overlay 处理（向前兼容）。
 */
export function pushRunEvent(msg) {
  if (!msg || msg.type !== 'se' || !RUN_EVENT_KINDS.has(msg.ev)) return false
  const entry = { ev: msg.ev, time: timestamp() }
  if (msg.path !== undefined) entry.path = msg.path
  if (msg.desc !== undefined) entry.desc = msg.desc
  if (msg.ok !== undefined) entry.ok = !!msg.ok
  if (msg.error !== undefined) entry.error = msg.error
  if (msg.target !== undefined) entry.target = msg.target
  if (msg.depth !== undefined) entry.depth = msg.depth
  if (msg.template !== undefined) entry.template = msg.template
  if (msg.found !== undefined) entry.found = !!msg.found
  if (msg.score !== undefined) entry.score = msg.score
  if (msg.kind !== undefined) entry.kind = msg.kind
  switch (msg.ev) {
    case 'run_start':
      // 新运行：清空上一轮 feed 与高亮（运行结束/新运行重置）
      state.list = []
      state.activePath = ''
      state.errorPath = ''
      state.running = true
      break
    case 'step_start':
      state.activePath = msg.path || ''
      state.errorPath = ''
      break
    case 'step_end':
      // ok = 恢复；失败 = 标红 errorPath 并退出 active（该步已终止）
      state.activePath = ''
      if (!msg.ok) state.errorPath = msg.path || ''
      break
    case 'run_end':
      state.running = false
      state.activePath = ''
      break
    // call_start / vision / budget：只进 feed，不改高亮
  }
  state.list.push(entry)
  if (state.list.length > MAX_EVENTS) state.list.shift()
  return true
}
