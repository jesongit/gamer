/**
 * 结构化诊断（契约 §5.1）：code + message + resource + step_path + field 五元组。
 * 前端据 {code, step_path, field} 定位卡片与控件，message 仅用于展示，禁止解析文案定位。
 */

export interface Diagnostic {
  code: string
  message: string
  /** 出错资源 ID；前端测试中以用例 ID 代替，可为空。 */
  resource?: string
  /** 定位路径，见契约 §5.2；顶层/整文件错误为 ''。 */
  step_path: string
  /** 出错字段名；顶层错误 = 顶层键名，语法错误 = 'yaml'。 */
  field: string
}

export function diag(code: string, stepPath: string, field: string, message: string): Diagnostic {
  return { code, step_path: stepPath, field, message }
}

/**
 * step_path 字符串工具（契约 §5.2）：
 * - 脚本：params[0] / config / steps[0] / steps[1].then[0] / steps[2].candidates[1].steps[0]
 * - 函数库：<函数名>.steps[0] / <函数名>.params[1]
 */
export function joinStepPath(base: string, seg: string | number): string {
  if (base === '') {
    return typeof seg === 'number' ? `[${seg}]` : seg
  }
  return typeof seg === 'number' ? `${base}[${seg}]` : `${base}.${seg}`
}

/** 错误码常量（契约 §5.3 命名空间清单 + 阶段 0 fixture 覆盖码）。 */
export const CODES = {
  // 资源
  resourceTmplNotFound: 'resource.tmpl.not_found',
  resourceTmplAmbiguous: 'resource.tmpl.ambiguous',
  resourceScriptNotFound: 'resource.script.not_found',
  resourceFuncNotFound: 'resource.func.not_found',
  // 参数
  paramDeclQuoteStyle: 'param.decl.quote_style',
  paramDeclFormat: 'param.decl.format',
  paramDeclNameInvalid: 'param.decl.name_invalid',
  paramDeclNameDuplicate: 'param.decl.name_duplicate',
  paramDefaultEmpty: 'param.default.empty',
  paramDefaultInvalid: 'param.default.invalid',
  paramRefUnknown: 'param.ref.unknown',
  paramRefTypeMismatch: 'param.ref.type_mismatch',
  paramArgsUnknown: 'param.args.unknown',
  paramArgsMissingRequired: 'param.args.missing_required',
  paramArgsTypeMismatch: 'param.args.type_mismatch',
  // 步骤
  stepUnknownAction: 'step.unknown_action',
  stepMultiAction: 'step.multi_action',
  stepFieldMissing: 'step.field.missing',
  stepFieldTypeMismatch: 'step.field.type_mismatch',
  stepFieldUnknown: 'step.field.unknown',
  stepMatchCandidateDuplicate: 'step.match.candidate_duplicate',
  stepMatchElseInCandidates: 'step.match.else_in_candidates',
  stepMatchCandidatesType: 'step.match.candidates_type',
  stepIfNonBoolCond: 'step.if.non_bool_cond',
  stepColorDuplicate: 'step.color.duplicate',
  stepColorFormat: 'step.color.format',
  stepCoordRange: 'step.coord.range',
  stepTimeFormat: 'step.time.format',
  stepWaitRangeInvalid: 'step.wait.range_invalid',
  stepLoopEmptySteps: 'step.loop.empty_steps',
  stepReturnInScript: 'step.return.in_script',
  stepNestingDepth: 'step.nesting.depth',
  stepListType: 'step.list_type',
  // 引用
  refCallPathTraversal: 'ref.call.path_traversal',
  refCallSelfCycle: 'ref.call.self_cycle',
  refFuncPathTraversal: 'ref.func.path_traversal',
  refFuncSyntax: 'ref.func.syntax',
  refFuncMissingArgs: 'ref.func.missing_args',
  // 顶层 / 文件（阶段 0 覆盖码）
  scriptTopLevelLegacyFormat: 'script.top_level.legacy_format',
  scriptTopLevelUnknownKey: 'script.top_level.unknown_key',
  scriptRootType: 'script.root_type',
  funcRecordUnknownKey: 'func.record_unknown_key',
  funcRecordType: 'func.record_type',
  yamlSyntaxError: 'yaml.syntax_error',
} as const
