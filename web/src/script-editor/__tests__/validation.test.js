import { describe, expect, it } from 'vitest'
import { validateSource, validateScript, validateFunctionLibrary } from '../validation'
import { parseFunctionLibrary, parseScript } from '../codec'
import { makeStep } from '../factories'

/**
 * 结构化客户端校验（v3）：引用路径/类型/范围/流程上下文/call 命名空间/defaults。
 * 错误码 yaml.v3.* 与服务端对齐。
 */

describe('validation：字面量类型与范围', () => {
  it('coord 超出 0~1 → yaml.v3.coord.range', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - tap: [1.5, 0.5]\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.coord.range', step_path: 'steps[0]', field: 'at',
    }))
  })

  it('time 缺单位 → yaml.v3.duration；0ms 与裸毫秒数字合法', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - wait: soon\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({ code: 'yaml.v3.duration', field: 'min' }))

    const ok = parseScript('version: 3\nsteps:\n  - wait: 0ms\n  - wait: 250\n')
    expect(validateScript(ok.model)).toEqual([])
  })

  it('未知按键 → yaml.v3.field.type（按键枚举见 schema.KEY_ENUM）', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - key: LAUNCH_MISSILE\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({ code: 'yaml.v3.field.type', field: 'key' }))
  })

  it('threshold 超范围 → yaml.v3.threshold.range（find/check/候选）', () => {
    const { model } = parseScript([
      'version: 3',
      'steps:',
      '  - find: {template: a.png, threshold: 1.5}',
      '  - check: {template: a.png, threshold: -0.1}',
      '  - match_first:',
      '      candidates:',
      '        - template: a.png',
      '          threshold: 2',
    ].join('\n'))
    const diags = validateScript(model)
    expect(diags.filter((d) => d.code === 'yaml.v3.threshold.range')).toHaveLength(3)
  })

  it('find 非空模板必需（field.missing 由 parse 产出，此处校验空串）', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - find: {template: ""}\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.field.missing', step_path: 'steps[0]', field: 'template',
    }))
  })
})

describe('validation：引用路径', () => {
  it('合法属性路径（$reward.center / $list[0]）不校验「已声明」（v3 动态上下文）', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - tap: $reward.center\n  - tap: $list[0]\n  - tap: $match.score\n')
    expect(validateScript(model)).toEqual([])
  })

  it('模型内非法引用路径 → yaml.v3.ref.path_invalid', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - tap: [0.5, 0.5]\n')
    model.steps[0].at = { ref: '1bad.path' }
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.ref.path_invalid', field: 'at',
    }))
  })
})

describe('validation：结构约束', () => {
  it('loop 子流程为空 → yaml.v3.flow.loop_empty_steps', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - loop: {times: 3, steps: []}\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.flow.loop_empty_steps', step_path: 'steps[0]', field: 'steps',
    }))
  })

  it('break 只能出现在 loop 子流程内', () => {
    const outside = parseScript('version: 3\nsteps:\n  - break\n')
    expect(validateScript(outside.model)).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.flow.break_outside_loop', step_path: 'steps[0]',
    }))

    const inside = parseScript([
      'version: 3',
      'steps:',
      '  - loop:',
      '      steps:',
      '        - if: {cond: true, then: [{break}]}',
    ].join('\n'))
    expect(validateScript(inside.model)).not.toContainEqual(expect.objectContaining({
      code: 'yaml.v3.flow.break_outside_loop',
    }))
  })

  it('wait 随机区间起点大于终点 → yaml.v3.wait.range_invalid', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - wait: {min: 3s, max: 1s}\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.wait.range_invalid', step_path: 'steps[0]', field: 'max',
    }))
  })

  it('set 变量名必填', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - set: {name: "", value: 1}\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.field.string', step_path: 'steps[0]', field: 'name',
    }))
  })

  it('return 出现在脚本 → yaml.v3.flow.return_in_script；函数库中合法', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - return: true\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.flow.return_in_script', step_path: 'steps[0]',
    }))
    const lib = { file: 'common', functions: [{ name: 'f', params: [], steps: [makeStep('return')] }] }
    expect(validateFunctionLibrary(lib)).toEqual([])
  })

  it('嵌套超限 → yaml.v3.flow.nesting_depth（默认 32 层）', () => {
    // 34 层嵌套 loop（直接构造模型，避免深层 YAML 缩进构造）
    let step = makeStep('log')
    for (let i = 0; i < 33; i++) {
      const loop = makeStep('loop')
      loop.steps = [step]
      step = loop
    }
    const model = { version: 3, params: [], defaults: null, steps: [step] }
    const diags = validateScript(model)
    expect(diags.some((d) => d.code === 'yaml.v3.flow.nesting_depth')).toBe(true)
  })

  it('变量名重复/非法 → yaml.v3.params.name_duplicate（parse 期诊断）', () => {
    const { diagnostics } = validateSource(
      "version: 3\nparams:\n  - 'bool:a:开关:true'\n  - 'bool:a:再来一个:false'\nsteps: []\n",
      'script',
    )
    expect(diagnostics).toContainEqual(expect.objectContaining({ code: 'yaml.v3.params.name_duplicate', step_path: 'params[1]' }))
  })
})

describe('validation：defaults 范围', () => {
  it('threshold 超范围 / timing 非法时间串', () => {
    const { model } = parseScript([
      'version: 3',
      'defaults:',
      '  vision:',
      '    threshold: 1.4',
      '  timing:',
      '    after_tap: 300',
      'steps: []',
    ].join('\n'))
    const diags = validateScript(model)
    expect(diags).toContainEqual(expect.objectContaining({ code: 'yaml.v3.threshold.range', step_path: 'defaults.vision.threshold' }))
    // 裸数字 300 是合法毫秒 → 无 timing 错误
    expect(diags.filter((d) => d.code === 'yaml.v3.duration')).toHaveLength(0)
  })

  it('timing 缺单位字符串报错', () => {
    const { model } = parseScript('version: 3\ndefaults:\n  timing:\n    after_tap: soon\nsteps: []\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.duration', step_path: 'defaults.timing.after_tap',
    }))
  })
})

describe('validation：call 命名空间（契约 §2）', () => {
  it('裸 target → yaml.v3.call.namespace', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - call: {target: sub_task}\n')
    expect(validateScript(model)).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.call.namespace', step_path: 'steps[0]', field: 'target',
    }))
  })

  it('function: 缺函数名段 → namespace；路径穿越 → path_traversal', () => {
    const bad = parseScript('version: 3\nsteps:\n  - call: {target: "function:login"}\n')
    expect(validateScript(bad.model)).toContainEqual(expect.objectContaining({ code: 'yaml.v3.call.namespace' }))

    const trav = parseScript('version: 3\nsteps:\n  - call: {target: "script:../etc"}\n')
    expect(validateScript(trav.model)).toContainEqual(expect.objectContaining({ code: 'yaml.v3.call.path_traversal' }))
  })

  it('script: 自环（selfScript 命中）→ yaml.v3.call.self_cycle', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - call: {target: "script:daily/login"}\n')
    expect(validateScript(model, { selfScript: 'daily/login' })).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.call.self_cycle',
    }))
  })

  it('with 绑定：未知键 / 必填缺失 / 目标不存在（resolver 由调用方传入）', () => {
    const { model } = parseScript([
      'version: 3',
      'steps:',
      '  - call:',
      '      target: script:sub',
      '      with:',
      '        enable: true',
      '        junk: 1',
    ].join('\n'))
    const diags = validateScript(model, {
      resolveCall: (target) => (target === 'script:sub'
        ? {
            params: [
              { type: 'boolean', name: 'enable', remark: '开关', default: null },
              { type: 'string', name: 'other', remark: '其他', default: null },
            ],
          }
        : null),
    })
    expect(diags).toContainEqual(expect.objectContaining({ code: 'yaml.v3.call.args_unknown', field: 'with' }))
    expect(diags).toContainEqual(expect.objectContaining({ code: 'yaml.v3.call.args_missing_required', field: 'with' }))

    const diags2 = validateScript(model, { resolveCall: () => null })
    expect(diags2).toContainEqual(expect.objectContaining({ code: 'yaml.v3.call.script_not_found' }))
  })

  it('function: 目标存在性走 resolveFunction', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - call: {target: "function:common/login"}\n')
    const diags = validateScript(model, { resolveFunction: () => null })
    expect(diags).toContainEqual(expect.objectContaining({ code: 'yaml.v3.call.function_not_found' }))
  })

  it('未提供 resolver 时跳过目标存在性检查', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - call: {target: "script:whatever"}\n')
    expect(validateScript(model)).toEqual([])
  })
})

describe('validation：模板存在性（resolver 可选）', () => {
  it('模板不存在仍报 yaml.v3.resource.tmpl_not_found', () => {
    const { model } = parseScript('version: 3\nsteps:\n  - check: {template: logo.png}\n')
    const diags = validateScript(model, { resolveTemplate: () => false })
    expect(diags).toContainEqual(expect.objectContaining({
      code: 'yaml.v3.resource.tmpl_not_found', step_path: 'steps[0]', field: 'template',
    }))
    expect(validateScript(model, { resolveTemplate: () => true })).toEqual([])
  })
})

describe('validation：函数库', () => {
  it('函数内 return 合法；call 上下文校验按函数路径', () => {
    const { model } = parseFunctionLibrary([
      'login:',
      '  steps:',
      '    - return: $ok',
    ].join('\n'), { file: 'common' })
    expect(validateFunctionLibrary(model)).toEqual([])
  })

  it('函数级参数名重复报 yaml.v3.params.name_duplicate', () => {
    const { model, diagnostics } = parseFunctionLibrary([
      'login:',
      '  params:',
      "    - 'bool:a:开:true'",
      "    - name: a",
      '      type: boolean',
      '  steps:',
      '    - return: true',
    ].join('\n'), { file: 'common' })
    expect(diagnostics).toContainEqual(expect.objectContaining({ code: 'yaml.v3.params.name_duplicate', step_path: 'login.params[1]' }))
  })
})
