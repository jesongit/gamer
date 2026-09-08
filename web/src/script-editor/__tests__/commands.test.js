import { describe, expect, it } from 'vitest'
import { parseFunctionLibrary, parseScript } from '../codec'
import { paths, CommandStack } from '../commands'

describe('V1 CommandStack', () => {
  const newScriptModel = () => parseScript('run:\n  - log: one\n  - log: two\n').model

  it('run path resolves; insert/remove/undo', () => {
    const model = newScriptModel()
    const stack = new CommandStack(model)
    expect(stack.apply({ type: 'insert_step', path: paths.run(), index: 1, step: { uuid: 'u3', kind: 'call', fn: 'tap', args: { kind: 'value', cell: { lit: [0.5, 0.5] } }, as: null } })).toBe(true)
    expect(model.run).toHaveLength(3)
    expect(model.run[1].fn).toBe('tap')
    stack.undo()
    expect(model.run).toHaveLength(2)
    stack.redo()
    expect(model.run).toHaveLength(3)
  })

  it('update_step rewrites call fields', () => {
    const model = parseScript('run:\n  - log: hi\n').model
    const stack = new CommandStack(model)
    stack.apply({ type: 'update_step', path: ['run', 0], fields: { fn: 'sleep', args: { kind: 'value', cell: { lit: '1s' } } } })
    expect(model.run[0]).toMatchObject({ kind: 'call', fn: 'sleep' })
    stack.undo()
    expect(model.run[0].fn).toBe('log')
  })

  it('branch child paths (then/body) work', () => {
    const model = parseScript('run:\n  - if: $x\n    then:\n      - log: a\n  - repeat: 2\n    do:\n      - log: b\n').model
    const stack = new CommandStack(model)
    stack.apply({ type: 'insert_step', path: ['run', 0, 'else'], index: 0, step: { uuid: 'ue', kind: 'call', fn: 'log', args: { kind: 'value', cell: { lit: 'e' } }, as: null } })
    stack.apply({ type: 'insert_step', path: ['run', 1, 'body'], index: 0, step: { uuid: 'ub', kind: 'call', fn: 'tap', args: { kind: 'none' }, as: null } })
    expect(model.run[0].else).toHaveLength(1)
    expect(model.run[1].body).toHaveLength(2)
    stack.undo()
    stack.undo()
    expect(model.run[0].else).toHaveLength(0)
    expect(model.run[1].body).toHaveLength(1)
  })

  it('set_params and function params path', () => {
    const model = parseFunctionLibrary(
      'functions:\n  greet:\n    run:\n      - log: hi\n',
    ).model
    const stack = new CommandStack(model)
    stack.apply({ type: 'insert_param', path: paths.functionParams('greet'), index: 0, decl: { name: 'who', type: 'string', required: true, default: null, desc: '称呼' } })
    expect(model.functions[0].params).toEqual([{ name: 'who', type: 'string', required: true, default: null, desc: '称呼' }])
    stack.apply({ type: 'update_param', path: paths.functionParams('greet'), index: 0, decl: { name: 'who', type: 'duration', required: true, default: null, desc: '' } })
    expect(model.functions[0].params[0].type).toBe('duration')
    stack.undo()
    expect(model.functions[0].params[0].type).toBe('string')
  })

  it('set_vars replaces the literal table', () => {
    const model = parseScript('vars:\n  a: 1\nrun: []\n').model
    const stack = new CommandStack(model)
    stack.apply({ type: 'set_vars', vars: { a: 2, b: 'x' } })
    expect(model.vars).toEqual({ a: 2, b: 'x' })
    stack.undo()
    expect(model.vars).toEqual({ a: 1 })
  })

  it('insert_function / rename_function / remove_function', () => {
    const model = parseFunctionLibrary('functions:\n  a:\n    run: []\n').model
    const stack = new CommandStack(model)
    expect(stack.apply({ type: 'insert_function', name: 'a' })).toBe(false, '重名拒绝')
    expect(stack.apply({ type: 'insert_function', name: 'b' })).toBe(true)
    expect(model.functions.map((f) => f.name)).toEqual(['a', 'b'])
    expect(model.functions[1]).toMatchObject({ description: '', params: [], vars: {}, returns: null, run: [] })
    expect(stack.apply({ type: 'rename_function', from: 'b', to: 'c' })).toBe(true)
    expect(model.functions[1].name).toBe('c')
    expect(stack.apply({ type: 'remove_function', name: 'c' })).toBe(true)
    expect(model.functions.map((f) => f.name)).toEqual(['a'])
    expect(stack.apply({ type: 'remove_function', name: 'a' })).toBe(false, '至少保留一个函数')
  })

  it('move_step rejects moving into own subtree', () => {
    const model = parseScript('run:\n  - if: $x\n    then:\n      - log: a\n').model
    const stack = new CommandStack(model)
    // 把 if 步骤（run[0]）移进自己的 then（run[0].then）必须被拒绝
    const ok = stack.apply({ type: 'move_step', from: { path: ['run'], index: 0 }, to: { path: ['run', 0, 'then'], index: 0 } })
    expect(ok).toBe(false)
  })

  it('transaction merges into one undo step', () => {
    const model = newScriptModel()
    const stack = new CommandStack(model)
    stack.transaction(() => {
      stack.apply({ type: 'insert_step', path: ['run'], index: 2, step: { uuid: 'u1', kind: 'call', fn: 'a', args: { kind: 'none' }, as: null } })
      stack.apply({ type: 'insert_step', path: ['run'], index: 3, step: { uuid: 'u2', kind: 'call', fn: 'b', args: { kind: 'none' }, as: null } })
    })
    expect(model.run).toHaveLength(4)
    stack.undo()
    expect(model.run).toHaveLength(2)
  })

  it('duplicate_step clones with fresh uuids', () => {
    const model = parseScript('run:\n  - if: $x\n    then:\n      - log: a\n').model
    const stack = new CommandStack(model)
    expect(stack.apply({ type: 'duplicate_step', path: ['run'], index: 0 })).toBe(true)
    expect(model.run).toHaveLength(2)
    expect(model.run[1].uuid).not.toBe(model.run[0].uuid)
    expect(model.run[1].then[0].uuid).not.toBe(model.run[0].then[0].uuid)
    expect(model.run[1].then[0].fn).toBe('log')
  })
})
