/**
 * Console 录制接线（阶段 6 后半，plan §11）。
 *
 * 职责分界（录制核心服务在 web/src/recording/，本 composable 是 Console 侧的消费/接线层）：
 * - recording/service.js createRecordingController：指针时序（DOWN 先冻结帧再透传 down）、
 *   手势分类、有序队列与状态机的编排层；经注入回调写模型/发触控。
 * - 本层：模型写入（占位插入 / 最终替换，各为一次 CommandStack 事务）、上传与裁切流程
 *   （setUploading → 上传 → setReady / setFailed，重试/坐标降级/丢弃）、插入目标锁定
 *   （anchor 快照随 start() 传入，插入位置按序推进）、命名冲突（默认短名建议来自控制器，
 *   分区冲突时顺延序号，冲突要求改名不覆盖）、状态栏数据、Alt 作用域、多指透传提示、
 *   停止排空与离开保护。
 *
 * 与 recording/service.js 的对齐契约（已按 E1 提交实现核对）：
 * - insert(kind, anchorInfo) → uuid：本层构建 find/match 占位插入锁定锚点并返回 uuid；
 * - buildStep(kind, payload) → Step：click → find[短名]；swipe → match 候选[短名]{swipe}
 *   + else throw + timeout 30s（payload = {draft, uuid, name?}）；
 * - freezeFrame() → {dataUrl,width,height}|null（video drawImage 离屏 canvas，原始像素）；
 * - sendTouch(event, {relX,relY})：'down'|'move'|'up' 相对坐标 → 设备像素即时发送
 *   （move 走 rAF 合并，不等待编码/上传）；
 * - controller.state（'idle'|'recording'|'stopping'）+ onChange(快照) 驱动 phase；
 * - controller.queue（RecordingQueue）为唯一草稿账本：本层读 list()/pendingCount()
 *   驱动时间线与二次裁切面板，写 setUploading/setCropping/setReady/setFailed/retry/
 *   discard/downgradeToTap（「也可绕过 completeDraft 直接 setReady」见 service.js 注释）。
 */
import { computed, reactive, ref } from 'vue'
import { createRecordingController } from '../recording/service'
import { searchRectAuto, searchRectManual, toRelative } from '../recording/crop'
import { defaultShortName, isValidShortName } from '../recording/naming'
import { createStep } from '../script-editor/factories'
import { lit } from '../script-editor/model'
import { resolveStepList } from '../script-editor/commands'
import { breadcrumb, findStepLocation } from '../script-editor/selection'

/** Alt 作用域拆分（plan §11.2）：录制中投屏 Alt 特殊语义暂停，模板/取色 Alt 保留。 */
export function altScopeFlags(altModeActive, recordingActive) {
  return {
    projection: !!altModeActive && !recordingActive,
    template: !!altModeActive,
    crop: !!altModeActive,
  }
}

export const PROJECTION_ALT_HINT = '投屏 Alt 暂停；模板添加与取色仍可用'

const SWIPE_THROW_MSG = '未找到滑动起点'
const SWIPE_TIMEOUT = '30s'
const TERMINAL_STATES = new Set(['ready', 'discarded'])

/** 相对坐标夹取 + 保留 4 位（与 shell 插入同精度）。 */
function round4(n) {
  const v = Number(n)
  if (!Number.isFinite(v)) return 0
  return Number(Math.min(1, Math.max(0, v)).toFixed(4))
}

/** 模板短名：去 #区域后缀（与 Console.tplShortName 同规则）。 */
function stripRegionSuffix(name) {
  return String(name || '').replace(/#[^#./\\]+(\.(png|jpe?g))$/i, '$1')
}

export function useRecording({
  shell,
  activePkg,
  connected,
  videoElement,
  templatesData,
  api,
  notify,
  sendControl,
  sendTouchMove = null,
  clientToDevice = null,
  hasVideo = null,
  freezeFrame: freezeFrameOverride = null,
  cropPng: cropPngOverride = null,
  now: nowOverride = null,
} = {}) {
  const nowFn = nowOverride || (() => Date.now())

  // ---- 会话状态 ----
  const phase = ref('idle') // 镜像 controller.state：'idle' | 'recording' | 'stopping'
  const targetLabel = ref('')
  const queueTick = ref(0) // 队列变更计数（驱动时间线/派生值重算）
  const lockPkg = ref('') // 开始录制时的应用分区（录制期间禁止切换）
  let lockedAnchor = null // start() 传给控制器的锚点快照 {containerPath, index}
  let insertCount = 0 // 锚点内已插入数（保序推进：index = anchor.index + insertCount）
  let multiTouchToasted = false
  const usedNames = new Set() // 本次会话已占用/在途的短名
  const finalizing = new Set() // 上传去重（自动定稿与手动确认竞争）
  const inflight = new Set() // 在途上传 Promise
  const passthroughPointers = new Map() // pointerId -> {dx,dy}（第二只指针透传）
  let gestureDims = null // 当前手势冻结帧尺寸（sendTouch rel → 设备像素换算基准）

  // ---- 冻结帧（deliverable 4）：video 元素 drawImage 到离屏 canvas，原始像素尺寸基准 ----
  function defaultFreezeFrame() {
    const v = videoElement && 'value' in videoElement ? videoElement.value : videoElement
    if (!v || !v.videoWidth || !v.videoHeight) return null
    const c = document.createElement('canvas')
    c.width = v.videoWidth
    c.height = v.videoHeight
    c.getContext('2d').drawImage(v, 0, 0)
    return { dataUrl: c.toDataURL('image/png'), width: v.videoWidth, height: v.videoHeight }
  }

  function freezeFrame() {
    return freezeFrameOverride ? freezeFrameOverride() : defaultFreezeFrame()
  }

  // ---- 触控透传（deliverable 3）：rel → 设备像素即时发送；move 复用 Console 的 rAF 合并 ----
  function liveDims() {
    const v = videoElement && 'value' in videoElement ? videoElement.value : videoElement
    return v && v.videoWidth ? { w: v.videoWidth, h: v.videoHeight } : (gestureDims || { w: 0, h: 0 })
  }

  function sendTouch(event, payload = {}) {
    const dims = gestureDims || liveDims()
    const x = Math.round((payload.relX || 0) * dims.w)
    const y = Math.round((payload.relY || 0) * dims.h)
    if (event === 'move' && sendTouchMove) {
      sendTouchMove(x, y)
      return
    }
    sendControl({ type: 'touch', action: event, x, y })
  }

  // ---- 锁定插入目标（deliverable 7，plan §11.8）----
  function describeTarget(model, sel) {
    if (!sel) return '主流程末尾'
    const loc = findStepLocation(model, sel)
    if (!loc) return '主流程末尾'
    const path = breadcrumb(model, sel).map(n => n.label).join(' / ')
    return `${path} · 第 ${loc.index + 1} 步之后`
  }

  /** 锁定锚点的下一个插入位（锚点 index + 已插入数；越界回退容器末尾；容器被删回退主流程）。 */
  function nextAnchor() {
    let base = lockedAnchor || { containerPath: ['steps'], index: 0 }
    let idx = base.index + insertCount
    try {
      const list = resolveStepList(shell.model, base.containerPath)
      if (idx > list.length) idx = list.length
    } catch {
      base = { containerPath: ['steps'], index: 0 }
      try { idx = resolveStepList(shell.model, base.containerPath).length } catch { idx = 0 }
    }
    return { containerPath: base.containerPath, index: idx }
  }

  // ---- 命名冲突（plan §11.7）：默认短名建议来自控制器；冲突时顺延序号，要求改名不覆盖 ----
  function shortNameTaken(name) {
    const n = stripRegionSuffix(name)
    if (usedNames.has(n)) return true
    const pkg = activePkg && 'value' in activePkg ? activePkg.value : activePkg
    const list = templatesData && 'value' in templatesData ? templatesData.value : []
    return list.some(t => t.pkg === pkg && stripRegionSuffix(t.name) === n)
  }

  /** 建议短名被占用时按同前缀顺延序号（record_click_20260829_001 → _002 …）。 */
  function nextFreeName(suggested) {
    let name = suggested
    const m = /^(.*?)(\d+)(\.png)$/.exec(suggested)
    let seq = m ? Number(m[2]) : 0
    const stem = m ? m[1] : String(suggested).replace(/\.png$/, '_')
    while (shortNameTaken(name)) {
      seq += 1
      name = `${stem}${String(seq).padStart(3, '0')}.png`
    }
    return name
  }

  /** 面板默认短名：控制器建议 → 冲突顺延。 */
  function nextShortName(kind, suggested = '') {
    return nextFreeName(suggested || defaultShortName(kind, new Date(nowFn()), 1))
  }

  // ---- 步骤构建（deliverable 5/6）----
  function buildFindStep(name) {
    return createStep('find', { template: lit(name), block: [], verify: false, timeout: null, then: [], else: [] })
  }

  function buildSwipeMatchStep({ name, from, to, durationMs }) {
    const ms = Math.max(1, Math.round(durationMs || 500))
    return createStep('match', {
      candidates: [{
        template: lit(name),
        steps: [createStep('swipe', { from: lit(from), to: lit(to), time: lit(`${ms}ms`) })],
      }],
      else: [createStep('throw', { message: SWIPE_THROW_MSG })],
      timeout: lit(SWIPE_TIMEOUT),
    })
  }

  function buildTapStep(relX, relY) {
    return createStep('tap', { at: lit([round4(relX), round4(relY)]) })
  }

  function buildAltSwipeStep(from, to, durationMs) {
    const ms = Math.max(1, Math.round(durationMs || 1000))
    return createStep('swipe', {
      from: lit([round4(from[0]), round4(from[1])]),
      to: lit([round4(to[0]), round4(to[1])]),
      time: lit(`${ms}ms`),
    })
  }

  function buildColorStep(at, hex) {
    return createStep('color', { at: lit([round4(at[0]), round4(at[1])]), expect: [{ color: lit(hex), steps: [] }], else: [] })
  }

  /**
   * 注入 controller 的 buildStep（deliverable 5 语义）：
   * click → find[短名]；swipe → match 候选[短名]{swipe fm/to/time} + else throw 未找到滑动起点 + timeout 30s。
   */
  function buildStep(kind, payload = {}) {
    const d = payload.draft || {}
    const name = payload.name || d.shortName || ''
    if (kind === 'swipe') {
      const end = d.relEnd || [d.relX, d.relY]
      return buildSwipeMatchStep({
        name,
        from: [round4(d.relX ?? 0.5), round4(d.relY ?? 0.5)],
        to: [round4(end[0] ?? 0.5), round4(end[1] ?? 0.5)],
        durationMs: d.durationMs,
      })
    }
    return buildFindStep(name)
  }

  /** 同 kind 占位 → 最终步骤：update_step 一次事务（uuid 稳定），返回模型步骤。 */
  function patchPlaceholder(uuid, finalStep, label) {
    const r = shell.replaceStepFields(uuid, finalStep, label)
    return r ? r.step : null
  }

  /** 跨 kind 替换（坐标降级 find/match → tap）：同事务 remove+insert（保留目标 step 原样）。 */
  function swapPlaceholder(uuid, step, label) {
    const loc = findStepLocation(shell.model, uuid)
    if (!loc) return null
    let ok = false
    shell.stack.transaction(() => {
      const r1 = shell.stack.apply({ type: 'remove_step', path: loc.containerPath, index: loc.index }, label)
      const r2 = shell.stack.apply({ type: 'insert_step', path: loc.containerPath, index: loc.index, step }, label)
      ok = r1 && r2
    }, label)
    return ok ? step : null
  }

  // ---- 控制器注入回调 ----

  /** 占位构建（控制器 placeDraft → insert(kind, anchorInfo)）。 */
  function placeholderStep(kind) {
    return kind === 'swipe'
      ? createStep('match', { candidates: [{ template: lit(''), steps: [] }], else: [], timeout: null })
      : buildFindStep('')
  }

  /**
   * 注入 controller 的 insert：构建占位 → 锁定锚点插入（一次事务）→ 返回 uuid。
   * 插入前先自动定稿上一批待裁切草稿（下一次录制动作触发，plan §11.3）。
   * 插入失败仍返回哨兵 uuid（队列照常登记为失败草稿，用户可丢弃，不悄悄漏步）。
   */
  function insertPlaceholder(kind, anchorInfo) {
    // Alt/取色类不经此路径（onAltAdd 由本层直接插模型后调用控制器登记）。
    // 下一次录制动作触发当前待裁切草稿定稿（A + 自动搜索区，plan §11.3）
    autoFinalizeUnsettled()
    const anchor = nextAnchor()
    const step = placeholderStep(kind)
    let ok = false
    try {
      ok = shell.insertStepWithAnchor(step, '录制占位', anchor)
    } catch (e) {
      notify(`录制占位插入失败：${e.message}`, 'error')
    }
    if (!ok) {
      notify('录制占位插入失败：插入目标不可用（可能已被删除）', 'error')
      return `lost-${nowFn()}-${insertCount}`
    }
    insertCount += 1
    return step.uuid
  }

  const controller = createRecordingController({
    insert: insertPlaceholder,
    buildStep,
    freezeFrame,
    sendTouch,
    now: nowFn,
  })

  controller.onChange((snap) => {
    if (snap && typeof snap.state === 'string') phase.value = snap.state
    if (snap && typeof snap.targetLabel === 'string' && snap.targetLabel) targetLabel.value = snap.targetLabel
    queueTick.value += 1
  })

  // ---- 队列视图（响应式投影；账本唯一来源 = controller.queue）----
  function entriesView() {
    void queueTick.value
    return controller.queue.list()
  }

  /** 自动定稿所有待裁切草稿（pending/cropping；下一次录制动作 / 停止录制触发，plan §11.3）。 */
  function autoFinalizeUnsettled() {
    for (const entry of controller.queue.list()) {
      if (!entry.draft) continue
      if ((entry.state === 'pending' || entry.state === 'cropping') && !finalizing.has(entry.uuid)) {
        const draft = entry.draft
        void finalizeEntry(entry, {
          rect: draft.aRect,
          searchRect: draft.searchRect,
          name: nextShortName(entry.kind, draft.shortName),
        })
      }
    }
  }

  // ---- 裁切/上传（deliverable 5）----
  // 最近一次裁切尝试（重试沿用同一裁切结果与短名）
  const attemptMeta = new Map() // uuid -> {rect, searchRect, name}
  function defaultCropPng(frameDataUrl, rect) {
    return new Promise((resolve, reject) => {
      const img = new Image()
      img.onload = () => {
        try {
          const w = Math.max(1, Math.round(rect.w))
          const h = Math.max(1, Math.round(rect.h))
          const c = document.createElement('canvas')
          c.width = w
          c.height = h
          c.getContext('2d').drawImage(img, rect.x, rect.y, rect.w, rect.h, 0, 0, w, h)
          const url = c.toDataURL('image/png')
          resolve(url.slice(url.indexOf(',') + 1))
        } catch (e) { reject(e) }
      }
      img.onerror = () => reject(new Error('冻结帧解码失败'))
      img.src = frameDataUrl
    })
  }

  const cropPng = cropPngOverride || defaultCropPng

  /** 像素矩形 → 相对 [x1,y1,x2,y2]（模板名 #后缀 / 搜索区域参数用）。 */
  function regionOf(rect, frameW, frameH) {
    const [x1, y1] = toRelative(rect.x, rect.y, frameW, frameH)
    const [x2, y2] = toRelative(rect.x + rect.w, rect.y + rect.h, frameW, frameH)
    return [x1, y1, x2, y2]
  }

  async function finalizeEntry(entry, { rect, searchRect, name }) {
    if (!entry || !entry.draft) return
    if (TERMINAL_STATES.has(entry.state) || entry.state === 'uploading' || finalizing.has(entry.uuid)) return
    const pkg = activePkg && 'value' in activePkg ? activePkg.value : activePkg
    if (!pkg) {
      try { controller.queue.setFailed(entry.uuid, { reason: '未选择应用分区' }) } catch { /* 容错 */ }
      return
    }
    finalizing.add(entry.uuid)
    attemptMeta.set(entry.uuid, { rect, searchRect, name })
    const p = (async () => {
      try { controller.queue.setUploading(entry.uuid) } catch { /* cropping 直传时不重复标记 */ }
      try {
        const b64 = await cropPng(entry.draft.frameDataUrl, rect)
        await api.uploadTemplateRegion(name, b64, pkg, regionOf(searchRect, entry.draft.frameW, entry.draft.frameH))
        try { templatesData.value = await api.listTemplates() } catch { /* 列表刷新失败不阻塞步骤定稿 */ }
        usedNames.add(name)
        const step = buildStep(entry.kind, { name, draft: entry.draft, uuid: entry.uuid })
        const modelStep = patchPlaceholder(
          entry.uuid,
          step,
          entry.kind === 'swipe' ? `录制滑动 → match ${name}` : `录制点击 → find ${name}`,
        )
        if (!modelStep) throw new Error('占位步骤已不存在（可能已被撤销）')
        controller.queue.setReady(entry.uuid, modelStep)
        notify(`模板 ${name} 已上传，步骤已定稿`, 'success')
      } catch (e) {
        try { controller.queue.setFailed(entry.uuid, { reason: e?.message || String(e) }) } catch { /* 容错 */ }
        notify(`模板上传失败：${e?.message || e}（可重试、改用坐标或丢弃）`, 'error')
      }
    })()
    inflight.add(p)
    try { await p } finally { inflight.delete(p); finalizing.delete(entry.uuid) }
  }

  /** 面板确认：短名校验（冲突要求改名不覆盖）→ 上传 → 定稿。 */
  function confirmCrop(entry, { name, rect, adjusted }) {
    const real = controller.queue.get(entry?.uuid) || entry
    if (!real || !real.draft || TERMINAL_STATES.has(real.state) || real.state === 'uploading') return false
    const nm = String(name || '').trim()
    if (!nm || !isValidShortName(nm)) {
      notify('模板短名不合法（仅字母数字 - _ ，以 .png 结尾）', 'warn')
      return false
    }
    if (shortNameTaken(nm)) {
      notify(`短名 ${nm} 已存在，请改名（不会覆盖）`, 'warn')
      return false
    }
    const searchRect = adjusted
      ? searchRectManual(real.draft.aRect, rect, real.draft.frameW, real.draft.frameH)
      : (real.draft.searchRect || searchRectAuto(real.draft.aRect, real.draft.frameW, real.draft.frameH))
    void finalizeEntry(real, { rect, searchRect, name: nm })
    return true
  }

  /** 「只使用坐标」降级（plan §11.3）：队列生成 tap 步骤，占位整体替换（一次事务）。 */
  function downgrade(entry) {
    const real = controller.queue.get(entry?.uuid) || entry
    if (!real || TERMINAL_STATES.has(real.state) || real.state === 'uploading') return false
    let tapEntry
    try {
      tapEntry = controller.queue.downgradeToTap(real.uuid)
    } catch (e) {
      notify(`坐标降级失败：${e.message}`, 'error')
      return false
    }
    const swapped = swapPlaceholder(real.uuid, tapEntry.step, '录制降级 → tap')
    if (!swapped) {
      notify('坐标降级失败：占位步骤已不存在', 'error')
      return false
    }
    notify('已改为坐标点击步骤', 'info')
    return true
  }

  /** 丢弃草稿：占位一并移除（显式用户决定，不悄悄漏步）。 */
  function discard(entry) {
    const real = controller.queue.get(entry?.uuid) || entry
    if (!real || TERMINAL_STATES.has(real.state) || real.state === 'uploading') return false
    const loc = findStepLocation(shell.model, real.uuid)
    if (loc) {
      shell.stack.apply({ type: 'remove_step', path: loc.containerPath, index: loc.index }, '丢弃录制草稿')
    }
    try { controller.queue.discard(real.uuid) } catch (e) { notify(`丢弃失败：${e.message}`, 'error'); return false }
    return true
  }

  /** 失败重试：failed → pending，沿用上次裁切结果（或 A + 自动搜索区）重新上传。 */
  function retry(entry) {
    const real = controller.queue.get(entry?.uuid) || entry
    if (!real || real.state !== 'failed') return false
    try { controller.queue.retry(real.uuid) } catch (e) { notify(`重试失败：${e.message}`, 'error'); return false }
    const draft = real.draft
    if (!draft) return true // 无草稿条目（如画面缺失失败）：保持 pending，等待用户丢弃/降级
    const last = attemptMeta.get(real.uuid) || {}
    void finalizeEntry(real, {
      rect: last.rect || draft.aRect,
      searchRect: last.searchRect || draft.searchRect,
      name: last.name || nextShortName(real.kind, draft.shortName),
    })
    return true
  }

  // ---- 多指透传（deliverable 3）：第二只指针只透传并提示一次；pointercancel 清理 ----
  function onWinPointerDown(e) {
    if (phase.value !== 'recording') return
    if (e.isPrimary) return // 主指针由投屏 mouse 事件链路驱动录制
    passthroughPointers.set(e.pointerId, { dx: null, dy: null })
    if (!multiTouchToasted) {
      multiTouchToasted = true
      notify('不支持多指录制：第二只指针仅透传，不生成步骤', 'warn')
    }
    const dev = clientToDevice ? clientToDevice(e.clientX, e.clientY) : null
    if (dev) sendControl({ type: 'touch', action: 'down', x: dev.x, y: dev.y })
  }

  function onWinPointerMove(e) {
    const t = passthroughPointers.get(e.pointerId)
    if (!t) return
    const dev = clientToDevice ? clientToDevice(e.clientX, e.clientY) : null
    if (!dev) return
    if (t.dx === null || Math.abs(dev.x - t.dx) + Math.abs(dev.y - t.dy) > 6) {
      t.dx = dev.x
      t.dy = dev.y
      sendControl({ type: 'touch', action: 'move', x: dev.x, y: dev.y })
    }
  }

  function passthroughUp(e) {
    if (!passthroughPointers.has(e.pointerId)) return false
    passthroughPointers.delete(e.pointerId)
    const dev = clientToDevice ? clientToDevice(e.clientX, e.clientY) : null
    if (dev) sendControl({ type: 'touch', action: 'up', x: dev.x, y: dev.y })
    return true
  }

  function onWinPointerUp(e) { passthroughUp(e) }

  function onWinPointerCancel(e) {
    if (passthroughUp(e)) return
    // 主指针被浏览器接管（滚动等）→ 取消当前录制手势（补发 UP + 失败草稿）
    if (phase.value === 'recording') cancelGesture('pointercancel')
  }

  function onWinBlur() {
    if (phase.value === 'recording') cancelGesture('blur')
  }

  let listenersInstalled = false
  function installWindowListeners() {
    if (listenersInstalled || typeof window === 'undefined') return
    window.addEventListener('pointerdown', onWinPointerDown, true)
    window.addEventListener('pointermove', onWinPointerMove, true)
    window.addEventListener('pointerup', onWinPointerUp, true)
    window.addEventListener('pointercancel', onWinPointerCancel, true)
    window.addEventListener('blur', onWinBlur)
    listenersInstalled = true
  }

  function removeWindowListeners() {
    if (!listenersInstalled || typeof window === 'undefined') return
    window.removeEventListener('pointerdown', onWinPointerDown, true)
    window.removeEventListener('pointermove', onWinPointerMove, true)
    window.removeEventListener('pointerup', onWinPointerUp, true)
    window.removeEventListener('pointercancel', onWinPointerCancel, true)
    window.removeEventListener('blur', onWinBlur)
    listenersInstalled = false
    passthroughPointers.clear()
  }

  // ---- 指针入口（Console 投屏事件换算后调用，deliverable 3）----
  function onPointerDown(m) {
    if (phase.value !== 'recording') return
    gestureDims = { w: m.frameW, h: m.frameH }
    controller.onPointerDown(m)
  }

  function onPointerMove(m) {
    if (phase.value !== 'recording') return
    controller.onPointerMove(m)
  }

  function onPointerUp(m) {
    if (phase.value !== 'recording') return
    try {
      controller.onPointerUp(m)
    } finally {
      gestureDims = null
    }
  }

  function onPointerCancel() {
    if (phase.value !== 'recording') return
    controller.onPointerCancel()
  }

  /** 取消原因 → 失败草稿可读文本（reason 亦作队列失败原因展示）。 */
  const CANCEL_REASON_TEXT = {
    pointercancel: '触摸被取消（pointercancel）',
    blur: '窗口失焦，触摸被取消',
    leave: '指针离开画面，触摸被取消',
    'link-lost': '投屏链路丢失',
  }

  function cancelGesture(reason) {
    if (phase.value !== 'recording') return
    try { controller.cancelCurrent(CANCEL_REASON_TEXT[reason] || reason) } catch { /* 容错 */ }
    gestureDims = null
  }

  /** Alt 模板/取色添加（录制中走锁定锚点保序，不进上传流程，deliverable 6/§11.8）。 */
  function altAdd(step, label) {
    if (phase.value !== 'recording' || !step) return false
    const anchor = nextAnchor()
    let ok = false
    try {
      ok = shell.insertStepWithAnchor(step, label || 'Alt 插入步骤', anchor)
    } catch (e) {
      notify(`Alt 插入失败：${e.message}`, 'error')
      return false
    }
    if (!ok) return false
    insertCount += 1
    try { controller.onAltAdd(step) } catch (e) { notify(`Alt 登记失败：${e.message}`, 'warn') }
    return true
  }

  // ---- 启动 / 停止（deliverable 1/8）----
  const available = computed(() => {
    const pkg = activePkg && 'value' in activePkg ? activePkg.value : activePkg
    const conn = connected && 'value' in connected ? connected.value : connected
    const v = videoElement && 'value' in videoElement ? videoElement.value : videoElement
    const videoOk = hasVideo ? !!hasVideo() : !!(v && v.videoWidth > 0)
    return !!(shell.hasModel && shell.kind === 'script' && pkg && conn && videoOk)
  })

  const unavailableReason = computed(() => {
    if (shell.hasModel && shell.kind !== 'script') return '函数库不支持录制'
    if (!shell.hasModel) return '请先进入脚本编辑态'
    if (!(activePkg && 'value' in activePkg ? activePkg.value : activePkg)) return '请先选择应用分区'
    if (!(connected && 'value' in connected ? connected.value : connected)) return '请先连接设备'
    return '设备画面不可用'
  })

  const buttonTitle = computed(() => {
    if (phase.value === 'stopping') return '正在停止并处理录制队列…'
    if (phase.value !== 'idle') return '停止录制并处理队列'
    if (!available.value) return `不可录制：${unavailableReason.value}`
    return '录制点击与滑动为可视化步骤（触控透传，不阻塞设备操作）'
  })

  function start() {
    if (phase.value !== 'idle') return false
    if (!available.value) {
      notify(`不可录制：${unavailableReason.value}`, 'warn')
      return false
    }
    // 锁定插入目标（默认主流程末尾；有选中则其后），锚点快照随 start() 传给控制器
    const model = shell.model
    const sel = shell.selectedUuid
    const loc = sel ? findStepLocation(model, sel) : null
    const containerPath = loc ? loc.containerPath : ['steps']
    let startIdx = 0
    try {
      startIdx = loc ? loc.index + 1 : resolveStepList(model, containerPath).length
    } catch { startIdx = 0 }
    lockedAnchor = { containerPath, index: startIdx }
    insertCount = 0
    targetLabel.value = describeTarget(model, sel)
    lockPkg.value = activePkg && 'value' in activePkg ? activePkg.value : activePkg
    multiTouchToasted = false
    usedNames.clear()
    controller.start({ targetLabel: targetLabel.value, anchor: lockedAnchor })
    phase.value = controller.state
    installWindowListeners()
    notify(`开始录制：${targetLabel.value}`, 'info')
    return true
  }

  /**
   * 停止录制：待裁切草稿按 A + 自动搜索区定稿（plan §11.3）→ 控制器 stopping →
   * 队列排空（失败条目阻塞到用户重试/降级/丢弃）→ idle。phase 迁移由 onChange 镜像。
   */
  function stop() {
    if (phase.value !== 'recording') return Promise.resolve()
    autoFinalizeUnsettled()
    removeWindowListeners()
    gestureDims = null
    return controller.stop().catch((e) => {
      notify(`停止录制异常：${e?.message || e}`, 'warn')
    })
  }

  function toggle() {
    if (phase.value === 'idle') start()
    else if (phase.value === 'recording') void stop()
  }

  /** 投屏链路丢失（viewer 断开/被接管）：取消手势并停止录制，队列继续处理。 */
  function onLinkLost() {
    if (phase.value === 'idle') return
    try { controller.cancelCurrent('投屏链路丢失') } catch { /* 容错 */ }
    if (phase.value === 'recording') {
      notify('投屏已断开，录制已停止（草稿继续处理）', 'warn')
      void stop()
    }
  }

  // ---- 派生（状态栏 / 离开保护 / 面板）----
  const active = computed(() => phase.value === 'recording')
  const pendingCount = computed(() => {
    void queueTick.value
    return controller.queue.pendingCount()
  })
  const failedCount = computed(() => {
    void queueTick.value
    return controller.queue.list().filter(e => e.state === 'failed').length
  })
  const uploading = computed(() => {
    void queueTick.value
    return controller.queue.list().some(e => e.state === 'uploading')
  })
  /** 离开/保存保护：录制或停止中、或有未完成草稿（含失败待处理）都视为忙。 */
  const busy = computed(() => phase.value !== 'idle' || pendingCount.value > 0)
  /** 队列条目视图（新对象：queueTick 变化时引用更新，模板/computed 可追踪状态迁移）。 */
  function entryView(e) {
    return { uuid: e.uuid, seq: e.seq, kind: e.kind, state: e.state, failure: e.failure, downgraded: e.downgraded, draft: e.draft }
  }

  /** 二次裁切面板草稿：最早的待裁切（pending/cropping 有草稿），其次最早的失败草稿。 */
  const panelDraft = computed(() => {
    void queueTick.value
    const list = controller.queue.list()
    const picked = list.find(e => (e.state === 'pending' || e.state === 'cropping') && e.draft)
      || list.find(e => e.state === 'failed')
    return picked ? entryView(picked) : null
  })
  /** 面板展示/操作条目（时间线视图：uuid + 状态 + 草稿 + 失败原因，按 seq 有序）。 */
  const timeline = computed(() => {
    void queueTick.value
    return controller.queue.list().map(entryView)
  })

  /** 面板用：当前搜索区域（未调整 = A 外扩自动框；已调整 = union(A,M)+25px 裁剪）。 */
  function computeSearchRect(entry, mRect, adjusted) {
    const d = entry && entry.draft
    if (!d) return null
    return adjusted && mRect
      ? searchRectManual(d.aRect, mRect, d.frameW, d.frameH)
      : searchRectAuto(d.aRect, d.frameW, d.frameH)
  }

  return reactive({
    phase, targetLabel, lockPkg, active, busy, uploading,
    pendingCount, failedCount, panelDraft, timeline, available, unavailableReason, buttonTitle,
    start, stop, toggle, onLinkLost,
    onPointerDown, onPointerMove, onPointerUp, onPointerCancel, cancelGesture,
    altAdd, confirmCrop, downgrade, discard, retry,
    shortNameTaken, nextShortName, regionOf, computeSearchRect,
    buildFindStep, buildSwipeMatchStep, buildTapStep, buildAltSwipeStep, buildColorStep,
    PROJECTION_ALT_HINT,
  })
}
