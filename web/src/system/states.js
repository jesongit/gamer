// system/update 前端展示层共享元数据（契约：release/contracts/system-api-v1.md，冻结）。
// 内容：§5.1 的 11 个展示状态、§4.2 状态×动作受理矩阵、§4.3 install 门禁 blocking 枚举、
// §5.2 journal detail 映射、§2.1 依赖/部署/启动阶段枚举的中文标签与展示工具。
// 字段名与枚举值以契约 + fixtures 为准；本文件只维护文案/视觉，不改契约语义。

/** §5.1 展示状态枚举（11 个，冻结；前端业务分支只允许依赖 update.state） */
export const UPDATE_STATES = Object.freeze([
  'idle', 'checking', 'available', 'downloading', 'staged', 'waiting',
  'installing', 'restarting', 'failed', 'rolling_back', 'manual_recovery',
])

export function isUpdateState(state) {
  return UPDATE_STATES.includes(state)
}

/** §5.1 状态元数据：label（状态名）/ desc（一句话描述）/ tone（.tag 颜色类） */
export const STATE_META = Object.freeze({
  idle:            { label: '空闲',          desc: '无进行中的更新事务', tone: '' },
  checking:        { label: '正在检查更新',   desc: '正在检查远端是否有新版本', tone: 'run' },
  available:       { label: '有可用更新',     desc: '已发现新版本候选，尚未下载', tone: 'info' },
  downloading:     { label: '正在下载',       desc: '正在后台下载新版本并校验', tone: 'run' },
  staged:          { label: '已就绪待安装',   desc: '新版本已下载并校验完成，等待安装', tone: 'ok' },
  waiting:         { label: '等待维护窗口',   desc: '将在维护窗口与空闲门禁满足后自动安装', tone: 'warn' },
  installing:      { label: '正在安装',       desc: '停机/快照/迁移进行中，服务即将重启', tone: 'run' },
  restarting:      { label: '正在重启',       desc: '新版本进程启动与激活中，服务可能短暂不可达', tone: 'run' },
  failed:          { label: '更新失败',       desc: '事务在提交前失败，旧版本仍在服务，可重试或回滚', tone: 'err' },
  rolling_back:    { label: '正在回滚',       desc: '正在恢复旧版本与升级前数据快照', tone: 'run' },
  manual_recovery: { label: '需要人工恢复',   desc: '升级与自动回滚均失败，已停止全部自动重试', tone: 'err' },
})

/**
 * §4.2 状态×动作受理矩阵（冻结）：true = 服务端会受理（202）；false = 必被同步拒绝。
 * staged/waiting/failed 的 install 为「门禁判定」——按钮可点，由服务端裁决
 *（不满足时 409 update_not_ready，details.blocking 列出全部未满足项）。
 */
const ACTION_MATRIX = Object.freeze({
  idle:            { check: true,  download: false, install: false, rollback: false },
  checking:        { check: true,  download: false, install: false, rollback: false },
  available:       { check: true,  download: true,  install: false, rollback: false },
  downloading:     { check: true,  download: true,  install: false, rollback: false },
  staged:          { check: true,  download: true,  install: true,  rollback: true },
  waiting:         { check: true,  download: true,  install: true,  rollback: true },
  installing:      { check: false, download: false, install: false, rollback: false },
  restarting:      { check: false, download: false, install: false, rollback: false },
  failed:          { check: true,  download: true,  install: true,  rollback: true },
  rolling_back:    { check: false, download: false, install: false, rollback: false },
  manual_recovery: { check: false, download: false, install: false, rollback: false },
})

/** 取某状态的受理矩阵行（未知状态按全拒绝处理，保守不误导用户点击） */
export function allowedActions(state) {
  const row = ACTION_MATRIX[state]
  return row
    ? { ...row }
    : { check: false, download: false, install: false, rollback: false }
}

/** §4.3 install 门禁 blocking 枚举（冻结）→ 中文说明 */
export const BLOCKING_LABELS = Object.freeze({
  staging_not_ready:   '新版本组件未完整就位（下载/验签/校验/staging）',
  active_run:          '存在运行中的脚本任务',
  update_transaction:  '存在其他升级/回滚/备份/迁移/维护事务',
  cron_freeze_window:  '下一次定时任务触发时间在冻结窗口内',
  launcher_unreachable:'升级器（launcher）不可达',
  insufficient_space:  '磁盘空间不足',
})

/** blocking 枚举数组 → 中文标签数组（未知值原样透出，不吞） */
export function blockingLabels(list) {
  return (Array.isArray(list) ? list : []).map((k) => BLOCKING_LABELS[k] || String(k))
}

/** §5.2 state ↔ journal detail 映射（诊断展示用；前端业务分支不得依赖 detail） */
export const DETAIL_LABELS = Object.freeze({
  idle: '空闲', committed: '已提交', cleaning: '清理中',
  checking: '检查中', checked: '检查完成',
  downloading: '下载中', verifying: '校验中',
  staged: '已就位', waiting_idle: '等待空闲窗口',
  draining: '停机排空中', stopped: '已停止',
  snapshotting: '快照中', snapshot_verified: '快照已校验',
  migrating: '迁移中', switched: '已切换',
  candidate_starting: '候选版本启动中', candidate_ready: '候选版本就绪', activating: '激活中',
  failed: '失败', rolling_back: '回滚中', manual_recovery_required: '需要人工恢复',
})

/** 字节人性化（progress.bytes_* / candidate.size_bytes）；非法输入原样返回 */
export function formatBytes(n) {
  const v = Number(n)
  if (!Number.isFinite(v) || v < 0) return String(n ?? '—')
  if (v < 1024) return `${v} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let x = v
  let i = -1
  while (x >= 1024 && i < units.length - 1) { x /= 1024; i++ }
  return `${x >= 100 ? Math.round(x) : x.toFixed(1)} ${units[i]}`
}

// ---- §2.1 /api/system/info 枚举的中文标签 ----

export const DEPLOYMENT_LABELS = Object.freeze({
  launcher: '便携托管（launcher）',
  direct: '直跑',
  docker: '容器（Docker）',
})

export const STRATEGY_LABELS = Object.freeze({
  managed: '升级器托管',
  external: '外部管理',
  unsupported: '不支持自动更新',
})

export const CHANNEL_LABELS = Object.freeze({
  stable: 'stable', beta: 'beta', dev: 'dev', unknown: '未知渠道',
})

export const STARTUP_STAGE_META = Object.freeze({
  starting:         { label: '启动中', cls: 'warn' },
  maintenance_gate: { label: '维护闸内', cls: 'warn' },
  ready:            { label: '就绪', cls: 'ok' },
})

export const DEP_STATUS_META = Object.freeze({
  ready:   { label: '正常', cls: 'ok' },
  missing: { label: '缺失', cls: 'err' },
  broken:  { label: '损坏', cls: 'err' },
  unknown: { label: '未知', cls: 'warn' },
})

export const DEP_SOURCE_LABELS = Object.freeze({
  managed: '托管', system: '系统', custom: '自定义',
})

export const DEP_BINDING_LABELS = Object.freeze({
  runtime: '运行时组件', application: '随应用分发', external: '外部',
})

/**
 * dev/unknown 构建信息判定（§2.1：dev 构建如实显示，不允许伪装正式版）：
 * channel 为 dev/unknown、版本带 -dev 后缀、或 commit/built_at 为 unknown 即视为开发构建。
 */
export function isDevBuild(app) {
  if (!app || typeof app !== 'object') return false
  return app.channel === 'dev'
    || app.channel === 'unknown'
    || String(app.version ?? '').endsWith('-dev')
    || app.commit === 'unknown'
    || app.built_at === 'unknown'
}

/** 长 id/哈希截短展示（boot_id / commit）；不足则原样 */
export function shortId(s, n = 8) {
  const str = String(s ?? '')
  if (!str || str === 'unknown') return str
  return str.length <= n ? str : str.slice(0, n)
}
