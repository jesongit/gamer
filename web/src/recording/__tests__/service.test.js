import { describe, expect, it, vi } from 'vitest'
import { createRecordingController } from '../service'

/**
 * 录制控制器（plan §11.1–§11.5、§11.8；§16.3 纯逻辑项）：
 * freezeFrame 先于 sendTouch down 的顺序断言、点击/滑动/长按分类语义、
 * pointercancel 清理、Alt 占位保序、停止排空、失败不静默、状态机。
 */

/** 统一 harness：记录依赖调用顺序，时钟可推进。日期固定为本地 2026-08-29。 */
function makeHarness(overrides = {}) {
  const calls = []
  const T0 = new Date(2026, 7, 29, 12, 0, 0).getTime()
  let t = T0
  const deps = {
    insert: (kind, anchorInfo) => {
      calls.push({ op: 'insert', kind, anchorInfo })
      return `ph-${calls.filter((c) => c.op === 'insert').length}`
    },
    buildStep: (kind, payload) => {
      calls.push({ op: 'buildStep', kind, payload })
      return { uuid: `built-${calls.length}`, kind: kind === 'click' ? 'find' : 'match', payload }
    },
    freezeFrame: () => {
      calls.push({ op: 'freeze' })
      return { dataUrl: 'data:image/png;base64,AAAA', width: 1080, height: 1920 }
    },
    sendTouch: (event, payload) => calls.push({ op: 'touch', event, payload }),
    now: () => t,
  }
  const controller = createRecordingController({ ...deps, ...overrides })
  return {
    controller,
    calls,
    queue: controller.queue,
    ops: () => calls.map((c) => c.op),
    touches: () => calls.filter((c) => c.op === 'touch'),
    advance: (ms) => {
      t += ms
    },
    T0,
  }
}

const DOWN = { relX: 0.5, relY: 0.5, frameW: 1080, frameH: 1920 }

describe('recording/service：状态机与订阅', () => {
  it('idle → recording → stopping → idle；非法迁移抛错', async () => {
    const h = makeHarness()
    expect(h.controller.state).toBe('idle')
    h.controller.start({ targetLabel: '主流程末尾' })
    expect(h.controller.state).toBe('recording')
    expect(() => h.controller.start({})).toThrow(/当前状态/)
    const p = h.controller.stop()
    expect(h.controller.state).toBe('stopping')
    await p
    expect(h.controller.state).toBe('idle')
    // idle 下 stop 直接 resolve、flushAndFinish 可用
    await h.controller.stop()
    await h.controller.flushAndFinish()
    expect(h.controller.state).toBe('idle')
  })

  it('onChange 广播状态迁移与待处理数；退订生效', async () => {
    const h = makeHarness()
    const snapshots = []
    const off = h.controller.onChange((s) => snapshots.push(s))
    h.controller.start({ targetLabel: '主流程末尾' })
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerUp(DOWN)
    expect(snapshots.map((s) => s.state)).toContain('recording')
    expect(snapshots.some((s) => s.hasActiveGesture === true)).toBe(true)
    expect(snapshots.some((s) => s.pendingCount === 1)).toBe(true)
    expect(h.controller.targetLabel).toBe('主流程末尾')
    off()
    const stopPromise = h.controller.stop()
    h.controller.completeDraft('ph-1', {})
    await stopPromise
    expect(snapshots.every((s) => s.state !== 'idle')).toBe(true) // 退订后不再收到
  })

  it('stop() 期间重复调用返回同一 Promise；stopping 中指针事件被忽略', async () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerUp(DOWN)
    const p1 = h.controller.stop()
    const p2 = h.controller.stop()
    expect(p2).toBe(p1)
    const before = h.calls.length
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerMove(DOWN)
    h.controller.onPointerUp(DOWN)
    expect(h.calls.length).toBe(before) // 无新增依赖调用
    h.controller.completeDraft('ph-1', {})
    await p1
    expect(h.controller.state).toBe('idle')
  })

  it('idle 状态下指针事件完全 no-op', () => {
    const h = makeHarness()
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerUp(DOWN)
    h.controller.onAltAdd({ uuid: 'x', kind: 'find' })
    expect(h.calls).toEqual([])
    expect(h.queue.list()).toEqual([])
  })
})

describe('recording/service：核心时序——先冻结帧再透传触摸', () => {
  it('onPointerDown 顺序：freeze → touch(down)，触摸不等编码上传', () => {
    const h = makeHarness()
    h.controller.start({ targetLabel: '主流程末尾' })
    h.controller.onPointerDown(DOWN)
    expect(h.ops()).toEqual(['freeze', 'touch'])
    expect(h.touches()[0].event).toBe('down')
    expect(h.touches()[0].payload).toEqual({ relX: 0.5, relY: 0.5 })
  })

  it('冻结抛错/返回 null/返回空对象都不阻断触摸透传', () => {
    for (const bad of [
      () => {
        throw new Error('no frame')
      },
      () => null,
      () => ({}),
    ]) {
      const h = makeHarness({ freezeFrame: bad })
      h.controller.start({})
      h.controller.onPointerDown(DOWN)
      expect(h.touches().map((c) => c.event)).toEqual(['down']) // 触摸已透传
      h.controller.onPointerUp(DOWN)
      const entry = h.queue.list()[0]
      expect(entry.state).toBe('failed')
      expect(entry.failure).toContain('画面不可用')
    }
  })

  it('按下后立即透传 move（滑动过程持续发设备），up 立即透传且先于占位', () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerMove({ relX: 0.51, relY: 0.5, frameW: 1080, frameH: 1920 })
    h.controller.onPointerMove({ relX: 0.52, relY: 0.5, frameW: 1080, frameH: 1920 })
    h.controller.onPointerUp(DOWN)
    expect(h.touches().map((c) => c.event)).toEqual(['down', 'move', 'move', 'up'])
    // 占位发生在最后一次触摸之后（透传优先，不等待任何处理）
    expect(h.ops().indexOf('insert')).toBeGreaterThan(h.ops().lastIndexOf('touch'))
  })

  it('非法输入抛错（相对坐标/帧尺寸）', () => {
    const h = makeHarness()
    h.controller.start({})
    expect(() => h.controller.onPointerDown({ relX: NaN, relY: 0.5, frameW: 1080, frameH: 1920 })).toThrow(/relX/)
    expect(() => h.controller.onPointerDown({ relX: 0.5, relY: 0.5, frameW: 0, frameH: 1920 })).toThrow(/帧尺寸/)
    // move 校验需要先有活动手势
    h.controller.onPointerDown(DOWN)
    expect(() => h.controller.onPointerMove({ relX: Infinity, relY: 0 })).toThrow(/relX/)
  })
})

describe('recording/service：点击录制（find 语义）', () => {
  it('click → 占位 + 草稿（A/搜索区域/相对坐标/短名/durationMs）', () => {
    const h = makeHarness()
    h.controller.start({ targetLabel: '主流程末尾', anchor: { containerPath: ['steps'], index: 3 } })
    h.controller.onPointerDown(DOWN)
    h.advance(120)
    h.controller.onPointerUp(DOWN)
    expect(h.controller.state).toBe('recording') // 录制不因单次点击结束
    // 触摸 up 先于 insert（透传优先）
    expect(h.ops()).toEqual(['freeze', 'touch', 'touch', 'insert'])
    // insert 收到 kind 与锚点快照
    expect(h.calls[3].kind).toBe('click')
    expect(h.calls[3].anchorInfo).toEqual({
      targetLabel: '主流程末尾',
      anchor: { containerPath: ['steps'], index: 3 },
    })
    // 队列条目与草稿
    const e = h.queue.get('ph-1')
    expect(e.kind).toBe('click')
    expect(e.state).toBe('pending')
    expect(e.seq).toBe(1)
    expect(e.draft.frameDataUrl).toBe('data:image/png;base64,AAAA')
    expect(e.draft.frameW).toBe(1080)
    expect(e.draft.frameH).toBe(1920)
    expect(e.draft.aRect).toEqual({ x: 515, y: 935, w: 50, h: 50 })
    expect(e.draft.searchRect).toEqual({ x: 490, y: 910, w: 100, h: 100 })
    expect(e.draft.relX).toBe(0.5)
    expect(e.draft.relY).toBe(0.5)
    expect(e.draft.relEnd).toEqual([0.5, 0.5])
    expect(e.draft.durationMs).toBe(120)
    expect(e.draft.shortName).toBe('record_click_20260829_001.png')
  })

  it('completeDraft 经注入 buildStep 组装并置 ready；orderedSteps 输出', () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerUp(DOWN)
    const step = h.controller.completeDraft('ph-1', { template: 'record_click_20260829_001.png' })
    expect(h.calls.find((c) => c.op === 'buildStep').kind).toBe('click')
    const payload = h.calls.find((c) => c.op === 'buildStep').payload
    expect(payload.template).toBe('record_click_20260829_001.png')
    expect(payload.uuid).toBe('ph-1')
    expect(payload.draft.aRect).toEqual({ x: 515, y: 935, w: 50, h: 50 })
    expect(step.kind).toBe('find')
    expect(h.queue.get('ph-1').state).toBe('ready')
    expect(h.queue.orderedSteps()).toEqual([step])
  })

  it('默认短名按 kind+日期独立计数：click 001→002，swipe 独立 001', () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerUp(DOWN)
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerUp(DOWN)
    h.controller.onPointerDown({ relX: 0.2, relY: 0.5, frameW: 1080, frameH: 1920 })
    h.advance(400)
    h.controller.onPointerUp({ relX: 0.8, relY: 0.5, frameW: 1080, frameH: 1920 })
    const names = h.queue.list().map((e) => e.draft.shortName)
    expect(names).toEqual([
      'record_click_20260829_001.png',
      'record_click_20260829_002.png',
      'record_swipe_20260829_001.png',
    ])
  })
})

describe('recording/service：滑动录制（match→swipe 语义）', () => {
  it('超阈值位移 → swipe：记录 durationMs 与终点，A 取滑动起点', () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown({ relX: 0.2, relY: 0.5, frameW: 1080, frameH: 1920 })
    h.controller.onPointerMove({ relX: 0.5, relY: 0.5, frameW: 1080, frameH: 1920 })
    h.advance(800)
    h.controller.onPointerUp({ relX: 0.8, relY: 0.5, frameW: 1080, frameH: 1920 })
    const e = h.queue.get('ph-1')
    expect(e.kind).toBe('swipe')
    expect(e.state).toBe('pending')
    expect(e.draft.durationMs).toBe(800)
    expect(e.draft.relX).toBe(0.2)
    expect(e.draft.relEnd).toEqual([0.8, 0.5])
    // A 以按下点 (216, 960) 为中心
    expect(e.draft.aRect).toEqual({ x: 191, y: 935, w: 50, h: 50 })
    expect(e.draft.shortName).toBe('record_swipe_20260829_001.png')
    // 只截一帧（按下时），移动不反复截图
    expect(h.ops().filter((op) => op === 'freeze')).toHaveLength(1)
  })
})

describe('recording/service：长按与取消（不静默漏步）', () => {
  it('阈值内超 600ms → longpress 失败草稿，不转成点击，可降级为 tap', () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    h.advance(700)
    h.controller.onPointerUp(DOWN)
    const e = h.queue.get('ph-1')
    expect(e.kind).toBe('click')
    expect(e.state).toBe('failed')
    expect(e.failure).toContain('长按')
    expect(e.draft).not.toBeNull() // 冻结帧保留，供人工处置
    expect(h.queue.orderedSteps()).toEqual([])
    // 用户选择降级为坐标点击
    const entry = h.queue.downgradeToTap('ph-1')
    expect(entry.step.kind).toBe('tap')
    expect(entry.step.at).toEqual({ lit: [0.5, 0.5] })
    expect(h.queue.orderedSteps()).toEqual([entry.step])
  })

  it('onPointerCancel：补发 UP 释放设备 + 失败草稿（用最后轨迹点）', () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerMove({ relX: 0.52, relY: 0.5, frameW: 1080, frameH: 1920 })
    h.controller.onPointerCancel()
    const ups = h.touches().filter((c) => c.event === 'up')
    expect(ups).toHaveLength(1)
    expect(ups[0].payload).toEqual({ relX: 0.52, relY: 0.5 })
    const e = h.queue.list()[0]
    expect(e.state).toBe('failed')
    expect(e.failure).toContain('取消')
    expect(h.controller.state).toBe('recording') // 录制继续，可进行下一次手势
    // 无活动手势时重复取消为 no-op
    const before = h.calls.length
    h.controller.cancelCurrent('again')
    h.controller.onPointerCancel()
    expect(h.calls.length).toBe(before)
  })

  it('cancelCurrent 显式原因透传给失败草稿', () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    h.controller.cancelCurrent('插入锚点被删除')
    expect(h.queue.list()[0].failure).toBe('插入锚点被删除')
  })
})

describe('recording/service：Alt 添加占位保序', () => {
  it('find/color 步骤即时 ready，占位 seq 与手势交错', () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerUp(DOWN) // seq1 click
    const altStep = { uuid: 'tpl-1', kind: 'find', template: { lit: 'confirm.png' } }
    h.controller.onAltAdd(altStep) // seq2 alt
    h.controller.onPointerDown({ relX: 0.2, relY: 0.5, frameW: 1080, frameH: 1920 })
    h.advance(300)
    h.controller.onPointerUp({ relX: 0.8, relY: 0.5, frameW: 1080, frameH: 1920 }) // seq3 swipe
    const colorStep = { uuid: 'color-1', kind: 'color', at: { lit: [0.5, 0.5] } }
    h.controller.onAltAdd(colorStep) // seq4 color
    const [c, a, s, col] = h.queue.list()
    expect(c.seq).toBe(1)
    expect(a.kind).toBe('alt')
    expect(a.state).toBe('ready')
    expect(a.step).toBe(altStep)
    expect(s.kind).toBe('swipe')
    expect(col.kind).toBe('color')
    expect(col.step).toBe(colorStep)
    // 手势条目完成后：顺序完全按占位序
    h.controller.completeDraft('ph-1', {})
    h.controller.completeDraft('ph-2', {})
    expect(h.queue.orderedSteps()).toHaveLength(4)
    expect(h.queue.list().map((e) => e.uuid)).toEqual(['ph-1', 'tpl-1', 'ph-2', 'color-1'])
  })

  it('onAltAdd 非法步骤抛错', () => {
    const h = makeHarness()
    h.controller.start({})
    expect(() => h.controller.onAltAdd({ kind: 'find' })).toThrow(/uuid/)
    expect(() => h.controller.onAltAdd(null)).toThrow(/uuid/)
  })
})

describe('recording/service：停止与排空', () => {
  it('stop 等待未完成条目排空后才回到 idle', async () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerUp(DOWN)
    let settled = false
    const p = h.controller.stop().then(() => {
      settled = true
    })
    expect(h.controller.state).toBe('stopping')
    await Promise.resolve()
    expect(settled).toBe(false) // 排空中
    h.controller.completeDraft('ph-1', {})
    await p
    expect(settled).toBe(true)
    expect(h.controller.state).toBe('idle')
  })

  it('手势进行中 stop：先取消（补 UP + 失败草稿），失败条目阻塞排空直到处置', async () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    let settled = false
    const p = h.controller.stop().then(() => {
      settled = true
    })
    expect(h.touches().at(-1).event).toBe('up') // 设备已释放
    const e = h.queue.list()[0]
    expect(e.state).toBe('failed')
    expect(e.failure).toContain('停止录制')
    await Promise.resolve()
    expect(settled).toBe(false)
    h.queue.downgradeToTap(e.uuid, { relX: 0.5, relY: 0.5 })
    await p
    expect(settled).toBe(true)
    expect(h.controller.state).toBe('idle')
    expect(h.queue.orderedSteps()[0].kind).toBe('tap')
  })

  it('flushAndFinish 是 stop 的宽容别名（recording 下等效）', async () => {
    const h = makeHarness()
    h.controller.start({})
    await h.controller.flushAndFinish()
    expect(h.controller.state).toBe('idle')
  })
})

describe('recording/service：冻结帧尺寸优先与多指', () => {
  it('模板数学以冻结帧 width/height 为基准（§11.6），事件尺寸仅兜底', () => {
    const h = makeHarness({
      freezeFrame: () => ({ dataUrl: 'data:image/png;base64,BBBB', width: 2160, height: 3840 }),
    })
    h.controller.start({})
    h.controller.onPointerDown({ relX: 0.5, relY: 0.5, frameW: 1080, frameH: 1920 })
    h.controller.onPointerUp({ relX: 0.5, relY: 0.5, frameW: 1080, frameH: 1920 })
    const d = h.queue.get('ph-1').draft
    expect(d.frameW).toBe(2160)
    expect(d.frameH).toBe(3840)
    // 中心 (1080, 1920)：A = {1055,1895,50,50}
    expect(d.aRect).toEqual({ x: 1055, y: 1895, w: 50, h: 50 })
    expect(d.searchRect).toEqual({ x: 1030, y: 1870, w: 100, h: 100 })
  })

  it('第二指按下被忽略（多指路由由接线方处理），首指手势正常完成', () => {
    const h = makeHarness()
    h.controller.start({})
    h.controller.onPointerDown(DOWN)
    h.controller.onPointerDown({ relX: 0.9, relY: 0.9, frameW: 1080, frameH: 1920 }) // 忽略
    expect(h.calls.filter((c) => c.op === 'freeze')).toHaveLength(1)
    h.controller.onPointerUp({ relX: 0.9, relY: 0.9, frameW: 1080, frameH: 1920 })
    expect(h.queue.list()).toHaveLength(1)
    // 位移按首指起点计算：0.4*1080 > 8 → swipe
    expect(h.queue.get('ph-1').kind).toBe('swipe')
  })
})

describe('recording/service：事件依赖注入校验', () => {
  it('缺少任一依赖抛错', () => {
    expect(() => createRecordingController({})).toThrow(/insert/)
    expect(() => createRecordingController({ insert: () => 'x' })).toThrow(/buildStep/)
    expect(() =>
      createRecordingController({ insert: () => 'x', buildStep: () => ({}), freezeFrame: () => null }),
    ).toThrow(/sendTouch/)
  })

  it('now 缺省用 Date.now（冒烟）', () => {
    const controller = createRecordingController({
      insert: () => 'ph-1',
      buildStep: (kind) => ({ uuid: 's', kind }),
      freezeFrame: () => ({ dataUrl: 'data:', width: 1080, height: 1920 }),
      sendTouch: () => {},
    })
    controller.start({})
    controller.onPointerDown(DOWN)
    controller.onPointerUp(DOWN)
    expect(controller.queue.get('ph-1').draft.shortName).toMatch(/^record_click_\d{8}_001\.png$/)
  })

  it('onChange 返回退订函数（冒烟，重复退订安全）', () => {
    const h = makeHarness()
    h.controller.start({})
    const off = h.controller.onChange(vi.fn())
    off()
    off()
    expect(() => h.controller.stop()).not.toThrow()
  })
})
