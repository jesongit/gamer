/**
 * 结构化客户端校验（YAML V1）：返回 {code, step_path, field, message} 列表，
 * 前端据 code + step_path + field 定位卡片与控件，message 仅展示。
 *
 * 职责边界：
 * - codec 解析期诊断（语法结构错误）由 parse 产出，validateSource 合并两者；
 * - 本模块负责 Model 层可判定的约束：引用路径语法、参数默认值类型、
 *   repeat 次数形态、as/函数名合法性、模板存在性（resolver 提供时）、
 *   函数存在性与调用面（knownFunctions 提供时）。
 *   V1 无 loop/defaults/call 命名空间等概念，相应校验随旧语法删除。
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
import { checkLiteral, hasParamDefault, isRefPath, isFunctionName } from './schema'

// ---------- 校验上下文 ----------

export interface ValidationContext {
  /** 当前可用函数名全集（原生插件函数 + 当前 Package 函数 + 编辑中文件自身）；缺省跳过存在性校验。 */
  knownFunctions?: Set<string>
  resolveParams?: (name: string) => ParamDecl[] | null
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
  } else if (step.kind === 'match_templates') {
    for (const c of step.cases) {
      const local = new Set<string>()
      for (const child of c.body) collectAsNames(child, local)
      for (const name of local) if (name !== c.as) out.add(name)
    }
    for (const child of step.else) collectAsNames(child, out)
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
  // 未提供完整目录时与脚本校验一致，跳过函数存在性检查。
  // 本文件的函数名只是局部信息，不能据此拒绝原生函数或其他文件中的函数。
  const knownFunctions = ctx.knownFunctions
    ? new Set([...ctx.knownFunctions, ...names])
    : undefined
  for (const fn of model.functions) {
    validateParamDecls(fn.params, `functions.${fn.name}.params`, diags)
    const asNames = new Set<string>()
    for (const step of fn.run) collectAsNames(step, asNames)
    const declaredVars = new Set<string>([
      'name',
      ...fn.params.map((p) => p.name),
      ...Object.keys(fn.vars),
      ...asNames,
    ])
    validateStepList(fn.run, `functions.${fn.name}.run`, declaredVars, { ...ctx, knownFunctions }, diags, 1)
  }
  return diags
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
    if (hasParamDefault(decl)) {
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
  inLoop = false,
): void {
  const maxDepth = ctx.maxDepth ?? 32
  steps.forEach((step, i) => {
    const path = `${basePath}[${i}]`
    validateStep(step, path, declaredVars, ctx, diags)
    if (step.kind === 'break' && !inLoop) diags.push(diag('yaml.break.outside_loop', path, 'break', 'break 只能在当前脚本或函数的 repeat 循环内使用'))
    if (depth >= maxDepth) {
      diags.push(diag('yaml.flow.nesting_depth', path, '', `步骤嵌套超过 ${maxDepth} 层`))
      return
    }
    if (step.kind === 'match_templates') {
      step.cases.forEach((c, n) => validateStepList(c.body, `${path}.cases[${n}].do`, new Set([...declaredVars, ...(c.as ? [c.as] : [])]), ctx, diags, depth + 1, inLoop))
      validateStepList(step.else, `${path}.else`, declaredVars, ctx, diags, depth + 1, inLoop)
    } else for (const child of childStepLists(step)) {
      validateStepList(child.list, `${path}.${child.key}`, declaredVars, ctx, diags, depth + 1, inLoop || step.kind === 'repeat')
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
      if (!isFunctionName(step.fn)) {
        diags.push(diag(CODES.nameInvalid, path, step.fn, `函数名 ${JSON.stringify(step.fn)} 非法——允许中文、小写字母、数字、下划线，不能以数字开头`))
      }
      if (ctx.knownFunctions && !ctx.knownFunctions.has(step.fn)) {
        diags.push(diag(CODES.fnNotFound, path, step.fn, `函数 ${step.fn} 不存在（可用：原生插件函数 + 当前 Package 函数）`))
      }
      validateArgs(step.args, path, declaredVars, ctx, diags)
      const displayName = step.args.kind === 'map' ? step.args.entries.name : null
      if (displayName && !isRefCell(displayName) && typeof displayName.lit !== 'string') {
        diags.push(diag('yaml.args.type', path, 'name', 'name 必须是字符串或变量引用'))
      }
      const schema = ctx.resolveParams?.(step.fn)
      const templateLists = schema?.filter(p => p.type === 'list' && p.items?.type === 'template').map(p => p.name)
        ?? (step.fn === 'wait_find' ? ['obstacles'] : [])
      for (const name of templateLists) {
        const cell = step.args.kind === 'map' ? step.args.entries[name] : null
        if (!cell || isRefCell(cell)) continue
        if (!Array.isArray(cell.lit)) {
          diags.push(diag('yaml.args.type', path, name, `${name} 必须是模板列表`))
          continue
        }
        cell.lit.forEach((value, index) => {
          if (typeof value !== 'string' || !value.trim()) {
            diags.push(diag('yaml.args.type', path, name, `${name}[${index}] 必须是非空模板名`))
          } else if (!value.startsWith('$') && ctx.resolveTemplate && !ctx.resolveTemplate(value)) {
            diags.push(diag('yaml.resource.tmpl_not_found', path, name, `模板 ${value} 在当前 Package 不存在`))
          }
        })
      }
      const templateParams = schema?.filter(p => p.type === 'template').map(p => p.name)
        ?? (['find', 'wait_find', 'tap_template', 'wait_disappear'].includes(step.fn) ? ['template'] : [])
      for (const name of templateParams) {
        const isFirstParam = schema ? schema[0]?.name === name : name === 'template'
        const cell = step.args.kind === 'map' ? step.args.entries[name]
          : step.args.kind === 'value' && isFirstParam ? step.args.cell : null
        if (cell && !isRefCell(cell) && typeof cell.lit === 'string' && cell.lit.trim() && ctx.resolveTemplate && !ctx.resolveTemplate(cell.lit)) {
          diags.push(diag('yaml.resource.tmpl_not_found', path, name, `模板 ${cell.lit} 在当前 Package 不存在`))
        }
      }
      if (step.as !== null && !isIdentifierSafe(step.as)) {
        diags.push(diag(CODES.asInvalid, path, 'as', `as 变量名 ${JSON.stringify(step.as)} 非法——只允许小写字母、数字、下划线`))
      }
      break
    }
    case 'match_templates': {
      if (!step.cases.length || step.cases.length > 64) diags.push(diag('yaml.match_templates.cases', path, 'cases', '必须有 1..64 个模板分支'))
      validateCell(step.threshold, path, 'threshold', declaredVars, ctx, diags)
      if (!isRefCell(step.threshold) && (typeof step.threshold.lit !== 'number' || !Number.isFinite(step.threshold.lit) || step.threshold.lit < 0 || step.threshold.lit > 1)) {
        diags.push(diag('yaml.match_templates.threshold', path, 'threshold', '匹配阈值必须为 0..1 数字或引用'))
      }
      step.cases.forEach((c, i) => {
        const field = `cases[${i}].template`
        validateCell(c.template, path, field, declaredVars, ctx, diags)
        if (!isRefCell(c.template)) {
          if (typeof c.template.lit !== 'string' || !c.template.lit.trim()) diags.push(diag('yaml.match_templates.template', path, field, '请选择模板'))
          else if (ctx.resolveTemplate && !ctx.resolveTemplate(c.template.lit)) diags.push(diag('yaml.resource.tmpl_not_found', path, field, `模板 ${c.template.lit} 在当前 Package 不存在`))
        }
        if (c.as !== null && !isIdentifierSafe(c.as)) diags.push(diag(CODES.asInvalid, path, `cases[${i}].as`, '匹配结果变量名无效'))
      })
      break
    }
    case 'if': {
      validateCell(step.cond, path, 'if', declaredVars, ctx, diags)
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
  // 容器中的字符串保留 YAML 表达式原文，递归检查引用，$$ 是字面量转义。
  function visit(value: unknown): void {
    if (typeof value === 'string' && value.startsWith('$') && !value.startsWith('$$') && value !== '$') {
      validateRef(value.slice(1), path, field, declaredVars, diags)
    } else if (Array.isArray(value)) {
      value.forEach(visit)
    } else if (value && typeof value === 'object') {
      Object.values(value).forEach(visit)
    }
  }
  if (cell.lit && typeof cell.lit === 'object') visit(cell.lit)
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
