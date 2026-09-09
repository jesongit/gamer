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

import type { ParamDecl, ParamType } from './model'
import { PARAM_TYPES } from './model'

export { PARAM_TYPES }

/** CellEditor 需要的唯一控件类型映射。Schema 原文类型不再由各组件自行猜测。 */
export type ParamControlType = 'expr' | 'text' | 'number' | 'bool' | 'time' | 'coord' | 'tmpl' | 'key' | 'json'

/**
 * 正式 ParamType → 编辑控件类型。
 *
 * `list`/`object` 使用 json 控件契约：控件可以用 JSON 文本实现，但提交值
 * 必须仍是数组/对象，不能把 JSON 文本作为字符串写回模型。
 */
export function paramControlType(type: ParamType | string): ParamControlType {
  switch (normalizeParamType(String(type))) {
    case 'any': return 'expr'
    case 'boolean': return 'bool'
    case 'integer':
    case 'number': return 'number'
    case 'string': return 'text'
    case 'list':
    case 'object': return 'json'
    case 'duration': return 'time'
    case 'point': return 'coord'
    case 'template': return 'tmpl'
    case 'key': return 'key'
    default: return 'text'
  }
}

/** 类型切换/勾选“有默认值”时使用的真实 JSON 值。每次返回新容器。 */
const PARAM_DEFAULTS: Readonly<Record<ParamType, unknown>> = {
  any: null,
  boolean: true,
  integer: 0,
  number: 0,
  string: '',
  list: [],
  object: {},
  duration: '1s',
  point: [0.5, 0.5],
  template: '',
  key: 'BACK',
}

/** 返回类型化默认值；list/object 不共享可变引用。 */
export function defaultLiteralForType(type: ParamType | string): unknown {
  const normalized = normalizeParamType(String(type))
  if (!(PARAM_TYPES as readonly string[]).includes(normalized)) return null
  return cloneSchemaValue(PARAM_DEFAULTS[normalized])
}

/** 必填无默认值的编辑占位值。null 表示未填，不伪造合法坐标/模板/数字。 */
export function missingLiteralForType(_type: ParamType | string): null {
  return null
}

/** JSON 安全的 Schema 值深拷贝；保留 false/0/null、数组和对象的真实类型。 */
export function cloneSchemaValue<T>(value: T): T {
  if (value === undefined || value === null) return value
  if (typeof structuredClone === 'function') {
    try {
      return structuredClone(value)
    } catch {
      // Vue reactive proxy 等非原生可克隆对象继续走 JSON 安全值路径。
    }
  }
  return JSON.parse(JSON.stringify(value)) as T
}

/**
 * 是否显式存在默认值。
 *
 * null 在旧 ParamDecl 形态中表示“未声明”，只有 parser 明确写入
 * `hasDefault: true` 时才表示 YAML 中显式的 `default: null`。false/0/''
 * 都是合法的已声明默认值，不能用 truthy 判断。
 */
export function hasParamDefault(
  param: Pick<ParamDecl, 'default'> & { hasDefault?: boolean },
): boolean {
  if (param.hasDefault === true) return true
  if (param.hasDefault === false) return false
  return param.default !== null && param.default !== undefined
}

/** 参数声明输入的宽松形态（服务端 descriptor、YAML model、编辑命令共用）。 */
export interface ParamDeclInput {
  name?: unknown
  type?: unknown
  required?: unknown
  default?: unknown
  hasDefault?: boolean
  desc?: unknown
}

/**
 * 规范化一条参数声明，统一 type/required/default/desc 的模型形态。
 * 默认值存在时 required 自动降为 false；显式 null 需用 hasDefault 标记。
 */
export function normalizeParamDecl(input: ParamDeclInput): ParamDecl {
  const rawType = typeof input.type === 'string' ? input.type : 'string'
  const normalized = normalizeParamType(rawType)
  const type = (PARAM_TYPES as readonly string[]).includes(normalized)
    ? normalized
    : 'string'
  const hasDefault = input.hasDefault === true
    || (input.hasDefault !== false && input.default !== null && input.default !== undefined)
  const decl: ParamDecl = {
    name: typeof input.name === 'string' ? input.name : '',
    type,
    required: input.required === true && !hasDefault,
    default: hasDefault ? cloneSchemaValue(input.default === undefined ? null : input.default) : null,
    desc: typeof input.desc === 'string' ? input.desc : '',
  }
  // 只有显式 null 才需要额外字段，非 null 值由 hasParamDefault 推断，保持旧
  // 模型快照和现有命令结构稳定。
  if (input.hasDefault === true && decl.default === null) decl.hasDefault = true
  return decl
}

export function normalizeParamDecls(inputs: readonly ParamDeclInput[] | null | undefined): ParamDecl[] {
  return Array.isArray(inputs) ? inputs.map((input) => normalizeParamDecl(input)) : []
}

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
      if (value === undefined) {
        return { code: 'yaml.field.type', message: 'any 值不能是 undefined（请填写 null 或其他 JSON 字面量）' }
      }
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
      if (typeof value === 'string' && value.trim() !== '') return null
      return { code: 'yaml.field.type', message: `string 值应为非空字符串，收到 ${got}` }
    case 'template':
      if (typeof value === 'string' && value.trim() !== '') return null
      return { code: 'yaml.field.type', message: `template 值应为非空字符串，收到 ${got}` }
    case 'key':
      if (typeof value === 'string' && value.trim() !== '') {
        if (!isKnownKey(value)) return { code: 'yaml.field.type', message: `未知按键 ${JSON.stringify(value)}` }
        return null
      }
      return { code: 'yaml.field.type', message: `key 值应为按键名或数字 keycode，收到 ${got}` }
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
  switch (type.trim().toLowerCase()) {
    case 'bool': return 'boolean'
    case 'int': return 'integer'
    case 'float': return 'number'
    case 'text': return 'string'
    default:
      return (type.trim().toLowerCase() as ParamType)
  }
}
