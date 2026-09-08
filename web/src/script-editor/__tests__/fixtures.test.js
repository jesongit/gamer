import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parseFunctionLibrary, parseScript, serialize } from '../codec'

const fixture = (name) => readFileSync(join(__dirname, '../__fixtures__/yaml', name), 'utf8')

function stripUuids(value) {
  if (Array.isArray(value)) return value.map(stripUuids)
  if (value && typeof value === 'object') {
    const out = {}
    for (const [k, v] of Object.entries(value)) {
      if (k === 'uuid') continue
      out[k] = stripUuids(v)
    }
    return out
  }
  return value
}

describe('V1 fixtures roundtrip', () => {
  const FILES = [
    'v1_minimal_script.yaml',
    'v1_actions.yaml',
    'v1_find_match.yaml',
    'v1_params.yaml',
    'v1_function_library.yaml',
  ]

  for (const name of FILES) {
    it(`${name}: parse→serialize→parse 语义一致且序列化幂等`, () => {
      const text = fixture(name)
      const isLibrary = name.includes('function_library')
      const parse = isLibrary
        ? (t) => parseFunctionLibrary(t, { file: 'lib' })
        : parseScript
      const first = parse(text)
      expect(first.diagnostics).toEqual([])
      const once = serialize(first.model)
      const second = parse(once)
      expect(stripUuids(second.model)).toEqual(stripUuids(first.model))
      expect(serialize(second.model)).toBe(once)
    })
  }

  it('v1_actions covers the full native function vocabulary', () => {
    const model = parseScript(fixture('v1_actions.yaml')).model
    const fns = model.run.filter((s) => s.kind === 'call').map((s) => s.fn)
    for (const fn of [
      'launch', 'stop_app', 'tap', 'swipe', 'key', 'input_text', 'sleep', 'log',
      'find', 'wait_find', 'tap_template', 'wait_disappear', 'eq',
    ]) {
      expect(fns).toContain(fn)
    }
  })

  it('v1_function_library preserves declaration order and fields', () => {
    const model = parseFunctionLibrary(fixture('v1_function_library.yaml'), { file: 'lib' }).model
    expect(model.functions.map((f) => f.name)).toEqual(['claim_daily', 'greet'])
    expect(model.functions[0].description).toBe('领取每日奖励')
    expect(model.functions[0].params[0].default).toBe('5s')
    expect(model.functions[0].vars).toEqual({ tag: 'local' })
    expect(model.functions[0].returns).toEqual({ type: 'boolean' })
    expect(model.functions[1].run).toHaveLength(2)
  })

  it('canonical serialization has no blank lines', () => {
    const text = fixture('v1_minimal_script.yaml')
    const once = serialize(parseScript(text).model)
    expect(once.split('\n').some((line, i, all) => line === '' && i < all.length - 1)).toBe(false)
  })
})
