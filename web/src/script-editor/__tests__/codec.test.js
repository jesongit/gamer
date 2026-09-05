import { describe, expect, it } from 'vitest'
import { parseScript, parseFunctionLibrary, serialize } from '../codec'
import { stripUuids } from './helpers'

/**
 * codec（v3）：decode → encode 幂等（parse(serialize(parse(y))) 模型深度相等）、
 * 规范输出确定性、v2/未知结构明确拒绝。
 */

describe('codec：decode(encode(model)) == model（幂等往返）', () => {
  const SCRIPTS = [
    ['最小脚本', 'version: 3\nsteps: []\n'],
    ['动作全集', [
      'version: 3',
      'steps:',
      '  - app.start: com.a',
      '  - app.stop',
      '  - tap: [0.5, 0.5]',
      '  - tap: $reward.center',
      '  - swipe:',
      '      from: [0.3, 0.5]',
      '      to: [0.7, 0.5]',
      '      duration: 500ms',
      '  - key: BACK',
      '  - key: {key: VOL_UP, action: down}',
      '  - text: "你好"',
      '  - wait: 300ms',
      '  - wait: {min: 300ms, max: 700ms}',
      '  - log: 普通',
      '  - log: {level: warn, message: 警告}',
      '  - set: {name: count, value: 3}',
      '  - if:',
      '      cond: $flag',
      '      then:',
      '        - log: 是',
      '      else:',
      '        - log: 否',
      '  - loop:',
      '      times: 5',
      '      steps:',
      '        - log: 体',
      '  - loop:',
      '      steps:',
      '        - break',
      '  - call:',
      '      target: script:daily/login',
      '      with:',
      '        account: $user',
      '      save: result',
      '  - invoke:',
      '      capability: vision.match',
      '      with:',
      '        template: a.png',
      '      save: m',
      '  - return: $total',
      '  - throw: 终止',
      '  - find:',
      '      template: reward',
      '      timeout: 10s',
      '      threshold: 0.9',
      '      save: reward',
      '      then:',
      '        - tap: $reward.center',
      '      else:',
      '        - log: 未找到',
      '      verify:',
      '        template: home',
      '        timeout: 5s',
      '  - match_first:',
      '      candidates:',
      '        - template: a.png',
      '          threshold: 0.9',
      '          steps:',
      '            - tap: $match.center',
      '        - template: b.png',
      '          steps:',
      '            - log: b',
      '      else:',
      '        - log: 都没有',
      '  - check:',
      '      template: t.png',
      '      timeout: 5s',
      '      threshold: 0.85',
    ].join('\n')],
    ['params 双形态 + defaults', [
      'version: 3',
      'params:',
      "  - 'int:count:次数:3'",
      '  - name: mode',
      '    type: string',
      '    default: auto',
      '    remark: 模式',
      '  - name: ratio',
      '    type: number',
      '    default: 0.5',
      'defaults:',
      '  vision:',
      '    threshold: 0.85',
      '  timing:',
      '    after_tap: 300ms',
      '    poll_interval: 100ms',
      'steps:',
      '  - tap: [0.5, 0.5]',
    ].join('\n')],
  ]

  for (const [name, yaml] of SCRIPTS) {
    it(`${name}：幂等且确定`, () => {
      const first = parseScript(yaml)
      expect(first.diagnostics).toEqual([])
      const once = serialize(first.model)
      const second = parseScript(once)
      expect(stripUuids(second.model)).toEqual(stripUuids(first.model))
      // 同 model 恒同输出
      expect(serialize(second.model)).toBe(once)
    })
  }

  it('函数库 bare-map：多函数往返', () => {
    const yaml = [
      'login:',
      '  params:',
      "    - 'string:account:账号:foo'",
      '  steps:',
      '    - return: true',
      'helper:',
      '  steps:',
      '    - log: hi',
      '    - return: $x',
    ].join('\n')
    const parsed = parseFunctionLibrary(yaml, { file: 'common' })
    expect(parsed.diagnostics).toEqual([])
    expect(parsed.model.functions.map((f) => f.name)).toEqual(['login', 'helper'])
    const once = serialize(parsed.model)
    const again = parseFunctionLibrary(once, { file: 'common' })
    expect(stripUuids(again.model)).toEqual(stripUuids(parsed.model))
    expect(serialize(again.model)).toBe(once)
  })
})

describe('codec：空文档与语法错误', () => {
  it('空文档 → version.missing 诊断且模型为空壳（不误解析）', () => {
    const result = parseScript('\n')
    expect(result.model).toEqual({ version: 3, params: [], defaults: null, steps: [] })
    expect(result.diagnostics.map((d) => d.code)).toEqual(['yaml.v3.version.missing'])
  })

  it('语法错误 → yaml.v3.syntax', () => {
    const result = parseScript('version: 3\nsteps:\n  - log: [未闭合\n')
    expect(result.diagnostics.map((d) => d.code)).toEqual(['yaml.v3.syntax'])
  })

  it('顶层映射形态非法（列表/标量）→ root_type', () => {
    expect(parseScript('- a\n- b\n').diagnostics.map((d) => d.code)).toEqual(['yaml.v3.root_type'])
    expect(parseScript('version: 3\nsteps: []\nextra: 1\n').diagnostics.map((d) => d.code)).toEqual(['yaml.v3.top_level.unknown_key'])
  })
})

describe('codec：v2 与非 v3 结构明确拒绝（不崩溃不误解析）', () => {
  it('缺失 version → version.missing', () => {
    const r = parseScript('steps:\n  - log: v2 脚本\n')
    expect(r.diagnostics.map((d) => d.code)).toEqual(['yaml.v3.version.missing'])
    expect(r.model.steps).toEqual([])
  })

  it('version: 2 → yaml.v3.version，模型空壳', () => {
    const r = parseScript('version: 2\nconfig:\n  interval: 500ms\nsteps:\n  - log: x\n')
    expect(r.diagnostics.map((d) => d.code)).toEqual(['yaml.v3.version'])
    expect(r.model.steps).toEqual([])
    expect(r.model.defaults).toBeNull()
  })

  it('v2 步骤（func/match/str_app/color）→ step.unknown 带迁移提示', () => {
    const r = parseScript([
      'version: 3',
      'steps:',
      '  - func: common/login',
      '  - match:',
      '      - a.png: []',
      '  - str_app',
      '  - color: {at: [0.5, 0.5], expect: []}',
    ].join('\n'))
    const msgs = r.diagnostics.map((d) => d.message).join('\n')
    expect(r.diagnostics.every((d) => d.code === 'yaml.v3.step.unknown')).toBe(true)
    expect(msgs).toContain('call')
    expect(msgs).toContain('match_first')
    expect(msgs).toContain('app.start')
    expect(r.model.steps).toEqual([])
  })

  it('find.click（契约废除的 click 语法）→ field.unknown 提示', () => {
    const r = parseScript('version: 3\nsteps:\n  - find: {template: a.png, click: true}\n')
    expect(r.diagnostics.map((d) => d.code)).toEqual(['yaml.v3.field.unknown'])
    expect(r.diagnostics[0].message).toContain('click')
  })
})

describe('codec：字段级解码', () => {
  it('tap 双形态（标量坐标 / {point: $ref}）与 swipe time 别名', () => {
    const r = parseScript([
      'version: 3',
      'steps:',
      '  - tap: {point: $reward.center}',
      '  - tap: {at: [0.2, 0.3]}',
      '  - swipe: {from: [0, 0], to: [1, 1], time: 2s}',
    ].join('\n'))
    expect(r.diagnostics).toEqual([])
    expect(r.model.steps[0].at).toEqual({ ref: 'reward.center' })
    expect(r.model.steps[1].at).toEqual({ lit: [0.2, 0.3] })
    expect(r.model.steps[2].duration).toEqual({ lit: '2s' })
  })

  it('wait 标量与 {duration} 服务端别名都归一为固定等待', () => {
    const r = parseScript('version: 3\nsteps:\n  - wait: {duration: 5s}\n  - wait: {min: 100ms, max: 200ms}\n')
    expect(r.diagnostics).toEqual([])
    expect(r.model.steps[0].min).toEqual({ lit: '5s' })
    expect(r.model.steps[0].max).toBeNull()
    expect(r.model.steps[1].min).toEqual({ lit: '100ms' })
    expect(r.model.steps[1].max).toEqual({ lit: '200ms' })
  })

  it('call 的 args 兼容别名并入 with；canonical 输出用 with', () => {
    const r = parseScript('version: 3\nsteps:\n  - call: {target: function:common/login, args: {account: $u}}\n')
    expect(r.diagnostics).toEqual([])
    expect(r.model.steps[0].with).toEqual({ account: { ref: 'u' } })
    expect(serialize(r.model)).toContain('with:')
    expect(serialize(r.model)).not.toContain('args:')
  })

  it('set 单键映射别名与 {name, value} 等价', () => {
    const r = parseScript('version: 3\nsteps:\n  - set: {count: 3}\n  - set: {name: total, value: $count}\n')
    expect(r.diagnostics).toEqual([])
    expect(r.model.steps[0]).toMatchObject({ name: 'count', value: { lit: 3 } })
    expect(r.model.steps[1]).toMatchObject({ name: 'total', value: { ref: 'count' } })
  })

  it('loop 省略 times = 无限循环', () => {
    const r = parseScript('version: 3\nsteps:\n  - loop: {steps: [{log: x}]}\n')
    expect(r.diagnostics).toEqual([])
    expect(r.model.steps[0].times).toBeNull()
  })

  it('缺失必需字段/未知字段给结构化诊断', () => {
    // check.throw 是服务端 v3 合法字段（自定义超时文案），未知字段用 bogus 锁定
    const r = parseScript('version: 3\nsteps:\n  - find: {timeout: 5s}\n  - check: {template: t.png, throw: 已死}\n  - check: {template: t.png, bogus: 1}\n')
    const codes = r.diagnostics.map((d) => d.code)
    expect(codes).toContain('yaml.v3.field.missing')
    expect(codes).toContain('yaml.v3.field.unknown')
    const missing = r.diagnostics.find((d) => d.code === 'yaml.v3.field.missing')
    expect(missing.step_path).toBe('steps[0].find.template')
    expect(missing.field).toBe('template')
    const unknown = r.diagnostics.find((d) => d.code === 'yaml.v3.field.unknown')
    expect(unknown.step_path).toBe('steps[2].check.bogus')
  })
})
