// 服务端时区推导（任务页展示用）。
//
// 背景：冻结契约（release/contracts/system-api-v1.md SYS-001）禁止 /api/system/info
// 暴露 timezone 字段，服务端测试断言其不存在——时区信息只能从别处推导。
// /api/tasks 的时间戳形态（server/src/api/tasks.rs / scheduler.rs）：
//   next_run    → 服务端本地墙钟 `%Y-%m-%d %H:%M:%S`（DateTime<Local> 格式化，无偏移）；
//   last_run_at → `Utc::now().to_rfc3339_opts(Millis, true)` 固定 UTC `Z` 串（与本地时区无关）。
// 因此只有当时间戳本身带偏移（含 Z 或 ±HH:MM）时才可推导服务端时区；
// 推导不出时页面显示兜底文案（见 TaskBoard.vue）。

const OFFSET_RE = /(Z|z|[+-]\d{2}:?\d{2})$/
// 数字偏移（非 Z）：last_run_at 固定为 UTC Z 串，不携带本地时区信息，
// 只有显式 ±HH:MM 才可能反映服务端本地偏移
const NUMERIC_OFFSET_RE = /[+-]\d{2}:?\d{2}$/

/** 从 RFC3339 串解析 UTC 偏移（分钟）。无偏移/非法输入返回 null。
 * 例：'2026-09-01T08:00:00+08:00' → 480；'…-05:30' → -330；'…Z' → 0；'-' → null。 */
export function parseOffsetMinutes(value) {
  if (typeof value !== 'string') return null
  const m = value.match(OFFSET_RE)
  if (!m) return null
  const token = m[1]
  if (token === 'Z' || token === 'z') return 0
  const sign = token[0] === '-' ? -1 : 1
  const hh = Number(token.slice(1, 3))
  const mm = Number(token.slice(-2))
  if (!Number.isFinite(hh) || !Number.isFinite(mm) || mm > 59) return null
  return sign * (hh * 60 + mm)
}

/** 偏移分钟 → 'UTC+08:00' 样式标签；不可格式化的输入返回 null */
export function formatUtcOffset(minutes) {
  if (typeof minutes !== 'number' || !Number.isFinite(minutes)) return null
  const sign = minutes < 0 ? '-' : '+'
  const abs = Math.abs(minutes)
  const hh = String(Math.floor(abs / 60)).padStart(2, '0')
  const mm = String(abs % 60).padStart(2, '0')
  return `UTC${sign}${hh}:${mm}`
}

/**
 * 从任务列表推导服务端时区标签（'UTC+08:00'）；推导不出返回 null。
 * 口径：
 * - 优先 next_run（服务端本地墙钟的语义载体，带任意偏移含 Z 均可信）；
 * - last_run_at 仅接受显式数字偏移——它当前固定为 UTC Z 串（scheduler.rs
 *   now_utc_string），Z 不随 TZ 变化，当作本地偏移会在 TZ≠UTC 部署下说谎。
 */
export function serverTzLabelFromTasks(tasks) {
  if (!Array.isArray(tasks)) return null
  for (const t of tasks) {
    const minutes = parseOffsetMinutes(t?.next_run)
    if (minutes !== null) return formatUtcOffset(minutes)
  }
  for (const t of tasks) {
    if (!(typeof t?.last_run_at === 'string' && NUMERIC_OFFSET_RE.test(t.last_run_at))) continue
    const minutes = parseOffsetMinutes(t.last_run_at)
    if (minutes !== null) return formatUtcOffset(minutes)
  }
  return null
}
