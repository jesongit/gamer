/**
 * 函数调用候选与参数解析的宿主注入契约（V1）。
 *
 * StepCard 自身不拉数据（组件纯受控、可独立挂载测试）：宿主页面（Console/ScriptRunner）
 * 持有函数目录（原生插件函数 + 当前 Package 函数），经 provide(SE_TARGET_OPTIONS) 注入；
 * 未注入时 StepCard 回退自由文本输入框。
 *
 * V1 调用即函数名（无 script:/function: 前缀——脚本是入口不是可调用目标，
 * Package 函数是唯一复用单元，计划 Phase 3）。
 */
import type { InjectionKey } from 'vue'
import type { ParamDecl } from './model'

/** 候选项：name 即函数名。 */
export interface SeTargetOption {
  /** 函数名（调用书写形态）。 */
  target: string
  /** 下拉展示名（缺省用 target）。 */
  label?: string
  /** 来源分组（原生插件函数 / 当前 Package 函数）。 */
  group?: 'plugin' | 'package'
  /** 说明（目录 description）。 */
  hint?: string
}

export interface SeTargetOptions {
  /** 当前可用函数候选（宿主负责组合原生目录 + 当前 Package 函数 + 编辑中文件自身）。 */
  targets: SeTargetOption[]
  /**
   * 解析函数参数声明（async，宿主内部缓存）；未知/加载失败返回 null。
   * StepCard 在函数切换后用它重生成实参（默认值预填）。
   */
  resolveParams(target: string): Promise<ParamDecl[] | null>
  /** 同步缓存命中形态（未缓存返回 null）：已有实参的类型回显，不触发加载。 */
  resolveParamsSync?(target: string): ParamDecl[] | null
}

export const SE_TARGET_OPTIONS: InjectionKey<SeTargetOptions> = Symbol('seTargetOptions')
