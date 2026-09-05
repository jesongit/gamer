/**
 * call 步骤目标候选与参数解析的宿主注入契约（v3 命名空间 target）。
 *
 * StepCard 自身不拉数据（组件纯受控、可独立挂载测试）：宿主页面（Console/ScriptRunner）
 * 持有分区脚本/函数库清单与解析缓存，经 provide(SE_TARGET_OPTIONS) 注入；
 * 未注入时 StepCard 回退自由文本输入框。
 *
 * target 语法（契约 §2 / ADR-YAML-02）：`script:<资源id>` / `function:<文件短路径>/<函数名>`。
 */
import type { InjectionKey } from 'vue'
import type { ParamDecl } from './model'
import { splitCallTarget } from './validation'

/** 候选项：target 即 call 书写形态（含命名空间前缀）。 */
export interface SeTargetOption {
  /** 完整 target 串，如 'script:daily/login'、'function:common/login'。 */
  target: string
  /** 下拉展示名（缺省用 target）。 */
  label?: string
  /** 分组展示用命名空间（宿主可省略，组件按 target 前缀推断）。 */
  group?: 'script' | 'function'
}

export interface SeTargetOptions {
  /** 当前分区 call 候选（脚本 + 函数；宿主负责排除脚本自身自引用）。 */
  targets: SeTargetOption[]
  /**
   * 解析目标参数声明（async，宿主内部缓存）；未知/加载失败返回 null。
   * StepCard 在目标切换后用它重生成 with（默认值预填）。
   */
  resolveParams(target: string): Promise<ParamDecl[] | null>
  /** 同步缓存命中形态（未缓存返回 null）：已有实参的类型回显，不触发加载。 */
  resolveParamsSync?(target: string): ParamDecl[] | null
}

export const SE_TARGET_OPTIONS: InjectionKey<SeTargetOptions> = Symbol('seTargetOptions')

/**
 * `function:<文件短路径>/<函数名>` → [文件短路径, 函数名]；非 function: target 返回 null。
 * 文件短路径按最后一个 `/` 分割（ADR-YAML-02，同 v2 split_func_path 语法）。
 */
export function splitFunctionTarget(target: string): [string, string] | null {
  const parsed = splitCallTarget(target)
  if (!parsed || parsed.namespace !== 'function') return null
  const idx = parsed.path.lastIndexOf('/')
  if (idx <= 0 || idx === parsed.path.length - 1) return null
  return [parsed.path.slice(0, idx), parsed.path.slice(idx + 1)]
}
