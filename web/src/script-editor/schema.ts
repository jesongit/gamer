/**
 * 字段、类型与上下文约束（契约 §3.3 / §3.6 / §6.1）。
 *
 * 七类参数字面量的解析与校验集中在这里，供两处消费：
 * - codec：ParamDecl 默认值解析（保存前类型校验，非法 → param.default.invalid）；
 * - validation：步骤字段 Cell 字面量校验（coord 范围 / time 格式 / color 格式 / key 枚举等）。
 *
 * 按键枚举提取自 server/src/engine.rs::key_code（只读参考，2026-08-29 版本）。
 */

import type { ParamLiteral, ParamType } from './model'

// ---------- 按键枚举（server/src/engine.rs::key_code 的命名键；大小写不敏感） ----------

/**
 * 命名按键 + 数字按键（"0"~"9"）+ 任意数字串（原始 Android keycode 透传）。
 * engine::key_code 对 APP_SWITCH/RECENTS、VOL_UP/VOLUME_UP、DEL/BACKSPACE 提供别名，
 * 规范化统一用第一组拼写。
 */
export const KEY_ENUM: readonly string[] = [
  'HOME', 'BACK', 'APP_SWITCH', 'RECENTS', 'MENU',
  'VOL_UP', 'VOLUME_UP', 'VOL_DOWN', 'VOLUME_DOWN', 'POWER',
  'ENTER', 'DEL', 'BACKSPACE', 'TAB', 'SPACE', 'ESC', 'SEARCH',
  'CAMERA', 'FOCUS', 'NOTIFICATION', 'SETTINGS', 'MUTE', 'HEADSETHOOK',
  'WAKEUP', 'SLEEP',
  '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
]

/** 是否为 engine::key_code 可解析的按键：命名枚举（大小写不敏感）或纯数字 keycode。 */
export function isKnownKey(value: string): boolean {
  if (/^[0-9]+$/.test(value)) return true
  return (KEY_ENUM as readonly string[]).includes(value.toUpperCase())
}

// ---------- 时间（契约：单位 ms/s/m/min/h/d，必须带单位且 >0；m ≡ min 可小数） ----------

const TIME_RE = /^([0-9]+(?:\.[0-9]+)?)(ms|s|m|min|h|d)$/

/** 解析带单位时间串为毫秒；非法（缺单位/未知单位/≤0/非数值）返回 null。单位大小写不敏感。 */
export function parseTimeMs(raw: string): number | null {
  const m = TIME_RE.exec(raw.trim())
  if (!m) return null
  const n = Number(m[1])
  if (!Number.isFinite(n) || n <= 0) return null
  switch (m[2]) {
    case 'ms': return n
    case 's': return n * 1000
    case 'm': case 'min': return n * 60_000
    case 'h': return n * 3_600_000
    case 'd': return n * 86_400_000
    default: return null
  }
}

// ---------- 颜色（契约 §4.2：所有位置统一 6 位十六进制小写、无 #、字符串） ----------

/** 6 位十六进制（大小写不敏感）。 */
export function isColorLiteral(value: string): boolean {
  return /^[0-9a-fA-F]{6}$/.test(value)
}

/** 归一化为小写；非颜色串原样返回。 */
export function normalizeColor(value: string): string {
  return isColorLiteral(value) ? value.toLowerCase() : value
}

// ---------- 坐标（两个 0~1 的数字） ----------

export function isCoordLit(value: unknown): value is [number, number] {
  return (
    Array.isArray(value) && value.length === 2
    && Number.isFinite(value[0]) && Number.isFinite(value[1])
  )
}

/** coord 范围校验：0~1。 */
export function coordInRange(lit: [number, number]): boolean {
  return lit[0] >= 0 && lit[0] <= 1 && lit[1] >= 0 && lit[1] <= 1
}

// ---------- 变量名 ----------

export const PARAM_NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/

// ---------- ParamDecl 默认值：原始尾串 → 类型化字面量（契约 §3.3 表） ----------

export interface LiteralParseResult {
  ok: boolean
  value?: ParamLiteral
  /** 解析失败原因提示（供 message 拼装）。 */
  reason?: string
}

/** YAML 双引号风格反转义（`\\`、`\"`、`\n`、`\r`、`\t`）；悬空/未知转义返回 null（与 params.rs 对称）。 */
function unescapeDoubleQuoted(s: string): string | null {
  let out = ''
  for (let i = 0; i < s.length; i++) {
    const c = s[i]
    if (c !== '\\') {
      out += c
      continue
    }
    switch (s[++i]) {
      case '\\': out += '\\'; break
      case '"': out += '"'; break
      case 'n': out += '\n'; break
      case 'r': out += '\r'; break
      case 't': out += '\t'; break
      default: return null
    }
  }
  return out
}

/** 按声明类型解析默认值原始尾串。空尾串由调用方先判（param.default.empty）。 */
export function parseParamLiteral(type: ParamType, raw: string): LiteralParseResult {
  switch (type) {
    case 'bool':
      if (raw === 'true') return { ok: true, value: true }
      if (raw === 'false') return { ok: true, value: false }
      return { ok: false, reason: `bool 默认值只能是 true/false，收到 ${JSON.stringify(raw)}` }
    case 'coord': {
      const m = /^\[\s*([+-]?[0-9.]+)\s*,\s*([+-]?[0-9.]+)\s*\]$/.exec(raw)
      if (!m) return { ok: false, reason: `coord 默认值应为 [x, y] 形态，收到 ${JSON.stringify(raw)}` }
      const x = Number(m[1])
      const y = Number(m[2])
      if (!Number.isFinite(x) || !Number.isFinite(y)) {
        return { ok: false, reason: 'coord 默认值必须是两个数字' }
      }
      return { ok: true, value: [x, y] }
    }
    case 'color': {
      if (!isColorLiteral(raw)) {
        return { ok: false, reason: `color 默认值应为 6 位十六进制，收到 ${JSON.stringify(raw)}` }
      }
      return { ok: true, value: normalizeColor(raw) }
    }
    case 'time': {
      if (parseTimeMs(raw) === null) {
        return { ok: false, reason: `time 默认值须带单位（ms/s/m/min/h/d）且 >0，收到 ${JSON.stringify(raw)}` }
      }
      return { ok: true, value: raw }
    }
    case 'key': {
      if (!isKnownKey(raw)) {
        return { ok: false, reason: `key 默认值不在服务端按键枚举内，收到 ${JSON.stringify(raw)}` }
      }
      return { ok: true, value: raw }
    }
    case 'text': {
      // 与服务端同构（params.rs Text）：外层双引号可选——有则剥离并反转义，无则取
      // 原始尾串整体为默认值（可含冒号/空格）；空尾串由调用方先判（param.default.empty）。
      if (raw.length >= 2 && raw.startsWith('"') && raw.endsWith('"')) {
        const unescaped = unescapeDoubleQuoted(raw.slice(1, -1))
        if (unescaped === null) {
          return { ok: false, reason: `text 默认值 ${JSON.stringify(raw)} 的转义序列非法` }
        }
        return { ok: true, value: unescaped }
      }
      return { ok: true, value: raw }
    }
    case 'tmpl': {
      if (raw.length === 0) return { ok: false, reason: 'tmpl 默认值不能为空' }
      return { ok: true, value: raw }
    }
  }
}

// ---------- 步骤字段 Cell 字面量校验（validation 用） ----------

/** 字段类型 → 字面量校验；返回错误码（null = 合法）。code 取值见 diagnostics.ts CODES。 */
export function checkCellLiteral(
  type: ParamType,
  lit: unknown,
): { code: string; message: string } | null {
  switch (type) {
    case 'coord':
      if (!isCoordLit(lit)) return { code: 'step.field.type_mismatch', message: '坐标字面量应为 [x, y] 两个数字' }
      if (!coordInRange(lit)) return { code: 'step.coord.range', message: `坐标超出 0~1：[${lit[0]}, ${lit[1]}]` }
      return null
    case 'color':
      if (typeof lit !== 'string' || !isColorLiteral(lit)) {
        return { code: 'step.color.format', message: `颜色应为 6 位十六进制，收到 ${JSON.stringify(lit)}` }
      }
      return null
    case 'time':
      if (typeof lit !== 'string' || parseTimeMs(lit) === null) {
        return { code: 'step.time.format', message: `时间须带单位（ms/s/m/min/h/d）且 >0，收到 ${JSON.stringify(lit)}` }
      }
      return null
    case 'key':
      if (typeof lit !== 'string' || !isKnownKey(lit)) {
        return { code: 'step.field.type_mismatch', message: `未知按键 ${JSON.stringify(lit)}（服务端按键枚举见 schema.KEY_ENUM）` }
      }
      return null
    case 'bool':
      if (typeof lit !== 'boolean') return { code: 'step.field.type_mismatch', message: '布尔字面量应为 true/false' }
      return null
    case 'text':
      if (typeof lit !== 'string') return { code: 'step.field.type_mismatch', message: '文本字面量应为字符串' }
      return null
    case 'tmpl':
      if (typeof lit !== 'string' || lit.length === 0) {
        return { code: 'step.field.missing', message: '模板短名不能为空' }
      }
      return null
  }
}

/** 字段类型（用于引用类型检查 param.ref.type_mismatch）。 */
export type CellFieldType = ParamType

/** 各步骤字段对应的参数类型（契约 §3.5 Model 字段列）。 */
export const STEP_CELL_FIELD_TYPES: Record<string, ParamType> = {
  'tap.at': 'coord',
  'swipe.from': 'coord',
  'swipe.to': 'coord',
  'swipe.time': 'time',
  'key.key': 'key',
  'text.value': 'text',
  'log.message': 'text',
  'wait.duration': 'time',
  'wait.duration_max': 'time',
  'find.template': 'tmpl',
  'find.block': 'tmpl',
  'find.timeout': 'time',
  'match.candidates.template': 'tmpl',
  'match.timeout': 'time',
  'color.at': 'coord',
  'color.expect.color': 'color',
  'if.cond': 'bool',
  'return.value': 'bool',
}
