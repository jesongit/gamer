/**
 * YAML ↔ Model 双向转换（契约 §3 / §4）。
 *
 * 解析：js-yaml 5 事件级 AST（eventsToAst），同时拿到标量的值与书写样式——
 * params 项「整条单引号」契约（§3.3 规则 2）在 plain-object load 下无法校验
 * （`'x:y'` 与 `x:y` 得到相同字符串），必须走样式感知的解析层。
 *
 * 序列化：手写规范输出器。js-yaml dump 无法按节点控制缩进/引号样式，而规范形态
 * 是混合缩进（match 候选与候选分支为 indentless 序列，其余 +2）与混合引号
 * （params 整条单引号、text 双引号、纯数字色单引号），故逐节点自行排版；
 * 单个标量的「是否需要引号/如何引」复用 js-yaml dump 的判定（lineWidth=-1 禁折行）。
 *
 * 验收锚点：对每个合法 fixture，serialize(parse(fixture.yaml)) 与 fixture 原文逐字节一致。
 */

import { dump, parseEvents, eventsToAst, CORE_SCHEMA } from 'js-yaml'
import {
  allocateUuids,
  type Cell,
  type FunctionLibraryModel,
  type FunctionModel,
  type ParamDecl,
  type ParamType,
  type ScriptConfig,
  type ScriptModel,
  type Step,
  type StepKind,
  ACTION_KEYS,
  PARAM_TYPES,
  newStepUuid,
} from './model'
import { CODES, diag, type Diagnostic } from './diagnostics'
import { isColorLiteral, isCoordLit, isKnownKey, normalizeColor, parseParamLiteral, parseTimeMs, PARAM_NAME_RE } from './schema'

// ---------- 公共 API ----------

export interface ParseOptions {
  /** 函数库文件短路径（FunctionLibraryModel.file）；脚本可省略。 */
  file?: string
}

export interface ScriptParseResult {
  kind: 'script'
  model: ScriptModel
  diagnostics: Diagnostic[]
}

export interface FunctionLibraryParseResult {
  kind: 'function_library'
  model: FunctionLibraryModel
  diagnostics: Diagnostic[]
}

export type ParseResult = ScriptParseResult | FunctionLibraryParseResult

/** 解析可执行脚本（yaml/ 目录类型；目录即类型，不做内容推断）。 */
export function parseScript(text: string, opts: ParseOptions = {}): ScriptParseResult {
  const root = parseDocument(text)
  const diags: Diagnostic[] = []
  if (root === null) {
    return {
      kind: 'script',
      model: { params: [], config: null, steps: [] },
      diagnostics: [diag(CODES.yamlSyntaxError, '', 'yaml', rootError(text) ?? 'YAML 解析失败')],
    }
  }
  const model = parseScriptRoot(root, diags)
  return { kind: 'script', model: withUuids(model), diagnostics: diags }
}

/** 解析函数库（func/ 目录类型；顶层键 = 函数名，记录只允许 params/steps）。 */
export function parseFunctionLibrary(text: string, opts: ParseOptions = {}): FunctionLibraryParseResult {
  const root = parseDocument(text)
  const diags: Diagnostic[] = []
  if (root === null) {
    return {
      kind: 'function_library',
      model: { file: opts.file ?? '', functions: [] },
      diagnostics: [diag(CODES.yamlSyntaxError, '', 'yaml', rootError(text) ?? 'YAML 解析失败')],
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
export function serialize(model: ScriptModel | FunctionLibraryModel): string {
  return 'functions' in model ? serializeFunctionLibrary(model) : serializeScript(model)
}

// ---------- 规范 YAML 输出器 ----------

/** 参数声明原始串（规范形态，契约 §3.3 规则 7；前端 golden 测试同构）。 */
export function paramDeclToRawString(decl: ParamDecl): string {
  const base = `${decl.type}:${decl.name}:${decl.remark}`
  if (decl.default === null || decl.default === undefined) return base
  let rawDefault: string
  switch (decl.type) {
    case 'bool':
      rawDefault = decl.default ? 'true' : 'false'
      break
    case 'coord':
      rawDefault = isCoordLit(decl.default) ? `[${fmtNum(decl.default[0])}, ${fmtNum(decl.default[1])}]` : 'null'
      break
    case 'text':
      rawDefault = JSON.stringify(String(decl.default))
      break
    default:
      rawDefault = String(decl.default)
  }
  return `${base}:${rawDefault}`
}

function fmtNum(n: number): string {
  return String(n)
}

/**
 * 首选 plain 的字符串标量：交由 js-yaml dump 判定 plain 安全性（需要引号时按其
 * 默认单引号风格输出）；含换行的串退回双引号（避免块标量的多行排版）。
 */
function plainScalar(s: string): string {
  if (/[\n\r]/.test(s)) return JSON.stringify(s)
  const out = dump(s, { lineWidth: -1 })
  return out.endsWith('\n') ? out.slice(0, -1) : out
}

/** 整条单引号（params 项；单引号样式仅需把 ' 翻倍转义）。 */
function singleQuoted(s: string): string {
  return `'${s.replace(/'/g, "''")}'`
}

function serializeScript(model: ScriptModel): string {
  const lines: string[] = []
  if (model.params.length > 0) {
    lines.push('params:')
    for (const decl of model.params) {
      lines.push(`  - ${singleQuoted(paramDeclToRawString(decl))}`)
    }
  }
  if (model.config) {
    lines.push('config:')
    if (model.config.interval !== null && model.config.interval !== undefined) {
      lines.push(`  interval: ${plainScalar(String(model.config.interval))}`)
    }
    if (model.config.threshold !== null && model.config.threshold !== undefined) {
      lines.push(`  threshold: ${fmtNum(model.config.threshold)}`)
    }
    if (model.config.log_level !== null && model.config.log_level !== undefined) {
      lines.push(`  log_level: ${plainScalar(String(model.config.log_level))}`)
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

function serializeFunctionLibrary(model: FunctionLibraryModel): string {
  const lines: string[] = []
  model.functions.forEach((fn, i) => {
    if (i > 0) lines.push('')
    lines.push(`${plainScalar(fn.name)}:`)
    if (fn.params.length > 0) {
      lines.push('  params:')
      for (const decl of fn.params) {
        lines.push(`    - ${singleQuoted(paramDeclToRawString(decl))}`)
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

function emitStepSeq(steps: Step[], col: number, lines: string[]): void {
  for (const step of steps) emitStep(step, col, lines)
}

/** 分支列表：键在 col（内容列），列表项在 col+2；空列表省略。 */
function emitBranch(key: string, list: Step[], col: number, lines: string[]): void {
  if (list.length === 0) return
  lines.push(`${' '.repeat(col)}${key}:`)
  emitStepSeq(list, col + 2, lines)
}

function emitArgs(args: Record<string, Cell>, col: number, lines: string[]): void {
  const names = Object.keys(args)
  if (names.length === 0) return
  lines.push(`${' '.repeat(col)}args:`)
  for (const name of names) {
    lines.push(`${' '.repeat(col + 2)}${plainScalar(name)}: ${argCellInline(args[name])}`)
  }
}

function emitStep(step: Step, col: number, lines: string[]): void {
  const pad = ' '.repeat(col)
  const head = `${pad}- `
  const contentCol = col + 2
  switch (step.kind) {
    case 'str_app':
      lines.push(`${head}str_app`)
      return
    case 'cls_app':
      lines.push(`${head}cls_app`)
      return
    case 'throw':
      if (step.message === null || step.message === undefined) {
        lines.push(`${head}throw`)
      } else {
        lines.push(`${head}throw: ${plainScalar(String(step.message))}`)
      }
      return
    case 'tap':
      lines.push(`${head}tap: ${cellInline(step.at, 'coord')}`)
      return
    case 'key':
      lines.push(`${head}key: ${cellInline(step.key, 'key')}`)
      return
    case 'text':
      lines.push(`${head}text: ${cellInline(step.value, 'text')}`)
      return
    case 'log':
      lines.push(`${head}log: ${logCellInline(step.message)}`)
      return
    case 'return':
      lines.push(`${head}return: ${cellInline(step.value, 'bool')}`)
      return
    case 'wait':
      if (step.duration_max === null) {
        lines.push(`${head}wait: ${cellInline(step.duration, 'time')}`)
      } else {
        lines.push(`${head}wait: [${cellInline(step.duration, 'time')}, ${cellInline(step.duration_max, 'time')}]`)
      }
      return
    case 'if':
      lines.push(`${head}if: ${cellInline(step.cond, 'bool')}`)
      emitBranch('then', step.then, contentCol, lines)
      emitBranch('else', step.else, contentCol, lines)
      return
    case 'swipe':
      lines.push(`${head}swipe:`)
      lines.push(`${' '.repeat(contentCol + 2)}fm: ${cellInline(step.from, 'coord')}`)
      lines.push(`${' '.repeat(contentCol + 2)}to: ${cellInline(step.to, 'coord')}`)
      lines.push(`${' '.repeat(contentCol + 2)}time: ${cellInline(step.time, 'time')}`)
      return
    case 'find':
      lines.push(`${head}find: ${cellInline(step.template, 'tmpl')}`)
      if (step.block.length > 0) {
        lines.push(`${' '.repeat(contentCol)}block:`)
        for (const b of step.block) {
          lines.push(`${' '.repeat(contentCol + 2)}- ${cellInline(b, 'tmpl')}`)
        }
      }
      if (step.verify) lines.push(`${' '.repeat(contentCol)}verify: true`)
      if (step.timeout !== null) {
        lines.push(`${' '.repeat(contentCol)}timeout: ${cellInline(step.timeout, 'time')}`)
      }
      emitBranch('then', step.then, contentCol, lines)
      emitBranch('else', step.else, contentCol, lines)
      return
    case 'match': {
      lines.push(`${head}match:`)
      // 紧凑缩进（契约 §4.1）：候选列表是 match 键下的无缩进序列（与键内容列同列）。
      for (const cand of step.candidates) {
        lines.push(`${' '.repeat(contentCol)}- ${cellInline(cand.template, 'tmpl')}:`)
        if (cand.click) {
          // 命中点击候选 = 映射形态（契约 §4.1）；映射值不能与键同列，比候选键深两级。
          lines.push(`${' '.repeat(contentCol + 4)}click: true`)
          if (cand.steps.length > 0) {
            lines.push(`${' '.repeat(contentCol + 4)}steps:`)
            emitStepSeq(cand.steps, contentCol + 6, lines)
          }
        } else {
          // 候选分支步骤 = 候选键的值序列，同样无缩进（项与键内容列同列）。
          emitStepSeq(cand.steps, contentCol + 2, lines)
        }
      }
      emitBranch('else', step.else, contentCol, lines)
      if (step.timeout !== null) {
        lines.push(`${' '.repeat(contentCol)}timeout: ${cellInline(step.timeout, 'time')}`)
      }
      return
    }
    case 'check':
      lines.push(`${head}check: ${cellInline(step.template, 'tmpl')}`)
      lines.push(`${' '.repeat(contentCol)}throw: ${plainScalar(step.throw)}`)
      return
    case 'color': {
      lines.push(`${head}color:`)
      const mapCol = contentCol + 2
      lines.push(`${' '.repeat(mapCol)}at: ${cellInline(step.at, 'coord')}`)
      if (step.expect.length > 0) {
        lines.push(`${' '.repeat(mapCol)}expect:`)
        for (const exp of step.expect) {
          const candCol = mapCol + 2
          lines.push(`${' '.repeat(candCol)}- ${cellInline(exp.color, 'color')}:`)
          if (exp.click) {
            // 命中点击候选 = 映射形态（契约 §4.2）；映射键比候选键深两级。
            lines.push(`${' '.repeat(candCol + 4)}click: true`)
            if (exp.steps.length > 0) {
              lines.push(`${' '.repeat(candCol + 4)}steps:`)
              emitStepSeq(exp.steps, candCol + 6, lines)
            }
          } else {
            emitStepSeq(exp.steps, candCol + 2, lines)
          }
        }
      }
      // 规范形态（fixture 冻结）：color 的 else 写在步骤级，与 color 键同列（兄弟键）。
      emitBranch('else', step.else, contentCol, lines)
      return
    }
    case 'loop':
      lines.push(`${head}loop:`)
      if (step.times !== null) {
        lines.push(`${' '.repeat(contentCol + 2)}times: ${fmtNum(step.times)}`)
      }
      if (step.steps.length === 0) {
        lines.push(`${' '.repeat(contentCol + 2)}steps: []`)
      } else {
        lines.push(`${' '.repeat(contentCol + 2)}steps:`)
        emitStepSeq(step.steps, contentCol + 4, lines)
      }
      return
    case 'call':
      lines.push(`${head}call: ${plainScalar(step.target)}`)
      emitArgs(step.args, contentCol, lines)
      return
    case 'func':
      lines.push(`${head}func: ${plainScalar(step.target)}`)
      emitArgs(step.args, contentCol, lines)
      emitBranch('then', step.then, contentCol, lines)
      emitBranch('else', step.else, contentCol, lines)
      return
  }
}

/** log 消息：规范 YAML 首选 plain（区别于 text 的一律双引号，契约 §3.5）。 */
function logCellInline(cell: Cell | null): string {
  if (cell === null || cell === undefined) return 'null'
  if (typeof cell.ref === 'string') return `$${cell.ref}`
  return typeof cell.lit === 'string' ? plainScalar(cell.lit) : fallbackScalar(cell.lit)
}

/** 步骤字段取值单元格行内渲染（lit 形态由字段类型约束）。 */
function cellInline(cell: Cell | null, type: ParamType): string {
  if (cell === null || cell === undefined) return 'null'
  if (typeof cell.ref === 'string') return `$${cell.ref}`
  let v = cell.lit
  switch (type) {
    case 'coord':
      return isCoordLit(v) ? `[${fmtNum(v[0])}, ${fmtNum(v[1])}]` : fallbackScalar(v)
    case 'bool':
      return v === true ? 'true' : v === false ? 'false' : fallbackScalar(v)
    case 'text':
      return typeof v === 'string' ? JSON.stringify(v) : fallbackScalar(v)
    default:
      // tmpl / color / time / key：字符串首选 plain（纯数字色会被 dump 自动单引号化）。
      if (type === 'color' && typeof v === 'string') v = normalizeColor(v)
      if (typeof v === 'string') return plainScalar(v)
      return fallbackScalar(v)
  }
}

/**
 * args 实参单元格：类型未知（取决于目标声明），字符串字面量按值形态选择引号——
 * time/key/color 形态的串首选 plain（parse 后仍是同一字符串，模型往返稳定）；
 * 其余（text/tmpl 等）双引号。布尔/数字/坐标按本体渲染。
 * 规范依据：fixture v09（text 实参双引号）与 v10（time 实参 plain）。
 */
function argCellInline(cell: Cell | null): string {
  if (cell === null || cell === undefined) return 'null'
  if (typeof cell.ref === 'string') return `$${cell.ref}`
  const v = cell.lit
  if (isCoordLit(v)) return `[${fmtNum(v[0])}, ${fmtNum(v[1])}]`
  if (v === true) return 'true'
  if (v === false) return 'false'
  if (typeof v === 'string') return argScalarRender(v)
  if (typeof v === 'number') return fmtNum(v)
  return 'null'
}

function argScalarRender(s: string): string {
  if (parseTimeMs(s) !== null || isKnownKey(s) || isColorLiteral(s)) return plainScalar(s)
  return JSON.stringify(s)
}

function fallbackScalar(v: unknown): string {
  if (v === null || v === undefined) return 'null'
  if (typeof v === 'string') return JSON.stringify(v)
  if (typeof v === 'number' && Number.isFinite(v)) return fmtNum(v)
  if (typeof v === 'boolean') return String(v)
  return 'null'
}

// ---------- 解析层：js-yaml 事件 AST ----------

interface YScalar {
  kind: 'scalar'
  tag: string
  value: string
  singleQuoted: boolean
}
interface YSeq {
  kind: 'seq'
  items: YNode[]
}
interface YMap {
  kind: 'map'
  entries: { key: YNode; value: YNode | null }[]
}
type YNode = YScalar | YSeq | YMap

const TAG_PREFIX = 'tag:yaml.org,2002:'

let parseError: string | null = null

function parseDocument(text: string): YNode | null {
  parseError = null
  try {
    const events = [...parseEvents(text, {})]
    const docs = eventsToAst(events, { source: text, schema: CORE_SCHEMA })
    const contents = docs.length > 0 ? docs[0].contents : null
    if (contents === null || contents === undefined) return mapNode([]) // 空文档按空映射处理
    return fromJsYamlNode(contents)
  } catch (e) {
    parseError = e instanceof Error ? e.message : String(e)
    return null
  }
}

function rootError(text: string): string | null {
  return parseError ?? (text.trim() === '' ? '空文档' : null)
}

function fromJsYamlNode(node: any): YNode {
  switch (node.kind) {
    case 'scalar':
      return {
        kind: 'scalar',
        tag: typeof node.tag === 'string' ? node.tag : `${TAG_PREFIX}str`,
        value: String(node.value ?? ''),
        singleQuoted: node.style?.singleQuoted === true,
      }
    case 'sequence':
      return { kind: 'seq', items: (node.items ?? []).map(fromJsYamlNode) }
    case 'mapping':
      return {
        kind: 'map',
        entries: (node.items ?? []).map((it: any) => ({ key: fromJsYamlNode(it.key), value: it.value ? fromJsYamlNode(it.value) : null })),
      }
    default:
      // alias 等不在契约内的节点按空标量处理（引用/锚点不是规范形态的一部分）。
      return { kind: 'scalar', tag: `${TAG_PREFIX}null`, value: '', singleQuoted: false }
  }
}

function mapNode(entries: YMap['entries']): YMap {
  return { kind: 'map', entries }
}

function isNullScalar(node: YNode | null): boolean {
  return node !== null && node.kind === 'scalar' && node.tag === `${TAG_PREFIX}null`
}

function scalarValue(node: YNode): unknown {
  if (node.kind !== 'scalar') return null
  switch (node.tag) {
    case `${TAG_PREFIX}bool`:
      return node.value === 'true'
    case `${TAG_PREFIX}int`:
    case `${TAG_PREFIX}float`:
      return toNumber(node.value)
    case `${TAG_PREFIX}null`:
      return null
    default:
      return node.value
  }
}

function toNumber(raw: string): number {
  const n = Number(raw)
  return Number.isFinite(n) ? n : NaN
}

/** 步骤动作键集合（YAML 键与 kind 同名）。 */
const ACTION_KEY_SET = new Set<string>(ACTION_KEYS)

// ---------- 顶层解析 ----------

function parseScriptRoot(root: YNode, diags: Diagnostic[]): ScriptModel {
  if (root.kind !== 'map') {
    diags.push(diag(CODES.scriptRootType, '', '', '脚本顶层必须是映射（params/config/steps）'))
    return { params: [], config: null, steps: [] }
  }
  const model: ScriptModel = { params: [], config: null, steps: [] }
  let hasSteps = false
  for (const entry of root.entries) {
    const key = entry.key.kind === 'scalar' ? entry.key.value : ''
    const value = entry.value
    switch (key) {
      case 'params':
        model.params = parseParamDecls(value, 'params', diags)
        break
      case 'config':
        model.config = parseConfig(value, diags)
        break
      case 'steps':
        hasSteps = true
        model.steps = parseStepsNode(value, 'steps', diags)
        break
      case '':
        break
      default:
        diags.push(diag(CODES.scriptTopLevelUnknownKey, '', key, `未知顶层键 ${key}，只允许 params/config/steps`))
        break
    }
  }
  if (!hasSteps) {
    diags.push(diag(CODES.scriptRootType, '', 'steps', '脚本缺少必需的顶层 steps（可为空列表，不可省略）'))
  }
  return model
}

function parseFunctionRoot(root: YNode, file: string, diags: Diagnostic[]): FunctionLibraryModel {
  const functions: FunctionModel[] = []
  if (root.kind !== 'map') {
    diags.push(diag(CODES.scriptRootType, '', '', '函数库顶层必须是「函数名: 记录」映射'))
    return { file, functions }
  }
  for (const entry of root.entries) {
    const name = entry.key.kind === 'scalar' ? entry.key.value : ''
    const basePath = name
    if (entry.value === null || entry.value.kind !== 'map') {
      diags.push(diag(CODES.funcRecordType, basePath, '', `函数 ${name} 的记录必须是映射（params/steps）`))
      continue
    }
    const fn: FunctionModel = { name, params: [], steps: [] }
    for (const rec of entry.value.entries) {
      const key = rec.key.kind === 'scalar' ? rec.key.value : ''
      if (key === 'params') {
        fn.params = parseParamDecls(rec.value, `${basePath}.params`, diags)
      } else if (key === 'steps') {
        fn.steps = parseStepsNode(rec.value, `${basePath}.steps`, diags)
      } else {
        diags.push(diag(CODES.funcRecordUnknownKey, basePath, key, `函数 ${name} 记录只允许 params/steps，出现 ${key}`))
      }
    }
    functions.push(fn)
  }
  return { file, functions }
}

function parseConfig(node: YNode | null, diags: Diagnostic[]): ScriptConfig | null {
  if (node === null || isNullScalar(node)) return null
  if (node.kind !== 'map') {
    diags.push(diag(CODES.stepFieldTypeMismatch, 'config', '', 'config 必须是映射（interval/threshold/log_level）'))
    return null
  }
  const config: ScriptConfig = { interval: '500ms', threshold: 0.85, log_level: 'info' }
  for (const entry of node.entries) {
    const key = entry.key.kind === 'scalar' ? entry.key.value : ''
    const raw = entry.value
    switch (key) {
      case 'interval':
        if (raw !== null && raw.kind === 'scalar' && typeof scalarValue(raw) === 'string') {
          config.interval = String(scalarValue(raw))
        } else if (raw !== null && !isNullScalar(raw)) {
          diags.push(diag(CODES.stepFieldTypeMismatch, 'config', 'interval', 'interval 必须是带单位时间串（如 500ms）'))
        }
        break
      case 'threshold':
        if (raw !== null && raw.kind === 'scalar' && typeof scalarValue(raw) === 'number') {
          config.threshold = scalarValue(raw) as number
        } else if (raw !== null && !isNullScalar(raw)) {
          diags.push(diag(CODES.stepFieldTypeMismatch, 'config', 'threshold', 'threshold 必须是 0~1 的数字'))
        }
        break
      case 'log_level':
        if (raw !== null && raw.kind === 'scalar' && typeof scalarValue(raw) === 'string') {
          config.log_level = String(scalarValue(raw)) as ScriptConfig['log_level']
        } else if (raw !== null && !isNullScalar(raw)) {
          diags.push(diag(CODES.stepFieldTypeMismatch, 'config', 'log_level', 'log_level 必须是 debug/info/warn/error'))
        }
        break
      case '':
        break
      default:
        diags.push(diag(CODES.stepFieldUnknown, 'config', key, `未知 config 键 ${key}，只允许 interval/threshold/log_level`))
        break
    }
  }
  return config
}

// ---------- 参数声明解析（契约 §3.3） ----------

function parseParamDecls(node: YNode | null, basePath: string, diags: Diagnostic[]): ParamDecl[] {
  if (node === null || isNullScalar(node)) return []
  if (node.kind !== 'seq') {
    diags.push(diag(CODES.stepListType, basePath, '', 'params 必须是列表'))
    return []
  }
  const decls: ParamDecl[] = []
  const seen = new Set<string>()
  node.items.forEach((item, i) => {
    const path = `${basePath}[${i}]`
    if (item.kind !== 'scalar') {
      diags.push(diag(CODES.paramDeclFormat, path, 'declaration', 'params 项必须是整条单引号标量（如 \'bool:enable:开关:true\'）'))
      return
    }
    if (!item.singleQuoted) {
      diags.push(diag(CODES.paramDeclQuoteStyle, path, 'style', 'params 项必须整条单引号书写（无引号 plain 标量丢失样式，无法校验）'))
    }
    const raw = item.value
    const parts = splitn(raw, ':', 4)
    // 备注段允许为空（ParamEditor 新建行 remark=''；序列化端 paramDeclToRawString 同样可产出）,
    // 只要求 类型/变量名 非空：'text:tag:'（无默认值）与 'text:tag::x'（空备注+默认值）均可回解析
    if (parts.length < 3 || parts[0] === '' || parts[1] === '') {
      diags.push(diag(CODES.paramDeclFormat, path, 'declaration', `参数声明应为 类型:变量名:备注[:默认值] 四段式，收到 ${JSON.stringify(raw)}`))
      return
    }
    const [type, name, remark, defaultTail] = parts
    if (!PARAM_TYPES.includes(type as ParamType)) {
      diags.push(diag(CODES.paramDeclFormat, path, 'declaration', `未知参数类型 ${type}（七类：tmpl/coord/color/time/key/text/bool）`))
      return
    }
    if (/[\n\r]/.test(raw)) {
      diags.push(diag(CODES.paramDeclFormat, path, 'declaration', '参数声明不能包含换行'))
      return
    }
    if (!PARAM_NAME_RE.test(name)) {
      diags.push(diag(CODES.paramDeclNameInvalid, path, 'name', `变量名 ${name} 不符合 [A-Za-z_][A-Za-z0-9_]*`))
      return
    }
    if (seen.has(name)) {
      diags.push(diag(CODES.paramDeclNameDuplicate, path, 'name', `变量名 ${name} 在同一参数表内重复`))
      return
    }
    seen.add(name)
    let defaultValue: ParamDecl['default'] = null
    if (parts.length === 4) {
      if (defaultTail === '') {
        diags.push(diag(CODES.paramDefaultEmpty, path, 'default', '空默认值非法（不等价于没有默认值；空字符串须写 ""）'))
      } else {
        const parsed = parseParamLiteral(type as ParamType, defaultTail)
        if (parsed.ok) {
          defaultValue = parsed.value ?? null
        } else {
          diags.push(diag(CODES.paramDefaultInvalid, path, 'default', parsed.reason ?? '默认值不能按声明类型解析'))
        }
      }
    }
    decls.push({ type: type as ParamType, name, remark, default: defaultValue })
  })
  return decls
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

function parseStepsNode(node: YNode | null, basePath: string, diags: Diagnostic[]): Step[] {
  if (node === null || isNullScalar(node)) return []
  if (node.kind !== 'seq') {
    diags.push(diag(CODES.stepListType, basePath, '', '步骤必须是列表'))
    return []
  }
  const steps: Step[] = []
  node.items.forEach((item, i) => {
    const step = parseStepNode(item, `${basePath}[${i}]`, diags)
    if (step !== null) steps.push(step)
  })
  return steps
}

type RawCell =
  | { t: 'ref'; name: string }
  | { t: 'lit'; v: unknown }
  | { t: 'seq'; items: RawCell[] }
  | { t: 'none' }

/** 值节点 → 原始单元格（`$name` 完整值引用；序列保留给 coord/等待区间使用）。 */
function parseCellRaw(node: YNode | null): RawCell {
  if (node === null) return { t: 'none' }
  if (node.kind === 'scalar') {
    if (isNullScalar(node)) return { t: 'none' }
    if (node.tag === `${TAG_PREFIX}str`) {
      const m = /^\$([A-Za-z_][A-Za-z0-9_]*)$/.exec(node.value)
      if (m) return { t: 'ref', name: m[1] }
      return { t: 'lit', v: node.value }
    }
    return { t: 'lit', v: scalarValue(node) }
  }
  if (node.kind === 'seq') {
    return { t: 'seq', items: node.items.map(parseCellRaw) }
  }
  return { t: 'none' }
}

const NONE_CELL: Cell = { lit: null }

/** 必填单元格；缺失 → step.field.missing，占位 {lit:null}。 */
function requireCell(raw: RawCell, path: string, field: string, diags: Diagnostic[]): Cell {
  if (raw.t === 'ref') return { ref: raw.name }
  if (raw.t === 'lit') return { lit: raw.v }
  if (raw.t === 'seq') return { lit: seqLit(raw.items) }
  diags.push(diag(CODES.stepFieldMissing, path, field, `缺少必需字段 ${field}`))
  return { lit: null }
}

/** 可选单元格；缺失 → null（序列化时省略）。 */
function optionalCell(raw: RawCell): Cell | null {
  if (raw.t === 'ref') return { ref: raw.name }
  if (raw.t === 'lit') return { lit: raw.v }
  if (raw.t === 'seq') return { lit: seqLit(raw.items) }
  return null
}

/** coord 单元格：[x, y] 数字序列；引用或（不合法的）其他值保留给校验层判错。 */
function coordCell(raw: RawCell, path: string, field: string, diags: Diagnostic[]): Cell {
  if (raw.t === 'ref') return { ref: raw.name }
  if (raw.t === 'lit') return { lit: raw.v }
  if (raw.t === 'seq') {
    const pair = seqLit(raw.items)
    if (pair !== null) return { lit: pair }
    diags.push(diag(CODES.stepFieldTypeMismatch, path, field, `${field} 应为 [x, y] 坐标`))
    return { lit: null }
  }
  diags.push(diag(CODES.stepFieldMissing, path, field, `缺少必需字段 ${field}`))
  return { lit: null }
}

function seqLit(items: RawCell[]): unknown {
  if (items.length === 2) {
    const a = items[0]
    const b = items[1]
    if (a.t === 'lit' && b.t === 'lit' && typeof a.v === 'number' && typeof b.v === 'number') {
      return [a.v, b.v]
    }
  }
  return null
}

/** wait 值：标量 = 固定等待；二元序列 = [最短, 最长] 随机区间。 */
function parseWaitCells(
  raw: RawCell,
  path: string,
  diags: Diagnostic[],
): { duration: Cell; duration_max: Cell | null } {
  if (raw.t === 'seq' && raw.items.length === 2) {
    return { duration: requireCell(raw.items[0], path, 'duration', diags), duration_max: optionalCell(raw.items[1]) }
  }
  if (raw.t === 'seq') {
    diags.push(diag(CODES.stepFieldTypeMismatch, path, 'duration', 'wait 随机区间应为 [最短, 最长] 两项'))
    return { duration: { lit: null }, duration_max: null }
  }
  return { duration: requireCell(raw, path, 'duration', diags), duration_max: null }
}

/** 候选键位单元格（match 模板 / color 颜色）：$ref 或原始串字面量（数字色重新字符串化）。 */
function candidateKeyCell(node: YNode | null): Cell {
  if (node === null || node.kind !== 'scalar') return { lit: null }
  if (node.tag === `${TAG_PREFIX}str`) {
    const m = /^\$([A-Za-z_][A-Za-z0-9_]*)$/.exec(node.value)
    if (m) return { ref: m[1] }
    return { lit: node.value }
  }
  if (node.tag === `${TAG_PREFIX}null`) return { lit: '' }
  // 纯数字色等被 schema 解析成数字的键：按原始串重新字符串化（契约 §4.2）。
  return { lit: node.value }
}

function entryKey(entry: { key: YNode; value: YNode | null }): string {
  return entry.key.kind === 'scalar' ? entry.key.value : ''
}

function parseStepNode(node: YNode | null, path: string, diags: Diagnostic[]): Step | null {
  if (node === null) return null
  if (node.kind === 'scalar') {
    if (node.value === 'str_app') return { uuid: newStepUuid(), kind: 'str_app' }
    if (node.value === 'cls_app') return { uuid: newStepUuid(), kind: 'cls_app' }
    if (node.value === 'throw') return { uuid: newStepUuid(), kind: 'throw', message: null }
    if (!isNullScalar(node)) {
      diags.push(diag(CODES.stepUnknownAction, path, '', `未知动作 ${JSON.stringify(node.value)}`))
    }
    return null
  }
  if (node.kind !== 'map') {
    diags.push(diag(CODES.stepListType, path, '', '步骤项必须是标量动作或「动作键: 字段」映射'))
    return null
  }
  // check 的 throw 是兄弟字段（未命中终止原因），与 throw 动作键同名词：
  // 步骤内存在 check 键时把 throw 降级为字段，避免误判多动作键。
  const hasCheck = node.entries.some((e) => entryKey(e) === 'check')
  const actionEntries = node.entries.filter(
    (e) => ACTION_KEY_SET.has(entryKey(e)) && !(hasCheck && entryKey(e) === 'throw'),
  )
  const fieldEntries = node.entries.filter(
    (e) => !ACTION_KEY_SET.has(entryKey(e)) || (hasCheck && entryKey(e) === 'throw'),
  )
  if (actionEntries.length === 0) {
    diags.push(diag(CODES.stepUnknownAction, path, '', '步骤缺少动作键'))
    return null
  }
  if (actionEntries.length > 1) {
    diags.push(diag(CODES.stepMultiAction, path, '', `一个步骤只允许一个动作键，收到 ${actionEntries.map(entryKey).join('、')}`))
  }
  const actionKey = entryKey(actionEntries[0]) as StepKind
  const value = actionEntries[0].value
  const fieldRaw = new Map<string, YNode | null>()
  for (const e of fieldEntries) {
    const k = entryKey(e)
    if (k === '') continue
    if (!isKnownField(actionKey, k)) {
      diags.push(diag(CODES.stepFieldUnknown, path, k, `动作 ${actionKey} 不支持字段 ${k}`))
      continue
    }
    fieldRaw.set(k, e.value)
  }
  return parseStepFields(actionKey, value, fieldRaw, path, diags)
}

function isKnownField(kind: StepKind, key: string): boolean {
  switch (kind) {
    case 'str_app':
    case 'cls_app':
    case 'wait':
      return false
    case 'tap':
    case 'key':
    case 'text':
    case 'log':
    case 'return':
    case 'throw':
      return false
    case 'swipe':
      return false // fm/to/time 由动作值映射携带
    case 'find':
      return ['block', 'verify', 'timeout', 'then', 'else'].includes(key)
    case 'match':
      return ['else', 'timeout'].includes(key)
    case 'check':
      return ['throw'].includes(key)
    case 'color':
      return ['else'].includes(key)
    case 'if':
      return ['then', 'else'].includes(key)
    case 'loop':
      return false // times/steps 由动作值映射携带
    case 'call':
      return ['args'].includes(key)
    case 'func':
      return ['args', 'then', 'else'].includes(key)
  }
}

function parseStepFields(
  kind: StepKind,
  value: YNode | null,
  fields: Map<string, YNode | null>,
  path: string,
  diags: Diagnostic[],
): Step | null {
  const base = { uuid: newStepUuid() }
  const branch = (key: string): Step[] => parseStepsNode(fields.get(key) ?? null, `${path}.${key}`, diags)
  switch (kind) {
    case 'str_app':
      if (value !== null && !isNullScalar(value)) {
        diags.push(diag(CODES.stepFieldTypeMismatch, path, '', 'str_app 只允许裸写，不能带值'))
      }
      return { ...base, kind: 'str_app' }
    case 'cls_app':
      if (value !== null && !isNullScalar(value)) {
        diags.push(diag(CODES.stepFieldTypeMismatch, path, '', 'cls_app 只允许裸写，不能带值'))
      }
      return { ...base, kind: 'cls_app' }
    case 'throw': {
      if (value === null || isNullScalar(value)) return { ...base, kind: 'throw', message: null }
      if (value.kind === 'scalar') return { ...base, kind: 'throw', message: value.value }
      diags.push(diag(CODES.stepFieldTypeMismatch, path, 'message', 'throw 的原因必须是标量'))
      return { ...base, kind: 'throw', message: null }
    }
    case 'tap':
      return { ...base, kind: 'tap', at: coordCell(parseCellRaw(value), path, 'at', diags) }
    case 'key':
      return { ...base, kind: 'key', key: requireCell(parseCellRaw(value), path, 'key', diags) }
    case 'text':
      return { ...base, kind: 'text', value: requireCell(parseCellRaw(value), path, 'value', diags) }
    case 'log':
      return { ...base, kind: 'log', message: requireCell(parseCellRaw(value), path, 'message', diags) }
    case 'return':
      return { ...base, kind: 'return', value: requireCell(parseCellRaw(value), path, 'value', diags) }
    case 'wait': {
      const { duration, duration_max } = parseWaitCells(parseCellRaw(value), path, diags)
      return { ...base, kind: 'wait', duration, duration_max }
    }
    case 'if':
      return {
        ...base,
        kind: 'if',
        cond: requireCell(parseCellRaw(value), path, 'cond', diags),
        then: branch('then'),
        else: branch('else'),
      }
    case 'swipe': {
      const map = value !== null && value.kind === 'map' ? value : null
      if (map === null) {
        diags.push(diag(CODES.stepFieldMissing, path, 'fm', 'swipe 需要 fm/to/time 字段'))
        return {
          ...base,
          kind: 'swipe',
          from: { lit: null },
          to: { lit: null },
          time: { lit: null },
        }
      }
      for (const e of map.entries) {
        const k = entryKey(e)
        if (k !== 'fm' && k !== 'to' && k !== 'time') {
          diags.push(diag(CODES.stepFieldUnknown, path, k, `swipe 不支持字段 ${k}`))
        }
      }
      const get = (k: string) => map.entries.find((e) => entryKey(e) === k)?.value ?? null
      const toMissing = get('to') === null
      if (get('fm') === null) diags.push(diag(CODES.stepFieldMissing, path, 'from', 'swipe 缺少 fm（起点坐标）'))
      if (toMissing) diags.push(diag(CODES.stepFieldMissing, path, 'to', 'swipe 缺少 to（终点坐标）'))
      const timeRaw = parseCellRaw(get('time'))
      return {
        ...base,
        kind: 'swipe',
        from: coordCell(parseCellRaw(get('fm')), path, 'from', diags),
        to: coordCell(parseCellRaw(get('to')), path, 'to', diags),
        time: timeRaw.t === 'none' ? { lit: null } : optionalCell(timeRaw) ?? { lit: null },
      }
    }
    case 'find':
      return {
        ...base,
        kind: 'find',
        template: requireCell(parseCellRaw(value), path, 'template', diags),
        block: parseTmplList(fields.get('block') ?? null, `${path}.block`, diags),
        verify: parseBool(fields.get('verify') ?? null, path, 'verify', diags) ?? false,
        timeout: optionalCell(parseCellRaw(fields.get('timeout') ?? null)),
        then: branch('then'),
        else: branch('else'),
      }
    case 'match': {
      const step: Step = {
        ...base,
        kind: 'match',
        candidates: [],
        else: branch('else'),
        timeout: optionalCell(parseCellRaw(fields.get('timeout') ?? null)),
      }
      parseMatchCandidates(value, path, step, diags)
      return step
    }
    case 'check': {
      const template = requireCell(parseCellRaw(value), path, 'template', diags)
      const throwNode = fields.get('throw') ?? null
      if (throwNode === null || isNullScalar(throwNode)) {
        diags.push(diag(CODES.stepFieldMissing, path, 'throw', 'check 缺少 throw（未命中时的终止原因）'))
      } else if (throwNode.kind !== 'scalar') {
        diags.push(diag(CODES.stepFieldTypeMismatch, path, 'throw', 'check 的 throw 必须是字符串标量'))
      }
      const throwMsg = throwNode !== null && throwNode.kind === 'scalar' ? throwNode.value : ''
      return { ...base, kind: 'check', template, throw: throwMsg }
    }
    case 'color': {
      const map = value !== null && value.kind === 'map' ? value : null
      if (map === null) {
        diags.push(diag(CODES.stepFieldMissing, path, 'at', 'color 需要 at/expect 字段'))
        return { ...base, kind: 'color', at: { lit: null }, expect: [], else: branch('else') }
      }
      const get = (k: string) => map.entries.find((e) => entryKey(e) === k)?.value ?? null
      for (const e of map.entries) {
        const k = entryKey(e)
        if (k !== 'at' && k !== 'expect') {
          diags.push(diag(CODES.stepFieldUnknown, path, k, `color 不支持字段 ${k}`))
        }
      }
      return {
        ...base,
        kind: 'color',
        at: coordCell(parseCellRaw(get('at')), path, 'at', diags),
        expect: parseColorExpect(get('expect'), path, `${path}.expect`, diags),
        else: branch('else'),
      }
    }
    case 'loop': {
      const map = value !== null && value.kind === 'map' ? value : null
      if (map === null) {
        diags.push(diag(CODES.stepFieldMissing, path, 'steps', 'loop 需要 times/steps 字段'))
        return { ...base, kind: 'loop', times: null, steps: [] }
      }
      for (const e of map.entries) {
        const k = entryKey(e)
        if (k !== 'times' && k !== 'steps') {
          diags.push(diag(CODES.stepFieldUnknown, path, k, `loop 不支持字段 ${k}`))
        }
      }
      const get = (k: string) => map.entries.find((e) => entryKey(e) === k)?.value ?? null
      const timesNode = get('times')
      let times: number | null = null
      if (timesNode !== null && !isNullScalar(timesNode)) {
        const v = timesNode.kind === 'scalar' ? scalarValue(timesNode) : null
        if (typeof v === 'number') times = v
        else diags.push(diag(CODES.stepFieldTypeMismatch, path, 'times', 'loop 的 times 必须是数字（省略 = 无限）'))
      }
      const stepsNode = get('steps')
      if (stepsNode === null) {
        diags.push(diag(CODES.stepFieldMissing, path, 'steps', 'loop 缺少 steps 子流程'))
      }
      return { ...base, kind: 'loop', times, steps: parseStepsNode(stepsNode, `${path}.steps`, diags) }
    }
    case 'call':
      return {
        ...base,
        kind: 'call',
        target: parseTargetScalar(value, path, diags),
        args: parseArgs(fields.get('args') ?? null, path, diags),
      }
    case 'func':
      return {
        ...base,
        kind: 'func',
        target: parseTargetScalar(value, path, diags),
        args: parseArgs(fields.get('args') ?? null, path, diags),
        then: branch('then'),
        else: branch('else'),
      }
  }
}

function parseTargetScalar(node: YNode | null, path: string, diags: Diagnostic[]): string {
  if (node !== null && node.kind === 'scalar' && !isNullScalar(node)) {
    const v = scalarValue(node)
    return typeof v === 'string' ? v : node.value
  }
  diags.push(diag(CODES.stepFieldTypeMismatch, path, 'target', '调用目标必须是字符串'))
  return ''
}

function parseBool(node: YNode | null, path: string, field: string, diags: Diagnostic[]): boolean | null {
  if (node === null || isNullScalar(node)) return null
  if (node.kind === 'scalar' && node.tag === `${TAG_PREFIX}bool`) return node.value === 'true'
  diags.push(diag(CODES.stepFieldTypeMismatch, path, field, `${field} 必须是布尔值`))
  return null
}

function parseTmplList(node: YNode | null, basePath: string, diags: Diagnostic[]): Cell[] {
  if (node === null || isNullScalar(node)) return []
  if (node.kind !== 'seq') {
    diags.push(diag(CODES.stepListType, basePath, '', 'block 必须是模板列表'))
    return []
  }
  return node.items.map((item) => optionalCell(parseCellRaw(item)) ?? { lit: null })
}

/** 候选值双形态（契约 §4.1/§4.2）：分支步骤列表（click=false，原形态），或
 *  `{click: true, steps: [...]}` 映射（steps 省略 = 空分支，命中即点）。
 *  诊断与错误码同服务端：step_path = 步骤路径，click 字段 = `<候选>[i].click`。 */
function parseCandidateBranch(
  value: YNode | null,
  path: string,
  candPrefix: string,
  stepsPath: string,
  diags: Diagnostic[],
): { click: boolean; steps: Step[] } {
  if (value !== null && value.kind === 'map') {
    for (const e of value.entries) {
      const k = entryKey(e)
      if (k !== 'click' && k !== 'steps') {
        diags.push(diag(CODES.stepFieldUnknown, path, k, `候选值不支持字段 ${k}（允许：click/steps）`))
      }
    }
    const clickNode = value.entries.find((e) => entryKey(e) === 'click')?.value ?? null
    const stepsNode = value.entries.find((e) => entryKey(e) === 'steps')?.value ?? null
    return {
      click: parseBool(clickNode, path, `${candPrefix}.click`, diags) ?? false,
      steps: parseStepsNode(stepsNode, stepsPath, diags),
    }
  }
  return { click: false, steps: parseStepsNode(value, stepsPath, diags) }
}

/** match 候选：每项是单键映射 `模板: [分支步骤]`；`else`/`timeout` 误入候选列表 → 恢复到兄弟键并报错（契约 §4.1）。 */
function parseMatchCandidates(
  node: YNode | null,
  path: string,
  step: Extract<Step, { kind: 'match' }>,
  diags: Diagnostic[],
): void {
  if (node === null || isNullScalar(node)) return
  if (node.kind !== 'seq') {
    diags.push(diag(CODES.stepMatchCandidatesType, path, 'candidates', 'match 的候选必须是列表'))
    return
  }
  node.items.forEach((item, i) => {
    const candPath = `${path}.candidates[${i}]`
    if (item === null || item.kind !== 'map') {
      diags.push(diag(CODES.stepMatchCandidatesType, candPath, 'candidates', 'match 候选必须是单键映射（模板: 分支步骤）'))
      return
    }
    const keys = item.entries.map(entryKey)
    if (keys.includes('else') || keys.includes('timeout')) {
      diags.push(diag(CODES.stepMatchElseInCandidates, path, 'candidates', 'else/timeout 必须是 match 步骤的兄弟键，不能写进候选列表'))
      for (const e of item.entries) {
        const k = entryKey(e)
        if (k === 'else' && step.else.length === 0) step.else = parseStepsNode(e.value, `${path}.else`, diags)
        if (k === 'timeout' && step.timeout === null) step.timeout = optionalCell(parseCellRaw(e.value))
      }
      return
    }
    if (item.entries.length !== 1) {
      diags.push(diag(CODES.stepMatchCandidatesType, candPath, 'candidates', 'match 候选必须是单键映射（模板: 分支步骤）'))
      return
    }
    const entry = item.entries[0]
    step.candidates.push({
      template: candidateKeyCell(entry.key),
      ...parseCandidateBranch(entry.value, path, `candidates[${i}]`, `${candPath}.steps`, diags),
    })
  })
}

function parseColorExpect(node: YNode | null, path: string, basePath: string, diags: Diagnostic[]): { color: Cell; click: boolean; steps: Step[] }[] {
  if (node === null || isNullScalar(node)) return []
  if (node.kind !== 'seq') {
    diags.push(diag(CODES.stepListType, basePath, '', 'expect 必须是有序候选列表（每项 单键映射 颜色: 分支步骤）'))
    return []
  }
  const expect: { color: Cell; click: boolean; steps: Step[] }[] = []
  node.items.forEach((item, i) => {
    const candPath = `${basePath}[${i}]`
    if (item === null || item.kind !== 'map' || item.entries.length !== 1) {
      diags.push(diag(CODES.stepListType, candPath, 'expect', 'color 候选必须是单键映射（颜色: 分支步骤）'))
      return
    }
    const entry = item.entries[0]
    expect.push({
      color: candidateKeyCell(entry.key),
      ...parseCandidateBranch(entry.value, path, `expect[${i}]`, `${candPath}.steps`, diags),
    })
  })
  return expect
}

function parseArgs(node: YNode | null, path: string, diags: Diagnostic[]): Record<string, Cell> {
  if (node === null || isNullScalar(node)) return {}
  if (node.kind !== 'map') {
    diags.push(diag(CODES.stepFieldTypeMismatch, path, 'args', 'args 必须是具名映射（参数名: 取值）'))
    return {}
  }
  const args: Record<string, Cell> = {}
  for (const entry of node.entries) {
    const name = entryKey(entry)
    if (name === '') continue
    args[name] = optionalCell(parseCellRaw(entry.value)) ?? { lit: null }
  }
  return args
}

// ---------- uuid ----------

function withUuids<T extends ScriptModel | FunctionLibraryModel>(model: T): T {
  if ('functions' in model) {
    for (const fn of model.functions) allocateUuids(fn.steps)
  } else {
    allocateUuids(model.steps)
  }
  return model
}
