import { describe, expect, it } from 'vitest'
import { makeStep, createStep, panelEntries, PANEL_GROUPS, DEFAULT_FACTORIES } from '../factories'
import { parseScript, serialize } from '../codec'
import { stripUuids } from './helpers'
import { STEP_KINDS } from '../model'

/**
 * 工厂与添加面板：19 类步骤工厂创建 + 序列化往返；面板分组（plan §8.5）、
 * return 仅函数上下文。
 */

describe('factories：19 类工厂创建 + 序列化往返', () => {
  for (const kind of STEP_KINDS) {
    it(`${kind}`, () => {
      const step = makeStep(kind)
      expect(step.kind).toBe(kind)
      expect(typeof step.uuid).toBe('string')
      // 包一层最小脚本做 codec 往返（返回前声明一个 bool 参数承载 return）
      const model = {
        params: kind === 'return' ? [{ type: 'bool', name: 'enable', remark: '开关', default: null }] : [],
        config: null,
        steps: [step],
      }
      const yaml = serialize(model)
      const reparsed = parseScript(yaml)
      expect(reparsed.diagnostics, `${kind} 序列化产物应可回解析：\n${yaml}`).toEqual([])
      expect(stripUuids(reparsed.model.steps[0])).toEqual(stripUuids(step))
      // 二次序列化逐字节稳定
      expect(serialize(reparsed.model)).toBe(yaml)
    })
  }

  it('overrides 生效', () => {
    const step = createStep('tap', { at: { lit: [0.1, 0.2] } })
    expect(step.at.lit).toEqual([0.1, 0.2])
    const step2 = createStep('throw', { message: '原因' })
    expect(step2.message).toBe('原因')
  })

  it('全部 19 类都在工厂表里', () => {
    expect(Object.keys(DEFAULT_FACTORIES).sort()).toEqual([...STEP_KINDS].sort())
  })
})

describe('factories：添加面板分组（plan §8.5）', () => {
  it('六个分组：应用/操作/识别/流程/复用/函数专用', () => {
    expect(PANEL_GROUPS.map((g) => g.id)).toEqual(['app', 'action', 'recognition', 'flow', 'reuse', 'function'])
    expect(PANEL_GROUPS.map((g) => g.label)).toEqual(['应用', '操作', '识别', '流程', '复用', '函数专用'])
  })

  it('分组条目覆盖全部 19 类且不重复', () => {
    const kinds = PANEL_GROUPS.flatMap((g) => g.entries.map((e) => e.kind))
    expect(kinds.sort()).toEqual([...STEP_KINDS].sort())
    expect(new Set(kinds).size).toBe(19)
  })

  it('return 仅函数上下文可见（break 在两种上下文均可添加，位置由校验约束）', () => {
    const scriptKinds = panelEntries('script').map((e) => e.kind)
    const functionKinds = panelEntries('function').map((e) => e.kind)
    expect(scriptKinds).not.toContain('return')
    expect(functionKinds).toContain('return')
    expect(scriptKinds).toHaveLength(18)
    expect(functionKinds).toHaveLength(19)
  })
})
