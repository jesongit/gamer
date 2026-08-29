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
  'params:',
  "  - 'bool:enable:是否启用:true'",
  "  - 'bool:retry:允许重试:false'",
  'steps:',
  '  - loop:',
  '      times: 3',
  '      steps:',
  '        - if: $retry',
  '          then:',
  '            - find: retry.png',
  '              block:',
  '                - popup.png',
  '                - dialog.png',
  '              verify: true',
  '              then:',
  '                - log: 清理障碍',
  '                - tap: [0.5, 0.5]',
  '          else:',
  '            - log: 无障碍物',
  '  - if: $enable',
  '    then:',
  '      - loop:',
  '          steps:',
  '            - wait: 1s',
  '    else:',
  '      - log: 已禁用',
].join('\n')

/** 直接构造的期望模型（编辑后的最终形态；不含 uuid）。 */
function expectedModel() {
  return {
    params: [
      { type: 'bool', name: 'enable', remark: '是否启用', default: true },
      { type: 'bool', name: 'retry', remark: '允许重试', default: false },
    ],
    config: null,
    steps: [
      {
        kind: 'loop',
        times: 5,
        steps: [
          {
            kind: 'if',
            cond: { ref: 'retry' },
            then: [
              {
                kind: 'find',
                template: { lit: 'relogin.png' },
                block: [{ lit: 'popup.png' }, { lit: 'dialog.png' }],
                verify: true,
                timeout: null,
                then: [
                  { kind: 'log', message: { lit: '清理障碍' } },
                  { kind: 'tap', at: { lit: [0.5, 0.5] } },
                ],
                else: [],
              },
            ],
            else: [{ kind: 'log', message: { lit: '无障碍物' } }],
          },
        ],
      },
      {
        kind: 'if',
        cond: { ref: 'enable' },
        then: [{ kind: 'loop', times: null, steps: [{ kind: 'wait', duration: { lit: '1s' }, duration_max: null }] }],
        else: [{ kind: 'log', message: { lit: '已禁用' } }],
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
    stack.apply({ type: 'update_step', path: ['steps', 0], fields: { times: 5 } }, '改次数')
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
      config: { interval: '500ms', threshold: 0.85, log_level: 'info' },
      params: [{ type: 'text', name: 'msg', remark: '提示', default: 'hi' }],
      steps: [
        createStep('swipe', { from: lit([0.1, 0.9]), to: lit([0.9, 0.1]), time: lit('800ms') }),
        createStep('match', {
          candidates: [{ template: lit('a.png'), steps: [makeStep('key')] }],
          else: [makeStep('throw')],
          timeout: lit('30s'),
        }),
        createStep('color', {
          at: lit([0.5, 0.5]),
          expect: [{ color: lit('123456'), steps: [makeStep('log')] }],
          else: [],
        }),
        createStep('func', { target: 'common/login', args: { account: lit('a.png') }, then: [], else: [] }),
      ],
    }
    const text = serialize(direct)
    const parsed = parseScript(text)
    expect(parsed.diagnostics).toEqual([])
    expect(strip(parsed.model)).toEqual(strip(direct))
  })

  it('serialize(parse(fixture)) 幂等（阶段 0 契约的组件层旁证）', () => {
    const once = serialize(parseScript(NESTED_YAML).model)
    const twice = serialize(parseScript(once).model)
    expect(twice).toBe(once)
  })
})
