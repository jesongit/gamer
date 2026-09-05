/**
 * 字段、类型与字面量约束（v3，契约 §1-§4）。
 *
 * 集中提供：
 * - 按键枚举 / 时间串 / 坐标 / 属性路径引用的基础判定（codec 与校验层共用）；
 * - checkCellLiteral：步骤字段 CellType 与参数 ParamType 双口径的字面量校验
 *   （运行参数表单 params.ts 与冻结组件 GamerYamlPayloadEditor 依赖此签名）；
 * - 参数默认值原始尾串解析（params 字符串声明形态，契约 §1 双形态）。
 *
 * 按键枚举提取自 server/src/engine.rs::key_code（只读参考）；时间单位与
 * v3 服务端 parse_duration_ms 对齐（ms/s/m/h，0 合法）。
 */

// ---------- 按键枚举 ----------

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

// ---------- 时间（v3：单位 ms/s/m/h；0 合法；裸整数 = 毫秒） ----------

export const TIME_UNITS = ['ms', 's', 'm', 'h'] as const

const TIME_RE = /^([0-9]+(?:\.[0-9]+)?)(ms|s|m|h)$/

/** 解析带单位时间串为毫秒；非法（缺单位/未知单位/负数/非数值）返回 null。单位大小写不敏感。 */
export function parseTimeMs(raw: string): number | null {
  const m = TIME_RE.exec(raw.trim())
  if (!m) return null
  const n = Number(m[1])
  if (!Number.isFinite(n) || n < 0) return null
  switch (m[2]) {
    case 'ms': return n
    case 's': return n * 1000
    case 'm': return n * 60_000
    case 'h': return n * 3_600_000
    default: return null
  }
}

/** 时间字面量合法性（字符串带单位或非负数字毫秒）。 */
export function isTimeLiteral(v: unknown): boolean {
  if (typeof v === 'number') return Number.isFinite(v) && v >= 0
  return typeof v === 'string' && parseTimeMs(v) !== null
}

// ---------- 坐标（两个数字，相对坐标 0~1） ----------

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

// ---------- 名称与引用路径 ----------

export const PARAM_NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/

/**
 * 表达式引用路径（Cell.ref，不含前导 $）：
 * `user` / `reward.center` / `match.score` / `list[0]` / `a.b[2].c`。
 */
export const REF_PATH_RE = /^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*|\[\d+\])*$/

export function isRefPath(v: string): boolean {
  return REF_PATH_RE.test(v)
}

// ---------- 字面量校验 ----------

type LiteralCheckOptions = { allowZeroTime?: boolean }

/**
 * 字面量校验（type 兼容步骤字段 CellType 与参数 ParamType 两个口径）。
 * 返回 {code, message}（null = 合法）。code 取值见 diagnostics.ts CODES。
 */
export function checkCellLiteral(
  type: string,
  lit: unknown,
  options: LiteralCheckOptions = {},
): { code: string; message: string } | null {
  switch (type) {
    // ---- 步骤字段口径 ----
    case 'coord':
      if (!isCoordLit(lit)) return { code: 'yaml.v3.field.type', message: '坐标字面量应为 [x, y] 两个数字' }
      if (!coordInRange(lit)) return { code: 'yaml.v3.coord.range', message: `坐标超出 0~1：[${lit[0]}, ${lit[1]}]` }
      return null
    case 'time': {
      if (isTimeLiteral(lit)) return null
      const allowZero = options.allowZeroTime === true
      return {
        code: 'yaml.v3.duration',
        message: `时间须带单位（${TIME_UNITS.join('/')}）且 >=0${allowZero ? '' : ''}，收到 ${JSON.stringify(lit)}`,
      }
    }
    case 'key':
      if (typeof lit !== 'string' || !isKnownKey(lit)) {
        return { code: 'yaml.v3.field.type', message: `未知按键 ${JSON.stringify(lit)}（服务端按键枚举见 schema.KEY_ENUM）` }
      }
      return null
    case 'bool':
      if (typeof lit !== 'boolean') return { code: 'yaml.v3.field.type', message: '布尔字面量应为 true/false' }
      return null
    case 'tmpl':
      if (typeof lit !== 'string' || lit.length === 0) {
        return { code: 'yaml.v3.field.missing', message: '模板短名不能为空' }
      }
      return null
    case 'expr':
      if (lit === undefined) return { code: 'yaml.v3.field.missing', message: '取值不能为空' }
      return null
    case 'number': {
      if (typeof lit === 'number' && Number.isFinite(lit)) return null
      if (typeof lit === 'string' && lit.trim() !== '' && Number.isFinite(Number(lit))) return null
      return { code: 'yaml.v3.number', message: `数字字面量应为数值，收到 ${JSON.stringify(lit)}` }
    }
    // ---- 参数口径（v3 规范五类 + 兼容别名） ----
    case 'text':
    case 'string':
    case 'enum':
      if (typeof lit !== 'string') {
        if (typeof lit === 'number' || typeof lit === 'boolean') return null
        return { code: 'yaml.v3.field.type', message: '字符串参数值应为字符串' }
      }
      return null
    case 'integer':
    case 'int':
      if (typeof lit === 'number' && Number.isInteger(lit)) return null
      if (typeof lit === 'string' && /^-?\d+$/.test(lit.trim())) return null
      return { code: 'yaml.v3.number', message: `整数参数值应为整数，收到 ${JSON.stringify(lit)}` }
    case 'float':
      if (typeof lit === 'number' && Number.isFinite(lit)) return null
      if (typeof lit === 'string' && lit.trim() !== '' && Number.isFinite(Number(lit))) return null
      return { code: 'yaml.v3.number', message: `数字参数值应为数值，收到 ${JSON.stringify(lit)}` }
    default:
      // coord/time/key 等历史 ty 名按字符串口径放行（rawForm 参数保真编辑）
      if (typeof lit === 'string' || typeof lit === 'number' || typeof lit === 'boolean') return null
      return { code: 'yaml.v3.field.type', message: `参数值类型不支持：${JSON.stringify(lit)}` }
  }
}

/** 各步骤字段对应的 Cell 类型（校验与 CellEditor 控件选择用）。 */
export const CELL_FIELD_TYPES: Record<string, string> = {
  'app_start.package': 'expr',
  'app_stop.package': 'expr',
  'tap.at': 'coord',
  'swipe.from': 'coord',
  'swipe.to': 'coord',
  'swipe.duration': 'time',
  'key.key': 'key',
  'text.value': 'text',
  'wait.min': 'time',
  'wait.max': 'time',
  'log.message': 'text',
  'set.value': 'expr',
  'if.cond': 'expr',
  'loop.times': 'number',
  'return.value': 'expr',
  'throw.message': 'expr',
  'find.template': 'tmpl',
  'find.timeout': 'time',
  'find.verify.template': 'tmpl',
  'find.verify.timeout': 'time',
  'match_first.candidates.template': 'tmpl',
  'check.template': 'tmpl',
  'check.timeout': 'time',
}

// ---------- 参数默认值：原始尾串 → 字面量（params 字符串声明形态） ----------

export interface LiteralParseResult {
  ok: boolean
  value?: ParamLiteralValue
  reason?: string
}

type ParamLiteralValue = string | number | boolean

/** YAML 双引号风格反转义（`\\`、`\"`、`\n`、`\r`、`\t`）；悬空/未知转义返回 null。 */
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

/**
 * 按声明类型解析默认值原始尾串。类型为声明原文（canonical 五类或 v2 别名）；
 * 字符串尾允许整体双引号（有则剥离反转义，无则取原文整体，可含冒号/空格）。
 */
export function parseParamLiteral(type: string, raw: string): LiteralParseResult {
  const canonical = normalizeParamType(type)
  if (raw.length >= 2 && raw.startsWith('"') && raw.endsWith('"')) {
    const unescaped = unescapeDoubleQuoted(raw.slice(1, -1))
    if (unescaped === null) {
      return { ok: false, reason: `默认值 ${JSON.stringify(raw)} 的转义序列非法` }
    }
    return stringDefault(canonical, unescaped)
  }
  switch (canonical) {
    case 'boolean':
      if (raw === 'true') return { ok: true, value: true }
      if (raw === 'false') return { ok: true, value: false }
      return { ok: false, reason: `boolean 默认值只能是 true/false，收到 ${JSON.stringify(raw)}` }
    case 'integer':
      if (/^[+-]?\d+$/.test(raw.trim())) return { ok: true, value: Number(raw.trim()) }
      return { ok: false, reason: `integer 默认值应为整数，收到 ${JSON.stringify(raw)}` }
    case 'number':
      if (raw.trim() !== '' && Number.isFinite(Number(raw))) return { ok: true, value: Number(raw) }
      return { ok: false, reason: `number 默认值应为数值，收到 ${JSON.stringify(raw)}` }
    default:
      return stringDefault(canonical, raw)
  }
}

function stringDefault(canonical: string, raw: string): LiteralParseResult {
  if (canonical === 'string' && raw.length === 0) {
    return { ok: true, value: '' }
  }
  return { ok: true, value: raw }
}

/** 参数类型原文 → 规范五类（契约 §7：v2 ty 名映射）。 */
export function normalizeParamType(type: string): ParamType {
  switch (type) {
    case 'number': case 'float':
      return 'number'
    case 'integer': case 'int':
      return 'integer'
    case 'boolean': case 'bool':
      return 'boolean'
    case 'enum':
      return 'enum'
    default:
      return 'string' // string/text/tmpl/key/time/color/coord 及未知 ty 名一律按字符串
  }
}
