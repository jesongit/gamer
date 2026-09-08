/**
 * YAML V1 ↔ Model 双向转换（Gamer V1 简化计划 Phase 1/4）。
 *
 * 与宿主侧 `server/src/extensions/gamer_yaml/syntax.rs` 语义对齐：
 * - 脚本顶层 = name? / params? / vars? / run；出现 `version` 字段报
 *   yaml.version.removed（旧 v3 源明确拒绝，无 fallback）；
 * - 步骤 = 恰好一个动作键（函数名或 if/repeat/return）+ 可选 as；
 *   then/else/do 为 if/repeat 的结构键；
 * - 表达式：字符串 `$path` → 引用（`$$text` → 字面量 `$text`）；其余字面量；
 * - 序列化：手写确定性规范输出器（decode(encode(model)) == model）。
 */

import { CORE_SCHEMA, dump, load } from 'js-yaml'
import {
  allocateUuids,
  isRefCell,
  type CallArgs,
  type Cell,
  type FunctionLibraryModel,
  type FunctionModel,
  type ParamDecl,
  type Program,
  type Step,
  newStepUuid,
  RESERVED_WORDS,
} from './model'
import { CODES, diag, type Diagnostic } from './diagnostics'
import {
  isIdentifier,
  isRefPath,
  normalizeParamType,
  PARAM_TYPES,
  parseTimeMs,
} from './schema'
import type { ParamType } from './model'

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

/** 解析可执行脚本（automations/ 类型）。 */
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

/** 解析函数库（functions/ 类型；顶层 functions: 包装）。 */
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
  return { name: null, params: [], vars: {}, run: [] }
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

function fallbackScalar(v: unknown): string {
  if (v === null || v === undefined) return 'null'
  if (typeof v === 'string') return plainScalar(v)
  if (typeof v === 'number') return fmtNum(v)
  if (typeof v === 'boolean') return String(v)
  // 纯数字数组（region/point）按 YAML flow 序列化（逗号+空格，规范形态）
  if (Array.isArray(v) && v.every((item) => typeof item === 'number' && Number.isFinite(item))) {
    return `[${v.map(fmtNum).join(', ')}]`
  }
  // 其余复合字面量（对象/嵌套）：紧凑 JSON 即合法 YAML flow 形态
  return JSON.stringify(v)
}

/** 取值单元格行内渲染（$ref 或字面量）。 */
function cellInline(cell: Cell | null | undefined): string {
  if (cell === null || cell === undefined) return 'null'
  if (isRefCell(cell)) return `$${cell.ref}`
  return fallbackScalar(cell.lit)
}

// ---------- 序列化：脚本 ----------

function serializeScript(model: Program): string {
  const lines: string[] = []
  if (model.name !== null && model.name !== '') {
    lines.push(`name: ${plainScalar(model.name)}`)
  }
  if (model.params.length > 0) {
    lines.push('params:')
    emitParamDecls(model.params, 2, lines)
  }
  const varNames = Object.keys(model.vars)
  if (varNames.length > 0) {
    lines.push('vars:')
    for (const name of varNames) {
      lines.push(`  ${plainScalar(name)}: ${fallbackScalar(model.vars[name])}`)
    }
  }
  emitRun(model.run, 0, lines)
  return lines.join('\n') + '\n'
}

// ---------- 序列化：函数库 ----------

function serializeFunctionLibrary(model: FunctionLibraryModel): string {
  const lines: string[] = []
  lines.push('functions:')
  model.functions.forEach((fn) => {
    lines.push(`  ${plainScalar(fn.name)}:`)
    if (fn.description !== '') {
      lines.push(`    description: ${plainScalar(fn.description)}`)
    }
    if (fn.params.length > 0) {
      lines.push('    params:')
      emitParamDecls(fn.params, 6, lines)
    }
    const varNames = Object.keys(fn.vars)
    if (varNames.length > 0) {
      lines.push('    vars:')
      for (const name of varNames) {
        lines.push(`      ${plainScalar(name)}: ${fallbackScalar(fn.vars[name])}`)
      }
    }
    if (fn.returns !== null && fn.returns !== undefined) {
      lines.push(`    returns: ${fallbackScalar(fn.returns)}`)
    }
    emitRun(fn.run, 4, lines)
  })
  return lines.join('\n') + '\n'
}

function emitParamDecls(decls: ParamDecl[], col: number, lines: string[]): void {
  for (const decl of decls) {
    lines.push(`${' '.repeat(col)}${plainScalar(decl.name)}:`)
    lines.push(`${' '.repeat(col + 2)}type: ${plainScalar(decl.type)}`)
    if (decl.required) lines.push(`${' '.repeat(col + 2)}required: true`)
    if (decl.default !== null && decl.default !== undefined) {
      lines.push(`${' '.repeat(col + 2)}default: ${fallbackScalar(decl.default)}`)
    }
    if (decl.desc !== '') {
      lines.push(`${' '.repeat(col + 2)}desc: ${plainScalar(decl.desc)}`)
    }
  }
}

// ---------- 序列化：步骤 ----------

/** run 键 + 步骤序列（空列表 → `run: []`）。 */
function emitRun(steps: Step[], col: number, lines: string[]): void {
  if (steps.length === 0) {
    lines.push(`${' '.repeat(col)}run: []`)
    return
  }
  lines.push(`${' '.repeat(col)}run:`)
  for (const step of steps) emitStepItem(step, col + 2, lines)
}

/** 序列项：`- ` 与步骤首键同行，后续键与首键同列。 */
function emitStepItem(step: Step, col: number, lines: string[]): void {
  const body: string[] = []
  emitStep(step, col + 2, body)
  if (body.length > 0) {
    lines.push(`${' '.repeat(col)}- ${body[0]!.trimStart()}`)
    lines.push(...body.slice(1))
  } else {
    lines.push(`${' '.repeat(col)}-`)
  }
}

function emitField(key: string, inline: string, col: number, lines: string[]): void {
  lines.push(`${' '.repeat(col)}${key}: ${inline}`)
}

function emitStep(step: Step, col: number, lines: string[]): void {
  switch (step.kind) {
    case 'call': {
      switch (step.args.kind) {
        case 'none':
          lines.push(`${' '.repeat(col)}${plainScalar(step.fn)}: {}`)
          break
        case 'value': {
          const inline = cellInline(step.args.cell)
          if (inline.includes('\n')) {
            lines.push(`${' '.repeat(col)}${plainScalar(step.fn)}:`)
            lines.push(`${' '.repeat(col + 2)}${inline}`)
          } else {
            lines.push(`${' '.repeat(col)}${plainScalar(step.fn)}: ${inline}`)
          }
          break
        }
        case 'map': {
          const names = Object.keys(step.args.entries)
          if (names.length === 0) {
            lines.push(`${' '.repeat(col)}${plainScalar(step.fn)}: {}`)
          } else {
            lines.push(`${' '.repeat(col)}${plainScalar(step.fn)}:`)
            for (const name of names) {
              lines.push(`${' '.repeat(col + 2)}${plainScalar(name)}: ${cellInline(step.args.entries[name])}`)
            }
          }
          break
        }
      }
      if (step.as !== null) {
        lines.push(`${' '.repeat(col)}as: ${plainScalar(step.as)}`)
      }
      return
    }
    case 'if': {
      lines.push(`${' '.repeat(col)}if: ${cellInline(step.cond)}`)
      lines.push(`${' '.repeat(col)}then:`)
      for (const child of step.then) emitStepItem(child, col + 2, lines)
      if (step.else.length > 0) {
        lines.push(`${' '.repeat(col)}else:`)
        for (const child of step.else) emitStepItem(child, col + 2, lines)
      }
      return
    }
    case 'repeat': {
      lines.push(`${' '.repeat(col)}repeat: ${cellInline(step.times)}`)
      lines.push(`${' '.repeat(col)}do:`)
      for (const child of step.body) emitStepItem(child, col + 2, lines)
      return
    }
    case 'return': {
      lines.push(`${' '.repeat(col)}return: ${cellInline(step.value)}`)
      return
    }
  }
}

// ---------- 解析层 ----------

let loadError: string | null = null

/** plain load（CORE_SCHEMA）；空文档按空映射处理。 */
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
  if (typeof doc !== 'object') return { __seq: true }
  return doc as Record<string, unknown>
}

function isSeqSentinel(v: unknown): boolean {
  return typeof v === 'object' && v !== null && (v as { __seq?: boolean }).__seq === true
}

// ---------- 顶层解析：脚本 ----------

const TOP_LEVEL_KEYS = new Set(['name', 'params', 'vars', 'run'])

function parseScriptRoot(root: Record<string, unknown> | { __seq: true }, diags: Diagnostic[]): Program {
  if (isSeqSentinel(root) || Array.isArray(root)) {
    diags.push(diag(CODES.rootType, '', '', '脚本顶层必须是映射（name/params/vars/run）'))
    return emptyProgram()
  }
  const map = root as Record<string, unknown>
  if ('version' in map) {
    diags.push(diag(
      CODES.versionRemoved, 'version', 'version',
      'V1 语法不再使用 version 字段——请删除该行（旧 v3 脚本不兼容，需按新语法重写）',
    ))
    return emptyProgram()
  }
  const model = emptyProgram()
  let hasRun = false
  for (const key of Object.keys(map)) {
    if (!TOP_LEVEL_KEYS.has(key)) {
      diags.push(diag(CODES.topLevelUnknownKey, '', key, `未知顶层字段 ${JSON.stringify(key)}——V1 只支持 name/params/vars/run`))
      continue
    }
    switch (key) {
      case 'name':
        model.name = typeof map.name === 'string' ? map.name : null
        break
      case 'params':
        model.params = parseParamDecls(map.params, 'params', diags)
        break
      case 'vars':
        model.vars = parseVars(map.vars, 'vars', diags)
        break
      case 'run':
        hasRun = true
        model.run = parseStepsNode(map.run, 'run', diags)
        break
    }
  }
  if (!hasRun) {
    diags.push(diag(CODES.runMissing, 'run', 'run', '脚本缺少顶层 run（可为空列表，不可省略）'))
  }
  const paramNames = new Set(model.params.map((p) => p.name))
  for (const name of Object.keys(model.vars)) {
    if (paramNames.has(name)) {
      diags.push(diag(CODES.varsConflict, 'vars', name, `变量 ${name} 与参数同名——参数与 vars 不得重名`))
    }
  }
  return model
}

// ---------- 顶层解析：函数库 ----------

const FUNCTION_KEYS = new Set(['description', 'params', 'vars', 'returns', 'run'])

function parseFunctionRoot(root: Record<string, unknown> | { __seq: true }, file: string, diags: Diagnostic[]): FunctionLibraryModel {
  const functions: FunctionModel[] = []
  if (isSeqSentinel(root) || Array.isArray(root)) {
    diags.push(diag(CODES.rootType, '', '', '函数文件顶层必须是映射（functions: {<函数名>: …}）'))
    return { file, functions }
  }
  const map = root as Record<string, unknown>
  const raw = map.functions
  if (raw === undefined || raw === null) {
    const looksLikeFn = Object.keys(map).some((key) => key === 'run' || key === 'params')
    diags.push(diag(
      CODES.functionsMissing, 'functions', 'functions',
      `函数文件缺少 functions: 顶层包装${looksLikeFn ? '（顶层看起来是单个函数定义——需要包在 functions: 下）' : ''}`,
    ))
    return { file, functions }
  }
  if (typeof raw !== 'object' || Array.isArray(raw)) {
    diags.push(diag(CODES.functionsShape, 'functions', 'functions', 'functions 必须是映射（函数名 → 定义）'))
    return { file, functions }
  }
  const seen = new Set<string>()
  for (const [name, value] of Object.entries(raw as Record<string, unknown>)) {
    if (!isIdentifier(name)) {
      diags.push(diag(CODES.nameInvalid, `functions.${name}`, name, `函数名 ${JSON.stringify(name)} 非法——只允许小写字母、数字、下划线（如 claim_daily）`))
      continue
    }
    if ((RESERVED_WORDS as readonly string[]).includes(name)) {
      diags.push(diag(CODES.fnReserved, `functions.${name}`, name, `函数名 ${name} 是保留关键字（if/repeat/return）`))
      continue
    }
    if (seen.has(name)) {
      diags.push(diag(CODES.fnDuplicate, 'functions', name, `函数 ${name} 在同一文件中重复定义`))
      continue
    }
    seen.add(name)
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
      diags.push(diag(CODES.functionsShape, `functions.${name}`, name, `函数 ${name} 的定义必须是映射`))
      continue
    }
    const fn: FunctionModel = { name, description: '', params: [], vars: {}, returns: null, run: [] }
    const def = value as Record<string, unknown>
    let hasRun = false
    for (const key of Object.keys(def)) {
      if (!FUNCTION_KEYS.has(key)) {
        diags.push(diag(CODES.functionsShape, `functions.${name}.${key}`, key, `函数 ${name} 定义不支持字段 ${JSON.stringify(key)}（description/params/vars/returns/run）`))
        continue
      }
      switch (key) {
        case 'description':
          fn.description = typeof def.description === 'string' ? def.description : ''
          break
        case 'params':
          fn.params = parseParamDecls(def.params, `functions.${name}.params`, diags)
          break
        case 'vars':
          fn.vars = parseVars(def.vars, `functions.${name}.vars`, diags)
          break
        case 'returns':
          fn.returns = def.returns ?? null
          break
        case 'run':
          hasRun = true
          fn.run = parseStepsNode(def.run, `functions.${name}.run`, diags)
          break
      }
    }
    if (!hasRun) {
      diags.push(diag(CODES.functionsShape, `functions.${name}.run`, 'run', `函数 ${name} 缺少 run 步骤列表`))
    }
    const paramNames = new Set(fn.params.map((p) => p.name))
    for (const varName of Object.keys(fn.vars)) {
      if (paramNames.has(varName)) {
        diags.push(diag(CODES.varsConflict, `functions.${name}.vars`, varName, `函数 ${name} 的变量 ${varName} 与参数同名`))
      }
    }
    functions.push(fn)
  }
  return { file, functions }
}

// ---------- 参数声明解析（映射形态：名 → {type, required?, default?, desc?}） ----------

export function parseParamDecls(node: unknown, basePath: string, diags: Diagnostic[]): ParamDecl[] {
  if (node === null || node === undefined) return []
  if (typeof node !== 'object' || Array.isArray(node)) {
    diags.push(diag(CODES.paramsType, basePath, '', 'params 必须是映射（参数名 → 声明）'))
    return []
  }
  const decls: ParamDecl[] = []
  const seen = new Set<string>()
  for (const [name, item] of Object.entries(node as Record<string, unknown>)) {
    if (!isIdentifier(name)) {
      diags.push(diag(CODES.paramsNameInvalid, basePath, name, `参数名 ${JSON.stringify(name)} 非法——只允许小写字母、数字、下划线`))
      continue
    }
    if (seen.has(name)) {
      diags.push(diag(CODES.paramsNameDuplicate, basePath, name, `参数 ${name} 重复声明`))
      continue
    }
    seen.add(name)
    if (item === null || typeof item !== 'object' || Array.isArray(item)) {
      diags.push(diag(CODES.paramsInvalid, `${basePath}.${name}`, name, `参数 ${name} 的声明必须是映射（type/required/default/desc）`))
      continue
    }
    const map = item as Record<string, unknown>
    let type: ParamType | null = null
    let required = false
    let defaultValue: unknown = null
    let hasDefault = false
    let desc = ''
    for (const key of Object.keys(map)) {
      if (key !== 'type' && key !== 'required' && key !== 'default' && key !== 'desc') {
        diags.push(diag(CODES.paramsUnknownKey, `${basePath}.${name}.${key}`, key, `参数 ${name} 声明不支持字段 ${JSON.stringify(key)}（type/required/default/desc）`))
        continue
      }
      switch (key) {
        case 'type': {
          const raw = map.type
          if (typeof raw !== 'string' || raw.trim() === '') {
            diags.push(diag(CODES.paramsType, `${basePath}.${name}.type`, 'type', `参数 ${name} 的 type 必须是字符串`))
            break
          }
          const normalized = normalizeParamType(raw.trim())
          if (!(PARAM_TYPES as readonly string[]).includes(normalized)) {
            diags.push(diag(CODES.paramsType, `${basePath}.${name}.type`, 'type', `参数 ${name} 的未知类型 ${JSON.stringify(raw)}——支持 any/boolean/integer/number/string/list/object/duration/point/template/key`))
            break
          }
          type = normalized
          break
        }
        case 'required':
          if (typeof map.required === 'boolean') required = map.required
          else diags.push(diag(CODES.paramsType, `${basePath}.${name}.required`, 'required', `参数 ${name} 的 required 必须是布尔值`))
          break
        case 'default':
          defaultValue = map.default ?? null
          hasDefault = true
          break
        case 'desc':
          desc = typeof map.desc === 'string' ? map.desc : ''
          break
      }
    }
    if (type === null) {
      diags.push(diag(CODES.paramsType, `${basePath}.${name}.type`, 'type', `参数 ${name} 缺少 type`))
      continue
    }
    if (hasDefault && defaultValue !== null) {
      // 默认值类型校验归校验层（checkLiteral），这里原样保留
    }
    decls.push({ name, type, required, default: defaultValue, desc })
  }
  return decls
}

// ---------- vars 解析（字面量表） ----------

function parseVars(node: unknown, basePath: string, diags: Diagnostic[]): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  if (node === null || node === undefined) return out
  if (typeof node !== 'object' || Array.isArray(node)) {
    diags.push(diag(CODES.varsType, basePath, '', 'vars 必须是映射（变量名 → 字面量）'))
    return out
  }
  for (const [name, value] of Object.entries(node as Record<string, unknown>)) {
    if (!isIdentifier(name)) {
      diags.push(diag(CODES.nameInvalid, `${basePath}.${name}`, name, `变量名 ${JSON.stringify(name)} 非法——只允许小写字母、数字、下划线`))
      continue
    }
    out[name] = value
  }
  return out
}

// ---------- 步骤解析 ----------

const KEYWORDS = new Set(['if', 'repeat', 'return'])
const STRUCTURAL_KEYS = new Set(['then', 'else', 'do'])

function parseStepsNode(node: unknown, basePath: string, diags: Diagnostic[]): Step[] {
  if (node === null || node === undefined) return []
  if (!Array.isArray(node)) {
    diags.push(diag(CODES.stepsType, basePath, '', '步骤必须是列表（- 开头）'))
    return []
  }
  const steps: Step[] = []
  node.forEach((item, i) => {
    const step = parseStepNode(item, `${basePath}[${i}]`, diags)
    if (step !== null) steps.push(step)
  })
  return steps
}

/** 值节点 → 表达式单元格：`$path` 引用；`$$text` → 字面量 `$text`；其余字面量。 */
function exprCell(v: unknown, path: string, field: string, diags: Diagnostic[]): Cell {
  if (v === null || v === undefined) return { lit: null }
  if (typeof v === 'string') {
    if (v.startsWith('$$')) return { lit: v.slice(1) }
    if (v.startsWith('$')) {
      const name = v.slice(1)
      if (name !== '' && isRefPath(name)) return { ref: name }
      diags.push(diag(CODES.exprInvalid, path, field, `非法变量引用 ${JSON.stringify(v)}——V1 只支持 $name 与 $name.field（$name 段为小写标识符）；字面量 $ 用 $$ 转义`))
      return { lit: v }
    }
    return { lit: v }
  }
  return { lit: v }
}

function parseStepNode(item: unknown, path: string, diags: Diagnostic[]): Step | null {
  if (item === null || item === undefined) return null
  if (typeof item !== 'object' || Array.isArray(item)) {
    diags.push(diag(CODES.stepShape, path, '', '步骤必须是映射（如 `- tap: [0.5, 0.5]`）'))
    return null
  }
  const map = item as Record<string, unknown>
  const keys = Object.keys(map)
  if (keys.length === 0) {
    diags.push(diag(CODES.stepMissing, path, '', '步骤为空——需要一个函数调用或 if/repeat/return'))
    return null
  }

  let keyword: string | null = null
  let keywordValue: unknown = null
  let action: string | null = null
  let actionValue: unknown = null
  let saveAs: string | null = null
  let structural = false

  for (const key of keys) {
    const value = map[key]
    if (key === 'as') {
      if (typeof value !== 'string' || !isIdentifier(value)) {
        diags.push(diag(CODES.asInvalid, path, 'as', 'as 必须是变量名（小写标识符）'))
        continue
      }
      saveAs = value
      continue
    }
    if (KEYWORDS.has(key)) {
      if (keyword !== null) {
        diags.push(diag(CODES.stepMulti, path, '', '一个步骤只能有一个控制流关键字（if/repeat/return）'))
        continue
      }
      keyword = key
      keywordValue = value
      continue
    }
    if (STRUCTURAL_KEYS.has(key)) {
      structural = true
      continue
    }
    if (action !== null) {
      diags.push(diag(CODES.stepMulti, path, '', `一个步骤只能有一个函数调用（本步同时出现 ${key} 等）`))
      continue
    }
    action = key
    actionValue = value
  }

  if (structural && keyword === null) {
    diags.push(diag(CODES.stepMulti, path, '', 'then/else/do 只能在 if/repeat 步骤内使用'))
    return null
  }

  if (keyword !== null && action === null) {
    if (saveAs !== null) {
      diags.push(diag(CODES.asInvalid, path, 'as', `${keyword} 步骤不支持 as（只有函数调用有返回值）`))
    }
    switch (keyword) {
      case 'if': {
        if (keywordValue === null || keywordValue === undefined) {
          diags.push(diag(CODES.exprInvalid, `${path}.if`, 'if', 'if 条件不能为空'))
        }
        const thenNode = map.then
        if (thenNode === undefined) {
          diags.push(diag(CODES.ifThenMissing, path, 'then', 'if 步骤缺少 then 分支'))
          return { uuid: newStepUuid(), kind: 'if', cond: exprCell(keywordValue, `${path}.if`, 'if', diags), then: [], else: [] }
        }
        return {
          uuid: newStepUuid(),
          kind: 'if',
          cond: exprCell(keywordValue, `${path}.if`, 'if', diags),
          then: parseStepsNode(thenNode, `${path}.then`, diags),
          else: parseStepsNode(map.else, `${path}.else`, diags),
        }
      }
      case 'repeat': {
        const times = exprCell(keywordValue, `${path}.repeat`, 'repeat', diags)
        if (
          times.lit !== undefined && typeof times.lit === 'number'
          && (!Number.isFinite(times.lit) || times.lit < 0 || !Number.isInteger(times.lit))
        ) {
          diags.push(diag(CODES.repeatTimesInvalid, `${path}.repeat`, 'repeat', 'repeat 次数必须是零或正整数'))
        }
        const doNode = map.do
        if (doNode === undefined) {
          diags.push(diag(CODES.repeatDoMissing, path, 'do', 'repeat 步骤缺少 do 循环体'))
          return { uuid: newStepUuid(), kind: 'repeat', times, body: [] }
        }
        return {
          uuid: newStepUuid(),
          kind: 'repeat',
          times,
          body: parseStepsNode(doNode, `${path}.do`, diags),
        }
      }
      case 'return':
        return { uuid: newStepUuid(), kind: 'return', value: exprCell(keywordValue, `${path}.return`, 'return', diags) }
    }
  }

  if (keyword !== null && action !== null) {
    diags.push(diag(CODES.stepMulti, path, '', `控制流关键字不能与函数调用 ${action} 同时出现在一步`))
    return null
  }

  if (action !== null) {
    if (!isIdentifier(action)) {
      diags.push(diag(CODES.nameInvalid, path, action, `函数名 ${JSON.stringify(action)} 非法——只允许小写字母、数字、下划线（如 tap、wait_find）`))
      return null
    }
    return {
      uuid: newStepUuid(),
      kind: 'call',
      fn: action,
      args: parseCallArgs(actionValue, `${path}.${action}`, diags),
      as: saveAs,
    }
  }

  diags.push(diag(CODES.stepMissing, path, '', '步骤只有 as——需要一个函数调用或 if/repeat/return'))
  return null
}

/** 函数实参：null → 无参；标量/数组/$ref → 位置值；映射 → 命名参数（值 = Cell）。 */
function parseCallArgs(value: unknown, path: string, diags: Diagnostic[]): CallArgs {
  if (value === null || value === undefined) return { kind: 'none' }
  if (Array.isArray(value)) return { kind: 'value', cell: exprCell(value, path, '', diags) }
  if (typeof value === 'object') {
    const entries: Record<string, Cell> = {}
    for (const [name, v] of Object.entries(value as Record<string, unknown>)) {
      entries[name] = exprCell(v, `${path}.${name}`, name, diags)
    }
    return { kind: 'map', entries }
  }
  return { kind: 'value', cell: exprCell(value, path, '', diags) }
}

// ---------- uuid ----------

function withUuids<T extends Program | FunctionLibraryModel>(model: T): T {
  if ('functions' in model) {
    for (const fn of model.functions) allocateUuids(fn.run)
  } else {
    allocateUuids(model.run)
  }
  return model
}

// ---------- 工具（面板/校验层共用） ----------

/** 参数默认值字面量校验（codec 内不拦，校验层消费 checkLiteral）。 */
export { parseTimeMs }
