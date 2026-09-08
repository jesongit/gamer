/**
 * 结构化客户端校验（YAML V1）：返回 {code, step_path, field, message} 列表，
 * 前端据 code + step_path + field 定位卡片与控件，message 仅展示。
 *
 * 职责边界：
 * - codec 解析期诊断（语法结构错误）由 parse 产出，validateSource 合并两者；
 * - 本模块负责 Model 层可判定的约束：引用路径语法、参数默认值类型、
 *   repeat 次数形态、as/函数名合法性、模板存在性（resolver 提供时）、
 *   函数存在性与调用面（knownFunctions 提供时）。
 *   V1 无 loop/break/defaults/call 命名空间等概念，相应校验随旧语法删除。
 */

import {
  parseScript,
  parseFunctionLibrary,
  type ParseResult,
} from './codec'
import { diag, CODES, type Diagnostic } from './diagnostics'
import {
  childStepLists,
  isRefCell,
  type Cell,
  type CallArgs,
  type FunctionLibraryModel,
  type ParamDecl,
  type Program,
  type Step,
} from './model'
import { checkLiteral, isRefPath } from './schema'

// ---------- 校验上下文 ----------

export interface ValidationContext {
  /** 当前可用函数名全集（原生插件函数 + 当前 Package 函数 + 编辑中文件自身）；缺省跳过存在性校验。 */
  knownFunctions?: Set<string>
  /** 模板短名在当前分区是否存在。 */
  resolveTemplate?: (name: string) => boolean
  /** 步骤嵌套深度上限（默认 32，与运行时 MAX_CALL_DEPTH 一致）。 */
  maxDepth?: number
}

// ---------- 入口 ----------

export function validateScript(model: Program, ctx: ValidationContext = {}): Diagnostic[] {
  const diags: Diagnostic[] = []
  validateParamDecls(model.params, 'params', diags)
  // as 赋值产生的变量与 params/vars 同为合法引用目标（顺序无关）
  const asNames = new Set<string>()
  for (const step of model.run) collectAsNames(step, asNames)
  const declaredVars = new Set<string>([
    ...model.params.map((p) => p.name),
    ...Object.keys(model.vars),
    ...asNames,
  ])
  validateStepList(model.run, 'run', declaredVars, ctx, diags, 1)
  return diags
}

function collectAsNames(step: Step, out: Set<string>): void {
  if (step.kind === 'call') {
    if (step.as) out.add(step.as)
  } else if (step.kind === 'if') {
    for (const child of step.then) collectAsNames(child, out)
    for (const child of step.else) collectAsNames(child, out)
  } else if (step.kind === 'repeat') {
    for (const child of step.body) collectAsNames(child, out)
  }
}

export function validateFunctionLibrary(
  model: FunctionLibraryModel,
  ctx: ValidationContext = {},
): Diagnostic[] {
  const diags: Diagnostic[] = []
  const names = new Set<string>(model.functions.map((f) => f.name))
  for (const fn of model.functions) {
    validateParamDecls(fn.params, `functions.${fn.name}.params`, diags)
    const asNames = new Set<string>()
    for (const step of fn.run) collectAsNames(step, asNames)
    const declaredVars = new Set<string>([
      ...fn.params.map((p) => p.name),
      ...Object.keys(fn.vars),
      ...asNames,
    ])
    validateStepList(fn.run, `functions.${fn.name}.run`, declaredVars, { ...ctx, knownFunctions: union(ctx.knownFunctions, names) }, diags, 1)
  }
  return diags
}

function union(a: Set<string> | undefined, b: Set<string>): Set<string> {
  const out = new Set<string>(a ?? [])
  for (const v of b) out.add(v)
  return out
}

/** 解析 + 校验一步到位（编辑器保存前 / 测试使用）。 */
export function validateSource(
  text: string,
  kind: 'script' | 'function_library',
  ctx: ValidationContext & { file?: string } = {},
): { result: ParseResult; diagnostics: Diagnostic[] } {
  const result = kind === 'script'
    ? parseScript(text)
    : parseFunctionLibrary(text, { file: ctx.file })
  const parsed = result.diagnostics
  const modelDiags = result.kind === 'script'
    ? validateScript(result.model, ctx)
    : validateFunctionLibrary(result.model, ctx)
  return { result, diagnostics: [...parsed, ...modelDiags] }
}

// ---------- 参数声明 ----------

function validateParamDecls(decls: ParamDecl[], basePath: string, diags: Diagnostic[]): void {
  decls.forEach((decl, i) => {
    if (decl.default !== null && decl.default !== undefined) {
      const problem = checkLiteral(decl.type, decl.default)
      if (problem) {
        diags.push(diag(
          CODES.paramsDefaultInvalid,
          `${basePath}.${decl.name}`,
          'default',
          `参数 ${decl.name} 默认值与类型 ${decl.type} 不符：${problem.message}`,
        ))
      }
    }
    void i
  })
}

// ---------- 步骤树 ----------

function validateStepList(
  steps: Step[],
  basePath: string,
  declaredVars: Set<string>,
  ctx: ValidationContext,
  diags: Diagnostic[],
  depth: number,
): void {
  const maxDepth = ctx.maxDepth ?? 32
  steps.forEach((step, i) => {
    const path = `${basePath}[${i}]`
    validateStep(step, path, declaredVars, ctx, diags)
    if (depth >= maxDepth) {
      diags.push(diag('yaml.flow.nesting_depth', path, '', `步骤嵌套超过 ${maxDepth} 层`))
      return
    }
    for (const child of childStepLists(step)) {
      validateStepList(child.list, `${path}.${child.key}`, declaredVars, ctx, diags, depth + 1)
    }
  })
}

function validateStep(
  step: Step,
  path: string,
  declaredVars: Set<string>,
  ctx: ValidationContext,
  diags: Diagnostic[],
): void {
  switch (step.kind) {
    case 'call': {
      if (!isIdentifierSafe(step.fn)) {
        diags.push(diag(CODES.nameInvalid, path, step.fn, `函数名 ${JSON.stringify(step.fn)} 非法——只允许小写字母、数字、下划线`))
      }
      if (ctx.knownFunctions && !ctx.knownFunctions.has(step.fn)) {
        diags.push(diag(CODES.fnNotFound, path, step.fn, `函数 ${step.fn} 不存在（可用：原生插件函数 + 当前 Package 函数）`))
      }
      validateArgs(step.args, path, declaredVars, ctx, diags)
      if (step.as !== null && !isIdentifierSafe(step.as)) {
        diags.push(diag(CODES.asInvalid, path, 'as', `as 变量名 ${JSON.stringify(step.as)} 非法——只允许小写字母、数字、下划线`))
      }
      break
    }
    case 'if': {
      validateCell(step.cond, path, 'if', declaredVars, ctx, diags)
      if (step.then.length === 0 && step.else.length === 0) {
        diags.push(diag(CODES.ifThenMissing, path, 'then', 'if 步骤的 then/else 分支均为空'))
      }
      break
    }
    case 'repeat': {
      if (isRefCell(step.times)) {
        validateRef(step.times.ref, path, 'repeat', declaredVars, diags)
      } else {
        const v = step.times.lit
        if (typeof v !== 'number' || !Number.isInteger(v) || v < 0) {
          diags.push(diag(CODES.repeatTimesInvalid, `${path}.repeat`, 'repeat', `repeat 次数必须是零或正整数，收到 ${JSON.stringify(v ?? null)}`))
        }
      }
      if (step.body.length === 0) {
        // 空转体合法（预算兜底），仅提示级校验留给运行时；不产生诊断
      }
      break
    }
    case 'return':
      validateCell(step.value, path, 'return', declaredVars, ctx, diags)
      break
  }
}

function validateArgs(
  args: CallArgs,
  path: string,
  declaredVars: Set<string>,
  ctx: ValidationContext,
  diags: Diagnostic[],
): void {
  switch (args.kind) {
    case 'value':
      validateCell(args.cell, path, '', declaredVars, ctx, diags)
      break
    case 'map':
      for (const [name, cell] of Object.entries(args.entries)) {
        validateCell(cell, `${path}.${name}`, name, declaredVars, ctx, diags)
      }
      break
    default:
      break
  }
}

function validateCell(
  cell: Cell,
  path: string,
  field: string,
  declaredVars: Set<string>,
  ctx: ValidationContext,
  diags: Diagnostic[],
): void {
  if (isRefCell(cell)) {
    validateRef(cell.ref, path, field, declaredVars, diags)
    return
  }
  // 字面量：模板名可达性（find/wait_find/tap_template/wait_disappear 的 template 实参）
  const template = templateNameOf(cell.lit)
  if (template !== null && ctx.resolveTemplate && !ctx.resolveTemplate(template)) {
    diags.push(diag('yaml.resource.tmpl_not_found', path, field, `模板 ${template} 在当前 Package 不存在`))
  }
}

function templateNameOf(value: unknown): string | null {
  if (typeof value === 'string' && value.trim() !== '') return value
  if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
    const t = (value as Record<string, unknown>).template
    if (typeof t === 'string' && t.trim() !== '') return t
  }
  return null
}

function validateRef(
  refPath: string,
  path: string,
  field: string,
  declaredVars: Set<string>,
  diags: Diagnostic[],
): void {
  if (!isRefPath(refPath)) {
    diags.push(diag(CODES.exprInvalid, path, field, `非法变量引用 $${refPath}——V1 只支持 $name 与 $name.field`))
    return
  }
  const head = refPath.split('.')[0]
  if (head && !declaredVars.has(head)) {
    diags.push(diag(CODES.varUndefined, path, field, `未定义变量 $${refPath}（可用：params/vars 声明或 as 赋值）`))
  }
}

function isIdentifierSafe(v: string): boolean {
  return /^[a-z_][a-z0-9_]*$/.test(v)
}
