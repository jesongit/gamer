import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseScript, parseFunctionLibrary, serialize } from '../codec'
import { stripUuids } from './helpers'

/**
 * 验收锚点（任务书 + docs/SCRIPT_EDITOR_CONTRACT.md §7）：
 * 对每个合法 fixture，serialize(parse(fixture.yaml)) 与 fixture 原文逐字节一致（含结尾单换行）。
 * fixture 即序列化 golden——这是前后端一致性的根。
 */

const here = path.dirname(fileURLToPath(import.meta.url))
const yamlDir = path.join(here, '..', '__fixtures__', 'yaml')

const SCRIPT_IDS = [
  'v01_minimal_script',
  'v02_all_actions',
  'v04_params_all_defaults',
  'v05_params_all_required',
  'v06_nested_if_loop',
  'v07_match_compact',
  'v08_color_branch',
  'v09_call_script',
  'v09_call_script.target',
  'v11_record_output',
  'v12_task_args_snapshot',
]

const FUNCTION_LIBRARY_IDS = [
  'v03_function_library',
  'v10_func_call_cross_file.common',
]

const CROSS_FILE_MAIN = 'v10_func_call_cross_file'

function readFixture(name) {
  return readFileSync(path.join(yamlDir, `${name}.yaml`), 'utf8')
}

describe('codec 往返：serialize(parse(fixture)) 逐字节一致', () => {
  for (const id of SCRIPT_IDS) {
    it(`${id}`, () => {
      const src = readFixture(id)
      const result = parseScript(src)
      expect(result.diagnostics, id).toEqual([])
      expect(serialize(result.model), id).toBe(src)
    })
  }

  for (const id of FUNCTION_LIBRARY_IDS) {
    it(`${id}（函数库）`, () => {
      const src = readFixture(id)
      const result = parseFunctionLibrary(src, { file: id })
      expect(result.diagnostics, id).toEqual([])
      expect(serialize(result.model), id).toBe(src)
    })
  }

  it(`${CROSS_FILE_MAIN}（脚本，跨文件函数调用主文件）`, () => {
    const src = readFixture(CROSS_FILE_MAIN)
    const result = parseScript(src)
    expect(result.diagnostics).toEqual([])
    expect(serialize(result.model)).toBe(src)
  })
})

describe('codec 解析幂等：parse(serialize(parse(y))) 模型深度相等', () => {
  for (const id of [...SCRIPT_IDS, ...FUNCTION_LIBRARY_IDS, CROSS_FILE_MAIN]) {
    it(`${id}`, () => {
      const src = readFixture(id)
      const first = parseScript(src)
      const second = parseScript(serialize(first.model))
      expect(stripUuids(second.model)).toEqual(stripUuids(first.model))
    })
  }
})

describe('codec 解析空文档与语法错误', () => {
  it('空文档 → script.root_type 诊断且模型为空壳', () => {
    const result = parseScript('\n')
    expect(result.model).toEqual({ params: [], config: null, steps: [] })
    expect(result.diagnostics.map((d) => d.code)).toContain('script.root_type')
  })

  it('语法错误 → yaml.syntax_error', () => {
    const result = parseScript('steps:\n  - log: [未闭合\n')
    expect(result.diagnostics.map((d) => d.code)).toEqual(['yaml.syntax_error'])
  })
})
