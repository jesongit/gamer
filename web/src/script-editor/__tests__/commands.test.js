import { describe, expect, it, vi } from 'vitest'
import { CommandStack, resolveStep, resolveStepList } from '../commands'
import { parseScript, parseFunctionLibrary, serialize } from '../codec'
import { makeStep } from '../factories'
import { stripUuids } from './helpers'

/**
 * 命令栈：插入/移动/复制/删除/改字段 + 事务合并；200 次 undo/redo 不丢步；
 * uuid 在撤销/重做间保持稳定（依赖对象引用还原）。
 */

function scriptWithSteps(n) {
  const { model } = parseScript('steps:\n' + Array.from({ length: n }, (_, i) => `  - log: 第${i}步\n`).join(''))
  return model
}

describe('commands：插入与删除', () => {
  it('insert + undo/redo', () => {
    const model = scriptWithSteps(0)
    const stack = new CommandStack(model)
    const step = makeStep('log')
    expect(stack.apply({ type: 'insert_step', path: ['steps'], index: 0, step })).toBe(true)
    expect(model.steps).toHaveLength(1)
    expect(model.steps[0]).toBe(step)
    stack.undo()
    expect(model.steps).toHaveLength(0)
    stack.redo()
    expect(model.steps[0]).toBe(step)
  })

  it('remove 保留原对象（uuid 稳定）', () => {
    const model = scriptWithSteps(3)
    const stack = new CommandStack(model)
    const removed = model.steps[1]
    stack.apply({ type: 'remove_step', path: ['steps'], index: 1 })
    expect(model.steps.map((s) => s.message.lit)).toEqual(['第0步', '第2步'])
    stack.undo()
    expect(model.steps[1]).toBe(removed)
  })

  it('越界命令被拒绝且不进历史', () => {
    const model = scriptWithSteps(1)
    const stack = new CommandStack(model)
    expect(stack.apply({ type: 'insert_step', path: ['steps'], index: 9, step: makeStep('log') })).toBe(false)
    expect(stack.apply({ type: 'remove_step', path: ['steps'], index: 5 })).toBe(false)
    expect(stack.depth).toBe(0)
  })
})

describe('commands：移动与复制', () => {
  it('同列表前后移动 + undo/redo 恢复', () => {
    const model = scriptWithSteps(3)
    const stack = new CommandStack(model)
    const order0 = model.steps.map((s) => s.uuid)
    // post-removal 语义：把 0 号移到末尾 → to.index = 2（删除后列表长度 2 的末尾）
    stack.apply({ type: 'move_step', from: { path: ['steps'], index: 0 }, to: { path: ['steps'], index: 2 } })
    expect(model.steps.map((s) => s.uuid)).toEqual([order0[1], order0[2], order0[0]])
    stack.undo()
    expect(model.steps.map((s) => s.uuid)).toEqual(order0)
    stack.redo()
    expect(model.steps.map((s) => s.uuid)).toEqual([order0[1], order0[2], order0[0]])
  })

  it('移动到自己的子树内被拒绝', () => {
    const { model } = parseScript('steps:\n  - if: true\n    then:\n      - log: 内\n')
    const stack = new CommandStack(model)
    const before = serialize(model)
    expect(stack.apply({
      type: 'move_step',
      from: { path: ['steps'], index: 0 },
      to: { path: ['steps', 0, 'then'], index: 0 },
    })).toBe(false)
    expect(stack.depth).toBe(0)
    expect(serialize(model)).toBe(before)
  })

  it('跨容器移动：顶层 → 分支内', () => {
    const { model } = parseScript('steps:\n  - if: true\n    then:\n      - log: 内\n  - log: 外\n')
    const stack = new CommandStack(model)
    const outer = model.steps[1]
    stack.apply({ type: 'move_step', from: { path: ['steps'], index: 1 }, to: { path: ['steps', 0, 'then'], index: 1 } })
    expect(model.steps).toHaveLength(1)
    expect(model.steps[0].then.map((s) => s.uuid)).toContain(outer.uuid)
    stack.undo()
    expect(model.steps).toHaveLength(2)
    expect(model.steps[1]).toBe(outer)
  })

  it('duplicate：副本 uuid 全新，撤销后消失', () => {
    const model = scriptWithSteps(1)
    const stack = new CommandStack(model)
    const original = model.steps[0]
    stack.apply({ type: 'duplicate_step', path: ['steps'], index: 0 })
    expect(model.steps).toHaveLength(2)
    const copy = model.steps[1]
    expect(copy.uuid).not.toBe(original.uuid)
    expect(stripUuids(copy)).toEqual(stripUuids(original))
    stack.undo()
    expect(model.steps).toEqual([original])
    stack.redo()
    expect(model.steps[1].uuid).toBe(copy.uuid)
  })
})

describe('commands：改字段与参数命令', () => {
  it('update_step 记录旧值', () => {
    const model = scriptWithSteps(1)
    const stack = new CommandStack(model)
    stack.apply({ type: 'update_step', path: ['steps', 0], fields: { message: { lit: '改过' } } })
    expect(model.steps[0].message.lit).toBe('改过')
    stack.undo()
    expect(model.steps[0].message.lit).toBe('第0步')
  })

  it('update_param 切换参数类型', () => {
    const { model } = parseScript("params:\n  - 'coord:pos:位置:[0.5, 0.5]'\nsteps: []\n")
    const stack = new CommandStack(model)
    stack.apply({ type: 'update_param', index: 0, decl: { type: 'bool', name: 'pos', remark: '位置', default: null } })
    expect(model.params[0].type).toBe('bool')
    stack.undo()
    expect(model.params[0].type).toBe('coord')
    expect(serialize(model)).toContain("'coord:pos:位置:[0.5, 0.5]'")
  })

  it('set_config / set_params', () => {
    const { model } = parseScript('steps: []\n')
    const stack = new CommandStack(model)
    stack.apply({ type: 'set_config', config: { interval: '1s', threshold: 0.9, log_level: 'debug' } })
    expect(model.config).toEqual({ interval: '1s', threshold: 0.9, log_level: 'debug' })
    stack.apply({ type: 'set_params', params: [{ type: 'text', name: 'a', remark: 'A', default: null }] })
    stack.undo()
    stack.undo()
    expect(model.config).toBeNull()
    expect(model.params).toEqual([])
  })
})

describe('commands：事务合并', () => {
  it('一次事务 = 一次 undo', () => {
    const model = scriptWithSteps(0)
    const stack = new CommandStack(model)
    stack.transaction(() => {
      stack.apply({ type: 'insert_step', path: ['steps'], index: 0, step: makeStep('log') })
      stack.apply({ type: 'insert_step', path: ['steps'], index: 1, step: makeStep('tap') })
      stack.apply({ type: 'insert_step', path: ['steps'], index: 2, step: makeStep('key') })
    }, '批量插入')
    expect(model.steps).toHaveLength(3)
    expect(stack.depth).toBe(1)
    stack.undo()
    expect(model.steps).toHaveLength(0)
    stack.redo()
    expect(model.steps).toHaveLength(3)
  })

  it('abort 回滚本事务且不进历史', () => {
    const model = scriptWithSteps(1)
    const stack = new CommandStack(model)
    stack.begin()
    stack.apply({ type: 'insert_step', path: ['steps'], index: 1, step: makeStep('log') })
    expect(model.steps).toHaveLength(2)
    stack.abort()
    expect(model.steps).toHaveLength(1)
    expect(stack.depth).toBe(0)
  })

  it('事务内的 redo 分支丢弃：新命令清空重做栈', () => {
    const model = scriptWithSteps(1)
    const stack = new CommandStack(model)
    stack.apply({ type: 'insert_step', path: ['steps'], index: 1, step: makeStep('log') })
    stack.undo()
    expect(stack.canRedo).toBe(true)
    stack.apply({ type: 'insert_step', path: ['steps'], index: 0, step: makeStep('tap') })
    expect(stack.canRedo).toBe(false)
  })

  it('onChange 触发', () => {
    const model = scriptWithSteps(0)
    const stack = new CommandStack(model)
    const fn = vi.fn()
    stack.onChange(fn)
    stack.apply({ type: 'insert_step', path: ['steps'], index: 0, step: makeStep('log') })
    stack.undo()
    stack.redo()
    expect(fn).toHaveBeenCalledTimes(3)
  })
})

describe('commands：200 次 undo/redo 不丢步', () => {
  it('插入 100 步 + undo×100 + redo×100', () => {
    const model = scriptWithSteps(0)
    const stack = new CommandStack(model)
    for (let i = 0; i < 100; i++) {
      stack.apply({ type: 'insert_step', path: ['steps'], index: i, step: makeStep('log') })
    }
    expect(model.steps).toHaveLength(100)
    const uuids = model.steps.map((s) => s.uuid)
    for (let i = 0; i < 100; i++) expect(stack.undo(), `undo #${i + 1}`).toBe(true)
    expect(model.steps).toHaveLength(0)
    for (let i = 0; i < 100; i++) expect(stack.redo(), `redo #${i + 1}`).toBe(true)
    expect(model.steps.map((s) => s.uuid)).toEqual(uuids)
    expect(stack.canUndo).toBe(true)
    expect(stack.canRedo).toBe(false)
  })

  it('混合操作 200 次往返，结构最终一致', () => {
    const model = scriptWithSteps(2)
    const stack = new CommandStack(model)
    stack.apply({ type: 'duplicate_step', path: ['steps'], index: 0 })
    stack.apply({ type: 'move_step', from: { path: ['steps'], index: 0 }, to: { path: ['steps'], index: 2 } })
    stack.apply({ type: 'update_step', path: ['steps', 1], fields: { message: { lit: '改' } } })
    stack.apply({ type: 'remove_step', path: ['steps'], index: 2 })
    const snapshot = serialize(model)
    for (let i = 0; i < 100; i++) stack.undo()
    for (let i = 0; i < 100; i++) stack.redo()
    expect(serialize(model)).toBe(snapshot)
  })
})

describe('commands：路径解析', () => {
  it('resolveStepList / resolveStep 支持脚本与函数库', () => {
    const { model } = parseScript('steps:\n  - if: true\n    then:\n      - log: x\n')
    const thenList = resolveStepList(model, ['steps', 0, 'then'])
    expect(thenList).toHaveLength(1)
    expect(resolveStep(model, ['steps', 0, 'then', 0]).kind).toBe('log')

    const lib = parseFunctionLibrary('login:\n  steps:\n    - return: true\n', { file: 'common' }).model
    const fnSteps = resolveStepList(lib, ['functions', 'login', 'steps'])
    expect(fnSteps).toHaveLength(1)
    expect(resolveStep(lib, ['functions', 'login', 'steps', 0]).kind).toBe('return')

    // match 候选分支
    const { model: m2 } = parseScript('steps:\n  - match:\n    - a.png:\n      - log: 命中\n')
    const candSteps = resolveStepList(m2, ['steps', 0, 'candidates', 0])
    expect(candSteps[0].kind).toBe('log')
  })
})
