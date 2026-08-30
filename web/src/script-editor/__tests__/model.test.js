import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseScript, parseFunctionLibrary, serialize } from '../codec'
import { stripUuids } from './helpers'

/**
 * Model 字段名断言：解析结果与 golden JSON（当前前端 Model 字段名）深度一致。
 * golden 就分布在 __fixtures__/json/（与服务端逐字节一致的只读副本）。
 */

const here = path.dirname(fileURLToPath(import.meta.url))
const yamlDir = path.join(here, '..', '__fixtures__', 'yaml')
const jsonDir = path.join(here, '..', '__fixtures__', 'json')

const VALID_IDS = [
  'v01_minimal_script',
  'v02_all_actions',
  'v03_function_library',
  'v04_params_all_defaults',
  'v05_params_all_required',
  'v06_nested_if_loop',
  'v07_match_compact',
  'v08_color_branch',
  'v09_call_script',
  'v10_func_call_cross_file',
  'v11_record_output',
  'v12_task_args_snapshot',
]

function readJson(name) {
  return JSON.parse(readFileSync(path.join(jsonDir, name), 'utf8'))
}

describe('model：parse 结果与 golden JSON 深度一致', () => {
  for (const id of VALID_IDS) {
    it(`${id}`, () => {
      const golden = readJson(`${id}.golden.json`)
      expect(golden.kind).toBe('valid')
      for (const entry of golden.files) {
        const text = readFileSync(path.join(yamlDir, entry.file), 'utf8')
        let model
        if (entry.model_kind === 'function_library') {
          const parsed = parseFunctionLibrary(text, { file: entry.model.file })
          expect(parsed.diagnostics, `${id}/${entry.file}`).toEqual([])
          model = parsed.model
        } else {
          const parsed = parseScript(text)
          expect(parsed.diagnostics, `${id}/${entry.file}`).toEqual([])
          model = parsed.model
        }
        expect(stripUuids(model), `${id}/${entry.file} model`).toEqual(entry.model)
      }
    })
  }

  it('v12：task_snapshot 的 args 快照与声明默认值一致', () => {
    const golden = readJson('v12_task_args_snapshot.golden.json')
    const model = golden.files[0].model
    for (const p of model.params) {
      expect(golden.task_snapshot.args[p.name]).toEqual(p.default)
    }
  })
})

describe('model：UUID 语义', () => {
  it('parse 为每步分配 uuid，重解析重新分配（UUID 不进 YAML）', () => {
    const first = parseScript('steps:\n  - log: a\n  - log: b\n')
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
    const { model } = parseScript('steps:\n  - if: true\n    then:\n      - log: x\n')
    const ifStep = model.steps[0]
    expect(typeof ifStep.uuid).toBe('string')
    expect(typeof ifStep.then[0].uuid).toBe('string')
    expect(ifStep.uuid).not.toBe(ifStep.then[0].uuid)
  })
})
