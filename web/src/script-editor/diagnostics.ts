/**
 * 结构化诊断（五元组）：code + message + resource + step_path + field。
 * 错误码命名与服务端（gamer_yaml/syntax.rs，yaml.*）对齐；前端据
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
 * step_path 字符串工具（与服务端降线路径同语法）：
 * - 脚本：run[0] / run[1].then[0] / run[0].do[2]
 * - 函数库：functions.<名>.run[0] / functions.<名>.params.<名>
 */
export function joinStepPath(base: string, seg: string | number): string {
  if (base === '') {
    return typeof seg === 'number' ? `[${seg}]` : seg
  }
  return typeof seg === 'number' ? `${base}[${seg}]` : `${base}.${seg}`
}

/** 错误码常量（yaml.* 命名，与宿主侧 syntax.rs / error.rs 对齐 + 编辑器侧扩展）。 */
export const CODES = {
  // 文件 / 顶层
  yamlSyntax: 'yaml.syntax_error',
  versionRemoved: 'yaml.version.removed',
  topLevelUnknownKey: 'yaml.top.unknown',
  rootType: 'yaml.top.shape',
  runMissing: 'yaml.run.missing',
  // params
  paramsType: 'yaml.param.decl',
  paramsInvalid: 'yaml.param.decl',
  paramsUnknownKey: 'yaml.param.decl',
  paramsNameInvalid: 'yaml.name.invalid',
  paramsNameDuplicate: 'yaml.param.decl',
  paramsDefaultInvalid: 'yaml.param.default.invalid',
  // vars
  varsType: 'yaml.vars.shape',
  varsConflict: 'yaml.vars.conflict',
  // steps 结构
  stepsType: 'yaml.step.list',
  stepShape: 'yaml.step.shape',
  stepMissing: 'yaml.step.missing',
  stepMulti: 'yaml.step.multi',
  stepUnknown: 'yaml.step.unknown',
  fieldUnknown: 'yaml.field.unknown',
  fieldType: 'yaml.field.type',
  fieldString: 'yaml.field.string',
  // 命名
  nameInvalid: 'yaml.name.invalid',
  asInvalid: 'yaml.as.invalid',
  exprInvalid: 'yaml.expr.invalid',
  // 控制流
  ifThenMissing: 'yaml.if.then',
  repeatTimesInvalid: 'yaml.repeat.times',
  repeatDoMissing: 'yaml.repeat.do',
  // 函数库
  functionsMissing: 'yaml.functions.missing',
  functionsShape: 'yaml.functions.shape',
  fnDuplicate: 'yaml.fn.duplicate',
  fnReserved: 'yaml.name.invalid',
  // 编辑器侧扩展（注册表对照）
  fnNotFound: 'yaml.fn.not_found',
  varUndefined: 'yaml.var.undefined',
} as const
