import { describe, expect, it, vi } from 'vitest'
import { CommandStack, paths, resolveStep, resolveStepList } from '../commands'
import { parseScript, parseFunctionLibrary, serialize } from '../codec'
import { makeStep } from '../factories'
import { setupFunctions } from './component_helpers'
import { stripUuids } from './helpers'

/**
 * 命令栈：插入/移动/复制/删除/改字段 + 事务合并；200 次 undo/redo 不丢步；
 * uuid 在撤销/重做间保持稳定（依赖对象引用还原）。
 */

function scriptWithSteps(n) {
  const { model } = parseScript('version: 3\nsteps:\n' + Array.from({ length: n }, (_, i) => `  - log: 第${i}步\n`).join(''))
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
    const { model } = parseScript('version: 3\nsteps:\n  - if: {cond: true, then: [{log: 内}]}\n')
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
    const { model } = parseScript('version: 3\nsteps:\n  - if: {cond: true, then: [{log: 内}]}\n  - log: 外\n')
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

  it('update_param 切换参数类型（rawForm 串保真）', () => {
    const { model } = parseScript("version: 3\nparams:\n  - 'coord:pos:位置:[0.5, 0.5]'\nsteps: []\n")
    const stack = new CommandStack(model)
    stack.apply({ type: 'update_param', index: 0, decl: { type: 'boolean', name: 'pos', remark: '位置', default: null, rawForm: false } })
    expect(model.params[0].type).toBe('boolean')
    stack.undo()
    expect(model.params[0].type).toBe('coord')
    expect(serialize(model)).toContain("'coord:pos:位置:[0.5, 0.5]'")
  })

  it('set_defaults / set_params', () => {
    const { model } = parseScript('version: 3\nsteps: []\n')
    const stack = new CommandStack(model)
    stack.apply({ type: 'set_defaults', defaults: { vision_threshold: 0.9, after_tap: '1s', after_match: null, poll_interval: null } })
    expect(model.defaults).toEqual({ vision_threshold: 0.9, after_tap: '1s', after_match: null, poll_interval: null })
    stack.apply({ type: 'set_params', params: [{ type: 'string', name: 'a', remark: 'A', default: null, rawForm: false }] })
    stack.undo()
    stack.undo()
    expect(model.defaults).toBeNull()
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
    const { model } = parseScript('version: 3\nsteps:\n  - if: {cond: true, then: [{log: x}]}\n')
    const thenList = resolveStepList(model, ['steps', 0, 'then'])
    expect(thenList).toHaveLength(1)
    expect(resolveStep(model, ['steps', 0, 'then', 0]).kind).toBe('log')

    const lib = parseFunctionLibrary('login:\n  steps:\n    - return: true\n', { file: 'common' }).model
    const fnSteps = resolveStepList(lib, ['functions', 'login', 'steps'])
    expect(fnSteps).toHaveLength(1)
    expect(resolveStep(lib, ['functions', 'login', 'steps', 0]).kind).toBe('return')

    // match_first 候选分支
    const { model: m2 } = parseScript('version: 3\nsteps:\n  - match_first:\n      candidates:\n        - template: a.png\n          steps:\n            - log: 命中\n')
    const candSteps = resolveStepList(m2, ['steps', 0, 'candidates', 0])
    expect(candSteps[0].kind).toBe('log')
  })
})

describe('commands：函数级 params（阶段 4 path 容器）', () => {
  const FN_YAML = `login:
  params:
    - 'tmpl:account:账号模板:account.png'
  steps:
    - return: true
`

  function setup() {
    const { model, stack } = setupFunctions(FN_YAML, 'common')
    const path = paths.functionParams('login')
    return { model, stack, path }
  }

  it('paths.functionParams 生成 [functions, 名, params] 容器路径', () => {
    expect(paths.functionParams('login')).toEqual(['functions', 'login', 'params'])
  })

  it('insert/update/remove/set 携带 path 后命中目标函数，undo 还原', () => {
    const { model, stack, path } = setup()
    expect(stack.apply({ type: 'insert_param', path, index: 1, decl: { type: 'string', name: 'tag', remark: '', default: null, rawForm: false } }, '添加函数参数')).toBe(true)
    expect(model.functions[0].params).toHaveLength(2)
    expect(model.functions[0].params[1].name).toBe('tag')

    expect(stack.apply({ type: 'update_param', path, index: 1, decl: { type: 'integer', name: 'count', remark: '', default: 3, rawForm: false } }, '编辑函数参数')).toBe(true)
    expect(model.functions[0].params[1]).toMatchObject({ type: 'integer', name: 'count', default: 3 })

    expect(stack.apply({ type: 'remove_param', path, index: 1 }, '删除函数参数')).toBe(true)
    expect(model.functions[0].params).toHaveLength(1)

    const reordered = [...model.functions[0].params]
    expect(stack.apply({ type: 'set_params', path, params: reordered }, '参数排序')).toBe(true)
    // 序列化后 params 只落回 login 函数，另一函数不受影响
    expect(serialize(model)).toContain('tmpl:account:账号模板:account.png')

    // undo×4 逐条还原：set → remove → update → insert，回到初始 [account]
    stack.undo()
    stack.undo()
    stack.undo()
    stack.undo()
    expect(model.functions[0].params).toHaveLength(1)
    expect(model.functions[0].params[0].name).toBe('account')
  })

  it('路径指向不存在的函数在执行期抛错且不进历史', () => {
    const { stack } = setup()
    const bad = ['functions', 'nope', 'params']
    expect(() => stack.apply({ type: 'insert_param', path: bad, index: 0, decl: { type: 'string', name: 'x', remark: '', default: null, rawForm: false } }, '添加函数参数'))
      .toThrow('函数不存在')
    expect(stack.canUndo).toBe(false)
  })

  it('函数库模型缺省路径（文件级 params）被拒绝', () => {
    const { stack } = setup()
    expect(() => stack.apply({ type: 'set_params', params: [] }, '编辑参数')).toThrow()
  })
})

describe('commands：insert_function（函数库新增函数）', () => {
  function fnModel() {
    const { model } = parseFunctionLibrary('f1:\n  steps:\n    - log: hello\n')
    return model
  }

  it('文件尾追加空函数；undo 移除 / redo 原样放回', () => {
    const model = fnModel()
    const stack = new CommandStack(model)
    expect(stack.apply({ type: 'insert_function', name: 'f2' })).toBe(true)
    expect(model.functions.map((f) => f.name)).toEqual(['f1', 'f2'])
    expect(model.functions[1].params).toEqual([])
    expect(model.functions[1].steps).toEqual([])
    stack.undo()
    expect(model.functions.map((f) => f.name)).toEqual(['f1'])
    stack.redo()
    expect(model.functions[1].name).toBe('f2')
  })

  it('重名拒绝且不进历史；脚本模型拒绝该命令', () => {
    const model = fnModel()
    const stack = new CommandStack(model)
    expect(stack.apply({ type: 'insert_function', name: 'f1' })).toBe(false)
    expect(stack.depth).toBe(0)
    const scriptStack = new CommandStack(scriptWithSteps(1))
    expect(scriptStack.apply({ type: 'insert_function', name: 'f1' })).toBe(false)
    expect(scriptStack.depth).toBe(0)
  })
})

describe('commands：remove_function（函数库删除函数）', () => {
  function fnModel() {
    const { model } = parseFunctionLibrary(
      'f1:\n  steps:\n    - log: one\nf2:\n  params:\n    - \'text:a:备注\'\n  steps:\n    - log: two\nf3:\n  steps:\n    - log: three\n',
    )
    return model
  }

  it('按名删除；undo 原位恢复（对象引用不变，uuid 稳定）/ redo 再删', () => {
    const model = fnModel()
    const stack = new CommandStack(model)
    const removed = model.functions[1]
    expect(stack.apply({ type: 'remove_function', name: 'f2' })).toBe(true)
    expect(model.functions.map((f) => f.name)).toEqual(['f1', 'f3'])
    stack.undo()
    expect(model.functions.map((f) => f.name)).toEqual(['f1', 'f2', 'f3'])
    expect(model.functions[1]).toBe(removed)
    stack.redo()
    expect(model.functions.map((f) => f.name)).toEqual(['f1', 'f3'])
    expect(model.functions[1]).not.toBe(removed)
  })

  it('仅剩一个函数时拒绝；未知函数名拒绝且不进历史', () => {
    const model = fnModel()
    const stack = new CommandStack(model)
    expect(stack.apply({ type: 'remove_function', name: 'nope' })).toBe(false)
    expect(stack.depth).toBe(0)
    stack.apply({ type: 'remove_function', name: 'f2' })
    stack.apply({ type: 'remove_function', name: 'f1' })
    expect(model.functions.map((f) => f.name)).toEqual(['f3'])
    expect(stack.apply({ type: 'remove_function', name: 'f3' })).toBe(false)
    expect(stack.depth).toBe(2)
    const scriptStack = new CommandStack(scriptWithSteps(1))
    expect(scriptStack.apply({ type: 'remove_function', name: 'f1' })).toBe(false)
    expect(scriptStack.depth).toBe(0)
  })

  it('删除后 undo 新函数插入再 undo 删除：仍恢复到原位置', () => {
    const model = fnModel()
    const stack = new CommandStack(model)
    stack.apply({ type: 'remove_function', name: 'f1' })
    stack.apply({ type: 'insert_function', name: 'f9' })
    expect(model.functions.map((f) => f.name)).toEqual(['f2', 'f3', 'f9'])
    stack.undo() // 撤销插入 f9
    stack.undo() // 撤销删除 f1 → 回到下标 0
    expect(model.functions.map((f) => f.name)).toEqual(['f1', 'f2', 'f3'])
  })
})

describe('commands：rename_function（函数库函数改名）', () => {
  function fnModel() {
    const { model } = parseFunctionLibrary('f1:\n  steps:\n    - log: one\nf2:\n  steps:\n    - log: two\n')
    return model
  }

  it('改名即改 YAML 顶层键；undo/redo 恢复', () => {
    const model = fnModel()
    const stack = new CommandStack(model)
    expect(stack.apply({ type: 'rename_function', from: 'f1', to: '登录' })).toBe(true)
    expect(model.functions.map((f) => f.name)).toEqual(['登录', 'f2'])
    stack.undo()
    expect(model.functions.map((f) => f.name)).toEqual(['f1', 'f2'])
    stack.redo()
    expect(model.functions[0].name).toBe('登录')
  })

  it('空名/重名/原名/未知函数拒绝且不进历史；脚本模型拒绝该命令', () => {
    const model = fnModel()
    const stack = new CommandStack(model)
    expect(stack.apply({ type: 'rename_function', from: 'f1', to: '  ' })).toBe(false)
    expect(stack.apply({ type: 'rename_function', from: 'f1', to: 'f2' })).toBe(false)
    expect(stack.apply({ type: 'rename_function', from: 'f1', to: 'f1' })).toBe(false)
    expect(stack.apply({ type: 'rename_function', from: 'nope', to: 'f9' })).toBe(false)
    expect(stack.depth).toBe(0)
    const scriptStack = new CommandStack(scriptWithSteps(1))
    expect(scriptStack.apply({ type: 'rename_function', from: 'f1', to: 'f9' })).toBe(false)
    expect(scriptStack.depth).toBe(0)
  })

  it('改名后序列化顶层键跟随（params/steps 原样保留）', () => {
    const model = fnModel()
    const stack = new CommandStack(model)
    stack.apply({ type: 'rename_function', from: 'f1', to: 'login' })
    const yaml = serialize(model)
    expect(yaml).toContain('login:')
    expect(yaml).toContain('- log: one')
    expect(yaml).not.toMatch(/^f1:/m)
  })
})
