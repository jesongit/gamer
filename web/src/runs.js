// 统一运行实例（RUN-003）前端语义工具：执行实例以 run_id 为主键，script_id 只标识脚本。
// 纯函数模块（不依赖 Vue 响应性），供 store 注册表与各视图复用，vitest 直测。
// 后端契约：
//   启动 POST run → 202 {run_id, state:"starting"}；冲突 409 {error:"device_busy", run_id, script_id, source, started_at}
//   单次查询 GET /api/runs/:run_id → RunRecord（state ∈ starting|running|stopping|success|failed|cancelled）
//   设备当前 GET /api/devices/:id/run → {active:true,...RunRecord} | {active:false}
//   取消 POST /api/runs/:run_id/cancel → 202 {cancelling:true}，终态以查询为准
// 兼容期旧形状：启动 {ok:true}、状态 {running:bool}、设备查询 {running, script_id?, script_name?}

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

/**
 * GET /api/devices/:id/run 响应归一化：
 * 新契约 {active:true,...RunRecord} | {active:false}；旧后端 {running:true,script_id,script_name} | {running:false}
 * → RunRecord 兼容对象 | null（无活动 / 无法识别 / 网络错残留）
 */
export function normalizeActiveRunResponse(rep) {
  if (!rep || typeof rep !== 'object') return null
  if (rep.active === true && rep.run_id) {
    return {
      run_id: rep.run_id,
      device_id: rep.device_id ?? null,
      script_id: rep.script_id ?? '',
      state: isActiveRunState(rep.state) ? rep.state : 'running',
      source: rep.source ?? null,
      task_id: rep.task_id ?? null,
      scheduled_at: rep.scheduled_at ?? null,
      started_at: rep.started_at ?? null,
      finished_at: rep.finished_at ?? null,
      error: rep.error ?? null,
    }
  }
  if (rep.running === true && rep.script_id) {
    // 旧后端兼容形状：无 run_id/state/来源，走降级句柄（script id 轮询）
    return {
      run_id: null,
      device_id: null,
      script_id: rep.script_id,
      script_name: rep.script_name ?? null,
      state: 'running',
      source: null,
      legacy: true,
    }
  }
  return null
}

/**
 * 启动响应归一化（脚本 run 与任务立即执行共用）：
 * 新契约 202 {run_id, state:"starting"}（任务侧仅 {run_id}）→ {run_id, state}；
 * 旧后端 200 {ok:true} 无 run_id → null（调用方走旧轮询/停止通道）。
 */
export function normalizeStartReply(rep) {
  if (rep && typeof rep === 'object' && rep.run_id) {
    return { run_id: rep.run_id, state: isActiveRunState(rep.state) ? rep.state : 'starting' }
  }
  return null
}

/**
 * 新增端点在旧后端上不可用的判定：404 或无 status 的网络错误
 * （fetch TypeError 抛出的错误没有 .status）。满足即静默降级为旧文案/旧通道。
 */
export function isMissingEndpointError(e) {
  return !e || e.status === undefined || e.status === null || e.status === 404
}

/** 是否设备占用冲突（契约：HTTP 409 + body.error === 'device_busy'） */
export function isDeviceBusyConflict(e) {
  return !!e && e.status === 409 && e.data?.error === 'device_busy'
}
