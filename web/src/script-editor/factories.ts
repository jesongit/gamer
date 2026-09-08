/**
 * 步骤工厂与添加面板分组（YAML V1）。
 *
 * V1 步骤只有 4 类：函数调用 / if / repeat / return。`tap`、`wait_find`
 * 等原生函数走 callFn 工厂（参数按函数 Schema 预填）；控制流走类型工厂。
 */

import type { Cell, CallArgs, Step } from './model'
import { lit, newStepUuid } from './model'

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

/** 按函数参数 Schema 预填命名实参（有默认值的参数预填默认值；无默认必填参数留字面量空串）。 */
export function argsFromSchema(
  params: { name: string; type: string; required: boolean; default: unknown }[],
): CallArgs {
  const entries: Record<string, Cell> = {}
  for (const param of params) {
    if (param.default !== null && param.default !== undefined) {
      entries[param.name] = lit(param.default)
    }
  }
  return Object.keys(entries).length > 0 ? { kind: 'map', entries } : { kind: 'none' }
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
