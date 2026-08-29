import { describe, expect, it, vi } from 'vitest'
import { RecordingQueue } from '../queue'

/**
 * 录制顺序队列（plan §11.3，§16.3 顺序/失败/排空项）：
 * seq 单调保序、状态机迁移校验、失败重试/丢弃/坐标降级、
 * Alt/取色占位保序、flush 排空、50 次混合操作随机完成顺序下步骤顺序 100% 一致。
 */

const DRAFT = {
  frameDataUrl: 'data:image/png;base64,AAAA',
  frameW: 1080,
  frameH: 1920,
  aRect: { x: 515, y: 935, w: 50, h: 50 },
  searchRect: { x: 490, y: 910, w: 100, h: 100 },
  relX: 0.5,
  relY: 0.5,
}

const stepOf = (name) => ({ uuid: `s-${name}`, kind: 'find', template: { lit: `${name}.png` } })

describe('recording/queue：reserve 与 seq', () => {
  it('自动分配 seq 从 1 起全局单调', () => {
    const q = new RecordingQueue()
    expect(q.reserve({ uuid: 'a', kind: 'click' }).seq).toBe(1)
    expect(q.reserve({ uuid: 'b', kind: 'swipe' }).seq).toBe(2)
    expect(q.reserve({ uuid: 'c', kind: 'alt' }).seq).toBe(3)
  })

  it('显式 atSeq 生效并推高后续自动分配', () => {
    const q = new RecordingQueue()
    expect(q.reserve({ uuid: 'a', kind: 'click', atSeq: 10 }).seq).toBe(10)
    expect(q.reserve({ uuid: 'b', kind: 'click' }).seq).toBe(11)
    expect(q.reserve({ uuid: 'c', kind: 'click', atSeq: 5 }).seq).toBe(5) // 显式低值允许，但不回退计数
    expect(q.reserve({ uuid: 'd', kind: 'click' }).seq).toBe(12)
  })

  it('uuid 重复 / 缺 uuid / 缺 kind 报错', () => {
    const q = new RecordingQueue()
    q.reserve({ uuid: 'a', kind: 'click' })
    expect(() => q.reserve({ uuid: 'a', kind: 'swipe' })).toThrow(/重复/)
    expect(() => q.reserve({ kind: 'click' })).toThrow(/uuid/)
    expect(() => q.reserve({ uuid: 'x' })).toThrow(/kind/)
  })

  it('get / list 按 seq 排序返回', () => {
    const q = new RecordingQueue()
    q.reserve({ uuid: 'a', kind: 'click', atSeq: 2 })
    q.reserve({ uuid: 'b', kind: 'click', atSeq: 1 })
    expect(q.list().map((e) => e.uuid)).toEqual(['b', 'a'])
    expect(q.get('a').kind).toBe('click')
    expect(q.get('zzz')).toBeUndefined()
  })
})

describe('recording/queue：状态机', () => {
  it('正常链路 pending → uploading → cropping → ready', () => {
    const q = new RecordingQueue()
    const e = q.reserve({ uuid: 'a', kind: 'click' })
    q.attachDraft('a', DRAFT)
    expect(e.state).toBe('pending')
    q.setUploading('a')
    expect(e.state).toBe('uploading')
    q.setCropping('a')
    expect(e.state).toBe('cropping')
    q.setReady('a', stepOf('a'))
    expect(e.state).toBe('ready')
    expect(e.step.kind).toBe('find')
    expect(e.draft).toEqual(DRAFT)
  })

  it('setReady 可从 pending / uploading / failed 直达', () => {
    const q = new RecordingQueue()
    q.reserve({ uuid: 'a', kind: 'click' })
    q.setReady('a', stepOf('a'))
    q.reserve({ uuid: 'b', kind: 'click' })
    q.setUploading('b')
    q.setReady('b', stepOf('b'))
    q.reserve({ uuid: 'c', kind: 'click' })
    q.setFailed('c', { reason: 'x' })
    q.setReady('c', stepOf('c'))
    expect(q.pendingCount()).toBe(0)
  })

  it('终态后再变更抛错（ready/discarded 不可逆）', () => {
    const q = new RecordingQueue()
    q.reserve({ uuid: 'a', kind: 'click' })
    q.setReady('a', stepOf('a'))
    expect(() => q.setUploading('a')).toThrow(/非法状态迁移/)
    expect(() => q.setReady('a', stepOf('a'))).toThrow(/非法状态迁移/)
    expect(() => q.attachDraft('a', DRAFT)).toThrow(/终态/)
    expect(() => q.downgradeToTap('a', { relX: 0, relY: 0 })).toThrow(/终态/)
    q.reserve({ uuid: 'b', kind: 'click' })
    q.discard('b')
    expect(() => q.retry('b')).toThrow(/failed/)
    expect(() => q.discard('b')).toThrow(/非法状态迁移/)
  })

  it('未知 uuid 操作抛错', () => {
    const q = new RecordingQueue()
    expect(() => q.attachDraft('nope', DRAFT)).toThrow(/没有条目/)
    expect(() => q.setUploading('nope')).toThrow(/没有条目/)
    expect(() => q.setReady('nope', {})).toThrow(/没有条目/)
    expect(() => q.retry('nope')).toThrow(/没有条目/)
  })

  it('retry：failed → pending，清空失败原因后可再次上传', () => {
    const q = new RecordingQueue()
    q.reserve({ uuid: 'a', kind: 'click' })
    q.attachDraft('a', DRAFT)
    q.setUploading('a')
    q.setFailed('a', { reason: '网络错误' })
    expect(q.get('a').state).toBe('failed')
    expect(q.get('a').failure).toBe('网络错误')
    q.retry('a')
    expect(q.get('a').state).toBe('pending')
    expect(q.get('a').failure).toBeNull()
    q.setUploading('a')
    q.setReady('a', stepOf('a'))
    expect(q.isSettled()).toBe(true)
  })

  it('discard：终态，保留草稿与失败原因供时间线展示', () => {
    const q = new RecordingQueue()
    q.reserve({ uuid: 'a', kind: 'swipe' })
    q.attachDraft('a', DRAFT)
    q.setFailed('a', { reason: '模板过纯' })
    q.discard('a')
    const e = q.get('a')
    expect(e.state).toBe('discarded')
    expect(e.failure).toBe('模板过纯')
    expect(e.draft).toEqual(DRAFT)
  })
})

describe('recording/queue：坐标降级 downgradeToTap', () => {
  it('生成 tap Step（编辑器 Step 形态）并置 ready', () => {
    const q = new RecordingQueue()
    q.reserve({ uuid: 'a', kind: 'click' })
    q.attachDraft('a', { ...DRAFT, relX: 0.25, relY: 0.75 })
    q.setFailed('a', { reason: '模板低纹理' })
    const e = q.downgradeToTap('a', { relX: 0.5, relY: 0.8 })
    expect(e.state).toBe('ready')
    expect(e.downgraded).toBe(true)
    expect(e.step.kind).toBe('tap')
    expect(e.step.at).toEqual({ lit: [0.5, 0.8] })
    expect(typeof e.step.uuid).toBe('string')
  })

  it('坐标缺省回退草稿起点', () => {
    const q = new RecordingQueue()
    q.reserve({ uuid: 'a', kind: 'click' })
    q.attachDraft('a', { ...DRAFT, relX: 0.25, relY: 0.75 })
    const e = q.downgradeToTap('a')
    expect(e.step.at).toEqual({ lit: [0.25, 0.75] })
  })

  it('无草稿且无坐标报错', () => {
    const q = new RecordingQueue()
    q.reserve({ uuid: 'a', kind: 'click' })
    expect(() => q.downgradeToTap('a')).toThrow(/缺少坐标/)
  })
})

describe('recording/queue：Alt/取色占位保序', () => {
  it('alt/color 不经历上传，直接 setReady 占位', () => {
    const q = new RecordingQueue()
    const a = q.reserve({ uuid: 'a', kind: 'click' })
    q.attachDraft('a', DRAFT)
    const alt = q.reserve({ uuid: 'tpl-1', kind: 'alt' })
    expect(alt.state).toBe('pending') // 由调用方立即 setReady
    const altStep = { uuid: 'tpl-1', kind: 'find', template: { lit: 'x.png' } }
    q.setReady('tpl-1', altStep)
    const color = q.reserve({ uuid: 'color-1', kind: 'color' })
    const colorStep = { uuid: 'color-1', kind: 'color' }
    q.setReady('color-1', colorStep)
    // 手势后完成：顺序仍按 seq 交错
    q.setUploading('a')
    q.setReady('a', stepOf('a'))
    expect([a.seq, alt.seq, color.seq]).toEqual([1, 2, 3])
    expect(q.orderedSteps()).toEqual([stepOf('a'), altStep, colorStep])
  })
})

describe('recording/queue：flush 排空', () => {
  it('空队列立即 resolve', async () => {
    await new RecordingQueue().flush()
  })

  it('有未完成条目时挂起，全部终态后释放；多个等待者同时释放', async () => {
    const q = new RecordingQueue()
    q.reserve({ uuid: 'a', kind: 'click' })
    q.reserve({ uuid: 'b', kind: 'swipe' })
    let done1 = false
    let done2 = false
    const p1 = q.flush().then(() => {
      done1 = true
    })
    const p2 = q.flush().then(() => {
      done2 = true
    })
    expect(done1).toBe(false)
    q.setReady('a', stepOf('a'))
    expect(done1).toBe(false) // 还剩 b 未终态
    q.discard('b')
    await p1
    await p2
    expect(done1).toBe(true)
    expect(done2).toBe(true)
    expect(q.isSettled()).toBe(true)
    await q.flush() // 已排空：立即 resolve
  })

  it('onSettled 每条目终态回调一次；onUpdate 每次变更回调', () => {
    const onSettled = vi.fn()
    const onUpdate = vi.fn()
    const q = new RecordingQueue({ onSettled, onUpdate })
    q.reserve({ uuid: 'a', kind: 'click' }) // onUpdate 1
    q.attachDraft('a', DRAFT) // onUpdate 2
    q.setUploading('a') // onUpdate 3
    q.setReady('a', stepOf('a')) // onUpdate 4 + onSettled
    expect(onSettled).toHaveBeenCalledTimes(1)
    expect(onSettled).toHaveBeenCalledWith(q.get('a'))
    q.reserve({ uuid: 'b', kind: 'alt' }) // onUpdate 5
    q.setReady('b', stepOf('b')) // onUpdate 6 + onSettled
    expect(onSettled).toHaveBeenCalledTimes(2)
    expect(onUpdate).toHaveBeenCalledTimes(6)
  })
})

describe('recording/queue：50 次混合操作——随机完成顺序下顺序与 seq 100% 一致', () => {
  it('50 条混合 click/swipe/alt/color，乱序完成（含失败重试/降级/丢弃），orderedSteps 严格按占位顺序', async () => {
    // 可复现伪随机（LCG）
    let seed = 20260829
    const rnd = () => {
      seed = (seed * 1103515245 + 12345) % 2147483648
      return seed / 2147483648
    }
    const onSettled = vi.fn()
    const q = new RecordingQueue({ onSettled })
    const kinds = ['click', 'swipe', 'alt', 'color']

    // 1) 依序占位 50 条（click/swipe 附草稿）
    const made = []
    for (let i = 0; i < 50; i++) {
      const kind = kinds[Math.floor(rnd() * kinds.length)]
      const e = q.reserve({ uuid: `u${i}`, kind })
      made.push(e)
      if (kind === 'click' || kind === 'swipe') q.attachDraft(`u${i}`, { ...DRAFT, relY: 0.5 })
    }
    // seq 严格递增且与占位顺序一致
    for (let i = 0; i < 50; i++) expect(made[i].seq).toBe(i + 1)

    // 2) Fisher–Yates 乱序完成
    const order = [...made]
    for (let i = order.length - 1; i > 0; i--) {
      const j = Math.floor(rnd() * (i + 1))
      ;[order[i], order[j]] = [order[j], order[i]]
    }

    // 期望输出：必须按「占位顺序」（seq）推导，与完成顺序无关——这正是队列要保证的
    const finalStep = new Map() // uuid → 终态步骤（丢弃的条目没有）
    const expected = []

    for (const e of order) {
      if (e.kind === 'alt' || e.kind === 'color') {
        const s = { uuid: e.uuid, kind: 'find', template: { lit: `${e.uuid}.png` } }
        q.setReady(e.uuid, s)
        finalStep.set(e.uuid, s)
        continue
      }
      const roll = rnd()
      if (roll < 0.55) {
        const s = { uuid: e.uuid, kind: 'find', template: { lit: `${e.uuid}.png` } }
        q.setUploading(e.uuid)
        q.setCropping(e.uuid)
        q.setReady(e.uuid, s)
        finalStep.set(e.uuid, s)
      } else if (roll < 0.75) {
        // 失败 → 重试 → 上传 → 完成
        q.setUploading(e.uuid)
        q.setFailed(e.uuid, { reason: 'upload-fail' })
        q.retry(e.uuid)
        const s = { uuid: e.uuid, kind: 'find', template: { lit: `${e.uuid}.png` } }
        q.setUploading(e.uuid)
        q.setReady(e.uuid, s)
        finalStep.set(e.uuid, s)
      } else if (roll < 0.9) {
        // 失败 → 坐标降级（tap）
        q.setFailed(e.uuid, { reason: 'low-texture' })
        const entry = q.downgradeToTap(e.uuid, { relX: 0.1, relY: 0.2 })
        finalStep.set(e.uuid, entry.step)
      } else {
        // 失败 → 丢弃（不产出步骤）
        q.setFailed(e.uuid, { reason: 'user-discard' })
        q.discard(e.uuid)
      }
    }

    // 3) 断言：终态全部 settled，onSettled 每条目一次，顺序 100% 一致
    expect(q.pendingCount()).toBe(0)
    expect(q.isSettled()).toBe(true)
    expect(onSettled).toHaveBeenCalledTimes(50)
    for (const e of made) {
      const s = finalStep.get(e.uuid)
      if (s) expected.push(s) // 按占位顺序收集非丢弃条目的步骤
    }
    const out = q.orderedSteps()
    expect(out).toHaveLength(expected.length)
    for (let i = 0; i < expected.length; i++) {
      expect(out[i]).toBe(expected[i]) // 对象同一性：逐位一致
    }
    // 降级条目都是 tap，正常条目都是 find
    for (const e of q.list()) {
      if (e.downgraded) expect(e.step.kind).toBe('tap')
    }
    await q.flush()
  })
})
