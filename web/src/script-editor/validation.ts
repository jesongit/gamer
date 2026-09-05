/**
 * 结构化客户端校验（v3）：返回 {code, step_path, field, message} 列表，
 * 前端据 code + step_path + field 定位卡片与控件，message 仅展示。
 *
 * 职责边界：
 * - codec 解析期诊断（yaml.v3.syntax / version / field.missing 等依赖源码结构的错误）
 *   由 parse 产出，validateSource 合并两者；
 * - 本模块负责 Model 层可判定的约束：引用路径、字面量类型、范围、重复、流程上下文
 *   （return 仅函数、break 仅 loop 内）、call 命名空间与路径安全、defaults 范围。
 *   v3 表达式动态类型（$find 保存值 / $match 上下文），引用不再校验「已声明参数」，
 *   只校验路径语法；目标信息（call/args 绑定）由调用方经 resolver 接口传入。
 */

import {
  parseScript,
  parseFunctionLibrary,
  type ParseResult,
} from './codec'
import { joinStepPath, type Diagnostic, diag, CODES } from './diagnostics'
import {
  childStepLists,
  type Cell,
  type DefaultsModel,
  type FunctionLibraryModel,
  type ParamDecl,
  type Program,
  type Step,
} from './model'
import { checkCellLiteral, isRefPath, parseTimeMs } from './schema'

// ---------- 校验上下文与 resolver 接口 ----------

export interface ValidationResolvers {
  /** 解析 call 目标（script:<id>）的参数声明；未知目标返回 null。 */
  resolveCall?: (target: string) => { params: ParamDecl[] } | null
  /** 解析 call 目标（function:<文件短路径>/<函数名>）的参数声明；未知返回 null。 */
  resolveFunction?: (target: string) => { params: ParamDecl[] } | null
  /** 模板短名在当前分区是否存在。 */
  resolveTemplate?: (name: string) => boolean
}

export interface ValidationContext extends ValidationResolvers {
  /** 编辑上下文：script 中 return 非法（yaml.v3.flow.return_in_script）。 */
  context?: 'script' | 'function'
  /** 当前脚本资源路径（script: 目标自环检测用）。 */
  selfScript?: string
  /** 步骤嵌套深度上限（默认 32，与运行时 max_call_depth 一致）。 */
  maxDepth?: number
}

// ---------- 入口 ----------

export function validateScript(model: Program, ctx: ValidationContext = {}): Diagnostic[] {
  const diags: Diagnostic[] = []
  validateParamDecls(model.params, 'params', diags)
  validateDefaults(model.defaults, diags)
  validateStepList(model.steps, 'steps', model.params, ctx, diags, 1, 0)
  return diags
}

export function validateFunctionLibrary(
  model: FunctionLibraryModel,
  ctx: ValidationContext = {},
): Diagnostic[] {
  const diags: Diagnostic[] = []
  for (const fn of model.functions) {
    validateParamDecls(fn.params, `${fn.name}.params`, diags)
    validateStepList(fn.steps, `${fn.name}.steps`, fn.params, { ...ctx, context: 'function' }, diags, 1, 0)
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
    : parseFunctionLibrary(text, { file: ctx.file ?? '' })
  const modelDiags = kind === 'script'
    ? validateScript(result.model as Program, ctx)
    : validateFunctionLibrary(result.model as FunctionLibraryModel, ctx)
  return { result, diagnostics: [...result.diagnostics, ...modelDiags] }
}

// ---------- 参数声明 ----------

function validateParamDecls(decls: ParamDecl[], basePath: string, diags: Diagnostic[]): void {
  const seen = new Set<string>()
  decls.forEach((decl, i) => {
    const path = `${basePath}[${i}]`
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(decl.name)) {
      diags.push(diag(CODES.paramsNameInvalid, path, 'name', `变量名 ${decl.name} 不符合 [A-Za-z_][A-Za-z0-9_]*`))
    }
    if (seen.has(decl.name)) {
      diags.push(diag(CODES.paramsNameDuplicate, path, 'name', `变量名 ${decl.name} 在同一参数表内重复`))
    }
    seen.add(decl.name)
    if (decl.default !== null && decl.default !== undefined) {
      const err = checkCellLiteral(decl.type, decl.default)
      if (err) {
        diags.push(diag(CODES.paramsDefaultInvalid, path, 'default', `默认值不合法：${err.message}`))
      }
    }
  })
}

function validateDefaults(defaults: DefaultsModel | null, diags: Diagnostic[]): void {
  if (!defaults) return
  const t = defaults.vision_threshold
  if (t !== null && t !== undefined && (typeof t !== 'number' || !Number.isFinite(t) || t < 0 || t > 1)) {
    diags.push(diag(CODES.thresholdRange, 'defaults.vision.threshold', 'threshold', 'threshold 必须是 0~1 的数字'))
  }
  for (const [key, raw] of [['after_tap', defaults.after_tap], ['after_match', defaults.after_match], ['poll_interval', defaults.poll_interval]] as const) {
    if (raw === null || raw === undefined) continue
    if (typeof raw === 'number' ? !(Number.isFinite(raw) && raw >= 0) : parseTimeMs(raw) === null) {
      diags.push(diag(CODES.duration, `defaults.timing.${key}`, key, `timing 项须带单位（ms/s/m/h）且 >=0，收到 ${JSON.stringify(raw)}`))
    }
  }
}

// ---------- 步骤树校验 ----------

function validateStepList(
  steps: Step[],
  basePath: string,
  params: ParamDecl[],
  ctx: ValidationContext,
  diags: Diagnostic[],
  depth: number,
  loopDepth: number,
): void {
  const maxDepth = ctx.maxDepth ?? 32
  steps.forEach((step, i) => {
    const path = joinStepPath(basePath, i)
    if (depth > maxDepth) {
      diags.push(diag(CODES.flowNestingDepth, path, '', `步骤嵌套超过 ${maxDepth} 层`))
    }
    validateStep(step, path, params, ctx, diags, loopDepth)
    const childLoopDepth = loopDepth + (step.kind === 'loop' ? 1 : 0)
    for (const child of childStepLists(step)) {
      const childBase = child.key === 'candidates'
        ? `${path}.${child.key}[${child.index}].steps`
        : `${path}.${child.key}`
      validateStepList(child.list, childBase, params, ctx, diags, depth + 1, childLoopDepth)
    }
  })
}

function validateStep(
  step: Step,
  path: string,
  params: ParamDecl[],
  ctx: ValidationContext,
  diags: Diagnostic[],
  loopDepth: number,
): void {
  switch (step.kind) {
    case 'app_start':
    case 'app_stop':
      if (step.package) checkCell(step.package, 'expr', path, 'package', diags)
      return
    case 'break':
      if (loopDepth === 0) {
        diags.push(diag(CODES.flowBreakOutsideLoop, path, '', 'break 只能出现在 loop 子流程内'))
      }
      return
    case 'tap':
      checkCell(step.at, 'coord', path, 'at', diags)
      return
    case 'swipe':
      checkCell(step.from, 'coord', path, 'from', diags)
      checkCell(step.to, 'coord', path, 'to', diags)
      checkCell(step.duration, 'time', path, 'duration', diags)
      return
    case 'key':
      checkCell(step.key, 'key', path, 'key', diags)
      return
    case 'text':
      checkCell(step.value, 'text', path, 'value', diags)
      return
    case 'log':
      checkCell(step.message, 'text', path, 'message', diags)
      return
    case 'set':
      if (!step.name.trim()) {
        diags.push(diag(CODES.fieldString, path, 'name', 'set 的变量名必须是非空字符串'))
      }
      checkCell(step.value, 'expr', path, 'value', diags)
      return
    case 'if':
      checkCell(step.cond, 'expr', path, 'cond', diags)
      return
    case 'wait':
      checkCell(step.min, 'time', path, 'min', diags)
      if (step.max !== null) {
        checkCell(step.max, 'time', path, 'max', diags)
        checkWaitRange(step.min, step.max, path, diags)
      }
      return
    case 'loop':
      checkCell(step.times, 'number', path, 'times', diags)
      if (step.steps.length === 0) {
        diags.push(diag(CODES.flowLoopEmptySteps, path, 'steps', 'loop 子流程为空'))
      }
      return
    case 'return':
      if (ctx.context !== 'function') {
        diags.push(diag(CODES.flowReturnInScript, path, '', 'return 只能出现在函数库（functions/）的函数体内'))
        return
      }
      checkCell(step.value, 'expr', path, 'value', diags)
      return
    case 'throw':
      checkCell(step.message, 'expr', path, 'message', diags)
      return
    case 'find':
      checkCell(step.template, 'tmpl', path, 'template', diags, ctx)
      checkCell(step.timeout, 'time', path, 'timeout', diags)
      checkThreshold(step.threshold, path, diags)
      if (step.save !== null && !step.save.trim()) {
        diags.push(diag(CODES.fieldString, path, 'save', 'save 必须是非空字符串'))
      }
      if (step.verify) {
        checkCell(step.verify.template, 'tmpl', path, 'verify.template', diags, ctx)
        checkCell(step.verify.timeout, 'time', path, 'verify.timeout', diags)
      }
      return
    case 'match_first': {
      const seen = new Set<string>()
      step.candidates.forEach((cand, i) => {
        const key = cellKeyString(cand.template)
        if (key !== null) {
          if (seen.has(key)) {
            diags.push(diag(CODES.matchFirstType, path, 'candidates', `候选模板 ${key} 重复`))
          }
          seen.add(key)
        }
        checkCell(cand.template, 'tmpl', path, `candidates[${i}].template`, diags, ctx)
        checkThreshold(cand.threshold, path, diags)
      })
      return
    }
    case 'check':
      checkCell(step.template, 'tmpl', path, 'template', diags, ctx)
      checkCell(step.timeout, 'time', path, 'timeout', diags)
      checkThreshold(step.threshold, path, diags)
      return
    case 'call':
      validateCallTarget(step, path, ctx, diags)
      validateArgs(step.with, step.target, ctx, diags, path, params)
      return
    case 'invoke': {
      if (!step.capability.trim()) {
        diags.push(diag(CODES.fieldString, path, 'capability', 'invoke 的 capability 必须是非空字符串'))
      }
      validateArgs(step.with, step.capability, ctx, diags, path, params)
      return
    }
  }
}

function checkThreshold(v: number | null, path: string, diags: Diagnostic[]): void {
  if (v === null) return
  if (!Number.isFinite(v) || v < 0 || v > 1) {
    diags.push(diag(CODES.thresholdRange, path, 'threshold', 'threshold 必须是 0~1 的数字'))
  }
}

// ---------- 单元格 ----------

function checkCell(
  cell: Cell | null | undefined,
  type: string,
  path: string,
  field: string,
  diags: Diagnostic[],
  ctx?: ValidationContext,
): void {
  if (cell === null || cell === undefined) return
  if (typeof cell.ref === 'string') {
    if (!isRefPath(cell.ref)) {
      diags.push(diag(CODES.refPathInvalid, path, field, `引用 $${cell.ref} 不是合法属性路径（形如 $user.level、$list[0]）`))
    }
    return
  }
  const err = checkCellLiteral(type, cell.lit)
  if (err) {
    diags.push(diag(err.code, path, field, err.message))
    return
  }
  // 模板存在性（资源校验需要分区上下文，仅在调用方提供 resolver 时检查）。
  if (type === 'tmpl' && ctx?.resolveTemplate && typeof cell.lit === 'string') {
    if (!ctx.resolveTemplate(cell.lit)) {
      diags.push(diag(CODES.resourceTmplNotFound, path, field, `模板 ${cell.lit} 在当前应用分区不存在`))
    }
  }
}

function cellKeyString(cell: Cell): string | null {
  if (typeof cell.ref === 'string') return `$${cell.ref}`
  if (typeof cell.lit === 'string') return cell.lit
  return null
}

function checkWaitRange(min: Cell, max: Cell, path: string, diags: Diagnostic[]): void {
  if (typeof min.ref === 'string' || typeof max.ref === 'string') return
  const minMs = typeof min.lit === 'number' ? min.lit : typeof min.lit === 'string' ? parseTimeMs(min.lit) : null
  const maxMs = typeof max.lit === 'number' ? max.lit : typeof max.lit === 'string' ? parseTimeMs(max.lit) : null
  if (minMs !== null && maxMs !== null && minMs > maxMs) {
    diags.push(diag(CODES.waitRangeInvalid, path, 'max', `随机区间起点 ${String(min.lit)} 大于终点 ${String(max.lit)}`))
  }
}

// ---------- call 目标（契约 §2：命名空间强制） ----------

/** target 解析：script:<id> / function:<文件短路径>/<函数名>；裸 target → null。 */
export function splitCallTarget(target: string): { namespace: 'script' | 'function'; path: string } | null {
  const raw = String(target || '').trim()
  if (raw.startsWith('script:')) {
    const path = raw.slice('script:'.length)
    return path !== '' ? { namespace: 'script', path } : null
  }
  if (raw.startsWith('function:')) {
    const path = raw.slice('function:'.length)
    const idx = path.lastIndexOf('/')
    // 文件短路径可含目录（common/login/is_logged_in → 文件 common/login、函数 is_logged_in）
    if (idx <= 0 || idx === path.length - 1) return null
    return { namespace: 'function', path }
  }
  return null
}

/** 路径穿越 / 绝对路径 / 反斜杠（ADR-YAML-02 沿用 split_func_path 校验）。 */
function isUnsafePath(path: string): boolean {
  return path.includes('..') || path.startsWith('/') || path.includes('\\') || /^[A-Za-z]:/.test(path)
}

function validateCallTarget(
  step: Extract<Step, { kind: 'call' }>,
  path: string,
  ctx: ValidationContext,
  diags: Diagnostic[],
): void {
  const target = step.target
  const parsed = splitCallTarget(target)
  if (!parsed) {
    diags.push(diag(CODES.callNamespace, path, 'target', `call 目标 ${JSON.stringify(target)} 缺少命名空间前缀（script:<资源id> 或 function:<文件短路径>/<函数名>）`))
    return
  }
  if (isUnsafePath(parsed.path)) {
    diags.push(diag(CODES.callPathTraversal, path, 'target', `call 目标 ${target} 含路径穿越/绝对路径/反斜杠`))
    return
  }
  if (parsed.namespace === 'script') {
    if (ctx.selfScript && parsed.path === ctx.selfScript) {
      diags.push(diag(CODES.callSelfCycle, path, 'target', `call 目标 ${target} 是脚本自身（自引用成环）`))
      return
    }
    if (ctx.resolveCall && ctx.resolveCall(target) === null) {
      diags.push(diag(CODES.callScriptNotFound, path, 'target', `call 目标脚本 ${target} 不存在`))
    }
  } else if (ctx.resolveFunction && ctx.resolveFunction(target) === null) {
    diags.push(diag(CODES.callFunctionNotFound, path, 'target', `call 目标函数 ${target} 不存在`))
  }
}

/** with 绑定：键须存在于目标声明；类型按声明校验；必填缺失报码（resolver 未提供时跳过）。 */
function validateArgs(
  withArgs: Record<string, Cell>,
  target: string,
  ctx: ValidationContext,
  diags: Diagnostic[],
  path: string,
  params: ParamDecl[],
): void {
  const parsed = splitCallTarget(target)
  const decls = parsed?.namespace === 'script'
    ? ctx.resolveCall?.(target)?.params
    : parsed?.namespace === 'function'
      ? ctx.resolveFunction?.(target)?.params
      : undefined
  if (!decls) return // 目标信息由调用方提供；未提供则跳过绑定检查（本层只留接口）
  const declMap = new Map(decls.map((p) => [p.name, p]))
  for (const [name, cell] of Object.entries(withArgs)) {
    const decl = declMap.get(name)
    if (!decl) {
      diags.push(diag(CODES.callArgsUnknown, path, 'with', `with 键 ${name} 不是目标 ${target} 的参数`))
      continue
    }
    if (typeof cell.ref === 'string') continue // 引用动态类型，交运行时判
    const err = checkCellLiteral(decl.type, cell.lit)
    if (err) {
      diags.push(diag(CODES.callArgsTypeMismatch, path, 'with', `实参 ${name} 类型不符：${err.message}`))
    }
  }
  void params
  for (const decl of decls) {
    if (decl.default === null && !(decl.name in withArgs)) {
      diags.push(diag(CODES.callArgsMissingRequired, path, 'with', `必填参数 ${decl.name} 未出现在 with 中`))
    }
  }
}
