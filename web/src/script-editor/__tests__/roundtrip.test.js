import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parseFunctionLibrary, parseScript, serialize } from '../codec'

const fixture = (name) => readFileSync(join(__dirname, '../__fixtures__/yaml', name), 'utf8')

describe('V1 roundtrip', () => {
  it('script decode(encode(decode)) stable', () => {
    const text = fixture('v1_minimal_script.yaml')
    const model = parseScript(text).model
    const once = serialize(model)
    const again = parseScript(once).model
    expect(serialize(again)).toBe(once)
    expect(again.params).toEqual(model.params)
    expect(again.vars).toEqual(model.vars)
    expect(again.run.map((s) => s.fn)).toEqual(model.run.map((s) => s.fn))
  })

  it('function library decode(encode(decode)) stable', () => {
    const text = fixture('v1_function_library.yaml')
    const model = parseFunctionLibrary(text, { file: 'lib' }).model
    const once = serialize(model)
    const again = parseFunctionLibrary(once, { file: 'lib' }).model
    expect(serialize(again)).toBe(once)
    expect(again.functions.map((f) => f.name)).toEqual(model.functions.map((f) => f.name))
  })
})
