/**
 * 字段、类型与字面量约束（YAML V1，与 server gamer_yaml/syntax.rs 对齐）。
 *
 * 集中提供：
 * - 标识符 / 引用路径 / 时间串 / 坐标的基础判定（codec 与校验层共用）；
 * - checkLiteral：V1 参数类型的字面量校验（运行参数表单 params.ts 与
 *   冻结组件 GamerYamlPayloadEditor 依赖此签名）；
 * - 参数类型别名归一（bool→boolean、int→integer、float→number、
 *   text→string）。
 */

import type { ParamType } from './model'
import { PARAM_TYPES } from './model'

export { PARAM_TYPES }

// ---------- 标识符与引用路径 ----------

/** V1 标识符：小写字母/下划线开头，仅小写字母、数字、下划线（函数/参数/变量名共用）。 */
export const IDENTIFIER_RE = /^[a-z_][a-z0-9_]*$/

export function isIdentifier(v: string): boolean {
  return IDENTIFIER_RE.test(v)
}

/**
 * 变量引用路径（Cell.ref，不含前导 $）：`$name` / `$name.field.sub`。
 * V1 只有点号字段访问（无动态索引），段均为小写标识符。
 */
export const REF_PATH_RE = /^[a-z_][a-z0-9_]*(?:\.[a-z_][a-z0-9_]*)*$/

export function isRefPath(v: string): boolean {
  return REF_PATH_RE.test(v)
}

// ---------- 时间（单位 ms/s/m/min/h/d；0 合法；裸数字 = 毫秒） ----------

export const TIME_UNITS = ['ms', 's', 'm', 'h', 'd'] as const

const TIME_RE = /^([0-9]+(?:\.[0-9]+)?)\s*(ms|s|m|min|h|d)$/

/** 解析带单位时间串为毫秒；非法（缺单位/未知单位/负数/非数值）返回 null。单位大小写不敏感。 */
export function parseTimeMs(raw: string): number | null {
  const m = TIME_RE.exec(raw.trim().toLowerCase())
  if (!m) return null
  const n = Number(m[1])
  if (!Number.isFinite(n) || n < 0) return null
  switch (m[2]) {
    case 'ms': return n
    case 's': return n * 1000
    case 'm': case 'min': return n * 60_000
    case 'h': return n * 3_600_000
    case 'd': return n * 86_400_000
    default: return null
  }
}

/** 时间字面量合法性（字符串带单位或非负数字毫秒）。 */
export function isTimeLiteral(v: unknown): boolean {
  if (typeof v === 'number') return Number.isFinite(v) && v >= 0
  return typeof v === 'string' && parseTimeMs(v) !== null
}

// ---------- 坐标（数组 [x, y] 或对象 {x, y}，相对坐标 0~1） ----------

export function isCoordLit(value: unknown): value is [number, number] {
  return (
    Array.isArray(value) && value.length === 2
    && Number.isFinite(value[0]) && Number.isFinite(value[1])
  )
}

export function isCoordObject(value: unknown): value is { x: number; y: number } {
  return (
    typeof value === 'object' && value !== null && !Array.isArray(value)
    && Number.isFinite((value as { x?: unknown }).x)
    && Number.isFinite((value as { y?: unknown }).y)
  )
}

/** 坐标范围校验：0~1（find 的 center / region 等运行产出与输入共用）。 */
export function coordInRange(x: number, y: number): boolean {
  return x >= 0 && x <= 1 && y >= 0 && y <= 1
}

// ---------- 按键 ----------

/** 命名按键（与宿主 key_code 接受表同步）+ 任意数字 keycode。 */
export const KEY_ENUM: readonly string[] = [
  'HOME', 'BACK', 'MENU', 'APP_SWITCH', 'RECENTS',
  'VOL_UP', 'VOLUME_UP', 'VOL_DOWN', 'VOLUME_DOWN',
  'ESC', 'ESCAPE', 'ENTER', 'RETURN', 'SPACE', 'TAB', 'BACKSPACE', 'DEL',
]

/** 是否为服务端可解析的按键：命名枚举（大小写不敏感）或纯数字 keycode。 */
export function isKnownKey(value: string): boolean {
  if (/^[0-9]+$/.test(value)) return true
  return (KEY_ENUM as readonly string[]).includes(value.toUpperCase())
}

// ---------- 字面量校验（V1 参数类型口径） ----------

/**
 * 字面量校验（V1 ParamType；字符串别名在 normalizeParamType 后到达这里）。
 * 返回 {code, message}（null = 合法）。code 取值见 diagnostics.ts CODES。
 */
export function checkLiteral(
  type: ParamType,
  value: unknown,
): { code: string; message: string } | null {
  const got = typeof value === 'string' ? JSON.stringify(value) : Array.isArray(value) ? '数组' : String(value)
  switch (type) {
    case 'any':
      return null
    case 'boolean':
      if (typeof value === 'boolean') return null
      return { code: 'yaml.field.type', message: `boolean 值应为 true/false，收到 ${got}` }
    case 'integer':
      if (typeof value === 'number' && Number.isInteger(value)) return null
      return { code: 'yaml.field.type', message: `integer 值应为整数，收到 ${got}` }
    case 'number':
      if (typeof value === 'number' && Number.isFinite(value)) return null
      return { code: 'yaml.field.type', message: `number 值应为数值，收到 ${got}` }
    case 'string':
    case 'template':
    case 'key': {
      if (typeof value === 'string' && value.trim() !== '') {
        if (type === 'key' && !isKnownKey(value)) {
          return { code: 'yaml.field.type', message: `未知按键 ${JSON.stringify(value)}` }
        }
        return null
      }
      return { code: 'yaml.field.type', message: `${type} 值应为非空字符串，收到 ${got}` }
    }
    case 'list':
      if (Array.isArray(value)) return null
      return { code: 'yaml.field.type', message: `list 值应为数组，收到 ${got}` }
    case 'object':
      if (typeof value === 'object' && value !== null && !Array.isArray(value)) return null
      return { code: 'yaml.field.type', message: `object 值应为对象，收到 ${got}` }
    case 'duration':
      if (isTimeLiteral(value)) return null
      return { code: 'yaml.field.type', message: `duration 须为带单位时间串（${TIME_UNITS.join('/')}）或非负毫秒数，收到 ${got}` }
    case 'point': {
      if (isCoordLit(value)) {
        return coordInRange(value[0], value[1])
          ? null
          : { code: 'yaml.field.type', message: `point 坐标须在 0~1：[${value[0]}, ${value[1]}]` }
      }
      if (isCoordObject(value)) {
        return coordInRange(value.x, value.y)
          ? null
          : { code: 'yaml.field.type', message: `point 坐标须在 0~1：{x: ${value.x}, y: ${value.y}}` }
      }
      return { code: 'yaml.field.type', message: `point 应为 [x, y] 或 {x, y}（0~1 相对坐标），收到 ${got}` }
    }
    default:
      return null
  }
}

// ---------- 参数类型别名归一 ----------

/** 参数类型原文 → 规范类型（bool→boolean、int→integer、float→number、text→string）。 */
export function normalizeParamType(type: string): ParamType {
  switch (type) {
    case 'bool': return 'boolean'
    case 'int': return 'integer'
    case 'float': return 'number'
    case 'text': return 'string'
    default:
      return (type as ParamType)
  }
}
