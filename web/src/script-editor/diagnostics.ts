/**
 * 结构化诊断（五元组）：code + message + resource + step_path + field。
 * 错误码命名与 v3 服务端（yaml_vnext.rs，yaml.v3.*）对齐；前端据
 * {code, step_path, field} 定位卡片与控件，message 仅用于展示，禁止解析文案定位。
 */

export interface Diagnostic {
  code: string
  message: string
  /** 出错资源 ID；可为空。 */
  resource?: string
  /** 定位路径；顶层/整文件错误为 ''。 */
  step_path: string
  /** 出错字段名；顶层错误 = 顶层键名，语法错误 = 'yaml'。 */
  field: string
}

export function diag(code: string, stepPath: string, field: string, message: string): Diagnostic {
  return { code, step_path: stepPath, field, message }
}

/**
 * step_path 字符串工具（surface step 稳定路径，契约 §6 语法一致）：
 * - 脚本：params[0] / defaults / steps[0] / steps[1].then[0] / steps[2].candidates[1].steps[0]
 * - 函数库：<函数名>.steps[0] / <函数名>.params[1]
 */
export function joinStepPath(base: string, seg: string | number): string {
  if (base === '') {
    return typeof seg === 'number' ? `[${seg}]` : seg
  }
  return typeof seg === 'number' ? `${base}[${seg}]` : `${base}.${seg}`
}

/** 错误码常量（yaml.v3.* 命名，与 yaml_vnext.rs 对齐 + 编辑器侧扩展）。 */
export const CODES = {
  // 文件 / 顶层
  yamlSyntax: 'yaml.v3.syntax',
  version: 'yaml.v3.version',
  versionMissing: 'yaml.v3.version.missing',
  topLevelUnknownKey: 'yaml.v3.top_level.unknown_key',
  stepsMissing: 'yaml.v3.steps.missing',
  rootType: 'yaml.v3.root_type',
  // params
  paramsType: 'yaml.v3.params.type',
  paramsInvalid: 'yaml.v3.params.invalid',
  paramsUnknownKey: 'yaml.v3.params.unknown_key',
  paramsNameInvalid: 'yaml.v3.params.name_invalid',
  paramsNameDuplicate: 'yaml.v3.params.name_duplicate',
  paramsDefaultInvalid: 'yaml.v3.params.default',
  // steps 结构
  stepsType: 'yaml.v3.steps.type',
  stepShape: 'yaml.v3.step.shape',
  stepUnknown: 'yaml.v3.step.unknown',
  fieldMissing: 'yaml.v3.field.missing',
  fieldUnknown: 'yaml.v3.field.unknown',
  fieldType: 'yaml.v3.field.type',
  fieldString: 'yaml.v3.field.string',
  duration: 'yaml.v3.duration',
  number: 'yaml.v3.number',
  // 视觉步骤
  matchFirstType: 'yaml.v3.match_first.type',
  // call / invoke（契约 §2：命名空间强制）
  callNamespace: 'yaml.v3.call.namespace',
  callPathTraversal: 'yaml.v3.call.path_traversal',
  callSelfCycle: 'yaml.v3.call.self_cycle',
  callScriptNotFound: 'yaml.v3.call.script_not_found',
  callFunctionNotFound: 'yaml.v3.call.function_not_found',
  callArgsUnknown: 'yaml.v3.call.args_unknown',
  callArgsTypeMismatch: 'yaml.v3.call.args_type_mismatch',
  callArgsMissingRequired: 'yaml.v3.call.args_missing_required',
  // 流程上下文（编辑器校验扩展）
  flowBreakOutsideLoop: 'yaml.v3.flow.break_outside_loop',
  flowReturnInScript: 'yaml.v3.flow.return_in_script',
  flowLoopEmptySteps: 'yaml.v3.flow.loop_empty_steps',
  flowNestingDepth: 'yaml.v3.flow.nesting_depth',
  coordRange: 'yaml.v3.coord.range',
  thresholdRange: 'yaml.v3.threshold.range',
  waitRangeInvalid: 'yaml.v3.wait.range_invalid',
  // defaults
  defaultsUnknownKey: 'yaml.v3.defaults.unknown_key',
  defaultsType: 'yaml.v3.defaults.type',
  // 引用
  refPathInvalid: 'yaml.v3.ref.path_invalid',
  // 资源
  resourceTmplNotFound: 'yaml.v3.resource.tmpl_not_found',
} as const
