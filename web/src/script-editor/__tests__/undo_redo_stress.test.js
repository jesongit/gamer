// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import StepCanvas from '../components/StepCanvas.vue'
import { stripUuids } from './helpers'
import { setupScript } from './component_helpers'

/**
 * 200 次随机「上移/下移/复制/删除」（卡片按钮驱动，含嵌套展开后的深层卡片）
 * 后：逐条 undo 回到初始模型、逐条 redo 回到最终模型（uuid 剥离后深等）。
 */

const MIXED_YAML = [
  'version: 3',
  'params:',
  "  - 'boolean:enable:开关:true'",
  'steps:',
  '  - log: 第0步',
  '  - tap: [0.5, 0.5]',
  '  - if: {cond: $enable, then: [], else: []}',
  '  - find:',
  '      template: a.png',
  '      then:',
  '        - log: hit',
  '      else:',
  '        - log: miss',
  '  - match_first:',
  '      candidates:',
  '        - template: m1.png',
  '          steps:',
  '            - wait: 2s',
  '        - template: m2.png',
  '          steps:',
  '            - key: BACK',
  '      else:',
  '        - throw: 未命中',
  '  - loop:',
  '      times: 2',
  '      steps:',
  '        - swipe: {from: [0.1, 0.9], to: [0.9, 0.1], duration: 800ms}',
  '        - text: "hi"',
  '  - log: 尾步',
].join('\n')

/** 可复现伪随机（LCG）。 */
function makeRng(seed) {
  let s = seed >>> 0
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0
    return s / 0x100000000
  }
}

function plainClone(value) {
  return JSON.parse(JSON.stringify(value))
}

describe('200 次随机编辑 + undo/redo 全量回放', () => {
  it('卡片按钮驱动：undo 全部 ≡ 初始；redo 全部 ≡ 最终', async () => {
    const created = setupScript(MIXED_YAML)
    const { model, stack } = created
    const initialSnapshot = plainClone(stripUuids(created.plain))

    const wrapper = mount(StepCanvas, {
      props: { model, stack, context: 'script' },
    })

    // 逐层全部展开（新露出的嵌套卡片也展开），让深层卡片的按钮也参与
    for (let round = 0; round < 8; round++) {
      const btns = [...document.querySelectorAll('button[title="展开编辑"]')]
      if (btns.length === 0) break
      for (const b of btns) b.click()
      await wrapper.vm.$nextTick()
    }

    const rng = makeRng(20260829)
    const ops = { up: 0, down: 0, dup: 0, del: 0, skipped: 0 }
    const TITLES = ['上移', '下移', '复制步骤', '删除步骤']

    for (let i = 0; i < 200; i++) {
      // 候选 = 有可点按钮的动作；卡片太少时禁用删除（复制会补充步骤，保证 200 次都有得做）
      const tooFew = wrapper.findAll('.step-card').length <= 3
      const available = TITLES.filter((t) => {
        if (t === '删除步骤' && tooFew) return false
        return wrapper.findAll(`button[title="${t}"]`).some((b) => !b.element.disabled)
      })
      if (available.length === 0) {
        ops.skipped++
        continue
      }
      const title = available[Math.floor(rng() * available.length)]
      const buttons = wrapper.findAll(`button[title="${title}"]`).filter((b) => !b.element.disabled)
      const idx = Math.floor(rng() * buttons.length)
      await buttons[idx].trigger('click')
      ops[{ 上移: 'up', 下移: 'down', 复制步骤: 'dup', 删除步骤: 'del' }[title]]++
    }

    // 全部展开/折叠按钮不进历史；历史条数 = 实际应用的操作数
    const applied = ops.up + ops.down + ops.dup + ops.del
    expect(applied).toBeGreaterThan(60)
    expect(stack.depth).toBe(applied)

    const finalSnapshot = plainClone(stripUuids(model))

    // undo 全部 → 初始模型（引用还原，uuid 也一致；strip 后深等）
    let undoCount = 0
    while (stack.canUndo) {
      expect(stack.undo()).toBe(true)
      undoCount++
    }
    expect(undoCount).toBe(applied)
    expect(plainClone(stripUuids(model))).toEqual(initialSnapshot)

    // redo 全部 → 最终模型
    let redoCount = 0
    while (stack.canRedo) {
      expect(stack.redo()).toBe(true)
      redoCount++
    }
    expect(redoCount).toBe(applied)
    expect(plainClone(stripUuids(model))).toEqual(finalSnapshot)
  })
})
