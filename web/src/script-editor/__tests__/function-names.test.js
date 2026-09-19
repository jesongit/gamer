import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { parseFunctionLibrary, serialize } from '../codec'
import { validateSource } from '../validation'
import { isFunctionName } from '../schema'

const names = JSON.parse(readFileSync(new URL('../../../../tools/yaml-tests/function-names.json', import.meta.url), 'utf8'))

describe('中文函数定义与调用使用相同命名规则', () => {
  for (const name of names.valid) it(`保存和重开 ${name}`, () => {
    const source = `functions:\n  ${name}:\n    run:\n      - return: true\n  caller:\n    run:\n      - ${name}: {}\n`
    const checked = validateSource(source, 'function_library')
    expect(checked.diagnostics).toEqual([])
    const saved = serialize(parseFunctionLibrary(source).model)
    expect(parseFunctionLibrary(saved).model.functions.map(f => f.name)).toEqual([name, 'caller'])
    expect(validateSource(`run:\n  - ${name}: {}\n`, 'script', { knownFunctions: new Set([name]) }).diagnostics).toEqual([])
  })
  for (const name of names.invalid) it(`拒绝非法名称 ${JSON.stringify(name)}`, () => {
    expect(isFunctionName(name)).toBe(false)
    const key = JSON.stringify(name)
    expect(validateSource(`functions:\n  ${key}:\n    run: []\n`, 'function_library').diagnostics.some(d => d.code === 'yaml.name.invalid')).toBe(true)
    expect(validateSource(`run:\n  - ${key}: {}\n`, 'script').diagnostics.some(d => d.code === 'yaml.name.invalid')).toBe(true)
  })
  it.each(['if', 'repeat', 'return'])('保留字 %s 不能定义为函数', name => {
    expect(validateSource(`functions:\n  ${name}:\n    run: []\n`, 'function_library').diagnostics.length).toBeGreaterThan(0)
  })
})
