import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { validateSource, validateScript, validateFunctionLibrary } from '../validation'
import { parseFunctionLibrary, parseScript } from '../codec'
import { makeStep } from '../factories'

/**
 * 结构化校验：非法 fixture（i01~i09）必须被标出 expected.json 中的每一个
 * {code, step_path, field}；另覆盖 Model 层引用/上下文/绑定检查。
 */

const here = path.dirname(fileURLToPath(import.meta.url))
const yamlDir = path.join(here, '..', '__fixtures__', 'yaml')
const jsonDir = path.join(here, '..', '__fixtures__', 'json')

const INVALID_IDS = [
  'i01_old_top_format',
  'i02_params_unquoted',
  'i03_default_type_mismatch',
  'i04_match_candidate_duplicate',
  'i05_func_path_traversal',
  'i06_call_cycle',
  'i07_unknown_top_key',
  'i08_else_in_candidates',
  'i09_empty_default',
]

describe('validation：非法 fixture i01~i09 全部标出期望错误', () => {
  for (const id of INVALID_IDS) {
    it(`${id}`, () => {
      const expected = JSON.parse(readFileSync(path.join(jsonDir, `${id}.expected.json`), 'utf8'))
      const text = readFileSync(path.join(yamlDir, `${id}.yaml`), 'utf8')
      const { diagnostics } = validateSource(text, 'script', { selfFile: `${id}.yaml` })
      for (const exp of expected.errors) {
        const hit = diagnostics.find((d) => d.code === exp.code && d.step_path === exp.step_path && d.field === exp.field)
        expect(hit, `${id}: 期望错误 ${exp.code}@${exp.step_path}.${exp.field}；实际：${JSON.stringify(diagnostics.map((d) => [d.code, d.step_path, d.field]))}`).toBeDefined()
      }
    })
  }

  it('合法 fixture 无诊断（validateSource 与 parse 对齐）', () => {
    for (const id of ['v01_minimal_script', 'v06_nested_if_loop', 'v08_color_branch']) {
      const text = readFileSync(path.join(yamlDir, `${id}.yaml`), 'utf8')
      const { diagnostics } = validateSource(text, 'script')
      expect(diagnostics, id).toEqual([])
    }
  })
})

describe('validation：引用与类型', () => {
  it('$name 引用未声明参数 → param.ref.unknown', () => {
    const { model } = parseScript('steps:\n  - tap: $nope\n')
    const diags = validateScript(model)
    expect(diags).toContainEqual(expect.objectContaining({
      code: 'param.ref.unknown',
      step_path: 'steps[0]',
      field: 'at',
    }))
  })

  it('引用类型与字段不符 → param.ref.type_mismatch', () => {
    const { model } = parseScript(
      "params:\n  - 'bool:enable:开关:true'\nsteps:\n  - tap: $enable\n",
    )
    const diags = validateScript(model)
    expect(diags).toContainEqual(expect.objectContaining({
      code: 'param.ref.type_mismatch',
      step_path: 'steps[0]',
      field: 'at',
    }))
  })

  it('参数类型切换后，引用错误随之传播（编辑器改类型场景）', () => {
    const { model } = parseScript(
      "params:\n  - 'coord:pos:位置:[0.5, 0.5]'\nsteps:\n  - tap: $pos\n",
    )
    expect(validateScript(model)).toEqual([])
    // 通过 update_param 把 coord 改成 bool（命令栈路径，见 commands.test）
    model.params[0].type = 'bool'
    const diags = validateScript(model)
    expect(diags).toContainEqual(expect.objectContaining({
      code: 'param.ref.type_mismatch',
      step_path: 'steps[0]',
      field: 'at',
    }))
  })

  it('coord 字面量超出 0~1 → step.coord.range', () => {
    const { model } = parseScript('steps:\n  - tap: [1.5, 0.5]\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'step.coord.range',
      step_path: 'steps[0]',
      field: 'at',
    }))
  })

  it('time 缺单位 → step.time.format；颜色非法 → step.color.format', () => {
    const { model } = parseScript('steps:\n  - wait: 30\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({ code: 'step.time.format' }))

    const { model: m2 } = parseScript("steps:\n  - color:\n      at: [0.1, 0.1]\n      expect:\n        - '12x456':\n          - log: a\n")
    expect(validateScript(m2)).toContainEqual(expect.objectContaining({ code: 'step.color.format' }))
  })

  it('if 条件非布尔 → step.if.non_bool_cond', () => {
    const { model } = parseScript('steps:\n  - if: yes\n    then: []\n')
    const diags = validateScript(model)
    expect(diags).toContainEqual(expect.objectContaining({ code: 'step.if.non_bool_cond', step_path: 'steps[0]' }))
  })

  it('未知按键 → step.field.type_mismatch（按键枚举见 schema.KEY_ENUM）', () => {
    const { model } = parseScript('steps:\n  - key: LAUNCH_MISSILE\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({ code: 'step.field.type_mismatch', field: 'key' }))
  })
})

describe('validation：结构约束', () => {
  it('loop 子流程为空 → step.loop.empty_steps', () => {
    const { model } = parseScript('steps:\n  - loop:\n      times: 3\n      steps: []\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'step.loop.empty_steps', step_path: 'steps[0]', field: 'steps',
    }))
  })

  it('wait 随机区间起点大于终点 → step.wait.range_invalid', () => {
    const { model } = parseScript('steps:\n  - wait: [3s, 1s]\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'step.wait.range_invalid', step_path: 'steps[0]', field: 'duration_max',
    }))
  })

  it('return 出现在脚本 → step.return.in_script；函数库中合法', () => {
    const { model } = parseScript('steps:\n  - return: true\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'step.return.in_script', step_path: 'steps[0]',
    }))
    const lib = { file: 'common', functions: [{ name: 'f', params: [], steps: [makeStep('return')] }] }
    expect(validateFunctionLibrary(lib)).toEqual([])
  })

  it('嵌套超限 → step.nesting.depth（默认 32 层）', () => {
    // 34 层嵌套 loop（直接构造模型，避免深层 YAML 缩进构造）
    let step = makeStep('log')
    for (let i = 0; i < 33; i++) {
      const loop = makeStep('loop')
      loop.steps = [step]
      step = loop
    }
    const model = { params: [], config: null, steps: [step] }
    const diags = validateScript(model)
    expect(diags.some((d) => d.code === 'step.nesting.depth')).toBe(true)
  })

  it('变量名重复/非法 → param.decl.name_duplicate（parse 期诊断）', () => {
    const { diagnostics } = validateSource(
      "params:\n  - 'bool:a:开关:true'\n  - 'bool:a:再来一个:false'\nsteps: []\n",
      'script',
    )
    expect(diagnostics).toContainEqual(expect.objectContaining({ code: 'param.decl.name_duplicate', step_path: 'params[1]' }))
  })
})

describe('validation：resolver 接口（目标信息由调用方传入）', () => {
  it('call：未知 args 键 / 必填缺失 / resolver 命中缺失目标', () => {
    const { model } = parseScript(
      "params:\n  - 'bool:enable:开关:true'\nsteps:\n  - call: sub.yaml\n    args:\n      enable: $enable\n      junk: 1\n",
    )
    const diags = validateScript(model, {
      // 目标声明：enable（传入）+ other（必填未传）
      resolveCall: (target) => (target === 'sub.yaml'
        ? {
            params: [
              { type: 'bool', name: 'enable', remark: '开关', default: null },
              { type: 'text', name: 'other', remark: '其他', default: null },
            ],
          }
        : null),
    })
    expect(diags).toContainEqual(expect.objectContaining({ code: 'param.args.unknown', field: 'args' }))
    expect(diags).toContainEqual(expect.objectContaining({ code: 'param.args.missing_required', field: 'args' }))

    // 目标不存在
    const diags2 = validateScript(model, { resolveCall: () => null })
    expect(diags2).toContainEqual(expect.objectContaining({ code: 'resource.script.not_found' }))
  })

  it('func：语法（缺 / 函数名）与 ref.func.missing_args', () => {
    const { model } = parseScript('steps:\n  - func: login\n    args: {}\n')
    const diags = validateScript(model, {
      resolveFunction: () => ({ params: [{ type: 'bool', name: 'on', remark: '开', default: null }] }),
    })
    expect(diags).toContainEqual(expect.objectContaining({ code: 'ref.func.syntax' }))
  })

  it('未提供 resolver 时跳过目标绑定检查（本层只留接口）', () => {
    const { model } = parseScript('steps:\n  - call: whatever.yaml\n    args: {}\n')
    expect(validateScript(model)).toEqual([])
  })
})

describe('validation：参数声明保存前校验（备注段非空，与服务端 param.decl.format 同构）', () => {
  it('备注段为空 → param.decl.format（codec 解析层宽容，校验层阻断保存）', () => {
    // 解析层允许空备注（ParamEditor 新建行 remark='' 中间态）：无解析期诊断
    const parsed = parseScript("params:\n  - 'text:tag:'\nsteps: []\n")
    expect(parsed.diagnostics).toEqual([])
    const diags = validateScript(parsed.model)
    expect(diags).toContainEqual(expect.objectContaining({
      code: 'param.decl.format',
      step_path: 'params[0]',
      field: 'declaration',
    }))
    expect(diags.find((d) => d.code === 'param.decl.format').message).toContain('备注不能为空')
  })

  it('空备注 + 默认值（第 4 段非空）同样被拦截', () => {
    const { model } = parseScript("params:\n  - 'text:tag::vip'\nsteps: []\n")
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'param.decl.format',
      step_path: 'params[0]',
      field: 'declaration',
    }))
  })

  it('函数库函数级参数空备注同样被拦截', () => {
    const { model } = parseFunctionLibrary("login:\n  params:\n    - 'bool:dry:'\n  steps:\n    - return: true\n", { file: 'common' })
    expect(validateFunctionLibrary(model)).toContainEqual(expect.objectContaining({
      code: 'param.decl.format',
      step_path: 'login.params[0]',
      field: 'declaration',
    }))
  })
})
