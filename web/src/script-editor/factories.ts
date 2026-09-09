/**
 * 步骤工厂与添加面板分组（YAML V1）。
 *
 * V1 步骤只有 4 类：函数调用 / if / repeat / return。`tap`、`wait_find`
 * 等原生函数走 callFn 工厂（参数按函数 Schema 预填）；控制流走类型工厂。
 */

import type { Cell, CallArgs, Step } from './model'
import { lit, missingLit, newStepUuid } from './model'
import { cloneSchemaValue, hasParamDefault, missingLiteralForType } from './schema'

/** 创建函数调用步骤：fn + 预填实参 + 可选 as。 */
export function createCall(fn: string, args: CallArgs = { kind: 'none' }, as: string | null = null): Step {
  return { uuid: newStepUuid(), kind: 'call', fn, args, as }
}

/** 创建控制流步骤（if/repeat/return）。 */
export function createControl(kind: 'if' | 'repeat' | 'return'): Step {
  switch (kind) {
    case 'if':
      return { uuid: newStepUuid(), kind: 'if', cond: lit(true), then: [], else: [] }
    case 'repeat':
      return { uuid: newStepUuid(), kind: 'repeat', times: lit(3), body: [] }
    case 'return':
      return { uuid: newStepUuid(), kind: 'return', value: lit(null) }
  }
}

/** 按函数参数 Schema 预填命名实参（兼容模式只预填默认值）。 */
export function argsFromSchema(
  params: { name: string; type: string; required: boolean; default: unknown }[],
  options: { includeRequired?: boolean } = {},
): CallArgs {
  const entries: Record<string, Cell> = {}
  for (const param of params) {
    if (hasParamDefault(param)) {
      entries[param.name] = lit(cloneSchemaValue(param.default))
    } else if (options.includeRequired && param.required) {
      // 必填字段必须在模型中占位，才能由后续类型化编辑器显示并提示填写。
      // 占位是 null，不伪造 0 坐标、空模板等合法值；仅在提交前被替换。
      entries[param.name] = missingLit(missingLiteralForType(param.type))
    }
  }
  return Object.keys(entries).length > 0 ? { kind: 'map', entries } : { kind: 'none' }
}

/** 新增/切换函数的统一初始化：默认值保留真实类型，必填无默认值可见。 */
export function initializeArgsFromSchema(
  params: { name: string; type: string; required: boolean; default: unknown }[],
): CallArgs {
  return argsFromSchema(params, { includeRequired: true })
}

/** 按 Schema 创建调用步骤；旧 makeCall/createCall 保留无 Schema 兼容形态。 */
export function createCallFromSchema(
  fn: string,
  params: { name: string; type: string; required: boolean; default: unknown }[],
  as: string | null = null,
): Step {
  return createCall(fn, initializeArgsFromSchema(params), as)
}

// ---------- 添加面板分组 ----------

/** 控制流面板条目（原生函数目录由面板动态拉取，不在此静态声明）。 */
export type ControlKind = 'if' | 'repeat' | 'return'

export interface ControlEntry {
  kind: ControlKind
  label: string
  hint: string
}

export const CONTROL_ENTRIES: ControlEntry[] = [
  { kind: 'if', label: '条件分支', hint: 'if $x → then / else' },
  { kind: 'repeat', label: '固定循环', hint: 'repeat N 次 → do' },
  { kind: 'return', label: '返回值', hint: '结束并返回一个值' },
]

/** 便捷入口：按函数名创建调用步骤（无 Schema 信息时的兜底形态）。 */
export function makeCall(fn: string): Step {
  return createCall(fn)
}
