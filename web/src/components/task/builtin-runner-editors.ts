/**
 * 内置 RunnerEditorContribution 注册（P11.1 §6.7 首个实现：gamer.yaml）。
 *
 * TaskBoard 挂载时调用 registerBuiltinRunnerEditors()；未来扩展 runner 的编辑器
 * 走同一契约（由扩展装载方注册），本文件只收 Core 自带的 gamer.yaml。
 */
import type { RunnerAppPackages, RunnerEditorContext } from './runner-editors'
import { registerRunnerEditor } from './runner-editors'
import ScriptPicker from '../ScriptPicker.vue'
import GamerYamlPayloadEditor from './GamerYamlPayloadEditor.vue'
import { ensureGamerYamlResources, gamerYamlEntrypointOptions } from './gamer-yaml-resources'

export const GAMER_YAML_RUNNER_ID = 'gamer.yaml'

/** gamer.yaml 的 app 包名约定：entrypoint = "<android_package>/<脚本文件名>"。 */
export function resolveGamerYamlAppPackages(entrypoint: string): RunnerAppPackages {
  const pkg = String(entrypoint || '').split('/')[0] || ''
  return { android_package: pkg, content_package: pkg || null }
}

/**
 * 注册内置贡献；返回反注册函数（测试用）。
 * - entrypointEditor 复用 ScriptPicker（纯 store 消费）；Console 挂载时经 ctx.androidPackage
 *   锁定当前分区，独立挂载保留双下拉自选分区行为；
 * - entrypoints() 为异步枚举口径（保障 store 就绪后给候选），供通用下拉/测试使用。
 */
export function registerGamerYamlRunnerEditor(): () => void {
  return registerRunnerEditor({
    runnerId: GAMER_YAML_RUNNER_ID,
    title: 'YAML 脚本',
    entrypoints: async (ctx: RunnerEditorContext) => {
      await ensureGamerYamlResources()
      return gamerYamlEntrypointOptions(ctx)
    },
    entrypointEditor: ScriptPicker,
    entrypointEditorProps: (ctx: RunnerEditorContext) => ({
      package: ctx.androidPackage ?? '',
      lockPackage: ctx.androidPackage !== null && ctx.androidPackage !== undefined,
      // 纯受控：脚本列表经贡献懒加载（晚于选择器挂载到达），不得自动改写外部已选目标
      autoPick: false,
    }),
    payloadEditor: GamerYamlPayloadEditor,
    resolveAppPackages: (entrypoint: string) => resolveGamerYamlAppPackages(entrypoint),
  })
}

/** TaskBoard 启动时注册全部内置贡献；返回统一反注册（测试用）。 */
export function registerBuiltinRunnerEditors(): () => void {
  const unregisters = [registerGamerYamlRunnerEditor()]
  return () => unregisters.forEach((u) => u())
}
