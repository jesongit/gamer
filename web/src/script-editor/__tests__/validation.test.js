import { describe, expect, it } from 'vitest'
import { parseScript } from '../codec'
import { validateFunctionLibrary, validateScript, validateSource } from '../validation'
import { parseFunctionLibrary } from '../codec'

describe('V1 script validation', () => {
  it('clean script passes', () => {
    const { model, diagnostics } = parseScript(
      'params:\n  t:\n    type: duration\n    default: 5s\nrun:\n  - sleep: $t\n',
    )
    expect(diagnostics).toEqual([])
    expect(validateScript(model)).toEqual([])
  })

  it('unknown function reported when knownFunctions provided', () => {
    const { model } = parseScript('run:\n  - nope_fn: {}\n')
    const diags = validateScript(model, { knownFunctions: new Set(['tap']) })
    expect(diags.some((d) => d.code === 'yaml.fn.not_found' && d.field === 'nope_fn')).toBe(true)
    expect(validateScript(model)).toEqual([]) // 不提供注册表则跳过
  })

  it('undefined variable ref reported', () => {
    const { model } = parseScript('run:\n  - tap: $home.center\n')
    const diags = validateScript(model)
    expect(diags.some((d) => d.code === 'yaml.var.undefined')).toBe(true)
  })

  it('bad ref path syntax reported at parse time (lit fallback in model)', () => {
    const { model, diagnostics } = parseScript('run:\n  - tap: $Bad.Path\n')
    expect(diagnostics.some((d) => d.code === 'yaml.expr.invalid')).toBe(true)
    expect(validateScript(model)).toEqual([])
  })

  it('params/vars/as satisfy declaredVars', () => {
    const { model } = parseScript(
      'params:\n  n:\n    type: integer\nvars:\n  tag: x\nrun:\n  - find: btn\n    as: hit\n  - log: $n\n  - log: $tag\n  - log: $hit\n',
    )
    expect(validateScript(model)).toEqual([])
  })

  it('repeat negative literal reported; ref passes', () => {
    const bad = parseScript('run:\n  - repeat: -2\n    do: []\n').model
    expect(validateScript(bad).some((d) => d.code === 'yaml.repeat.times')).toBe(true)
    const ok = parseScript('params:\n  n:\n    type: integer\n    default: 3\nrun:\n  - repeat: $n\n    do: []\n').model
    expect(validateScript(ok)).toEqual([])
  })

  it('template existence honored via resolveTemplate', () => {
    const { model } = parseScript('run:\n  - find:\n      template: ghost\n')
    const diags = validateScript(model, { resolveTemplate: () => false })
    expect(diags.some((d) => d.code === 'yaml.resource.tmpl_not_found')).toBe(true)
    expect(validateScript(model, { resolveTemplate: (n) => n === 'ghost' })).toEqual([])
  })

  it('bad param default type reported', () => {
    const { model } = parseScript('params:\n  n:\n    type: integer\n    default: 1.5\nrun: []\n')
    expect(validateScript(model).some((d) => d.code === 'yaml.param.default.invalid')).toBe(true)
  })
})

describe('V1 function library validation', () => {
  it('calls inside functions see sibling functions and natives', () => {
    const { model } = parseFunctionLibrary(
      'functions:\n  outer:\n    run:\n      - inner: {}\n      - tap: [0.5, 0.5]\n  inner:\n    run:\n      - log: hi\n',
    )
    const known = new Set(['tap', 'log', ...model.functions.map((f) => f.name)])
    expect(validateFunctionLibrary(model, { knownFunctions: known })).toEqual([])
  })

  it('missing sibling function reported', () => {
    const { model } = parseFunctionLibrary(
      'functions:\n  outer:\n    run:\n      - ghost_fn: {}\n',
    )
    const diags = validateFunctionLibrary(model, { knownFunctions: new Set(['tap']) })
    expect(diags.some((d) => d.code === 'yaml.fn.not_found')).toBe(true)
  })
})

describe('validateSource merges parse + model diagnostics', () => {
  it('parse diagnostics block', () => {
    const { diagnostics } = validateSource('version: 3\nsteps: []\n', 'script')
    expect(diagnostics.some((d) => d.code === 'yaml.version.removed')).toBe(true)
  })

  it('model diagnostics merge with parse ok', () => {
    const { diagnostics } = validateSource('run:\n  - tap: $ghost.x\n', 'script')
    expect(diagnostics.some((d) => d.code === 'yaml.var.undefined')).toBe(true)
  })
})
