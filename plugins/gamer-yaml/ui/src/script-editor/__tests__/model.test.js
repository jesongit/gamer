import { describe, expect, it } from 'vitest'
import { parseScript, serialize } from '../codec'
import { allocateUuids, cloneStepWithNewUuids, childStepLists, countSteps, walkSteps, newStepUuid } from '../model'

describe('V1 model / parse basics', () => {
  it('parses the plan example: name/params/vars/run', () => {
    const { model, diagnostics } = parseScript(
      'name: 每日签到\nparams:\n  retry:\n    type: integer\n    default: 3\nvars:\n  timeout: 15s\nrun:\n  - launch: com.example.game\n',
    )
    expect(diagnostics).toEqual([])
    expect(model.name).toBe('每日签到')
    expect(model.params).toEqual([
      { name: 'retry', type: 'integer', required: false, default: 3, desc: '' },
    ])
    expect(model.vars).toEqual({ timeout: '15s' })
    expect(model.run).toHaveLength(1)
    expect(model.run[0]).toMatchObject({ kind: 'call', fn: 'launch' })
    expect(model.run[0].args).toMatchObject({ kind: 'value', cell: { lit: 'com.example.game' } })
  })

  it('rejects version field with migration diagnostic', () => {
    const { model, diagnostics } = parseScript('version: 3\nsteps: []\n')
    expect(model.run).toEqual([])
    expect(diagnostics).toHaveLength(1)
    expect(diagnostics[0].code).toBe('yaml.version.removed')
  })

  it('rejects unknown top-level keys (old steps/defaults)', () => {
    const { diagnostics } = parseScript('steps: []\ndefaults: {}\n')
    expect(diagnostics.some((d) => d.code === 'yaml.top.unknown')).toBe(true)
    expect(diagnostics.some((d) => d.field === 'steps' || d.field === 'defaults')).toBe(true)
  })

  it('missing run reports diagnostic', () => {
    const { diagnostics } = parseScript('name: x\n')
    expect(diagnostics.some((d) => d.code === 'yaml.run.missing')).toBe(true)
  })

  it('parses shorthand args, named args, refs, $$ escape and as', () => {
    const { model, diagnostics } = parseScript(
      [
        'run:',
        '  - tap: [0.5, 0.8]',
        '  - find: login_button',
        '    as: button',
        '  - tap: $button.center',
        '  - log: $$price',
        '  - sleep:',
        '  - claim_daily: {}',
      ].join('\n'),
    )
    expect(diagnostics).toEqual([])
    expect(model.run[0].args).toEqual({ kind: 'value', cell: { lit: [0.5, 0.8] } })
    expect(model.run[1].args).toEqual({ kind: 'value', cell: { lit: 'login_button' } })
    expect(model.run[1].as).toBe('button')
    expect(model.run[2].args).toEqual({ kind: 'value', cell: { ref: 'button.center' } })
    // $$price → 字面量 $price
    expect(model.run[3].args).toEqual({ kind: 'value', cell: { lit: '$price' } })
    // sleep:（null）= 无参；claim_daily: {} = 空命名参数（运行语义同无参）
    expect(model.run[4].args).toEqual({ kind: 'none' })
    expect(model.run[5].args).toEqual({ kind: 'map', entries: {} })
  })

  it('parses named-args maps with refs as leaves', () => {
    const { model } = parseScript(
      'run:\n  - wait_find:\n      template: home\n      timeout: $timeout\n      region: [0, 0, 1, 1]\n',
    )
    expect(model.run[0].args).toEqual({
      kind: 'map',
      entries: {
        template: { lit: 'home' },
        timeout: { ref: 'timeout' },
        region: { lit: [0, 0, 1, 1] },
      },
    })
  })

  it('parses if/repeat/return control flow', () => {
    const { model, diagnostics } = parseScript(
      'vars:\n  flag: true\nrun:\n  - repeat: 2\n    do:\n      - log: tick\n  - if: $flag\n    then:\n      - return: done\n    else:\n      - log: no\n',
    )
    expect(diagnostics).toEqual([])
    expect(model.run[0].kind).toBe('repeat')
    expect(model.run[0].times).toEqual({ lit: 2 })
    expect(model.run[0].body).toHaveLength(1)
    expect(model.run[1].kind).toBe('if')
    expect(model.run[1].then[0]).toMatchObject({ kind: 'return' })
    expect(model.run[1].else[0]).toMatchObject({ kind: 'call', fn: 'log' })
  })

  it('reports structural diagnostics', () => {
    expect(parseScript('run:\n  - tap: [0.1]\n    repeat: 2\n    do: []\n').diagnostics[0].code).toBe('yaml.step.multi')
    expect(parseScript('run:\n  - as: x\n').diagnostics[0].code).toBe('yaml.step.missing')
    expect(parseScript('run:\n  - if: $x\n').diagnostics[0].code).toBe('yaml.if.then')
    expect(parseScript('run:\n  - repeat: 3\n').diagnostics[0].code).toBe('yaml.repeat.do')
    expect(parseScript('run:\n  - repeat: -1\n    do: []\n').diagnostics[0].code).toBe('yaml.repeat.times')
    expect(parseScript('run:\n  - Tap: [0.1, 0.1]\n').diagnostics[0].code).toBe('yaml.name.invalid')
    expect(parseScript('run:\n  - tap: $Foo Bar\n').diagnostics[0].code).toBe('yaml.expr.invalid')
    expect(parseScript('run:\n  - if: $x\n    as: y\n    then: []\n').diagnostics[0].code).toBe('yaml.as.invalid')
  })

  it('params declare type/required/default/desc', () => {
    const { model } = parseScript(
      'params:\n  msg:\n    type: string\n    default: "默认"\n    desc: 消息\n  secret:\n    type: string\n    required: true\nrun: []\n',
    )
    expect(model.params).toEqual([
      { name: 'msg', type: 'string', required: false, default: '默认', desc: '消息' },
      { name: 'secret', type: 'string', required: true, default: null, desc: '' },
    ])
  })

  it('vars conflict with params is diagnostic', () => {
    const { diagnostics } = parseScript('params:\n  a:\n    type: string\nvars:\n  a: x\nrun: []\n')
    expect(diagnostics.some((d) => d.code === 'yaml.vars.conflict')).toBe(true)
  })
})

describe('V1 serialize roundtrip', () => {
  it('empty run serializes to run: []', () => {
    expect(serialize(parseScript('run: []\n').model)).toBe('run: []\n')
  })

  it('script serialize is deterministic and reparses equal', () => {
    const text = [
      'name: 每日签到',
      'params:',
      '  retry:',
      '    type: integer',
      '    default: 3',
      'vars:',
      '  timeout: 15s',
      'run:',
      '  - launch: com.example.game',
      '  - wait_find:',
      '      template: home',
      '      timeout: $timeout',
      '    as: home',
      '  - if: $home',
      '    then:',
      '      - claim_daily: {}',
      '    else:',
      '      - log: 未进入主页',
      '  - return: true',
      '',
    ].join('\n')
    const first = serialize(parseScript(text).model)
    expect(first).toBe(text)
    expect(serialize(parseScript(first).model)).toBe(first)
  })

  it('call args forms roundtrip', () => {
    const text = [
      'run:',
      '  - tap: [0.5, 0.8]',
      '  - find: login',
      '    as: hit',
      '  - wait_find:',
      '      template: home',
      '      timeout: $t',
      '  - claim: {}',
      '',
    ].join('\n')
    const model = parseScript(text).model
    expect(model.run[1].as).toBe('hit')
    expect(serialize(model)).toBe(text.replace('[0.5, 0.8]', '[0.5, 0.8]'))
  })
})

describe('uuid / tree utilities (V1 shapes)', () => {
  const { model } = parseScript(
    'run:\n  - if: $flag\n    then:\n      - log: yes\n    else: []\n  - repeat: 2\n    do:\n      - log: t\n',
  )

  it('allocateUuids fills all steps including branches', () => {
    const steps = model.run
    allocateUuids(steps)
    walkSteps(steps, (step) => {
      expect(typeof step.uuid).toBe('string')
      expect(step.uuid.length).toBeGreaterThan(0)
    })
  })

  it('childStepLists exposes then/else/body', () => {
    const [ifStep, repeatStep] = model.run
    const ifChildren = childStepLists(ifStep).map((c) => c.key)
    expect(ifChildren).toEqual(['then', 'else'])
    expect(childStepLists(repeatStep).map((c) => c.key)).toEqual(['body'])
  })

  it('countSteps counts branches', () => {
    allocateUuids(model.run)
    expect(countSteps(model.run)).toBe(4)
  })

  it('cloneStepWithNewUuids reassigns all uuids', () => {
    allocateUuids(model.run)
    const clone = cloneStepWithNewUuids(model.run[0])
    walkSteps([clone], (step) => {
      expect(step.uuid).not.toBe(model.run[0].uuid)
    })
  })

  it('newStepUuid is unique', () => {
    expect(newStepUuid()).not.toBe(newStepUuid())
  })
})
