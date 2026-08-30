/**
 * call/func 步骤目标候选与参数解析的宿主注入契约。
 *
 * StepCard 自身不拉数据（组件纯受控、可独立挂载测试）：宿主页面（ScriptEditor）
 * 持有分区脚本/函数库清单与解析缓存，经 provide(SE_TARGET_OPTIONS) 注入；
 * 未注入时 StepCard 回退自由文本输入框（旧行为）。
 */
import type { InjectionKey } from 'vue'
import type { ParamDecl } from './model'

/** call 候选：target 即 call 书写形态（分区相对文件名，如 sub_task.yaml）。 */
export interface CallScriptOption {
  target: string
  /** 下拉展示名（缺省用 target）。 */
  label?: string
}

/** func 候选：一个函数库文件 + 其顶层函数名清单（按书写顺序）。 */
export interface FuncFileOption {
  file: string
  functions: string[]
}

export interface SeTargetOptions {
  /** 当前分区 call 候选脚本（宿主负责排除脚本自身自引用）。 */
  callScripts: CallScriptOption[]
  /** 当前分区 func 候选函数库文件。 */
  funcFiles: FuncFileOption[]
  /**
   * 解析目标参数声明（async，宿主内部缓存）；未知/加载失败返回 null。
   * StepCard 在目标切换后用它重生成 args（默认值预填）。
   */
  resolveParams(kind: 'call' | 'func', target: string): Promise<ParamDecl[] | null>
  /** 同步缓存命中形态（未缓存返回 null）：已有实参的类型回显，不触发加载。 */
  resolveParamsSync?(kind: 'call' | 'func', target: string): ParamDecl[] | null
}

export const SE_TARGET_OPTIONS: InjectionKey<SeTargetOptions> = Symbol('seTargetOptions')
