// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { parseScript, serialize } from '../codec'
import { CommandStack } from '../commands'
import { createStep, makeStep } from '../factories'
import { lit } from '../model'
import { stripUuids } from './helpers'
import { setupScript } from './component_helpers'

/**
 * 「编辑 → serialize → 再 parse」往返与直接构造模型一致（uuid 剥离后深等）。
 * 编辑全部经 CommandStack；直接构造用工厂 + 字面量，不共享任何引用。
 */

const NESTED_YAML = [
  'version: 3',
  'params:',
  "  - 'boolean:enable:是否启用:true'",
  "  - 'boolean:retry:允许重试:false'",
  'steps:',
  '  - loop:',
  '      times: 3',
  '      steps:',
  '        - if:',
  '            cond: $retry',
  '            then:',
  '              - find:',
  '                  template: retry.png',
  '                  timeout: 10s',
  '                  save: hit',
  '                  then:',
  '                    - log: 清理障碍',
  '                    - tap: [0.5, 0.5]',
  '              - check:',
  '                  template: home.png',
  '                  timeout: 5s',
  '            else:',
  '              - log: 无障碍物',
  '  - if:',
  '      cond: $enable',
  '      then:',
  '        - loop:',
  '            steps:',
  '              - wait: 1s',
  '      else:',
  '        - log: 已禁用',
].join('\n')

/** 直接构造的期望模型（编辑后的最终形态；不含 uuid）。 */
function expectedModel() {
  return {
    version: 3,
    params: [
      { type: 'boolean', name: 'enable', remark: '是否启用', default: true, rawForm: true },
      { type: 'boolean', name: 'retry', remark: '允许重试', default: false, rawForm: true },
    ],
    defaults: null,
    steps: [
      {
        kind: 'loop',
        times: { lit: 5 },
        steps: [
          {
            kind: 'if',
            cond: { ref: 'retry' },
            then: [
              {
                kind: 'find',
                template: { lit: 'relogin.png' },
                timeout: { lit: '10s' },
                threshold: null,
                region: null,
                save: 'hit',
                then: [
                  { kind: 'log', message: { lit: '清理障碍' }, level: null },
                  { kind: 'tap', at: { lit: [0.5, 0.5] } },
                ],
                else: [],
                verify: null,
              },
              { kind: 'check', template: { lit: 'home.png' }, timeout: { lit: '5s' }, threshold: null, throw: null },
            ],
            else: [{ kind: 'log', message: { lit: '无障碍物' }, level: null }],
          },
        ],
      },
      {
        kind: 'if',
        cond: { ref: 'enable' },
        then: [{ kind: 'loop', times: null, steps: [{ kind: 'wait', min: { lit: '1s' }, max: null }] }],
        else: [{ kind: 'log', message: { lit: '已禁用' }, level: null }],
      },
      { kind: 'tap', at: { lit: [0.5, 0.8] } },
      { kind: 'tap', at: { lit: [0.5, 0.8] } },
    ],
  }
}

function strip(value) {
  return JSON.parse(JSON.stringify(stripUuids(value)))
}

describe('编辑往返一致性', () => {
  it('经 CommandStack 编辑 → serialize → parse ≡ 直接构造模型', () => {
    const { model, stack } = setupScript(NESTED_YAML)

    // 1. loop times 3 → 5
    stack.apply({ type: 'update_step', path: ['steps', 0], fields: { times: { lit: 5 } } }, '改次数')
    // 2. find 主模板改短名
    stack.apply({ type: 'update_step', path: ['steps', 0, 'steps', 0, 'then', 0], fields: { template: { lit: 'relogin.png' } } }, '改模板')
    // 3. 末尾插入 tap 并复制（一次事务 = 一条历史）
    stack.transaction(() => {
      stack.apply({ type: 'insert_step', path: ['steps'], index: 2, step: makeStep('tap') }, '插入')
      stack.apply({ type: 'update_step', path: ['steps', 2], fields: { at: { lit: [0.5, 0.8] } } }, '改坐标')
      stack.apply({ type: 'duplicate_step', path: ['steps'], index: 2 }, '复制')
    }, '插入并复制 tap')

    // 编辑后模型 ≡ 期望
    expect(strip(model)).toEqual(expectedModel())

    // serialize → parse → strip ≡ 期望（codec 自动剥离 uuid，双重保险再 strip 一次）
    const text = serialize(model)
    const reparsed = parseScript(text)
    expect(reparsed.diagnostics).toEqual([])
    expect(strip(reparsed.model)).toEqual(expectedModel())
  })

  it('untracked 直接构造模型 serialize → parse 后与其自身一致', () => {
    const direct = {
      version: 3,
      defaults: { vision_threshold: 0.85, after_tap: '300ms', after_match: null, poll_interval: null },
      params: [{ type: 'string', name: 'msg', remark: '提示', default: 'hi', rawForm: false }],
      steps: [
        createStep('swipe', { from: lit([0.1, 0.9]), to: lit([0.9, 0.1]), duration: lit('800ms') }),
        createStep('match_first', {
          candidates: [
            { template: lit('a.png'), threshold: 0.9, steps: [] },
            { template: lit('b.png'), threshold: null, steps: [makeStep('key')] },
            { template: lit('c.png'), threshold: null, steps: [makeStep('log')] },
          ],
          else: [makeStep('throw')],
        }),
        createStep('find', {
          template: lit('reward'),
          timeout: lit('10s'),
          threshold: 0.9,
          region: null,
          save: 'reward',
          then: [createStep('tap', { at: { ref: 'reward.center' } })],
          else: [],
          verify: { template: lit('home'), timeout: lit('5s') },
        }),
        createStep('call', { target: 'function:common/login', with: { account: lit('a.png') }, save: null }),
        createStep('wait', { min: lit('300ms'), max: lit('700ms') }),
      ],
    }
    const text = serialize(direct)
    const parsed = parseScript(text)
    expect(parsed.diagnostics).toEqual([])
    expect(strip(parsed.model)).toEqual(strip(direct))
    // 规范形态断言：find 字段顺序、match_first 候选结构、call with 命名
    expect(text).toContain('- find:')
    expect(text.indexOf('save: reward')).toBeGreaterThan(-1)
    expect(text).toContain('verify:')
    expect(text).toContain('- template: a.png')
    expect(text).toContain('with:')
  })

  it('serialize(parse(fixture)) 幂等（组件层旁证）', () => {
    const once = serialize(parseScript(NESTED_YAML).model)
    const twice = serialize(parseScript(once).model)
    expect(twice).toBe(once)
  })
})
