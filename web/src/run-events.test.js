import { describe, expect, it } from 'vitest'
import { pushRunEvent, runEventTopIndex, useRunEvents } from './components/console/useRunEvents'

/**
 * 运行可视化事件分发与高亮映射（P12.6 / 契约 §6）：
 * - se 消息按 ev 白名单分发：运行结构事件进 feed/高亮，投屏标记类
 *   （tap/swipe/hit/miss）与未知 ev 返回 false 由 overlay 逻辑处理；
 * - run_start 重置上一轮 feed 与高亮，run_end 复位运行态；
 * - step_start / step_end(ok) / step_end(ok:false) 驱动 activePath/errorPath；
 * - 嵌套路径映射顶层祖先卡片（steps[2].then[1] → 2）。
 */

const se = (ev, extra = {}) => ({ type: 'se', ev, ...extra })

describe('pushRunEvent 分发', () => {
  it('运行结构事件被消费（返回 true），overlay 标记类不消费（返回 false）', () => {
    expect(pushRunEvent(se('step_start', { path: 'steps[0]', desc: 'log x' }))).toBe(true)
    expect(pushRunEvent(se('vision', { template: 'a', found: true, score: 0.9 }))).toBe(true)
    expect(pushRunEvent(se('tap', { x: 1, y: 2 }))).toBe(false)
    expect(pushRunEvent(se('swipe', { x1: 0, y1: 0, x2: 1, y2: 1 }))).toBe(false)
    expect(pushRunEvent(se('hit', { x: 0, y: 0, w: 1, h: 1 }))).toBe(false)
    expect(pushRunEvent(se('miss', { x: 0, y: 0, w: 1, h: 1 }))).toBe(false)
    // 未知 ev 向前兼容：按 overlay 处理
    expect(pushRunEvent(se('future_kind'))).toBe(false)
    // 非 se 信封直接拒绝
    expect(pushRunEvent({ type: 'conflict' })).toBe(false)
    expect(pushRunEvent(null)).toBe(false)
  })

  it('run_start 重置 feed 与高亮；事件按序入列并携带载荷', () => {
    const state = useRunEvents()
    pushRunEvent(se('run_start'))
    pushRunEvent(se('step_start', { path: 'steps[0]', desc: 'log start' }))
    pushRunEvent(se('step_end', { path: 'steps[0]', ok: true }))
    pushRunEvent(se('call_start', { target: 'script:daily', depth: 1 }))
    pushRunEvent(se('budget', { kind: 'STEP_BUDGET_EXCEEDED' }))
    expect(state.list.map((e) => e.ev)).toEqual([
      'run_start', 'step_start', 'step_end', 'call_start', 'budget',
    ])
    expect(state.running).toBe(true)
    const call = state.list.find((e) => e.ev === 'call_start')
    expect(call.target).toBe('script:daily')
    expect(call.depth).toBe(1)
    const budget = state.list.find((e) => e.ev === 'budget')
    expect(budget.kind).toBe('STEP_BUDGET_EXCEEDED')

    // 上一轮遗留 feed 在新 run_start 清空
    pushRunEvent(se('run_start'))
    expect(state.list).toHaveLength(1)
    expect(state.activePath).toBe('')
    expect(state.errorPath).toBe('')
    expect(state.running).toBe(true)
  })

  it('step_start 置高亮、step_end ok 复位、失败标红、run_end 收尾', () => {
    const state = useRunEvents()
    pushRunEvent(se('run_start'))
    pushRunEvent(se('step_start', { path: 'steps[1].then[0]', desc: 'tap' }))
    expect(state.activePath).toBe('steps[1].then[0]')
    pushRunEvent(se('step_end', { path: 'steps[1].then[0]', ok: true }))
    expect(state.activePath).toBe('')
    pushRunEvent(se('step_start', { path: 'steps[2]', desc: 'find x' }))
    pushRunEvent(se('step_end', { path: 'steps[2]', ok: false, error: 'FIND_TIMEOUT: x' }))
    expect(state.activePath).toBe('')
    expect(state.errorPath).toBe('steps[2]')
    pushRunEvent(se('run_end', { ok: false, error: 'FIND_TIMEOUT: x' }))
    expect(state.running).toBe(false)
    expect(state.activePath).toBe('')
  })

  it('feed 容量封顶（最近 MAX_EVENTS 条）', () => {
    const state = useRunEvents()
    pushRunEvent(se('run_start'))
    for (let i = 0; i < 130; i += 1) {
      pushRunEvent(se('vision', { template: `t${i}`, found: false }))
    }
    expect(state.list.length).toBeLessThanOrEqual(120)
    expect(state.list.at(-1).template).toBe('t129')
  })
})

describe('runEventTopIndex 高亮映射', () => {
  it('顶层路径直接映射卡片序号，嵌套路径映射顶层祖先', () => {
    expect(runEventTopIndex('steps[0]')).toBe(0)
    expect(runEventTopIndex('steps[7]')).toBe(7)
    expect(runEventTopIndex('steps[2].then[1]')).toBe(2)
    expect(runEventTopIndex('steps[3].else[0].steps[2]')).toBe(3)
    expect(runEventTopIndex('steps[1].candidates[0].steps[2]')).toBe(1)
  })

  it('非法路径返回 null', () => {
    expect(runEventTopIndex('')).toBe(null)
    expect(runEventTopIndex(undefined)).toBe(null)
    expect(runEventTopIndex('then[1]')).toBe(null)
    expect(runEventTopIndex('steps[x]')).toBe(null)
  })
})
