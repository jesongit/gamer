/**
 * 结构化客户端校验（契约 §5）：返回 {code, step_path, field, message} 列表，
 * 前端据 code + step_path + field 定位卡片与控件，message 仅展示。
 *
 * 职责边界：
 * - codec 解析期诊断（yaml.syntax_error / 顶层键 / param.decl.* / else_in_candidates 等
 *   依赖源码样式或结构的错误）由 parse 产出，validateSource 合并两者；
 * - 本模块负责 Model 层可判定的约束：引用、类型、范围、重复、上下文（return 仅函数）、
 *   路径安全与目标绑定。目标信息（call/func 的参数表）由调用方经 resolver 接口传入，
 *   本层只定义接口；未提供 resolver 时跳过资源存在性与 args 绑定检查。
 */

import {
  parseScript,
  parseFunctionLibrary,
  type ParseResult,
} from './codec'
import { joinStepPath, type Diagnostic, diag, CODES } from './diagnostics'
import { childStepLists, LOG_LEVELS, type Cell, type ParamDecl, type ScriptModel, type Step } from './model'
import { checkCellLiteral, parseTimeMs } from './schema'

// ---------- 校验上下文与 resolver 接口 ----------

export interface ValidationResolvers {
  /** 解析 call 目标脚本的参数声明；未知目标返回 null。 */
  resolveCall?: (target: string) => { params: ParamDecl[] } | null
  /** 解析 func 目标（`<文件短路径>/<函数名>`）的参数声明；未知返回 null。 */
  resolveFunction?: (target: string) => { params: ParamDecl[] } | null
  /** 模板短名在当前分区是否存在。 */
  resolveTemplate?: (name: string) => boolean
}

export interface ValidationContext extends ValidationResolvers {
  /** 编辑上下文：script 中 return 非法（step.return.in_script）。 */
  context?: 'script' | 'function'
  /** 当前脚本文件 ID（如 i06_call_cycle.yaml），用于 call 自环检测。 */
  selfFile?: string
  /** 步骤嵌套深度上限（默认 32，与运行时函数嵌套上限一致）。 */
  maxDepth?: number
}

// ---------- 入口 ----------

export function validateScript(model: ScriptModel, ctx: ValidationContext = {}): Diagnostic[] {
  const diags: Diagnostic[] = []
  const paramTypes = paramTypeMap(model.params)
  validateParamDecls(model.params, 'params', diags)
  validateConfig(model.config, diags)
  validateStepList(model.steps, 'steps', paramTypes, ctx, diags, 1)
  return diags
}

export function validateFunctionLibrary(
  model: { file: string; functions: { name: string; params: ParamDecl[]; steps: Step[] }[] },
  ctx: ValidationContext = {},
): Diagnostic[] {
  const diags: Diagnostic[] = []
  for (const fn of model.functions) {
    const paramTypes = paramTypeMap(fn.params)
    validateParamDecls(fn.params, `${fn.name}.params`, diags)
    validateStepList(fn.steps, `${fn.name}.steps`, paramTypes, { ...ctx, context: 'function' }, diags, 1)
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
    ? validateScript(result.model as ScriptModel, ctx)
    : validateFunctionLibrary(result.model as { file: string; functions: { name: string; params: ParamDecl[]; steps: Step[] }[] }, ctx)
  return { result, diagnostics: [...result.diagnostics, ...modelDiags] }
}

// ---------- 参数声明 ----------

function paramTypeMap(params: ParamDecl[]): Map<string, ParamDecl> {
  return new Map(params.map((p) => [p.name, p]))
}

function validateParamDecls(decls: ParamDecl[], basePath: string, diags: Diagnostic[]): void {
  const seen = new Set<string>()
  decls.forEach((decl, i) => {
    const path = `${basePath}[${i}]`
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(decl.name)) {
      diags.push(diag(CODES.paramDeclNameInvalid, path, 'name', `变量名 ${decl.name} 不符合 [A-Za-z_][A-Za-z0-9_]*`))
    }
    if (seen.has(decl.name)) {
      diags.push(diag(CODES.paramDeclNameDuplicate, path, 'name', `变量名 ${decl.name} 在同一参数表内重复`))
    }
    seen.add(decl.name)
  })
}

function validateConfig(config: ScriptModel['config'], diags: Diagnostic[]): void {
  if (!config) return
  if (typeof config.interval !== 'string' || parseTimeMs(config.interval) === null) {
    diags.push(diag(CODES.stepTimeFormat, 'config', 'interval', `interval 须带单位（ms/s/m/min/h/d）且 >0，收到 ${JSON.stringify(config.interval)}`))
  }
  if (typeof config.threshold !== 'number' || !Number.isFinite(config.threshold) || config.threshold < 0 || config.threshold > 1) {
    diags.push(diag(CODES.stepFieldTypeMismatch, 'config', 'threshold', 'threshold 必须是 0~1 的数字'))
  }
  if (!LOG_LEVELS.includes(config.log_level)) {
    diags.push(diag(CODES.stepFieldTypeMismatch, 'config', 'log_level', 'log_level 必须是 debug/info/warn/error'))
  }
}

// ---------- 步骤树校验 ----------

function validateStepList(
  steps: Step[],
  basePath: string,
  paramTypes: Map<string, ParamDecl>,
  ctx: ValidationContext,
  diags: Diagnostic[],
  depth: number,
): void {
  const maxDepth = ctx.maxDepth ?? 32
  steps.forEach((step, i) => {
    const path = joinStepPath(basePath, i)
    if (depth > maxDepth) {
      diags.push(diag(CODES.stepNestingDepth, path, '', `步骤嵌套超过 ${maxDepth} 层`))
    }
    validateStep(step, path, paramTypes, ctx, diags)
    for (const child of childStepLists(step)) {
      const childBase = child.key === 'candidates'
        ? `${path}.${child.key}[${child.index}].steps`
        : `${path}.${child.key}`
      validateStepList(child.list, childBase, paramTypes, ctx, diags, depth + 1)
    }
  })
}

function validateStep(
  step: Step,
  path: string,
  paramTypes: Map<string, ParamDecl>,
  ctx: ValidationContext,
  diags: Diagnostic[],
): void {
  switch (step.kind) {
    case 'str_app':
    case 'cls_app':
    case 'throw':
      return
    case 'tap':
      checkCell(step.at, 'coord', path, 'at', paramTypes, diags)
      return
    case 'swipe':
      checkCell(step.from, 'coord', path, 'from', paramTypes, diags)
      checkCell(step.to, 'coord', path, 'to', paramTypes, diags)
      checkCell(step.time, 'time', path, 'time', paramTypes, diags)
      return
    case 'key':
      checkCell(step.key, 'key', path, 'key', paramTypes, diags)
      return
    case 'text':
      checkCell(step.value, 'text', path, 'value', paramTypes, diags)
      return
    case 'log':
      checkCell(step.message, 'text', path, 'message', paramTypes, diags)
      return
    case 'return':
      if (ctx.context !== 'function') {
        diags.push(diag(CODES.stepReturnInScript, path, '', 'return 只能出现在函数库（func/）的函数体内'))
        return
      }
      checkCell(step.value, 'bool', path, 'value', paramTypes, diags)
      return
    case 'wait': {
      checkCell(step.duration, 'time', path, 'duration', paramTypes, diags)
      if (step.duration_max !== null) {
        checkCell(step.duration_max, 'time', path, 'duration_max', paramTypes, diags)
        checkWaitRange(step.duration, step.duration_max, path, diags)
      }
      return
    }
    case 'if':
      checkCondCell(step.cond, path, paramTypes, diags)
      return
    case 'find':
      checkCell(step.template, 'tmpl', path, 'template', paramTypes, diags, ctx)
      step.block.forEach((b, i) => checkCell(b, 'tmpl', path, `block[${i}]`, paramTypes, diags, ctx))
      checkCell(step.timeout, 'time', path, 'timeout', paramTypes, diags)
      return
    case 'match': {
      const seenTemplates = new Set<string>()
      step.candidates.forEach((cand, i) => {
        const key = cellKeyString(cand.template)
        if (key !== null) {
          if (seenTemplates.has(key)) {
            diags.push(diag(CODES.stepMatchCandidateDuplicate, path, 'candidates', `候选模板 ${key} 重复`))
          }
          seenTemplates.add(key)
        }
        checkCell(cand.template, 'tmpl', path, `candidates[${i}].template`, paramTypes, diags, ctx)
      })
      checkCell(step.timeout, 'time', path, 'timeout', paramTypes, diags)
      return
    }
    case 'color': {
      checkCell(step.at, 'coord', path, 'at', paramTypes, diags)
      const seenColors = new Set<string>()
      step.expect.forEach((exp, i) => {
        const key = cellKeyString(exp.color)
        if (key !== null) {
          if (seenColors.has(key)) {
            diags.push(diag(CODES.stepColorDuplicate, path, 'expect', `颜色候选 ${key} 重复`))
          }
          seenColors.add(key)
        }
        checkCell(exp.color, 'color', path, `expect[${i}].color`, paramTypes, diags)
      })
      return
    }
    case 'loop':
      if (step.steps.length === 0) {
        diags.push(diag(CODES.stepLoopEmptySteps, path, 'steps', 'loop 子流程为空'))
      }
      return
    case 'call':
      validateCallTarget(step.target, path, ctx, diags)
      validateArgs(step.args, step.target, 'call', ctx, diags, path, paramTypes)
      return
    case 'func':
      validateFuncTarget(step.target, path, ctx, diags)
      validateArgs(step.args, step.target, 'func', ctx, diags, path, paramTypes)
      return
  }
}

// ---------- 单元格 ----------

function checkCell(
  cell: Cell | null | undefined,
  type: Parameters<typeof checkCellLiteral>[0],
  path: string,
  field: string,
  paramTypes: Map<string, ParamDecl>,
  diags: Diagnostic[],
  ctx?: ValidationContext,
): void {
  if (cell === null || cell === undefined) return
  if (typeof cell.ref === 'string') {
    const decl = paramTypes.get(cell.ref)
    if (!decl) {
      diags.push(diag(CODES.paramRefUnknown, path, field, `$${cell.ref} 引用了未声明的参数`))
      return
    }
    if (decl.type !== type) {
      diags.push(diag(CODES.paramRefTypeMismatch, path, field, `参数 $${cell.ref} 是 ${decl.type} 型，不能用于 ${type} 型字段 ${field}`))
      return
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

/** if 条件：非布尔字面量 → 专用错误码 step.if.non_bool_cond。 */
function checkCondCell(
  cell: Cell,
  path: string,
  paramTypes: Map<string, ParamDecl>,
  diags: Diagnostic[],
): void {
  if (typeof cell.ref === 'string') {
    const decl = paramTypes.get(cell.ref)
    if (!decl) {
      diags.push(diag(CODES.paramRefUnknown, path, 'cond', `$${cell.ref} 引用了未声明的参数`))
    } else if (decl.type !== 'bool') {
      diags.push(diag(CODES.paramRefTypeMismatch, path, 'cond', `参数 $${cell.ref} 是 ${decl.type} 型，if 条件需要 bool 型`))
    }
    return
  }
  if (typeof cell.lit !== 'boolean') {
    diags.push(diag(CODES.stepIfNonBoolCond, path, 'cond', 'if 条件必须是布尔值（true/false 或布尔参数引用）'))
  }
}

/** 候选键用于重复检测的字符串形态（ref → $名，lit → 原值）。 */
function cellKeyString(cell: Cell): string | null {
  if (typeof cell.ref === 'string') return `$${cell.ref}`
  if (typeof cell.lit === 'string') return cell.lit
  return null
}

function checkWaitRange(
  duration: Cell,
  durationMax: Cell,
  path: string,
  diags: Diagnostic[],
): void {
  if (typeof duration.ref === 'string' || typeof durationMax.ref === 'string') return
  const min = typeof duration.lit === 'string' ? parseTimeMs(duration.lit) : null
  const max = typeof durationMax.lit === 'string' ? parseTimeMs(durationMax.lit) : null
  if (min !== null && max !== null && min > max) {
    diags.push(diag(CODES.stepWaitRangeInvalid, path, 'duration_max', `随机区间起点 ${duration.lit} 大于终点 ${durationMax.lit}`))
  }
}

// ---------- 引用目标 ----------

/** 路径穿越 / 绝对路径 / 反斜杠（契约 ref.call.path_traversal / ref.func.path_traversal）。 */
function isUnsafePath(target: string): boolean {
  return target.includes('..') || target.startsWith('/') || target.includes('\\') || /^[A-Za-z]:/.test(target)
}

function validateCallTarget(target: string, path: string, ctx: ValidationContext, diags: Diagnostic[]): void {
  if (isUnsafePath(target)) {
    diags.push(diag(CODES.refCallPathTraversal, path, 'target', `call 目标 ${target} 含路径穿越/绝对路径/反斜杠`))
    return
  }
  if (ctx.selfFile && target === ctx.selfFile) {
    diags.push(diag(CODES.refCallSelfCycle, path, 'target', `call 目标 ${target} 是脚本自身（自引用成环）`))
    return
  }
  if (ctx.resolveCall && ctx.resolveCall(target) === null) {
    diags.push(diag(CODES.resourceScriptNotFound, path, 'target', `call 目标脚本 ${target} 不存在`))
  }
}

function validateFuncTarget(target: string, path: string, ctx: ValidationContext, diags: Diagnostic[]): void {
  if (isUnsafePath(target)) {
    diags.push(diag(CODES.refFuncPathTraversal, path, 'target', `func 目标 ${target} 含路径穿越/绝对路径/反斜杠`))
    return
  }
  const segments = target.split('/')
  if (segments.length < 2 || segments.some((s) => s === '')) {
    diags.push(diag(CODES.refFuncSyntax, path, 'target', `func 目标应为 <文件短路径>/<函数名>，收到 ${JSON.stringify(target)}`))
    return
  }
  if (ctx.resolveFunction && ctx.resolveFunction(target) === null) {
    diags.push(diag(CODES.resourceFuncNotFound, path, 'target', `函数 ${target} 不存在`))
  }
}

/** args 绑定：键须存在于目标声明；类型按声明校验；必填缺失按 call/func 分别报码。 */
function validateArgs(
  args: Record<string, Cell>,
  target: string,
  kind: 'call' | 'func',
  ctx: ValidationContext,
  diags: Diagnostic[],
  path: string,
  paramTypes: Map<string, ParamDecl>,
): void {
  const resolve = kind === 'call' ? ctx.resolveCall : ctx.resolveFunction
  const decls = resolve?.(target)?.params
  if (!decls) return // 目标信息由调用方提供；未提供则跳过绑定检查（本层只留接口）
  const declMap = paramTypeMap(decls)
  for (const [name, cell] of Object.entries(args)) {
    const decl = declMap.get(name)
    if (!decl) {
      diags.push(diag(CODES.paramArgsUnknown, path, 'args', `args 键 ${name} 不是目标 ${target} 的参数`))
      continue
    }
    if (typeof cell.ref === 'string') {
      const source = paramTypes.get(cell.ref)
      if (source && source.type !== decl.type) {
        diags.push(diag(CODES.paramArgsTypeMismatch, path, 'args', `实参 ${name} 需要 ${decl.type} 型，$${cell.ref} 是 ${source.type} 型`))
      }
      continue
    }
    const err = checkCellLiteral(decl.type, cell.lit)
    if (err) {
      diags.push(diag(CODES.paramArgsTypeMismatch, path, 'args', `实参 ${name} 类型不符：${err.message}`))
    }
  }
  for (const decl of decls) {
    if (decl.default === null && !(decl.name in args)) {
      const code = kind === 'call' ? CODES.paramArgsMissingRequired : CODES.refFuncMissingArgs
      diags.push(diag(code, path, 'args', `必填参数 ${decl.name} 未出现在 args 中`))
    }
  }
}
