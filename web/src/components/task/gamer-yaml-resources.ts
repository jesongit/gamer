/**
 * gamer.yaml runner 的内部资源保障与口径工具（RunnerEditorContribution 实现私有）。
 *
 * TaskBoard 不感知本模块：脚本/模板等业务资源由贡献自行获取——分区脚本与模板
 * 短名候选仍走全局 store（scriptsData/templatesData，与步骤画布共享同一份数据），
 * 为空时在此拉取填充；脚本内容（参数声明的唯一权威来源）经 api.getScript 获取。
 */
import { api } from '../../api'
import { scriptsData, templatesData } from '../../store'
import type { RunnerEditorContext, RunnerEntrypointOption } from './runner-editors'

/** 去掉模板文件名上的区域/保色后缀，保持与步骤画布的短名输入口径一致（自 TaskBoard 迁入）。 */
export function templateShortName(name: string): string {
  return String(name || '')
    .replace(/#1(\.(png|jpe?g))$/i, '$1')
    .replace(/#[^#./\\]+(\.(png|jpe?g))$/i, '$1')
}

let scriptsInflight: Promise<void> | null = null
async function ensureScripts(packageId: string): Promise<void> {
  if (scriptsData.value.length) return
  if (!scriptsInflight) {
    scriptsInflight = api
      .listScripts(packageId)
      .then((list) => { scriptsData.value = Array.isArray(list) ? list : [] })
      .catch(() => { /* 拉取失败：ScriptPicker 显示「（无脚本）」，任务保存被必填校验阻断 */ })
      .finally(() => { scriptsInflight = null })
  }
  await scriptsInflight
}

let templatesInflight: Promise<void> | null = null
async function ensureTemplates(packageId: string): Promise<void> {
  if (templatesData.value.length) return
  if (!templatesInflight) {
    templatesInflight = api
      .listTemplates(packageId)
      .then((list) => { templatesData.value = Array.isArray(list) ? list : [] })
      .catch(() => { /* 拉取失败：tmpl 参数无候选（可手输短名），不阻断保存 */ })
      .finally(() => { templatesInflight = null })
  }
  await templatesInflight
}

/** 幂等保障脚本 + 模板候选进 store（payload 编辑器挂载与 entrypoints 枚举共用；
 * 数据上下文 = 调用方传入的当前 Package id，plan §39）。 */
export function ensureGamerYamlResources(packageId: string | null | undefined): Promise<void> {
  const pkg = String(packageId || '')
  return Promise.all([ensureScripts(pkg), ensureTemplates(pkg)]).then(() => undefined)
}

/** 当前脚本分区（store 列表命中优先，回退 entrypoint 分区前缀约定）。 */
export function scriptPackageOf(entrypoint: string): string {
  const s = scriptsData.value.find((x) => x.id === entrypoint)
  return s?.package || String(entrypoint || '').split('/')[0] || ''
}

/** 脚本条目的 Package id（资源 id 首段；Package 上下文，§39 命名口径）。 */
export function scriptPackageIdOf(entrypoint: string): string {
  return scriptPackageOf(entrypoint)
}

/** 执行目标候选 = 当前 store 快照（调用前应 ensureGamerYamlResources）。 */
export function gamerYamlEntrypointOptions(_ctx: RunnerEditorContext): RunnerEntrypointOption[] {
  return scriptsData.value.map((s) => ({ value: s.id, label: String(s.name || s.id) }))
}
