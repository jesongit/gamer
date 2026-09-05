/**
 * 运行/测试/定时任务的参数表单共享工具。
 *
 * 三类入口（Console 手动运行 / 函数测试 / TaskBoard 任务快照）共用：
 * - extractParams：从 YAML v3 源码提取 ParamDecl[]（canonical 类型规范化）；
 * - mapArgDiagnostics：服务端 400 invalid_args 诊断 → 表单字段定位（五元组同构）；
 * - describeResolvedArgs：202 resolved_args 摘要（「默认继承/显式覆盖」来源标注）；
 * - 覆盖建议缓存：最近一次显式输入存 localStorage（key 按脚本/函数文件 id），
 *   仅作显式覆盖建议预填，绝不遮蔽当前声明默认值。
 */
import type { ParamDecl, ParamLiteral } from './model'
import { parseFunctionLibrary, parseScript } from './codec'
import { checkCellLiteral } from './schema'

// ---------- 提取 ----------

/**
 * YAML v3 源码 → 参数声明列表。脚本取 Program.params；函数库取指定函数（缺省第一个）的 params。
 * type 保留声明原文（v2 别名 ty 名如 int/tmpl 保真，控件与标签层各自映射）；
 * 规范五类映射经 schema.normalizeParamType（参数 schema API 接入时使用）。
 * 解析失败/无声明/函数不存在 → 空数组（调用方按「无参数直接运行」处理）。
 */
export function extractParams(
  yamlText: string,
  kind: 'script' | 'function_library' = 'script',
  fnName: string | null = null,
): ParamDecl[] {
  try {
    if (kind === 'function_library') {
      const model = parseFunctionLibrary(yamlText ?? '').model
      const fns = Array.isArray(model.functions) ? model.functions : []
      const fn = fnName ? fns.find((f) => f.name === fnName) : fns[0]
      return fn && Array.isArray(fn.params) ? fn.params.map((d) => ({ ...d, rawForm: false })) : []
    }
    const model = parseScript(yamlText ?? '').model
    return Array.isArray(model.params)
      ? model.params.map((d) => ({ ...d, rawForm: false }))
      : []
  } catch {
    return []
  }
}

// ---------- 展示 ----------

export const ARG_TYPE_LABELS: Record<string, string> = {
  // canonical 五类（契约 §7）
  string: '文本', number: '数字', integer: '整数', boolean: '布尔', enum: '枚举',
  // v2 别名 ty 名（rawForm 声明保真展示）
  text: '文本', tmpl: '模板', coord: '坐标', color: '颜色', time: '时间', key: '按键',
  bool: '布尔', int: '整数', float: '数字',
}

/** 字面量 → 短展示串（默认值行 / 摘要 / 对比表共用）；undefined/null → '—'。 */
export function fmtLiteral(v: ParamLiteral | null | undefined): string {
  if (v === null || v === undefined) return '—'
  if (Array.isArray(v)) return `[${v[0]}, ${v[1]}]`
  if (typeof v === 'boolean') return v ? 'true' : 'false'
  return String(v)
}

/** 表单值深拷贝（args 值均为 JSON 安全形态：字符串/数字/布尔/[x,y]）。 */
export function cloneArg<T>(v: T): T {
  return JSON.parse(JSON.stringify(v ?? null))
}

/** 必填参数（无默认值）进入覆盖态时的控件初始字面量（与 CellEditor defaultLiteral 同口径）。 */
export const ARG_DEFAULT_LITERALS: Record<string, ParamLiteral | [number, number]> = {
  string: '', number: 0, integer: 0, boolean: true, enum: '',
  text: '', tmpl: '', coord: [0.5, 0.5], color: 'ff8800', time: '1s', key: 'BACK',
  bool: true, int: 0, float: 0,
}

// ---------- 服务端 400 invalid_args 诊断映射 ----------

export interface ArgDiagnostic {
  code?: string
  message?: string
  resource?: string
  step_path?: string
  field?: string
}

export interface MappedArgDiagnostics {
  /** 参数名 → 错误消息列表（ParamsForm 按 field 标红到行）。 */
  byName: Record<string, string[]>
  /** 无法定位到已声明参数的消息（表单顶部通用错误区展示）。 */
  other: string[]
}

/**
 * 400 {error:"invalid_args", diagnostics:[{code,message,resource,step_path,field}]} → 字段定位。
 * field 即参数名；step_path 形如 args.xxx 时取尾段兜底；两者都无法对上已声明参数 → other。
 */
export function mapArgDiagnostics(
  diagnostics: ArgDiagnostic[] | null | undefined,
  knownNames: string[],
): MappedArgDiagnostics {
  const byName: Record<string, string[]> = {}
  const other: string[] = []
  for (const d of diagnostics || []) {
    let field = typeof d?.field === 'string' ? d.field : ''
    if ((!field || !knownNames.includes(field)) && typeof d?.step_path === 'string') {
      const tail = d.step_path.split('.').pop() || ''
      if (knownNames.includes(tail)) field = tail
    }
    const message = String(d?.message || d?.code || '参数不合法')
    if (field && knownNames.includes(field)) {
      (byName[field] ||= []).push(message)
    } else {
      other.push(message)
    }
  }
  return { byName, other }
}

// ---------- resolved_args 摘要 ----------

/**
 * 202 响应摘要：「运行参数：a=1（覆盖）；b=500ms（默认）」。
 * resolved_args 缺失时按「声明默认值 + 本次显式 args」合成；无参数声明返回 ''。
 * 超长截断（toast/日志单行展示）。
 */
export function describeResolvedArgs(
  params: ParamDecl[],
  args: Record<string, unknown> | null | undefined,
  resolved: Record<string, unknown> | null | undefined,
): string {
  if (!params.length) return ''
  const parts = params.map((p) => {
    const overridden = !!args && Object.prototype.hasOwnProperty.call(args, p.name)
    const value = resolved && Object.prototype.hasOwnProperty.call(resolved, p.name)
      ? (resolved as Record<string, unknown>)[p.name]
      : overridden
        ? (args as Record<string, unknown>)[p.name]
        : p.default
    const source = overridden ? '覆盖' : p.default !== null ? '默认' : '必填'
    return `${p.name}=${fmtLiteral(value as ParamLiteral)}（${source}）`
  })
  let text = `运行参数：${parts.join('；')}`
  if (text.length > 240) text = `${text.slice(0, 240)}…`
  return text
}

// ---------- 覆盖建议缓存（localStorage，key 按脚本/函数文件 id） ----------

const RUN_ARGS_PREFIX = 'gb_run_args:'

export function runArgsCacheKey(id: string): string {
  return `${RUN_ARGS_PREFIX}${id}`
}

interface StorageLike {
  getItem: (k: string) => string | null
  setItem: (k: string, v: string) => void
  removeItem: (k: string) => void
}

function defaultStorage(): StorageLike | null {
  try {
    if (typeof localStorage !== 'undefined') return localStorage
  } catch { /* 隐私模式等存取抛错：按无缓存处理 */ }
  return null
}

/** 读覆盖建议：仅返回仍为对象的稀疏映射；损坏/缺失 → {}。 */
export function loadRunArgsSuggestion(id: string, storage: StorageLike | null = defaultStorage()): Record<string, unknown> {
  if (!id || !storage) return {}
  try {
    const raw = storage.getItem(runArgsCacheKey(id))
    if (!raw) return {}
    const v = JSON.parse(raw)
    return v && typeof v === 'object' && !Array.isArray(v) ? v : {}
  } catch {
    return {}
  }
}

/** 写覆盖建议（仅显式覆盖值；调用方保证只传本次进入 args 的稀疏映射）。 */
export function saveRunArgsSuggestion(
  id: string,
  args: Record<string, unknown>,
  storage: StorageLike | null = defaultStorage(),
): void {
  if (!id || !storage || !args || typeof args !== 'object') return
  try {
    storage.setItem(runArgsCacheKey(id), JSON.stringify(args))
  } catch { /* 配额/隐私模式失败静默（建议缓存非关键数据） */ }
}

// ---------- 客户端校验（schema.checkCellLiteral 同规则） ----------

export interface ArgFieldError {
  name: string
  message: string
}

/**
 * 稀疏 args → 按声明校验：缺必填（default === null 且未提供）→ missing；
 * 提供值类型不合规 → checkCellLiteral 的错误码/文案。未知参数名此处不查（表单只产已知名）。
 */
export function validateArgsAgainstParams(
  params: ParamDecl[],
  args: Record<string, unknown> | null | undefined,
): ArgFieldError[] {
  const errs: ArgFieldError[] = []
  for (const p of params) {
    const provided = !!args && Object.prototype.hasOwnProperty.call(args, p.name)
    if (!provided) {
      if (p.default === null) errs.push({ name: p.name, message: `必填参数 $${p.name} 缺失` })
      continue
    }
    const err = checkCellLiteral(p.type, (args as Record<string, unknown>)[p.name])
    if (err) errs.push({ name: p.name, message: err.message })
  }
  return errs
}
