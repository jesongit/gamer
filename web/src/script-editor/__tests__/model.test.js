import { describe, expect, it } from 'vitest'
import {
  allocateUuids,
  childStepLists,
  cloneStepWithNewUuids,
  countSteps,
  isRefCell,
  lit,
  ref,
  yamlKeyOf,
  STEP_KINDS,
  ACTION_KEYS,
} from '../model'
import { parseScript, serialize } from '../codec'
import { stripUuids } from './helpers'

/**
 * Model（v3）：Program 结构、Cell 双形态、19 类 Step 判别联合、子流程枚举、uuid 语义。
 */

const MINIMAL = 'version: 3\nsteps: []\n'

describe('model：Program 结构', () => {
  it('最小脚本：version 3 + 空 steps，defaults 缺省为 null', () => {
    const { model } = parseScript(MINIMAL)
    expect(model.version).toBe(3)
    expect(model.params).toEqual([])
    expect(model.defaults).toBeNull()
    expect(model.steps).toEqual([])
    expect(serialize(model)).toBe(MINIMAL)
  })

  it('defaults 三组字段可解码', () => {
    const { model, diagnostics } = parseScript([
      'version: 3',
      'defaults:',
      '  vision:',
      '    threshold: 0.8',
      '  timing:',
      '    after_tap: 300ms',
      '    after_match: 200ms',
      '    poll_interval: 100ms',
      'steps: []',
    ].join('\n'))
    expect(diagnostics).toEqual([])
    expect(model.defaults).toEqual({
      vision_threshold: 0.8,
      after_tap: '300ms',
      after_match: '200ms',
      poll_interval: '100ms',
    })
  })

  it('参数声明双形态：rawForm 与 map 形态并存', () => {
    const { model, diagnostics } = parseScript([
      'version: 3',
      'params:',
      "  - 'int:count:次数:3'",
      '  - name: mode',
      '    type: string',
      '    default: auto',
      '  - name: flag',
      '    type: boolean',
      '    default: true',
      '    remark: 开关',
      'steps: []',
    ].join('\n'))
    expect(diagnostics).toEqual([])
    expect(model.params).toHaveLength(3)
    expect(model.params[0]).toMatchObject({ type: 'int', name: 'count', remark: '次数', default: 3, rawForm: true })
    expect(model.params[1]).toMatchObject({ type: 'string', name: 'mode', default: 'auto', rawForm: false })
    expect(model.params[2]).toMatchObject({ type: 'boolean', name: 'flag', default: true, remark: '开关' })
  })
})

describe('model：Step 键与 kind 集合', () => {
  it('19 类 kind，ACTION_KEYS 含点号键', () => {
    expect(STEP_KINDS).toHaveLength(19)
    expect(ACTION_KEYS).toContain('app.start')
    expect(ACTION_KEYS).toContain('app.stop')
    expect(yamlKeyOf('app_start')).toBe('app.start')
    expect(yamlKeyOf('match_first')).toBe('match_first')
    expect(yamlKeyOf('tap')).toBe('tap')
  })
})

describe('model：Cell', () => {
  it('lit / ref 双形态与判别（ref 支持属性路径）', () => {
    expect(isRefCell(lit(1))).toBe(false)
    expect(isRefCell(ref('reward.center'))).toBe(true)
    const c = ref('reward.center')
    expect(c.ref).toBe('reward.center')
    expect(c.lit).toBeUndefined()
  })
})

describe('model：Step 联合与子流程枚举', () => {
  it('19 类动作均可解码为对应 kind', () => {
    const { model, diagnostics } = parseScript([
      'version: 3',
      'steps:',
      '  - app.start',
      '  - app.stop: com.x',
      '  - tap: [0.5, 0.5]',
      '  - swipe: {from: [0.1, 0.1], to: [0.2, 0.2], duration: 500ms}',
      '  - key: BACK',
      '  - text: "hi"',
      '  - wait: 1s',
      '  - log: hello',
      '  - set: {name: a, value: 1}',
      '  - if: {cond: $flag, then: [], else: []}',
      '  - loop: {times: 3, steps: []}',
      '  - break',
      '  - call: {target: script:x}',
      '  - invoke: {capability: vision.match}',
      '  - return: null',
      '  - throw: boom',
      '  - find: {template: t.png}',
      '  - match_first: {candidates: [{template: a.png}]}',
      '  - check: {template: t.png}',
    ].join('\n'))
    expect(diagnostics).toEqual([])
    expect(model.steps.map((s) => s.kind)).toEqual([
      'app_start', 'app_stop', 'tap', 'swipe', 'key', 'text', 'wait', 'log', 'set',
      'if', 'loop', 'break', 'call', 'invoke', 'return', 'throw', 'find', 'match_first', 'check',
    ])
    expect(countSteps(model.steps)).toBe(19)
  })

  it('childStepLists：if/find/match_first/loop 的分支容器', () => {
    const { model } = parseScript([
      'version: 3',
      'steps:',
      '  - if: {cond: true, then: [{log: a}], else: [{log: b}]}',
      '  - find:',
      '      template: t.png',
      '      then:',
      '        - log: hit',
      '      else:',
      '        - log: miss',
      '  - match_first:',
      '      candidates:',
      '        - template: a.png',
      '          steps:',
      '            - log: c1',
      '        - template: b.png',
      '          steps:',
      '            - log: c2',
      '      else:',
      '        - log: none',
      '  - loop: {times: 2, steps: [{log: body}]}',
    ].join('\n'))
    const [iff, find, mf, loop] = model.steps
    expect(childStepLists(iff).map((c) => c.key)).toEqual(['then', 'else'])
    expect(childStepLists(find).map((c) => c.key)).toEqual(['then', 'else'])
    const mfLists = childStepLists(mf)
    expect(mfLists.map((c) => `${c.key}:${c.index}`)).toEqual(['candidates:0', 'candidates:1', 'else:-1'])
    expect(mfLists[0].list[0].message.lit).toBe('c1')
    expect(childStepLists(loop).map((c) => c.key)).toEqual(['steps'])
    // 叶子步骤无子流程
    expect(childStepLists(find.then[0])).toEqual([])
  })

  it('find 完整字段（threshold/region/save/verify）与 $reward.center 属性引用解码', () => {
    const { model, diagnostics } = parseScript([
      'version: 3',
      'steps:',
      '  - find:',
      '      template: reward',
      '      timeout: 10s',
      '      threshold: 0.9',
      '      region: {left: 0.1, top: 0.1, right: 0.9, bottom: 0.9}',
      '      save: reward',
      '      then:',
      '        - tap: $reward.center',
      '      else:',
      '        - log: 未找到',
      '      verify:',
      '        template: home',
      '        timeout: 5s',
    ].join('\n'))
    expect(diagnostics).toEqual([])
    const find = model.steps[0]
    expect(find.kind).toBe('find')
    expect(find.template).toMatchObject({ lit: 'reward' })
    expect(find.timeout).toMatchObject({ lit: '10s' })
    expect(find.threshold).toBe(0.9)
    expect(find.save).toBe('reward')
    expect(find.verify).toMatchObject({ template: { lit: 'home' }, timeout: { lit: '5s' } })
    expect(isRefCell(find.then[0].at)).toBe(true)
    expect(find.then[0].at.ref).toBe('reward.center')
  })
})

describe('model：UUID 语义', () => {
  it('parse 为每步分配 uuid，重解析重新分配（UUID 不进 YAML）', () => {
    const first = parseScript('version: 3\nsteps:\n  - log: a\n  - log: b\n')
    const uuids1 = first.model.steps.map((s) => s.uuid)
    expect(uuids1).toHaveLength(2)
    expect(new Set(uuids1).size).toBe(2)
    expect(serialize(first.model)).not.toMatch(/[u]uid/)
    const second = parseScript(serialize(first.model))
    const uuids2 = second.model.steps.map((s) => s.uuid)
    expect(uuids2).not.toEqual(uuids1) // 新一轮编辑会话重新分配
    expect(stripUuids(second.model)).toEqual(stripUuids(first.model))
  })

  it('嵌套分支内的步骤同样有 uuid', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - if: {cond: true, then: [{log: x}]}\n')
    const ifStep = model.steps[0]
    expect(typeof ifStep.uuid).toBe('string')
    expect(typeof ifStep.then[0].uuid).toBe('string')
    expect(ifStep.uuid).not.toBe(ifStep.then[0].uuid)
  })

  it('allocateUuids 只补缺失；cloneStepWithNewUuids 副本 uuid 全新', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - if: {cond: true, then: [{log: x}]}\n')
    const step = model.steps[0]
    const before = step.uuid
    allocateUuids(model.steps)
    expect(step.uuid).toBe(before) // 已有保持
    const copy = cloneStepWithNewUuids(step)
    expect(copy.uuid).not.toBe(before)
    expect(copy.then[0].uuid).not.toBe(step.then[0].uuid)
    expect(stripUuids(copy)).toEqual(stripUuids(step))
  })
})
