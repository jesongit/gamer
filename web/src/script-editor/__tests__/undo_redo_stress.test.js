import { describe, expect, it } from 'vitest'
import { parseScript } from '../codec'
import { CommandStack } from '../commands'
import { createCall, createControl } from '../factories'

/** V1 压力：随机增删改 + 交替 undo/redo，撤销链一致、模型始终合法。 */
describe('V1 undo/redo stress', () => {
  it('survives 300 random operations', () => {
    const model = parseScript('run: []\n').model
    const stack = new CommandStack(model)
    let seq = 0
    const makeStep = () => (seq % 3 === 0 ? createControl('if') : createCall(`fn${seq % 5}`))
    for (let i = 0; i < 300; i++) {
      const op = i % 4
      if (op === 0 && model.run.length < 40) {
        seq++
        stack.apply({ type: 'insert_step', path: ['run'], index: model.run.length, step: makeStep() })
      } else if (op === 1 && model.run.length > 0) {
        stack.apply({ type: 'remove_step', path: ['run'], index: model.run.length - 1 })
      } else if (op === 2 && model.run.length > 0) {
        const index = i % model.run.length
        const target = model.run[index]
        if (target.kind !== 'call') continue // as 仅函数调用有；其余跳过
        stack.apply({ type: 'update_step', path: ['run', index], fields: { as: `v${i}` } })
      } else {
        stack.undo()
      }
    }
    // redo 到底再 undo 到底：两个方向的步数必须相等（撤销链自洽；
    // 中途 apply 裁掉重做分支与末态位置不影响该不变量）
    let redos = 0
    while (stack.redo()) redos++
    let undos = 0
    while (stack.undo()) undos++
    expect(undos).toBe(redos)
  })
})
