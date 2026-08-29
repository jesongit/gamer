/**
 * 录制控制器（plan §11.1–§11.5、§11.8 的编排层；纯逻辑、无 UI、无设备 IO）。
 *
 * Console 接线方注入边界依赖（本模块只依赖注入，便于测试与解耦）：
 * - insert(kind, anchorInfo) → uuid：经 factories + CommandStack 在锁定锚点插入占位步骤；
 *   anchorInfo = start() 传入的 { targetLabel, anchor } 快照；
 * - buildStep(kind, payload) → Step：find（点击）或 match→swipe（滑动）语义组装，
 *   走 CommandStack 事务（completeDraft 便捷入口内部调用；也可绕过直接 queue.setReady）；
 * - freezeFrame() → { dataUrl, width, height } | null：同步冻结当前最新帧；
 *   不可用时返回 null/抛错——触摸仍透传，条目记为失败草稿，不假报录制成功；
 * - sendTouch(event, payload)：向设备透传 'down' | 'move' | 'up'（不等待编码/上传）；
 * - now()：可注入时钟（毫秒；时长测量与命名日期）。
 *
 * 核心时序（§16.3 测试断言锚点）：
 * - 指针按下：先 freezeFrame 再 sendTouch('down')——模板永远截自操作发生前的画面；
 * - 指针抬起：立即 sendTouch('up')（透传优先），随后分类手势、插占位、建草稿；
 * - 手势分类：click → find 语义；swipe → match→swipe 语义（记录 durationMs）；
 *   longpress → 「未支持手势」失败草稿，不静默转成点击；
 * - pointercancel / cancelCurrent：补发 UP 释放设备 + 失败草稿，不悄悄漏步；
 * - 停止：recording → stopping（活动手势先按取消处理）→ 等队列排空 → idle。
 */

import { classifyGesture } from './gesture'
import { autoTemplateRect, searchRectAuto } from './crop'
import { defaultShortName } from './naming'
import { RecordingQueue } from './queue'

/** 默认时钟。 */
function defaultNow() {
  return Date.now()
}

function assertPointerPoint(relX, relY) {
  if (typeof relX !== 'number' || !Number.isFinite(relX) || typeof relY !== 'number' || !Number.isFinite(relY)) {
    throw new Error(`指针事件需要有限数字 relX/relY（0~1 相对坐标），收到 ${relX}, ${relY}`)
  }
}

function assertFrameSize(frameW, frameH) {
  if (!(frameW > 0) || !(frameH > 0)) {
    throw new Error(`帧尺寸非法：${frameW}x${frameH}`)
  }
}

/**
 * @param {{
 *   insert: (kind: string, anchorInfo: {targetLabel: string, anchor: unknown}) => string,
 *   buildStep: (kind: string, payload: Object) => import('../script-editor/model').Step,
 *   freezeFrame: () => { dataUrl: string, width: number, height: number } | null,
 *   sendTouch: (event: 'down'|'move'|'up', payload: {relX: number, relY: number}) => void,
 *   now?: () => number,
 * }} deps
 */
export function createRecordingController({ insert, buildStep, freezeFrame, sendTouch, now = defaultNow }) {
  const deps = { insert, buildStep, freezeFrame, sendTouch }
  for (const [name, fn] of Object.entries(deps)) {
    if (typeof fn !== 'function') throw new Error(`createRecordingController 缺少依赖：${name}`)
  }

  const queue = new RecordingQueue({ onUpdate: () => emit() })
  /** @type {Set<(snapshot: Object) => void>} */
  const listeners = new Set()
  /** @type {Map<string, number>} key = kind@本地年月日 → 已用序号 */
  const nameCounters = new Map()

  let state = 'idle'
  let targetLabel = ''
  let anchor = null
  /** @type {null | {startRel: [number, number], lastRel: [number, number], frameW: number, frameH: number, frame: {dataUrl: string, width: number, height: number} | null, startTimeMs: number}} */
  let active = null
  let stopPromise = null

  /** 广播快照（state / 待处理数 / 活动手势），onChange 订阅者与 queue 变更共用。 */
  function emit() {
    const snapshot = {
      state,
      targetLabel,
      pendingCount: queue.pendingCount(),
      hasActiveGesture: active !== null,
    }
    for (const fn of [...listeners]) fn(snapshot)
  }

  function setState(next) {
    state = next
    emit()
  }

  /** 默认短名建议：按 kind+日期独立计数（record_click_20260829_001 → 002 …）。 */
  function nextShortName(kind) {
    const date = new Date(now())
    const key = `${kind}@${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`
    const n = (nameCounters.get(key) || 0) + 1
    nameCounters.set(key, n)
    return defaultShortName(kind, date, n)
  }

  /**
   * 插占位 + 建草稿。模板数学一律以冻结帧原始尺寸为基准（§11.6），
   * 搜索区域初始为「无手动裁切」语义（以 A 为中心的 100×100）。
   */
  function placeDraft(g, kind, upRel, durationMs) {
    const startX = g.startRel[0] * g.frameW
    const startY = g.startRel[1] * g.frameH
    const aRect = autoTemplateRect(g.frameW, g.frameH, startX, startY)
    const searchRect = searchRectAuto(aRect, g.frameW, g.frameH)
    const shortName = nextShortName(kind)
    const uuid = deps.insert(kind, { targetLabel, anchor })
    queue.reserve({ uuid, kind })
    queue.attachDraft(uuid, {
      frameDataUrl: g.frame.dataUrl,
      frameW: g.frameW,
      frameH: g.frameH,
      aRect,
      searchRect,
      relX: g.startRel[0],
      relY: g.startRel[1],
      relEnd: [upRel[0], upRel[1]],
      durationMs,
      shortName,
    })
    return uuid
  }

  /** 失败草稿（长按/取消/无画面）：占位保序，用户可降级为点击或丢弃，不悄悄漏步。 */
  function placeFailed(g, kind, upRel, durationMs, reason) {
    if (g.frame) {
      const uuid = placeDraft(g, kind, upRel, durationMs)
      queue.setFailed(uuid, { reason })
      return uuid
    }
    // 无冻结帧：没有可上传的截图，仍占位记录「这一步发生过」，失败原因说明画面缺失
    const uuid = deps.insert(kind, { targetLabel, anchor })
    queue.reserve({ uuid, kind })
    queue.setFailed(uuid, { reason })
    return uuid
  }

  /** 取消当前手势：补发 UP 释放设备 + 失败草稿；无进行中手势时为 no-op。 */
  function cancelCurrent(reason = '触摸被取消') {
    if (!active) return
    const g = active
    active = null
    deps.sendTouch('up', { relX: g.lastRel[0], relY: g.lastRel[1] })
    placeFailed(g, 'click', g.lastRel, now() - g.startTimeMs, reason)
    emit()
  }

  const controller = {
    /** 录制顺序队列（E2 驱动 setUploading/setCropping/setReady/setFailed/retry/discard/downgradeToTap 并读 list() 渲染时间线）。 */
    queue,

    /** 'idle' | 'recording' | 'stopping'。 */
    get state() {
      return state
    },

    /** 录制目标展示名（录制栏「录制到：…」）。 */
    get targetLabel() {
      return targetLabel
    },

    /** 订阅变更（state 迁移、队列增删、手势起止）；回调收到快照。返回取消函数。 */
    onChange(fn) {
      listeners.add(fn)
      return () => listeners.delete(fn)
    },

    /**
     * 开始录制：idle → recording，锁定插入目标（§11.8）。
     * @param {{targetLabel?: string, anchor?: unknown}} [options]
     * anchor 为接线方的插入锚点快照（如 selection.ts 的 InsertionAnchor），随 anchorInfo 透传给 insert。
     */
    start({ targetLabel: label = '', anchor: anchorArg = null } = {}) {
      if (state !== 'idle') throw new Error(`无法开始录制：当前状态 ${state}`)
      targetLabel = label
      anchor = anchorArg
      setState('recording')
    },

    /**
     * 停止录制：recording → stopping（活动手势先按取消处理：补 UP + 失败草稿），
     * 返回 Promise，队列排空（全部 ready/discarded）后 resolve 并回到 idle。
     * 失败条目会阻塞排空——需用户重试/降级/丢弃后完成，与 §11.3「先处理当前草稿」一致。
     */
    stop() {
      if (state === 'idle') return Promise.resolve()
      if (state === 'stopping') return stopPromise
      if (active) cancelCurrent('停止录制：手势未完成')
      setState('stopping')
      stopPromise = queue.flush().then(() => {
        stopPromise = null
        setState('idle')
      })
      return stopPromise
    },

    /** stop() 的宽容别名：任意状态可调用，返回排空 Promise。 */
    flushAndFinish() {
      return controller.stop()
    },

    /**
     * 指针按下：先冻结「操作前」的最新帧，再透传 down（顺序断言核心，§16.3）。
     * 冻结失败不阻断透传：条目稍后记为失败草稿（画面不可用）。
     * 仅 recording 状态响应；已有活动手势时忽略（多指路由由接线方处理）。
     */
    onPointerDown({ relX, relY, frameW, frameH }) {
      if (state !== 'recording' || active) return
      assertPointerPoint(relX, relY)
      assertFrameSize(frameW, frameH)
      let frame = null
      try {
        frame = deps.freezeFrame()
      } catch {
        frame = null
      }
      if (!frame || typeof frame.dataUrl !== 'string' || !frame.dataUrl) frame = null
      deps.sendTouch('down', { relX, relY })
      // 模板数学以冻结帧原始尺寸为准；冻结失败退回事件携带的尺寸（仅用于手势分类）
      const fw = frame && Number.isFinite(frame.width) ? frame.width : frameW
      const fh = frame && Number.isFinite(frame.height) ? frame.height : frameH
      active = {
        startRel: [relX, relY],
        lastRel: [relX, relY],
        frameW: fw,
        frameH: fh,
        frame,
        startTimeMs: now(),
      }
      emit()
    },

    /**
     * 指针移动：透传 move（滑动过程持续发给设备），并只更新轨迹终点；
     * 不重截帧、不发广播（轨迹绘制由 UI 层自行处理）。
     */
    onPointerMove({ relX, relY }) {
      if (state !== 'recording' || !active) return
      assertPointerPoint(relX, relY)
      deps.sendTouch('move', { relX, relY })
      active.lastRel = [relX, relY]
    },

    /**
     * 指针抬起：立即透传 up，随后按帧原始像素位移 + 时长分类：
     * click → find 语义草稿；swipe → match→swipe 语义草稿（记录 durationMs）；
     * longpress → 「未支持手势」失败草稿，不静默转成点击。
     * 位移/模板以按下时的冻结帧尺寸为基准（中途旋转以按下帧为准）。
     */
    onPointerUp({ relX, relY } = {}) {
      if (state !== 'recording' || !active) return
      assertPointerPoint(relX, relY)
      const g = active
      active = null
      deps.sendTouch('up', { relX, relY })
      const durationMs = now() - g.startTimeMs
      const dxPx = (relX - g.startRel[0]) * g.frameW
      const dyPx = (relY - g.startRel[1]) * g.frameH
      const gesture = classifyGesture({ dxPx, dyPx, durationMs, frameW: g.frameW, frameH: g.frameH })
      if (!g.frame) {
        placeFailed(g, 'click', [relX, relY], durationMs, '画面不可用，未能截取模板')
      } else if (gesture === 'click') {
        placeDraft(g, 'click', [relX, relY], durationMs)
      } else if (gesture === 'swipe') {
        placeDraft(g, 'swipe', [relX, relY], durationMs)
      } else {
        placeFailed(g, 'click', [relX, relY], durationMs, `不支持的手势：长按 ${Math.round(durationMs)}ms，可降级为点击或丢弃`)
      }
      emit()
    },

    /**
     * pointercancel 清理（多指/cancel 的最终处理在接线方）：补发 UP 释放设备 + 失败草稿。
     */
    onPointerCancel() {
      cancelCurrent('触摸被取消（pointercancel）')
    },

    /** cancelCurrent 的对外别名：reason 作为失败草稿的原因文本。 */
    cancelCurrent,

    /**
     * Alt 添加（模板 find / 取色 color）：不进上传流程，仅占位保序。
     * step 由接线方经工厂 + CommandStack 插入模型后传入；color 步骤记 kind='color'，其余记 'alt'。
     * 仅 recording 状态响应（stopping/idle 忽略）。
     */
    onAltAdd(step) {
      if (state !== 'recording') return
      if (!step || typeof step.uuid !== 'string' || !step.uuid) {
        throw new Error('onAltAdd 需要带 uuid 的编辑器步骤')
      }
      const kind = step.kind === 'color' ? 'color' : 'alt'
      queue.reserve({ uuid: step.uuid, kind })
      queue.setReady(step.uuid, step)
    },

    /**
     * 草稿完成便捷入口：buildStep(kind, payload) 组装步骤后置 ready。
     * payload 会被补充 { draft, uuid }，接线方据此生成 find / match→swipe。
     * 也可绕过本方法直接 queue.setReady(uuid, step)。
     */
    completeDraft(uuid, payload = {}) {
      const entry = queue.get(uuid)
      if (!entry) throw new Error(`队列没有条目：${uuid}`)
      const step = deps.buildStep(entry.kind, { ...payload, draft: entry.draft, uuid })
      queue.setReady(uuid, step)
      return step
    },
  }

  return controller
}
