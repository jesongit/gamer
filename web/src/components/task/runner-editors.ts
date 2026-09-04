/**
 * RunnerEditorContribution 契约（P11.1 §6.7，轻量 V1）。
 *
 * TaskBoard 是通用任务表单（ADR-12：Task = 任意 ScheduleProvider + 任意 Runner），
 * 只认 runner_id / entrypoint / payload 三个抽象字段；某类执行器「执行目标长什么样、
 * 参数怎么编辑」由按 runner_id 注册的编辑器贡献提供：
 * - gamer.yaml（内置）：执行目标 = 分区脚本（ScriptPicker），参数 = 脚本 params 声明（ParamsForm）；
 * - 未来扩展 runner：安装对应前端贡献后自动获得同等编辑体验。
 *
 * 职责边界：
 * - TaskBoard 不 import ScriptPicker/ParamsForm，不读 store.scriptsData/templatesData；
 *   脚本/模板等业务资源由贡献内部自行获取（store 或 API 均可，TaskBoard 不感知）。
 * - 未注册贡献的 runner（含后端已注册但前端无编辑器的）：TaskBoard 渲染占位提示 +
 *   runner JSON 只读展示，任务仍可保存（entrypoint/payload 原样保留）。
 *
 * payload 编辑器组件契约（V1，按约定而非运行时校验）：
 * - props：`entrypoint: string`、`payload: Record<string, unknown>`、`ctx: RunnerEditorContext`；
 * - emits：`update:payload`（稀疏更新整体 payload 对象）；
 * - expose（可选）：`validate(): RunnerEditorIssue[]`（空数组 = 通过），保存前由 TaskBoard 调用。
 */
import type { Component } from 'vue'

/** TaskBoard 提供给贡献的编辑上下文（表单当前选择；贡献只读，不得反向写入）。 */
export interface RunnerEditorContext {
  /** 任务应用分区（android_package）；独立挂载未锁定分区时为 null */
  androidPackage: string | null
  /** 表单当前设备 id（可能为空串） */
  deviceId: string
}

/** 执行目标候选项：value = entrypoint 原文（保存进 task.runner.entrypoint）。 */
export interface RunnerEntrypointOption {
  value: string
  label: string
}

/** 贡献校验问题（保存前阻断用）；name 为可选定位字段（如参数名）。 */
export interface RunnerEditorIssue {
  name?: string
  message: string
}

/** 由 entrypoint/payload 推导任务 app 包名（gamer.yaml：分区前缀约定）。 */
export interface RunnerAppPackages {
  android_package: string
  content_package: string | null
}

/**
 * 按 runner_id 注册的编辑器贡献。`payloadEditor` 必填；执行目标编辑二选一：
 * 提供 `entrypointEditor` 组件（v-model 绑 entrypoint）时优先使用，否则 TaskBoard
 * 用通用下拉渲染 `entrypoints(ctx)` 候选（同步数组或 Promise 加载器均可）。
 */
export interface RunnerEditorContribution {
  /** 对应后端 runner 注册 id（如 'gamer.yaml'） */
  runnerId: string
  /** 下拉显示名（无贡献的 runner 显示 runner_id 原文） */
  title: string
  /** 执行目标候选（枚举口径；entrypointEditor 存在时供降级/测试使用） */
  entrypoints?: (
    ctx: RunnerEditorContext,
  ) => RunnerEntrypointOption[] | Promise<RunnerEntrypointOption[]>
  /** 自定义执行目标选择器（可选）：v-model 绑 entrypoint 字符串 */
  entrypointEditor?: Component
  /** entrypointEditor 附加 props（如 ScriptPicker 的 package/lockPackage） */
  entrypointEditorProps?: (ctx: RunnerEditorContext) => Record<string, unknown>
  /** payload 编辑器：props = {entrypoint, payload, ctx}，v-model:payload，可选 expose validate() */
  payloadEditor: Component
  /** 保存时推导 app 包名（缺省回退既有任务 app，新建回退空串） */
  resolveAppPackages?: (entrypoint: string, payload: Record<string, unknown>, ctx: RunnerEditorContext) => RunnerAppPackages
}

const registry = new Map<string, RunnerEditorContribution>()

/** 注册贡献（同 runnerId 重复注册以最后一次为准）；返回反注册函数。 */
export function registerRunnerEditor(contribution: RunnerEditorContribution): () => void {
  registry.set(contribution.runnerId, contribution)
  return () => unregisterRunnerEditor(contribution.runnerId)
}

export function unregisterRunnerEditor(runnerId: string): void {
  registry.delete(runnerId)
}

export function getRunnerEditor(runnerId: string): RunnerEditorContribution | undefined {
  return registry.get(runnerId)
}

/** 已注册贡献列表（按 runnerId 稳定排序）。 */
export function listRunnerEditors(): RunnerEditorContribution[] {
  return [...registry.values()].sort((a, b) => a.runnerId.localeCompare(b.runnerId))
}

/** 仅测试用：清空注册表（生产代码勿调）。 */
export function resetRunnerEditorsForTests(): void {
  registry.clear()
}
