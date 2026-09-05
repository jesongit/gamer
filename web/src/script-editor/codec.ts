/**
 * YAML v3 ↔ Model 双向转换（契约：docs/plans/phase12_v3_dsl_contract.md §1-§4）。
 *
 * 解析：js-yaml plain load（CORE_SCHEMA）；空文档视为空映射（→ version.missing）。
 * v3 无引号样式契约（v2 的 params 整条单引号规则已死），无需事件级 AST。
 * 非 version: 3 一律拒绝（yaml.v3.version / version.missing），不误解析不 fallback
 * （ADR-YAML-01）；v2 步骤名（func/match/color/str_app/cls_app 等）给出明确迁移诊断。
 *
 * 序列化：手写确定性规范输出器（同 model 恒同输出），统一 +2 缩进、无紧凑形态；
 * 验收锚点：decode(encode(model)) == model（结构相等）且 encode 语义稳定。
 */

import { CORE_SCHEMA, dump, load } from 'js-yaml'
import {
  allocateUuids,
  isRefCell,
  type Cell,
  type DefaultsModel,
  type FunctionLibraryModel,
  type FunctionModel,
  type ParamDecl,
  type Program,
  type Step,
  STEP_KINDS,
  type StepKind,
  yamlKeyOf,
  newStepUuid,
} from './model'
import { CODES, diag, type Diagnostic } from './diagnostics'
import {
  isCoordLit,
  isRefPath,
  parseParamLiteral,
  parseTimeMs,
  PARAM_NAME_RE,
} from './schema'

// ---------- 公共 API ----------

export interface ParseOptions {
  /** 函数库文件短路径（FunctionLibraryModel.file）；脚本可省略。 */
  file?: string
}

export interface ScriptParseResult {
  kind: 'script'
  model: Program
  diagnostics: Diagnostic[]
}

export interface FunctionLibraryParseResult {
  kind: 'function_library'
  model: FunctionLibraryModel
  diagnostics: Diagnostic[]
}

export type ParseResult = ScriptParseResult | FunctionLibraryParseResult

/** 解析可执行脚本（scripts/ 目录类型；version: 3 强制）。 */
export function parseScript(text: string, _opts: ParseOptions = {}): ScriptParseResult {
  const diags: Diagnostic[] = []
  const root = parseDocument(text, diags)
  if (root === null) {
    return {
      kind: 'script',
      model: emptyProgram(),
      diagnostics: [diag(CODES.yamlSyntax, '', 'yaml', `YAML 解析失败：${loadError ?? '文档为空'}`)],
    }
  }
  const model = parseScriptRoot(root, diags)
  return { kind: 'script', model: withUuids(model), diagnostics: diags }
}

/** 解析函数库（functions/ 目录类型；顶层键 = 函数名，bare-map，无 version 键）。 */
export function parseFunctionLibrary(text: string, opts: ParseOptions = {}): FunctionLibraryParseResult {
  const diags: Diagnostic[] = []
  const root = parseDocument(text, diags)
  if (root === null) {
    return {
      kind: 'function_library',
      model: { file: opts.file ?? '', functions: [] },
      diagnostics: [diag(CODES.yamlSyntax, '', 'yaml', `YAML 解析失败：${loadError ?? '文档为空'}`)],
    }
  }
  const model = parseFunctionRoot(root, opts.file ?? '', diags)
  return { kind: 'function_library', model: withUuids(model), diagnostics: diags }
}

export function parseSource(
  text: string,
  kind: 'script' | 'function_library',
  opts: ParseOptions = {},
): ParseResult {
  return kind === 'script' ? parseScript(text, opts) : parseFunctionLibrary(text, opts)
}

/** 规范序列化：按 model 形态自动分发（脚本 / 函数库）。输出以单个换行结尾。 */
export function serialize(model: Program | FunctionLibraryModel): string {
  return 'functions' in model ? serializeFunctionLibrary(model) : serializeScript(model)
}

export function emptyProgram(): Program {
  return { version: 3, params: [], defaults: null, steps: [] }
}

// ---------- 参数声明原始串（字符串声明形态，契约 §1 双形态） ----------

/** 参数声明原始串：'type:name:remark[:default]'（规范形态；序列化时整体单引号）。 */
export function paramDeclToRawString(decl: ParamDecl): string {
  const base = `${decl.type}:${decl.name}:${decl.remark}`
  if (decl.default === null || decl.default === undefined) return base
  let tail: string
  if (typeof decl.default === 'boolean') tail = decl.default ? 'true' : 'false'
  else if (typeof decl.default === 'number') tail = fmtNum(decl.default)
  else tail = /[\n\r]/.test(decl.default) ? JSON.stringify(decl.default) : decl.default
  return `${base}:${tail}`
}

// ---------- 标量输出辅助 ----------

function fmtNum(n: number): string {
  return Number.isFinite(n) ? String(n) : 'null'
}

/** 首选 plain 的字符串标量：交由 js-yaml dump 判定 plain 安全性；含换行退回双引号。 */
function plainScalar(s: string): string {
  if (/[\n\r]/.test(s)) return JSON.stringify(s)
  const out = dump(s, { lineWidth: -1 })
  return out.endsWith('\n') ? out.slice(0, -1) : out
}

/** 整条单引号（params 字符串声明形态；单引号样式仅需把 ' 翻倍转义）。 */
function singleQuoted(s: string): string {
  return `'${s.replace(/'/g, "''")}'`
}

function fallbackScalar(v: unknown): string {
  if (v === null || v === undefined) return 'null'
  if (typeof v === 'string') return plainScalar(v)
  if (typeof v === 'number') return fmtNum(v)
  if (typeof v === 'boolean') return String(v)
  // 复合字面量（expr/region 等保留值）：紧凑 JSON 即合法 YAML flow 形态
  return JSON.stringify(v)
}

/** 步骤字段取值单元格行内渲染（lit 形态由字段类型约束）。 */
function cellInline(cell: Cell | null | undefined, type: string): string {
  if (cell === null || cell === undefined) return 'null'
  if (isRefCell(cell)) return `$${cell.ref}`
  const v = cell.lit
  switch (type) {
    case 'coord':
      return isCoordLit(v) ? `[${fmtNum(v[0])}, ${fmtNum(v[1])}]` : fallbackScalar(v)
    case 'bool':
      return v === true ? 'true' : v === false ? 'false' : fallbackScalar(v)
    case 'text':
      return typeof v === 'string' ? JSON.stringify(v) : fallbackScalar(v)
    case 'number':
      return typeof v === 'number' ? fmtNum(v) : fallbackScalar(v)
    default:
      // tmpl / key / time / expr：字符串首选 plain（$ref 形态 plain 安全）
      return fallbackScalar(v)
  }
}

/** with/args 实参单元格：目标声明类型未知，按值形态选择渲染。 */
function argCellInline(cell: Cell | null | undefined): string {
  if (cell === null || cell === undefined) return 'null'
  if (isRefCell(cell)) return `$${cell.ref}`
  return fallbackScalar(cell.lit)
}

// ---------- 序列化：脚本 ----------

function serializeScript(model: Program): string {
  const lines: string[] = []
  lines.push('version: 3')
  if (model.params.length > 0) {
    lines.push('params:')
    for (const decl of model.params) {
      if (decl.rawForm) {
        lines.push(`  - ${singleQuoted(paramDeclToRawString(decl))}`)
      } else {
        lines.push(`  - name: ${plainScalar(decl.name)}`)
        lines.push(`    type: ${plainScalar(decl.type)}`)
        if (decl.default !== null && decl.default !== undefined) {
          lines.push(`    default: ${fallbackScalar(decl.default)}`)
        }
        if (decl.remark !== '') {
          lines.push(`    remark: ${plainScalar(decl.remark)}`)
        }
      }
    }
  }
  if (model.defaults) {
    const d = model.defaults
    lines.push('defaults:')
    const vision: string[] = []
    if (d.vision_threshold !== null && d.vision_threshold !== undefined) {
      vision.push(`    threshold: ${fmtNum(d.vision_threshold)}`)
    }
    if (vision.length > 0) {
      lines.push('  vision:')
      lines.push(...vision)
    }
    const timing: string[] = []
    if (d.after_tap !== null && d.after_tap !== undefined) timing.push(`    after_tap: ${fallbackScalar(d.after_tap)}`)
    if (d.after_match !== null && d.after_match !== undefined) timing.push(`    after_match: ${fallbackScalar(d.after_match)}`)
    if (d.poll_interval !== null && d.poll_interval !== undefined) timing.push(`    poll_interval: ${fallbackScalar(d.poll_interval)}`)
    if (timing.length > 0) {
      lines.push('  timing:')
      lines.push(...timing)
    }
    if (vision.length === 0 && timing.length === 0) {
      lines.splice(lines.length - 1, 1, 'defaults: {}')
    }
  }
  if (model.steps.length === 0) {
    lines.push('steps: []')
  } else {
    lines.push('steps:')
    emitStepSeq(model.steps, 2, lines)
  }
  return lines.join('\n') + '\n'
}

// ---------- 序列化：函数库 ----------

function serializeFunctionLibrary(model: FunctionLibraryModel): string {
  const lines: string[] = []
  model.functions.forEach((fn, i) => {
    if (i > 0) lines.push('')
    lines.push(`${plainScalar(fn.name)}:`)
    if (fn.params.length > 0) {
      lines.push('  params:')
      for (const decl of fn.params) {
        if (decl.rawForm) {
          lines.push(`    - ${singleQuoted(paramDeclToRawString(decl))}`)
        } else {
          lines.push(`    - name: ${plainScalar(decl.name)}`)
          lines.push(`      type: ${plainScalar(decl.type)}`)
          if (decl.default !== null && decl.default !== undefined) {
            lines.push(`      default: ${fallbackScalar(decl.default)}`)
          }
          if (decl.remark !== '') {
            lines.push(`      remark: ${plainScalar(decl.remark)}`)
          }
        }
      }
    }
    if (fn.steps.length === 0) {
      lines.push('  steps: []')
    } else {
      lines.push('  steps:')
      emitStepSeq(fn.steps, 4, lines)
    }
  })
  return lines.join('\n') + '\n'
}

// ---------- 序列化：步骤 ----------

function emitStepSeq(steps: Step[], col: number, lines: string[]): void {
  for (const step of steps) emitStep(step, col, lines)
}

/** 分支列表：键在 col，列表项在 col+2；空列表省略键（loop.steps 等必需键另行处理）。 */
function emitBranch(key: string, list: Step[], col: number, lines: string[]): void {
  if (list.length === 0) return
  lines.push(`${' '.repeat(col)}${key}:`)
  emitStepSeq(list, col + 2, lines)
}

function emitField(key: string, inline: string, col: number, lines: string[]): void {
  lines.push(`${' '.repeat(col)}${key}: ${inline}`)
}

function emitWithMap(withArgs: Record<string, Cell>, col: number, lines: string[]): void {
  const names = Object.keys(withArgs)
  if (names.length === 0) return
  lines.push(`${' '.repeat(col)}with:`)
  for (const name of names) {
    lines.push(`${' '.repeat(col + 2)}${plainScalar(name)}: ${argCellInline(withArgs[name])}`)
  }
}

function emitStep(step: Step, col: number, lines: string[]): void {
  const head = `${' '.repeat(col)}- `
  const F = col + 4 // 动作映射值内字段列
  switch (step.kind) {
    case 'app_start':
      if (step.package === null) lines.push(`${head}app.start`)
      else lines.push(`${head}app.start: ${cellInline(step.package, 'expr')}`)
      return
    case 'app_stop':
      if (step.package === null) lines.push(`${head}app.stop`)
      else lines.push(`${head}app.stop: ${cellInline(step.package, 'expr')}`)
      return
    case 'break':
      lines.push(`${head}break`)
      return
    case 'tap':
      lines.push(`${head}tap: ${cellInline(step.at, 'coord')}`)
      return
    case 'swipe':
      lines.push(`${head}swipe:`)
      emitField('from', cellInline(step.from, 'coord'), F, lines)
      emitField('to', cellInline(step.to, 'coord'), F, lines)
      emitField('duration', cellInline(step.duration, 'time'), F, lines)
      return
    case 'key':
      if (step.action === null || step.action === 'press') {
        lines.push(`${head}key: ${cellInline(step.key, 'key')}`)
      } else {
        lines.push(`${head}key:`)
        emitField('key', cellInline(step.key, 'key'), F, lines)
        emitField('action', plainScalar(step.action), F, lines)
      }
      return
    case 'text':
      lines.push(`${head}text: ${cellInline(step.value, 'text')}`)
      return
    case 'log':
      if (step.level === null || step.level === '' || step.level === 'info') {
        lines.push(`${head}log: ${logInline(step.message)}`)
      } else {
        lines.push(`${head}log:`)
        emitField('level', plainScalar(step.level), F, lines)
        emitField('message', logInline(step.message), F, lines)
      }
      return
    case 'wait':
      if (step.max === null) {
        lines.push(`${head}wait: ${cellInline(step.min, 'time')}`)
      } else {
        lines.push(`${head}wait:`)
        emitField('min', cellInline(step.min, 'time'), F, lines)
        emitField('max', cellInline(step.max, 'time'), F, lines)
      }
      return
    case 'set':
      lines.push(`${head}set:`)
      emitField('name', plainScalar(step.name), F, lines)
      emitField('value', cellInline(step.value, 'expr'), F, lines)
      return
    case 'if':
      lines.push(`${head}if:`)
      emitField('cond', cellInline(step.cond, 'expr'), F, lines)
      emitBranch('then', step.then, F, lines)
      emitBranch('else', step.else, F, lines)
      return
    case 'loop':
      lines.push(`${head}loop:`)
      if (step.times !== null) emitField('times', cellInline(step.times, 'number'), F, lines)
      if (step.steps.length === 0) {
        emitField('steps', '[]', F, lines)
      } else {
        lines.push(`${' '.repeat(F)}steps:`)
        emitStepSeq(step.steps, F + 2, lines)
      }
      return
    case 'call':
      lines.push(`${head}call:`)
      emitField('target', plainScalar(step.target), F, lines)
      emitWithMap(step.with, F, lines)
      if (step.save !== null) emitField('save', plainScalar(step.save), F, lines)
      return
    case 'invoke':
      lines.push(`${head}invoke:`)
      emitField('capability', plainScalar(step.capability), F, lines)
      emitWithMap(step.with, F, lines)
      if (step.save !== null) emitField('save', plainScalar(step.save), F, lines)
      return
    case 'return':
      lines.push(`${head}return: ${cellInline(step.value, 'expr')}`)
      return
    case 'throw':
      lines.push(`${head}throw: ${cellInline(step.message, 'expr')}`)
      return
    case 'find': {
      lines.push(`${head}find:`)
      emitField('template', cellInline(step.template, 'tmpl'), F, lines)
      if (step.timeout !== null) emitField('timeout', cellInline(step.timeout, 'time'), F, lines)
      if (step.threshold !== null) emitField('threshold', fmtNum(step.threshold), F, lines)
      if (step.region !== null && step.region !== undefined) emitField('region', fallbackScalar(step.region), F, lines)
      if (step.save !== null) emitField('save', plainScalar(step.save), F, lines)
      emitBranch('then', step.then, F, lines)
      emitBranch('else', step.else, F, lines)
      if (step.verify) {
        lines.push(`${' '.repeat(F)}verify:`)
        emitField('template', cellInline(step.verify.template, 'tmpl'), F + 2, lines)
        if (step.verify.timeout !== null) {
          emitField('timeout', cellInline(step.verify.timeout, 'time'), F + 2, lines)
        }
      }
      return
    }
    case 'match_first': {
      lines.push(`${head}match_first:`)
      lines.push(`${' '.repeat(F)}candidates:`)
      for (const cand of step.candidates) {
        lines.push(`${' '.repeat(F + 2)}- template: ${cellInline(cand.template, 'tmpl')}`)
        if (cand.threshold !== null) {
          lines.push(`${' '.repeat(F + 4)}threshold: ${fmtNum(cand.threshold)}`)
        }
        if (cand.steps.length > 0) {
          lines.push(`${' '.repeat(F + 4)}steps:`)
          emitStepSeq(cand.steps, F + 6, lines)
        }
      }
      emitBranch('else', step.else, F, lines)
      return
    }
    case 'check': {
      lines.push(`${head}check:`)
      emitField('template', cellInline(step.template, 'tmpl'), F, lines)
      if (step.timeout !== null) emitField('timeout', cellInline(step.timeout, 'time'), F, lines)
      if (step.threshold !== null) emitField('threshold', fmtNum(step.threshold), F, lines)
      if (step.throw !== null) emitField('throw', logInline(step.throw), F, lines)
      return
    }
  }
}

/** log 消息：规范 YAML 首选 plain（区别于 text 的一律双引号）。 */
function logInline(cell: Cell | null): string {
  if (cell === null || cell === undefined) return 'null'
  if (isRefCell(cell)) return `$${cell.ref}`
  return fallbackScalar(cell.lit)
}

// ---------- 解析层 ----------

let loadError: string | null = null

/** plain load（CORE_SCHEMA）；空文档按空映射处理（脚本 → version.missing）。 */
function parseDocument(text: string, diags: Diagnostic[]): Record<string, unknown> | { __seq: true } | null {
  loadError = null
  if (text.trim() === '') return {}
  let doc: unknown
  try {
    doc = load(text, { schema: CORE_SCHEMA, json: true })
  } catch (e) {
    loadError = e instanceof Error ? e.message : String(e)
    return null
  }
  if (doc === null || doc === undefined) return {}
  if (Array.isArray(doc)) return { __seq: true }
  if (typeof doc !== 'object') return { __seq: true }
  return doc as Record<string, unknown>
}

function isSeqSentinel(v: unknown): boolean {
  return typeof v === 'object' && v !== null && (v as { __seq?: boolean }).__seq === true
}

// ---------- 顶层解析 ----------

const TOP_LEVEL_KEYS = new Set(['version', 'params', 'defaults', 'steps'])

function parseScriptRoot(root: Record<string, unknown> | { __seq: true }, diags: Diagnostic[]): Program {
  if (isSeqSentinel(root) || Array.isArray(root)) {
    diags.push(diag(CODES.rootType, '', '', '脚本顶层必须是映射（version/params/defaults/steps）'))
    return emptyProgram()
  }
  const map = root as Record<string, unknown>
  // version 强制（ADR-YAML-01）：缺失/非 3 直接拒绝，不解析其余内容（不误解析 v2）
  if (!('version' in map)) {
    diags.push(diag(CODES.versionMissing, 'version', 'version', 'v3 脚本必须声明 version: 3（旧版 v2 脚本不受支持，请手动升级）'))
    return emptyProgram()
  }
  const version = map.version
  if (typeof version !== 'number' || !Number.isInteger(version) || version !== 3) {
    diags.push(diag(CODES.version, 'version', 'version', `当前只支持 version: 3，收到 ${JSON.stringify(version ?? null)}`))
    return emptyProgram()
  }
  const model = emptyProgram()
  let hasSteps = false
  for (const key of Object.keys(map)) {
    if (!TOP_LEVEL_KEYS.has(key)) {
      diags.push(diag(CODES.topLevelUnknownKey, '', key, `不支持顶层字段 ${JSON.stringify(key)}，只允许 version/params/defaults/steps`))
      continue
    }
    switch (key) {
      case 'params':
        model.params = parseParamDecls(map.params, 'params', diags)
        break
      case 'defaults':
        model.defaults = parseDefaults(map.defaults, diags)
        break
      case 'steps':
        hasSteps = true
        model.steps = parseStepsNode(map.steps, 'steps', diags)
        break
    }
  }
  if (!hasSteps) {
    diags.push(diag(CODES.stepsMissing, 'steps', 'steps', '脚本缺少必需的顶层 steps（可为空列表，不可省略）'))
  }
  return model
}

function parseFunctionRoot(root: Record<string, unknown> | { __seq: true }, file: string, diags: Diagnostic[]): FunctionLibraryModel {
  const functions: FunctionModel[] = []
  if (isSeqSentinel(root) || Array.isArray(root)) {
    diags.push(diag(CODES.rootType, '', '', '函数库顶层必须是「函数名: 记录」映射'))
    return { file, functions }
  }
  for (const [name, value] of Object.entries(root as Record<string, unknown>)) {
    const v = value as Record<string, unknown> | null | undefined
    if (v === null || v === undefined || typeof v !== 'object' || Array.isArray(v)) {
      diags.push(diag(CODES.fieldType, name, '', `函数 ${name} 的记录必须是映射（params/steps）`))
      continue
    }
    const fn: FunctionModel = { name, params: [], steps: [] }
    for (const key of Object.keys(v)) {
      if (key === 'params') {
        fn.params = parseParamDecls(v.params, `${name}.params`, diags)
      } else if (key === 'steps') {
        fn.steps = parseStepsNode(v.steps, `${name}.steps`, diags)
      } else {
        diags.push(diag(CODES.fieldUnknown, `${name}.${key}`, key, `函数 ${name} 记录只允许 params/steps，出现 ${key}`))
      }
    }
    functions.push(fn)
  }
  return { file, functions }
}

function parseDefaults(node: unknown, diags: Diagnostic[]): DefaultsModel | null {
  if (node === null || node === undefined) return null
  if (typeof node !== 'object' || Array.isArray(node)) {
    diags.push(diag(CODES.defaultsType, 'defaults', 'defaults', 'defaults 必须是映射（vision/timing）'))
    return null
  }
  const model: DefaultsModel = {
    vision_threshold: null,
    after_tap: null,
    after_match: null,
    poll_interval: null,
  }
  const map = node as Record<string, unknown>
  for (const key of Object.keys(map)) {
    if (key === 'vision') {
      const vision = map.vision
      if (vision !== null && typeof vision === 'object' && !Array.isArray(vision)) {
        for (const vk of Object.keys(vision as Record<string, unknown>)) {
          if (vk !== 'threshold') {
            diags.push(diag(CODES.defaultsUnknownKey, `defaults.vision.${vk}`, vk, `defaults.vision 不支持字段 ${vk}`))
            continue
          }
          const t = (vision as Record<string, unknown>).threshold
          if (typeof t === 'number' && Number.isFinite(t)) model.vision_threshold = t
          else diags.push(diag(CODES.defaultsType, 'defaults.vision.threshold', 'threshold', 'threshold 必须是 0~1 的数字'))
        }
      } else if (vision !== null && vision !== undefined) {
        diags.push(diag(CODES.defaultsType, 'defaults.vision', 'vision', 'defaults.vision 必须是映射'))
      }
    } else if (key === 'timing') {
      const timing = map.timing
      if (timing !== null && typeof timing === 'object' && !Array.isArray(timing)) {
        for (const tk of Object.keys(timing as Record<string, unknown>)) {
          if (tk !== 'after_tap' && tk !== 'after_match' && tk !== 'poll_interval') {
            diags.push(diag(CODES.defaultsUnknownKey, `defaults.timing.${tk}`, tk, `defaults.timing 不支持字段 ${tk}`))
            continue
          }
          const raw = (timing as Record<string, unknown>)[tk]
          if (typeof raw === 'string' || typeof raw === 'number') {
            model[tk as 'after_tap' | 'after_match' | 'poll_interval'] = raw
          } else {
            diags.push(diag(CODES.defaultsType, `defaults.timing.${tk}`, tk, 'timing 项必须是带单位时间串（如 300ms）'))
          }
        }
      } else if (timing !== null && timing !== undefined) {
        diags.push(diag(CODES.defaultsType, 'defaults.timing', 'timing', 'defaults.timing 必须是映射'))
      }
    } else {
      diags.push(diag(CODES.defaultsUnknownKey, `defaults.${key}`, key, `defaults 不支持字段 ${key}，只允许 vision/timing`))
    }
  }
  return model
}

// ---------- 参数声明解析（契约 §1：字符串 / 映射双形态） ----------

function parseParamDecls(node: unknown, basePath: string, diags: Diagnostic[]): ParamDecl[] {
  if (node === null || node === undefined) return []
  if (!Array.isArray(node)) {
    diags.push(diag(CODES.paramsType, basePath, '', 'params 必须是列表'))
    return []
  }
  const decls: ParamDecl[] = []
  const seen = new Set<string>()
  node.forEach((item, i) => {
    const path = `${basePath}[${i}]`
    if (typeof item === 'string') {
      const decl = parseRawParamDecl(item, path, diags)
      if (decl) {
        if (seen.has(decl.name)) duplicateDiag(diags, path, decl.name)
        else seen.add(decl.name)
        decls.push(decl)
      }
      return
    }
    if (item === null || typeof item !== 'object' || Array.isArray(item)) {
      diags.push(diag(CODES.paramsInvalid, path, 'declaration', '参数声明必须是字符串（type:name:remark[:default]）或映射（name/type/default/remark）'))
      return
    }
    const map = item as Record<string, unknown>
    for (const key of Object.keys(map)) {
      if (key !== 'name' && key !== 'type' && key !== 'default' && key !== 'remark') {
        diags.push(diag(CODES.paramsUnknownKey, `${path}.${key}`, key, `不支持参数字段 ${key}（允许：name/type/default/remark）`))
      }
    }
    const name = typeof map.name === 'string' ? map.name : ''
    if (name === '') {
      diags.push(diag(CODES.paramsInvalid, `${path}.name`, 'name', '参数映射缺少非空 name 字符串'))
      return
    }
    if (!PARAM_NAME_RE.test(name)) {
      diags.push(diag(CODES.paramsNameInvalid, `${path}.name`, 'name', `变量名 ${name} 不符合 [A-Za-z_][A-Za-z0-9_]*`))
      return
    }
    if (seen.has(name)) {
      duplicateDiag(diags, path, name)
      return
    }
    seen.add(name)
    const type = typeof map.type === 'string' && map.type !== '' ? map.type : 'string'
    const remark = typeof map.remark === 'string' ? map.remark : ''
    let defaultValue: ParamDecl['default'] = null
    if (map.default !== null && map.default !== undefined) {
      const v = map.default
      if (typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean') {
        defaultValue = v
      } else {
        diags.push(diag(CODES.paramsDefaultInvalid, `${path}.default`, 'default', '默认值必须是标量（字符串/数字/布尔）'))
      }
    }
    decls.push({ type, name, remark, default: defaultValue, rawForm: false })
  })
  return decls
}

function duplicateDiag(diags: Diagnostic[], path: string, name: string): void {
  diags.push(diag(CODES.paramsNameDuplicate, path, 'name', `变量名 ${name} 在同一参数表内重复`))
}

/** 字符串声明形态：'type:name[:remark[:default]]'（类型/变量名须非空；备注可为空）。 */
function parseRawParamDecl(raw: string, path: string, diags: Diagnostic[]): ParamDecl | null {
  const parts = splitn(raw, ':', 4)
  if (parts.length < 3 || parts[0] === '' || parts[1] === '') {
    diags.push(diag(CODES.paramsInvalid, path, 'declaration', `参数声明应为 类型:变量名:备注[:默认值] 四段式，收到 ${JSON.stringify(raw)}`))
    return null
  }
  const [type, name, remark, defaultTail] = parts
  if (/[\n\r]/.test(raw)) {
    diags.push(diag(CODES.paramsInvalid, path, 'declaration', '参数声明不能包含换行'))
    return null
  }
  if (!PARAM_NAME_RE.test(name)) {
    diags.push(diag(CODES.paramsNameInvalid, path, 'name', `变量名 ${name} 不符合 [A-Za-z_][A-Za-z0-9_]*`))
    return null
  }
  let defaultValue: ParamDecl['default'] = null
  if (parts.length === 4) {
    const parsed = parseParamLiteral(type, defaultTail)
    if (parsed.ok) defaultValue = parsed.value ?? null
    else diags.push(diag(CODES.paramsDefaultInvalid, path, 'default', parsed.reason ?? '默认值不能按声明类型解析'))
  }
  return { type, name, remark, default: defaultValue, rawForm: true }
}

function splitn(s: string, sep: string, maxParts: number): string[] {
  const parts: string[] = []
  let rest = s
  while (parts.length < maxParts - 1) {
    const idx = rest.indexOf(sep)
    if (idx === -1) break
    parts.push(rest.slice(0, idx))
    rest = rest.slice(idx + sep.length)
  }
  parts.push(rest)
  return parts
}

// ---------- 步骤解析 ----------

/** v2 步骤名 → v3 迁移提示（ADR-YAML-01/02/03：只诊断不解析）。 */
const V2_ACTION_HINTS: Record<string, string> = {
  str_app: 'v2 步骤 str_app 已移除，v3 使用 app.start',
  cls_app: 'v2 步骤 cls_app 已移除，v3 使用 app.stop',
  func: 'v2 步骤 func 已移除，v3 统一为 call（target 加 function: 前缀）',
  match: 'v2 步骤 match 已移除，v3 使用 match_first（候选 steps 键）',
  color: 'v2 步骤 color 已移除（v3 颜色分支暂无等价步骤）',
  find_click: 'click 语法已全面移除（ADR-YAML-03）',
}

function parseStepsNode(node: unknown, basePath: string, diags: Diagnostic[]): Step[] {
  if (node === null || node === undefined) return []
  if (!Array.isArray(node)) {
    diags.push(diag(CODES.stepsType, basePath, '', 'steps 必须是列表'))
    return []
  }
  const steps: Step[] = []
  node.forEach((item, i) => {
    const step = parseStepNode(item, `${basePath}[${i}]`, diags)
    if (step !== null) steps.push(step)
  })
  return steps
}

/** 值节点 → 表达式单元格：`$path` 属性路径引用；标量/复合值 → 字面量。 */
function exprCell(v: unknown, path: string, field: string, diags: Diagnostic[]): Cell {
  if (v === null || v === undefined) return { lit: null }
  if (typeof v === 'string') {
    if (v.startsWith('$')) {
      const name = v.slice(1)
      if (name !== '' && isRefPath(name)) return { ref: name }
      diags.push(diag(CODES.refPathInvalid, path, field, `引用 ${v} 不是合法属性路径（形如 $user.level、$list[0]）`))
      return { lit: v }
    }
    return { lit: v }
  }
  if (typeof v === 'number' || typeof v === 'boolean') return { lit: v }
  // 复合字面量（数组/映射）：整体保留（内部 $ 引用按普通字符串处理）
  return { lit: v }
}

/** 时间单元格：字符串（带单位）/ 非负数字（毫秒）/ $ref。 */
function timeCell(v: unknown, path: string, field: string, diags: Diagnostic[]): Cell | null {
  if (v === null || v === undefined) return null
  if (typeof v === 'string') {
    if (v.startsWith('$')) return exprCell(v, path, field, diags)
    if (parseTimeMs(v) !== null) return { lit: v }
    diags.push(diag(CODES.duration, `${path}.${field}`, field, `时间必须是如 100ms/2s/1m 的正值或 $引用，收到 ${JSON.stringify(v)}`))
    return { lit: v }
  }
  if (typeof v === 'number') {
    if (Number.isFinite(v) && v >= 0) return { lit: v }
    diags.push(diag(CODES.duration, `${path}.${field}`, field, `时间毫秒数必须非负，收到 ${v}`))
    return { lit: v }
  }
  diags.push(diag(CODES.duration, `${path}.${field}`, field, '时间必须是字符串或非负整数毫秒'))
  return { lit: null }
}

/** 坐标单元格：[x, y] 数字序列 / $ref；其余原样保留给校验层判错。 */
function coordCell(v: unknown, path: string, field: string, diags: Diagnostic[]): Cell {
  if (v === null || v === undefined) {
    diags.push(diag(CODES.fieldMissing, path, field, `缺少必需字段 ${field}`))
    return { lit: null }
  }
  if (Array.isArray(v)) {
    if (isCoordLit(v)) return { lit: [v[0], v[1]] }
    diags.push(diag(CODES.fieldType, `${path}.${field}`, field, `${field} 应为 [x, y] 坐标`))
    return { lit: v }
  }
  return exprCell(v, path, field, diags)
}

/** 可选单元格：缺失（null/undefined）返回 null。 */
function optionalCell(v: unknown, path: string, field: string, diags: Diagnostic[]): Cell | null {
  if (v === null || v === undefined) return null
  return exprCell(v, path, field, diags)
}

interface MapFields {
  get(key: string): unknown
  has(key: string): boolean
  rejectUnknown(allowed: string[]): void
}

function asMap(v: unknown, path: string, message: string, diags: Diagnostic[]): MapFields | null {
  if (v === null || v === undefined || typeof v !== 'object' || Array.isArray(v)) {
    diags.push(diag(CODES.stepShape, path, '', message))
    return null
  }
  const map = v as Record<string, unknown>
  return {
    get: (key) => map[key],
    has: (key) => key in map,
    rejectUnknown(allowed: string[]): void {
      for (const key of Object.keys(map)) {
        if (!allowed.includes(key)) {
          const hint = key === 'click' ? '（click 语法已全面移除，ADR-YAML-03：命中后动作写 then 步骤组）' : ''
          diags.push(diag(CODES.fieldUnknown, `${path}.${key}`, key, `不支持字段 ${JSON.stringify(key)}${hint}`))
        }
      }
    },
  }
}

const ACTION_KEY_SET = new Set<string>(STEP_KINDS.map(yamlKeyOf))

function parseStepNode(item: unknown, path: string, diags: Diagnostic[]): Step | null {
  if (item === null || item === undefined) return null
  // 裸标量动作：break / app.start / app.stop
  if (typeof item === 'string') {
    if (item === 'break') return { uuid: newStepUuid(), kind: 'break' }
    if (item === 'app.start') return { uuid: newStepUuid(), kind: 'app_start', package: null }
    if (item === 'app.stop') return { uuid: newStepUuid(), kind: 'app_stop', package: null }
    diags.push(diag(CODES.stepUnknown, path, '', V2_ACTION_HINTS[item] ?? `未知动作 ${JSON.stringify(item)}`))
    return null
  }
  if (typeof item !== 'object' || Array.isArray(item)) {
    diags.push(diag(CODES.stepShape, path, '', '步骤必须是裸动作标量或「动作键: 字段」单键映射'))
    return null
  }
  const map = item as Record<string, unknown>
  const keys = Object.keys(map)
  if (keys.length === 0) {
    diags.push(diag(CODES.stepShape, path, '', '步骤缺少动作键'))
    return null
  }
  if (keys.length > 1) {
    diags.push(diag(CODES.stepShape, path, '', `每个步骤必须恰好包含一个动作键，收到 ${keys.join('、')}`))
  }
  const action = keys[0]
  const value = map[action]
  const valuePath = `${path}.${action}`
  if (!ACTION_KEY_SET.has(action)) {
    diags.push(diag(CODES.stepUnknown, path, '', V2_ACTION_HINTS[action] ?? `未知 v3 动作 ${JSON.stringify(action)}`))
    return null
  }
  const base = { uuid: newStepUuid() }
  const m = (v: unknown): MapFields | null => asMap(v, valuePath, `${action} 必须是映射`, diags)
  const branch = (node: unknown, key: string): Step[] => parseStepsNode(node, `${valuePath}.${key}`, diags)
  const strField = (v: unknown, field: string): string | null => {
    if (v === null || v === undefined) return null
    if (typeof v === 'string') return v // 空串保留（命名空间等由校验层给明确诊断）
    diags.push(diag(CODES.fieldString, `${valuePath}.${field}`, field, `${field} 必须是字符串`))
    return null
  }
  const numDiag = (v: unknown, field: string): number | null => {
    if (v === null || v === undefined) return null
    if (typeof v === 'number' && Number.isFinite(v)) return v
    diags.push(diag(CODES.number, `${valuePath}.${field}`, field, `${field} 必须是数字`))
    return null
  }
  switch (action) {
    case 'tap': {
      if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
        const fm = m(value)
        if (!fm) return { ...base, kind: 'tap', at: { lit: null } }
        fm.rejectUnknown(['at', 'point'])
        const point = fm.has('point') ? fm.get('point') : fm.get('at')
        if (point === undefined || point === null) {
          diags.push(diag(CODES.fieldMissing, `${valuePath}.at`, 'at', '缺少字段 at/point'))
          return { ...base, kind: 'tap', at: { lit: null } }
        }
        return { ...base, kind: 'tap', at: coordCell(point, valuePath, 'at', diags) }
      }
      return { ...base, kind: 'tap', at: coordCell(value, valuePath, 'at', diags) }
    }
    case 'swipe': {
      const fm = m(value)
      if (!fm) return { ...base, kind: 'swipe', from: { lit: null }, to: { lit: null }, duration: { lit: null } }
      fm.rejectUnknown(['from', 'to', 'duration', 'time'])
      if (!fm.has('from')) diags.push(diag(CODES.fieldMissing, `${valuePath}.from`, 'from', 'swipe 缺少 from（起点坐标）'))
      if (!fm.has('to')) diags.push(diag(CODES.fieldMissing, `${valuePath}.to`, 'to', 'swipe 缺少 to（终点坐标）'))
      const durationRaw = fm.has('duration') ? fm.get('duration') : fm.get('time')
      const duration = durationRaw === undefined || durationRaw === null
        ? (diags.push(diag(CODES.fieldMissing, `${valuePath}.duration`, 'duration', 'swipe 缺少 duration')), { lit: null })
        : timeCell(durationRaw, valuePath, 'duration', diags) ?? { lit: null }
      return {
        ...base,
        kind: 'swipe',
        from: coordCell(fm.get('from') ?? null, valuePath, 'from', diags),
        to: coordCell(fm.get('to') ?? null, valuePath, 'to', diags),
        duration,
      }
    }
    case 'key': {
      if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
        const fm = m(value)
        if (!fm) return { ...base, kind: 'key', key: { lit: null }, action: null }
        fm.rejectUnknown(['key', 'action'])
        if (!fm.has('key')) diags.push(diag(CODES.fieldMissing, `${valuePath}.key`, 'key', '缺少字段 key'))
        const rawAction = fm.get('action')
        let actionValue: 'down' | 'up' | 'press' | null = null
        if (rawAction !== undefined && rawAction !== null) {
          if (rawAction === 'down' || rawAction === 'up' || rawAction === 'press') actionValue = rawAction
          else diags.push(diag(CODES.fieldType, `${valuePath}.action`, 'action', 'action 只能是 down/up/press'))
        }
        return { ...base, kind: 'key', key: exprCell(fm.get('key') ?? null, valuePath, 'key', diags), action: actionValue }
      }
      return { ...base, kind: 'key', key: exprCell(value, valuePath, 'key', diags), action: null }
    }
    case 'text': {
      if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
        const fm = m(value)
        if (!fm) return { ...base, kind: 'text', value: { lit: null } }
        fm.rejectUnknown(['value'])
        if (!fm.has('value')) diags.push(diag(CODES.fieldMissing, `${valuePath}.value`, 'value', '缺少字段 value'))
        return { ...base, kind: 'text', value: exprCell(fm.get('value') ?? null, valuePath, 'value', diags) }
      }
      return { ...base, kind: 'text', value: exprCell(value, valuePath, 'value', diags) }
    }
    case 'wait': {
      if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
        const fm = m(value)
        if (!fm) return { ...base, kind: 'wait', min: { lit: null }, max: null }
        if (fm.has('min') || fm.has('max')) {
          // 随机区间（契约 §4：min/max 同给）
          fm.rejectUnknown(['min', 'max'])
          if (!fm.has('min')) diags.push(diag(CODES.fieldMissing, `${valuePath}.min`, 'min', 'wait 随机区间需要 min/max 同给'))
          if (!fm.has('max')) diags.push(diag(CODES.fieldMissing, `${valuePath}.max`, 'max', 'wait 随机区间需要 min/max 同给'))
          return {
            ...base,
            kind: 'wait',
            min: timeCell(fm.get('min') ?? null, valuePath, 'min', diags) ?? { lit: null },
            max: timeCell(fm.get('max') ?? null, valuePath, 'max', diags),
          }
        }
        fm.rejectUnknown(['duration', 'time'])
        const raw = fm.has('duration') ? fm.get('duration') : fm.get('time')
        if (raw === undefined || raw === null) {
          diags.push(diag(CODES.fieldMissing, `${valuePath}.duration`, 'duration', 'wait 缺少 duration（或 min/max 随机区间）'))
          return { ...base, kind: 'wait', min: { lit: null }, max: null }
        }
        return { ...base, kind: 'wait', min: timeCell(raw, valuePath, 'duration', diags) ?? { lit: null }, max: null }
      }
      return { ...base, kind: 'wait', min: timeCell(value, valuePath, 'duration', diags) ?? { lit: null }, max: null }
    }
    case 'log': {
      if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
        const fm = m(value)
        if (!fm) return { ...base, kind: 'log', message: { lit: null }, level: null }
        fm.rejectUnknown(['level', 'message'])
        if (!fm.has('message')) diags.push(diag(CODES.fieldMissing, `${valuePath}.message`, 'message', '缺少字段 message'))
        const rawLevel = fm.get('level')
        const level = rawLevel !== undefined && rawLevel !== null
          ? (typeof rawLevel === 'string' && rawLevel.trim() !== '' ? rawLevel : (diags.push(diag(CODES.fieldString, `${valuePath}.level`, 'level', 'level 必须是非空字符串')), null))
          : null
        return { ...base, kind: 'log', message: exprCell(fm.get('message') ?? null, valuePath, 'message', diags), level }
      }
      return { ...base, kind: 'log', message: exprCell(value, valuePath, 'message', diags), level: null }
    }
    case 'set': {
      const fm = m(value)
      if (!fm) return { ...base, kind: 'set', name: '', value: { lit: null } }
      if (fm.has('name')) {
        fm.rejectUnknown(['name', 'value'])
        const name = strField(fm.get('name'), 'name')
        if (!fm.has('value')) diags.push(diag(CODES.fieldMissing, `${valuePath}.value`, 'value', 'set 缺少 value'))
        return { ...base, kind: 'set', name: name ?? '', value: exprCell(fm.get('value') ?? null, valuePath, 'value', diags) }
      }
      const keys = Object.keys(value as Record<string, unknown>)
      if (keys.length !== 1) {
        diags.push(diag(CODES.stepShape, valuePath, '', 'set 使用 {name, value} 或单键映射'))
        return { ...base, kind: 'set', name: keys[0] ?? '', value: { lit: null } }
      }
      const name = keys[0]
      return { ...base, kind: 'set', name, value: exprCell((value as Record<string, unknown>)[name], valuePath, name, diags) }
    }
    case 'if': {
      const fm = m(value)
      if (!fm) return { ...base, kind: 'if', cond: { lit: null }, then: [], else: [] }
      fm.rejectUnknown(['cond', 'then', 'else'])
      if (!fm.has('cond')) diags.push(diag(CODES.fieldMissing, `${valuePath}.cond`, 'cond', 'if 缺少 cond'))
      return {
        ...base,
        kind: 'if',
        cond: exprCell(fm.get('cond') ?? null, valuePath, 'cond', diags),
        then: branch(fm.get('then'), 'then'),
        else: branch(fm.get('else'), 'else'),
      }
    }
    case 'loop': {
      const fm = m(value)
      if (!fm) return { ...base, kind: 'loop', times: null, steps: [] }
      fm.rejectUnknown(['times', 'steps'])
      const rawTimes = fm.get('times')
      let times: Cell | null = null
      if (rawTimes !== undefined && rawTimes !== null) {
        if (typeof rawTimes === 'number' && Number.isFinite(rawTimes)) times = { lit: rawTimes }
        else if (typeof rawTimes === 'string') times = exprCell(rawTimes, valuePath, 'times', diags)
        else diags.push(diag(CODES.fieldType, `${valuePath}.times`, 'times', 'times 必须是数字或 $引用'))
      }
      if (!fm.has('steps')) diags.push(diag(CODES.fieldMissing, `${valuePath}.steps`, 'steps', 'loop 缺少 steps 子流程'))
      return { ...base, kind: 'loop', times, steps: branch(fm.get('steps'), 'steps') }
    }
    case 'break': {
      if (value !== null && value !== undefined) {
        diags.push(diag(CODES.stepShape, valuePath, '', 'break 只允许裸写，不能带值'))
      }
      return { ...base, kind: 'break' }
    }
    case 'call': {
      const fm = m(value)
      if (!fm) return { ...base, kind: 'call', target: '', with: {}, save: null }
      fm.rejectUnknown(['target', 'with', 'args', 'save'])
      return {
        ...base,
        kind: 'call',
        target: strField(fm.get('target'), 'target') ?? '',
        with: parseWithMap(fm, valuePath, diags),
        save: strField(fm.get('save'), 'save'),
      }
    }
    case 'invoke': {
      const fm = m(value)
      if (!fm) return { ...base, kind: 'invoke', capability: '', with: {}, save: null }
      fm.rejectUnknown(['capability', 'with', 'args', 'save'])
      return {
        ...base,
        kind: 'invoke',
        capability: strField(fm.get('capability'), 'capability') ?? '',
        with: parseWithMap(fm, valuePath, diags),
        save: strField(fm.get('save'), 'save'),
      }
    }
    case 'return':
      return { ...base, kind: 'return', value: exprCell(value, valuePath, 'value', diags) }
    case 'throw': {
      if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
        const fm = m(value)
        if (!fm) return { ...base, kind: 'throw', message: { lit: null } }
        fm.rejectUnknown(['message'])
        if (!fm.has('message')) diags.push(diag(CODES.fieldMissing, `${valuePath}.message`, 'message', 'throw 缺少 message'))
        return { ...base, kind: 'throw', message: exprCell(fm.get('message') ?? null, valuePath, 'message', diags) }
      }
      return { ...base, kind: 'throw', message: exprCell(value, valuePath, 'message', diags) }
    }
    case 'app.start':
    case 'app.stop': {
      const kind: StepKind = action === 'app.start' ? 'app_start' : 'app_stop'
      if (value === null || value === undefined) return { ...base, kind, package: null }
      if (typeof value === 'object' && !Array.isArray(value)) {
        const fm = m(value)
        if (!fm) return { ...base, kind, package: null }
        fm.rejectUnknown(['package', 'app'])
        const pkg = fm.has('package') ? fm.get('package') : fm.get('app')
        if (pkg === undefined || pkg === null) return { ...base, kind, package: null }
        return { ...base, kind, package: exprCell(pkg, valuePath, 'package', diags) }
      }
      return { ...base, kind, package: exprCell(value, valuePath, 'package', diags) }
    }
    case 'find': {
      const fm = m(value)
      if (!fm) return { ...base, kind: 'find', template: { lit: null }, timeout: null, threshold: null, region: null, save: null, then: [], else: [], verify: null }
      fm.rejectUnknown(['template', 'timeout', 'threshold', 'region', 'save', 'then', 'else', 'verify'])
      if (!fm.has('template')) diags.push(diag(CODES.fieldMissing, `${valuePath}.template`, 'template', 'find 缺少 template'))
      let verify: { template: Cell; timeout: Cell | null } | null = null
      const rawVerify = fm.get('verify')
      if (rawVerify !== undefined && rawVerify !== null) {
        const vm = asMap(rawVerify, `${valuePath}.verify`, 'verify 必须是映射', diags)
        if (vm) {
          vm.rejectUnknown(['template', 'timeout'])
          if (!vm.has('template')) diags.push(diag(CODES.fieldMissing, `${valuePath}.verify.template`, 'template', 'verify 缺少 template'))
          verify = {
            template: exprCell(vm.get('template') ?? null, `${valuePath}.verify`, 'template', diags),
            timeout: timeCell(vm.get('timeout') ?? null, `${valuePath}.verify`, 'timeout', diags),
          }
        }
      }
      const rawThreshold = fm.get('threshold')
      const threshold = numDiag(rawThreshold, 'threshold')
      return {
        ...base,
        kind: 'find',
        template: exprCell(fm.get('template') ?? null, valuePath, 'template', diags),
        timeout: timeCell(fm.get('timeout') ?? null, valuePath, 'timeout', diags),
        threshold,
        region: fm.has('region') ? fm.get('region') ?? null : null,
        save: strField(fm.get('save'), 'save'),
        then: branch(fm.get('then'), 'then'),
        else: branch(fm.get('else'), 'else'),
        verify,
      }
    }
    case 'match_first': {
      const fm = m(value)
      if (!fm) return { ...base, kind: 'match_first', candidates: [], else: [] }
      fm.rejectUnknown(['candidates', 'templates', 'else'])
      const raw = fm.has('candidates') ? fm.get('candidates') : fm.get('templates')
      const candidates: { template: Cell; threshold: number | null; steps: Step[] }[] = []
      if (raw === undefined || raw === null) {
        diags.push(diag(CODES.fieldMissing, valuePath, 'candidates', 'match_first 缺少 candidates'))
      } else if (!Array.isArray(raw)) {
        diags.push(diag(CODES.matchFirstType, `${valuePath}.candidates`, 'candidates', 'candidates 必须是列表'))
      } else {
        raw.forEach((item, i) => {
          const candPath = `${valuePath}.candidates[${i}]`
          if (item !== null && typeof item === 'object' && !Array.isArray(item)) {
            const cm = asMap(item, candPath, '候选必须是映射', diags)
            if (!cm) return
            cm.rejectUnknown(['template', 'threshold', 'steps'])
            if (!cm.has('template')) diags.push(diag(CODES.fieldMissing, `${candPath}.template`, 'template', '候选缺少 template'))
            candidates.push({
              template: exprCell(cm.get('template') ?? null, candPath, 'template', diags),
              threshold: numDiag(cm.get('threshold'), 'threshold'),
              steps: parseStepsNode(cm.get('steps'), `${candPath}.steps`, diags),
            })
          } else {
            candidates.push({
              template: exprCell(item, candPath, 'template', diags),
              threshold: null,
              steps: [],
            })
          }
        })
      }
      return { ...base, kind: 'match_first', candidates, else: branch(fm.get('else'), 'else') }
    }
    case 'check': {
      const fm = m(value)
      if (!fm) return { ...base, kind: 'check', template: { lit: null }, timeout: null, threshold: null, throw: null }
      fm.rejectUnknown(['template', 'timeout', 'threshold', 'throw'])
      if (!fm.has('template')) diags.push(diag(CODES.fieldMissing, `${valuePath}.template`, 'template', 'check 缺少 template'))
      return {
        ...base,
        kind: 'check',
        template: exprCell(fm.get('template') ?? null, valuePath, 'template', diags),
        timeout: timeCell(fm.get('timeout') ?? null, valuePath, 'timeout', diags),
        threshold: numDiag(fm.get('threshold'), 'threshold'),
        throw: fm.has('throw') ? exprCell(fm.get('throw') ?? null, valuePath, 'throw', diags) : null,
      }
    }
  }
  return null
}

/** with/args 实参映射（args 为兼容别名，契约 §2；两者都在时合并，with 优先）。 */
function parseWithMap(fm: MapFields, valuePath: string, diags: Diagnostic[]): Record<string, Cell> {
  const out: Record<string, Cell> = {}
  for (const key of ['args', 'with']) {
    const raw = fm.get(key)
    if (raw === undefined || raw === null) continue
    if (typeof raw !== 'object' || Array.isArray(raw)) {
      diags.push(diag(CODES.fieldType, `${valuePath}.${key}`, key, 'with/args 必须是映射（参数名: 取值）'))
      continue
    }
    for (const [name, v] of Object.entries(raw as Record<string, unknown>)) {
      out[name] = exprCell(v, `${valuePath}.${key}`, name, diags)
    }
  }
  return out
}

// ---------- uuid ----------

function withUuids<T extends Program | FunctionLibraryModel>(model: T): T {
  if ('functions' in model) {
    for (const fn of model.functions) allocateUuids(fn.steps)
  } else {
    allocateUuids(model.steps)
  }
  return model
}
