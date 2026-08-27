// 轻量全局状态（鉴权会话在 ./auth.js：Cookie 会话只存内存态，
// 不再有 localStorage 伪 token；authed 判定以 session.username 为准）
//
// 运行实例模型（OPTIMIZATION_PLAN 阶段3 / RUN-003）：执行实例以 run_id 为主键——
// runRegistry.byId 正查 + activeByDevice 反查 + last 最近终态归档；
// store.runScriptId 不再充当执行实例 ID，仅保留为旧后端兼容期的降级句柄。
// 纯语义工具（标签/时间/新旧响应归一化）见 ./runs.js。
import { reactive, ref } from 'vue'
import { isTerminalRunState } from './runs'

export const store = reactive({
  deviceId: null,           // 当前控制的设备
  running: false,           // 当前设备脚本运行状态（由 runRegistry 终态/活动迁移驱动）
  runScript: null,          // 正在运行的脚本展示名（含 "函数()" 或来源后缀修饰）
  runId: null,              // 当前展示的执行实例（run_id，run_id 主键的主消费位）
  runScriptId: null,        // 兼容降级句柄：仅旧后端无 run_id 路径写入（script id 轮询/停止用）
  runStep: '',              // 当前步骤描述
  runProgress: 0,           // 0-100
})

// 运行实例注册表：
//   byId            run_id → RunRecord（会话内历史，终态后仍保留——既作 last 归档源，
//                   也让迟到的非终态刷新能被「已终态」守卫拒绝）
//   activeByDevice  device_id → run_id 反查（一设备至多一个活动 run）
//   last            最近一条终态记录（success|failed|cancelled）
export const runRegistry = reactive({
  byId: {},
  activeByDevice: {},
  last: null,
})

export function findRun(runId) {
  return (runId && runRegistry.byId[runId]) || null
}

export function getActiveRun(deviceId) {
  const rid = deviceId && runRegistry.activeByDevice[deviceId]
  return rid ? findRun(rid) : null
}

/** 全局运行态复位（回空闲：按钮恢复运行、顶栏芯片消失） */
export function resetStoreRunState() {
  store.running = false
  store.runId = null
  store.runScriptId = null
  store.runScript = null
  store.runStep = ''
  store.runProgress = 0
}

/**
 * 登记/刷新一条运行记录并驱动状态机（轮询与启动回复共用入口）：
 * 活动态 → 建立反查标记；归属当前设备则同步全局 running 态；
 * 终态 → 归档最近一条 + 清理反查标记；属当前展示实例则立即复位全局空闲。
 * 已终态实例拒绝迟到的非终态刷新（防陈旧响应复活记录）。
 */
export function applyRunRecord(rec) {
  if (!rec || !rec.run_id) return null
  // display 展示名（"名字 · 函数()"/来源后缀修饰）不属于服务端契约字段：
  // 仅调用方显式传入时刷新，轮询增量不携带则保留旧值，避免把精心拼好的展示名覆盖回裸 script_id
  const { display, ...data } = rec
  const prev = runRegistry.byId[rec.run_id] || {}
  if (isTerminalRunState(prev.state) && !isTerminalRunState(data.state)) return prev
  const merged = { ...prev, ...data, run_id: rec.run_id }
  if (display) merged.display = display
  else if (!merged.display && data.script_name) merged.display = data.script_name
  runRegistry.byId[merged.run_id] = merged
  if (isTerminalRunState(merged.state)) {
    runRegistry.last = merged                    // 终态归档最近一条
    if (merged.device_id && runRegistry.activeByDevice[merged.device_id] === merged.run_id) {
      delete runRegistry.activeByDevice[merged.device_id]   // 清理设备 active 标记
    }
    if (store.runId === merged.run_id) resetStoreRunState()
  } else {
    if (merged.device_id) runRegistry.activeByDevice[merged.device_id] = merged.run_id
    if (merged.device_id === store.deviceId) {
      store.running = true
      store.runId = merged.run_id
      if (merged.display) store.runScript = merged.display
      if (merged.state === 'stopping') store.runStep = '正在停止…'
    }
  }
  return merged
}

/** 发起取消后的本地先行迁移：state → stopping（终态仍以服务端查询为准） */
export function beginCancel(runId) {
  const rec = findRun(runId)
  if (!rec || isTerminalRunState(rec.state)) return rec || null
  return applyRunRecord({ ...rec, state: 'stopping' })
}

// 设备占用冲突提示队列（409 device_busy）：视图逐条弹窗展示，
// 「仍要查看日志」跳控制台对应设备。元素形如 {device_id, script_id, source, started_at, run_id}
export const runConflicts = ref([])

export function pushRunConflict(info) {
  if (info && typeof info === 'object') runConflicts.value.push(info)
}

export function shiftRunConflict() {
  return runConflicts.value.shift() || null
}

// 简易 toast
export function useToast() {
  const wrap = () => document.querySelector('.toast-wrap')
  return (msg, type = 'info') => {
    let w = wrap()
    if (!w) {
      w = document.createElement('div')
      w.className = 'toast-wrap'
      document.body.appendChild(w)
    }
    const el = document.createElement('div')
    el.className = `toast ${type}`
    el.textContent = msg
    w.appendChild(el)
    setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity .3s'; setTimeout(() => el.remove(), 320) }, 2600)
  }
}

// 统一设备数据（由各页面从 API 拉取后写入）
export const devicesData = ref([])
export const scriptsData = ref([])
export const templatesData = ref([])
export const tasksData = ref([])
export const logsData = ref([])
