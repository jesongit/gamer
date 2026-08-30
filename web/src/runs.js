// 统一运行实例（RUN-003）前端语义工具：执行实例以 run_id 为主键，script_id 只标识脚本。
// 纯函数模块（不依赖 Vue 响应性），供 store 与各视图复用，vitest 直测。
// 当前后端契约：
//   启动 POST run → 202 {run_id, state:"starting"}
//   单次查询 GET /api/runs/:run_id → RunRecord
//   设备当前 GET /api/devices/:id/run → {active:true,run:RunRecord} | {active:false}
//   取消 POST /api/runs/:run_id/cancel → 202 {cancelling:true}，终态以查询为准

// 非终态（活动）与终态集合：终态后记录归档并清理 active 标记，不再接受迟到刷新
export const ACTIVE_RUN_STATES = ['starting', 'running', 'stopping']
export const TERMINAL_RUN_STATES = ['success', 'failed', 'cancelled']

export function isActiveRunState(s) { return ACTIVE_RUN_STATES.includes(s) }
export function isTerminalRunState(s) { return TERMINAL_RUN_STATES.includes(s) }

const TERMINAL_LABELS = { success: '成功', failed: '失败', cancelled: '已取消' }
/** 终态中文短标签（脚本已结束：成功/失败/已取消）；未知原样透出 */
export function terminalLabel(state) { return TERMINAL_LABELS[state] || state || '' }

// 来源中文标签：manual 手动 / scheduled 定时 / task_now 手动任务
const SOURCE_LABELS = { manual: '手动', scheduled: '定时', task_now: '手动任务' }
export function sourceLabel(source) {
  if (!source) return ''
  return SOURCE_LABELS[source] || String(source)
}

/** run_id 缩短展示（toast「已触发（run xxxxxxxx）」）：UUID 取首段最多 8 位，空值返回空串 */
export function shortRunId(id) {
  const s = String(id || '')
  if (!s) return ''
  return (s.split('-')[0] || s).slice(0, 8)
}

/** ISO8601（UTC）→ 本地时区 "YYYY-MM-DD HH:mm:ss"；非法输入原样返回。 */
export function formatLocalTime(iso) {
  if (!iso) return ''
  const d = iso instanceof Date ? iso : new Date(iso)
  if (Number.isNaN(d.getTime())) return String(iso)
  const p = n => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

/**
 * 409 冲突体 → 用户可读文案。data: {error:'device_busy', run_id, script_id, source, started_at}
 * 缺字段宽容（各槽位回退"未知"），设备上下文由调用方补充。
 */
export function describeConflict(data) {
  const d = data && typeof data === 'object' ? data : {}
  const script = d.script_id || '未知脚本'
  const src = sourceLabel(d.source) || '未知来源'
  const t = d.started_at ? formatLocalTime(d.started_at) : '未知时间'
  return `设备正被占用：${script} 正在运行（来源：${src} · 开始于 ${t}）`
}

/** 是否设备占用冲突（契约：HTTP 409 + body.error === 'device_busy'） */
export function isDeviceBusyConflict(e) {
  return !!e && e.status === 409 && e.data?.error === 'device_busy'
}
