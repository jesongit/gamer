import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseScript, parseFunctionLibrary, serialize } from './codec'
import { stripUuids } from './__tests__/helpers'

/**
 * v3 fixture 往返守护（契约：docs/plans/phase12_v3_dsl_contract.md）：
 * - 对每个合法 fixture：parse 无诊断，serialize(parse(fixture)) 与原文逐字节一致
 *   （fixture 即编辑器规范输出形态），且二次解析模型深度相等；
 * - v2 fixture：明确拒绝（yaml.v3.version 诊断、模型空壳），不误解析。
 */

const here = path.dirname(fileURLToPath(import.meta.url))
const yamlDir = path.join(here, '__fixtures__', 'yaml')

const SCRIPT_IDS = [
  'v3_minimal_script',
  'v3_actions',
  'v3_find_match',
  'v3_defaults_params',
]

const FUNCTION_LIBRARY_IDS = [
  'v3_function_library',
]

function readFixture(name) {
  return readFileSync(path.join(yamlDir, `${name}.yaml`), 'utf8').replace(/\r\n/g, '\n')
}

describe('v3 往返：serialize(parse(fixture)) 逐字节一致且幂等', () => {
  for (const id of SCRIPT_IDS) {
    it(`${id}`, () => {
      const src = readFixture(id)
      const result = parseScript(src)
      expect(result.diagnostics, id).toEqual([])
      const canon = serialize(result.model)
      expect(canon, id).toBe(src)
      const again = parseScript(canon)
      expect(again.diagnostics, `${id} 二次解析`).toEqual([])
      expect(stripUuids(again.model), `${id} 模型幂等`).toEqual(stripUuids(result.model))
    })
  }

  for (const id of FUNCTION_LIBRARY_IDS) {
    it(`${id}（函数库）`, () => {
      const src = readFixture(id)
      const result = parseFunctionLibrary(src, { file: id })
      expect(result.diagnostics, id).toEqual([])
      const canon = serialize(result.model)
      expect(canon, id).toBe(src)
      const again = parseFunctionLibrary(canon, { file: id })
      expect(stripUuids(again.model), `${id} 模型幂等`).toEqual(stripUuids(result.model))
    })
  }

  it('v3_actions：19 类动作全量在位', () => {
    const { model } = parseScript(readFixture('v3_actions'))
    expect(model.steps.map((s) => s.kind)).toEqual([
      'app_start', 'app_stop', 'tap', 'tap', 'swipe', 'key', 'key', 'text',
      'wait', 'wait', 'log', 'log', 'set', 'if', 'loop', 'loop', 'call',
      'invoke', 'return', 'throw', 'check',
    ])
  })

  it('v3_find_match：find 上下文引用（$reward.center / $match.center）可表达', () => {
    const { model } = parseScript(readFixture('v3_find_match'))
    const find = model.steps[0]
    expect(find.kind).toBe('find')
    expect(find.save).toBe('reward')
    expect(find.verify).toMatchObject({ template: { lit: 'home' } })
    expect(find.then[0].at).toEqual({ ref: 'reward.center' })
    const mf = model.steps[1]
    expect(mf.candidates[0].steps[0].at).toEqual({ ref: 'match.center' })
    expect(mf.candidates[0].threshold).toBe(0.9)
  })
})

describe('v2 fixture：明确拒绝（unsupported，不崩溃不误解析）', () => {
  it('v2_rejected → yaml.v3.version 诊断 + 空壳模型', () => {
    const result = parseScript(readFixture('v2_rejected'))
    expect(result.diagnostics.map((d) => d.code)).toEqual(['yaml.v3.version'])
    expect(result.diagnostics[0].message).toContain('version: 3')
    expect(result.model).toEqual({ version: 3, params: [], defaults: null, steps: [] })
  })

  it('缺失 version → yaml.v3.version.missing', () => {
    const result = parseScript('steps:\n  - log: x\n')
    expect(result.diagnostics.map((d) => d.code)).toEqual(['yaml.v3.version.missing'])
  })
})
