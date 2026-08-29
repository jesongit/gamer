/**
 * 录制顺序队列（plan §11.3 状态机与顺序队列）。
 *
 * - 所有录制动作（点击 / 滑动 / Alt 添加 / 取色）进入单一有序队列；
 *   条目带全局单调序号 seq，settled 步骤按 seq 排序输出 orderedSteps()，
 *   不因第二张模板上传更快而颠倒步骤顺序；
 * - Alt / 取色类不经历上传与裁切，但必须占位保序（reserve 后立即 setReady）；
 * - 上传失败保留草稿（截图 + 插入位置），支持重试 / 坐标降级 / 丢弃，不悄悄漏步；
 * - flush() 返回 Promise，全部条目到达终态（ready/discarded）后 resolve，
 *   供「停止录制」「保存」先排空队列（plan §11.3：排空期间显示进度并禁用重复保存）。
 *
 * 条目状态机：
 *   pending → uploading → cropping → ready
 *   pending/uploading/cropping → failed →（retry 回 pending | setReady | downgradeToTap | discard）
 *   任意非终态 → discarded；ready/discarded 为终态，再变更抛错。
 */

import { newStepUuid } from '../script-editor/model'

/** 条目状态全集（导出供 UI 时间线渲染参考）。 */
export const RECORDING_ENTRY_STATES = ['pending', 'uploading', 'cropping', 'ready', 'failed', 'discarded']

/** 终态集合。 */
const TERMINAL_STATES = new Set(['ready', 'discarded'])

/**
 * 状态迁移表：key = 目标状态，value = 允许的前置状态。
 * retry（failed → pending）与降级不走此表，单独校验。
 */
const TRANSITIONS = {
  uploading: ['pending'],
  cropping: ['pending', 'uploading'],
  ready: ['pending', 'uploading', 'cropping', 'failed'],
  failed: ['pending', 'uploading', 'cropping'],
  discarded: ['pending', 'uploading', 'cropping', 'failed'],
}

/**
 * @typedef {'click'|'swipe'|'alt'|'color'} RecordingEntryKind
 *
 * @typedef {Object} RecordingDraft
 * @property {string} frameDataUrl 冻结帧 dataURL（截自操作发生前）
 * @property {number} frameW 冻结帧原始宽（像素）
 * @property {number} frameH 冻结帧原始高（像素）
 * @property {import('./crop').Rect} aRect 自动模板区 A
 * @property {import('./crop').Rect} searchRect 搜索区域 S（二次裁切面板至少加载此范围）
 * @property {number} relX 按下点相对 X（0~1）
 * @property {number} relY 按下点相对 Y（0~1）
 * @property {[number, number]=} relEnd 抬起点相对坐标（滑动用）
 * @property {number=} durationMs 按下到抬起的真实时长（滑动用）
 * @property {string=} shortName 默认短名建议（record_click_YYYYMMDD_NNN.png）
 *
 * @typedef {Object} RecordingQueueEntry
 * @property {string} uuid 调用方提供的编辑器占位步骤 uuid
 * @property {number} seq 全局单调序号
 * @property {RecordingEntryKind} kind
 * @property {'pending'|'uploading'|'cropping'|'ready'|'failed'|'discarded'} state
 * @property {RecordingDraft | null} draft
 * @property {import('../script-editor/model').Step | null} step 终态 ready 时的目标步骤
 * @property {string | null} failure 失败原因（setFailed 写入，discard 保留供展示）
 * @property {boolean} downgraded 是否经 downgradeToTap 降级为 tap
 * @property {number} createdAt
 */
export class RecordingQueue {
  #seq = 1
  /** @type {Map<string, RecordingQueueEntry>} */
  #entries = new Map()
  /** @type {(() => void)[]} */
  #flushWaiters = []
  #onSettled
  #onUpdate

  /**
   * @param {{ onSettled?: (entry: RecordingQueueEntry) => void, onUpdate?: () => void }} [options]
   * - onSettled：单条目到达终态（ready/discarded）时回调一次，参数为该条目；
   * - onUpdate：任意变更后回调（UI 时间线刷新用，可选）。
   */
  constructor({ onSettled, onUpdate } = {}) {
    this.#onSettled = typeof onSettled === 'function' ? onSettled : null
    this.#onUpdate = typeof onUpdate === 'function' ? onUpdate : null
  }

  /**
   * 占位。uuid 来自调用方（编辑器占位步骤的 uuid）；seq 显式传入（atSeq）或自动分配，
   * 自动分配与显式传入共享同一单调计数（传入 atSeq 会把计数推到 atSeq+1）。
   * @param {{uuid: string, kind: RecordingEntryKind, atSeq?: number}} spec
   * @returns {RecordingQueueEntry}
   */
  reserve({ uuid, kind, atSeq } = {}) {
    if (!uuid) throw new Error('reserve 需要 uuid')
    if (!kind) throw new Error('reserve 需要 kind（click|swipe|alt|color）')
    if (this.#entries.has(uuid)) throw new Error(`队列条目 uuid 重复：${uuid}`)
    let seq
    if (atSeq === undefined || atSeq === null) {
      seq = this.#seq++
    } else {
      seq = atSeq
      this.#seq = Math.max(this.#seq, atSeq + 1)
    }
    /** @type {RecordingQueueEntry} */
    const entry = {
      uuid,
      seq,
      kind,
      state: 'pending',
      draft: null,
      step: null,
      failure: null,
      downgraded: false,
      createdAt: Date.now(),
    }
    this.#entries.set(uuid, entry)
    this.#notify()
    return entry
  }

  /**
   * 附加草稿（冻结帧 + 自动模板区 + 搜索区域 + 相对坐标；滑动另带 relEnd/durationMs）。
   * 终态条目不可附加。
   */
  attachDraft(uuid, draft) {
    const entry = this.#require(uuid)
    this.#assertNotTerminal(entry, 'attachDraft')
    entry.draft = { ...draft }
    this.#notify()
    return entry
  }

  /** pending → uploading。 */
  setUploading(uuid) {
    return this.#transition(uuid, 'uploading')
  }

  /** uploading → cropping（允许从 pending 直达，跳过上传态）。 */
  setCropping(uuid) {
    return this.#transition(uuid, 'cropping')
  }

  /**
   * 完成条目：step 为编辑器 Step（find 或 match→swipe），由调用方负责替换模型中的占位步骤。
   * failed 条目也允许直接 setReady（如人工确认仍用模板）。
   */
  setReady(uuid, step) {
    const entry = this.#require(uuid)
    this.#checkTransition(entry, 'ready')
    entry.step = step
    this.#applyTransition(entry, 'ready')
    return entry
  }

  /** 标记失败：保留草稿与插入位置，等待 retry / downgradeToTap / discard。 */
  setFailed(uuid, { reason } = {}) {
    const entry = this.#require(uuid)
    this.#checkTransition(entry, 'failed')
    entry.failure = reason ?? '未知原因'
    this.#applyTransition(entry, 'failed')
    return entry
  }

  /** 重试上传：failed → pending（清空失败原因，由调用方再次走 setUploading）。 */
  retry(uuid) {
    const entry = this.#require(uuid)
    if (entry.state !== 'failed') {
      throw new Error(`retry 只允许 failed 条目：${entry.uuid} 当前 ${entry.state}`)
    }
    entry.state = 'pending'
    entry.failure = null
    this.#notify()
    return entry
  }

  /** 丢弃：终态，不产出步骤（保留草稿与失败原因供时间线展示）。 */
  discard(uuid) {
    return this.#transition(uuid, 'discarded')
  }

  /**
   * 坐标动作降级（plan §11.3「改成坐标动作」）：生成 tap Step 并直接置 ready。
   * 坐标缺省时回退草稿起点；两者皆无则报错。
   * @returns {RecordingQueueEntry}
   */
  downgradeToTap(uuid, { relX, relY } = {}) {
    const entry = this.#require(uuid)
    this.#assertNotTerminal(entry, 'downgradeToTap')
    const d = entry.draft || {}
    const x = relX ?? d.relX
    const y = relY ?? d.relY
    if (typeof x !== 'number' || typeof y !== 'number') {
      throw new Error(`downgradeToTap 缺少坐标且草稿没有起点：${uuid}`)
    }
    entry.step = { uuid: newStepUuid(), kind: 'tap', at: { lit: [x, y] } }
    entry.downgraded = true
    this.#applyTransition(entry, 'ready')
    return entry
  }

  /** 未完成（非 ready/discarded）条目数。 */
  pendingCount() {
    let n = 0
    for (const e of this.#entries.values()) {
      if (!TERMINAL_STATES.has(e.state)) n += 1
    }
    return n
  }

  /** 全部条目 ready/discarded（没有条目时视为已排空）。 */
  isSettled() {
    return this.pendingCount() === 0
  }

  /** 全部条目 ready/discarded 后 resolve；已排空则立即 resolve。多个等待者同时释放。 */
  flush() {
    if (this.isSettled()) return Promise.resolve()
    return new Promise((resolve) => {
      this.#flushWaiters.push(resolve)
    })
  }

  /** settled 步骤按 seq 排序输出（Alt/取色占位同样参与排序；discarded 不产出）。 */
  orderedSteps() {
    return this.#bySeq()
      .filter((e) => e.state === 'ready')
      .map((e) => e.step)
  }

  /** 全部条目按 seq 排序（UI「录制待处理」时间线）。返回条目引用数组。 */
  list() {
    return this.#bySeq()
  }

  /** 按 uuid 取条目；不存在返回 undefined。 */
  get(uuid) {
    return this.#entries.get(uuid)
  }

  // ---------- 内部 ----------

  #bySeq() {
    return [...this.#entries.values()].sort((a, b) => a.seq - b.seq)
  }

  #require(uuid) {
    const entry = this.#entries.get(uuid)
    if (!entry) throw new Error(`队列没有条目：${uuid}`)
    return entry
  }

  #assertNotTerminal(entry, action) {
    if (TERMINAL_STATES.has(entry.state)) {
      throw new Error(`条目已终态（${entry.state}），不能 ${action}：${entry.uuid}`)
    }
  }

  #checkTransition(entry, to) {
    const allowed = TRANSITIONS[to]
    if (!allowed.includes(entry.state)) {
      throw new Error(`非法状态迁移：${entry.uuid} ${entry.state} → ${to}`)
    }
  }

  #applyTransition(entry, to) {
    entry.state = to
    if (TERMINAL_STATES.has(to) && this.#onSettled) this.#onSettled(entry)
    this.#notify()
  }

  #transition(uuid, to) {
    const entry = this.#require(uuid)
    this.#checkTransition(entry, to)
    this.#applyTransition(entry, to)
    return entry
  }

  #notify() {
    if (this.#onUpdate) this.#onUpdate()
    if (this.isSettled() && this.#flushWaiters.length > 0) {
      const waiters = this.#flushWaiters
      this.#flushWaiters = []
      for (const resolve of waiters) resolve()
    }
  }
}
