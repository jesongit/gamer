/**
 * 选择与插入（V1）：
 * - 步骤 uuid ↔ 路径互查（选中、错误定位到卡片）；
 * - 插入锚点（当前容器 + 位置）：选中卡片之后、未选中时当前流程末尾；
 * - 面包屑：主流程 / 命中 fn / 如果为真 / 循环体 …；
 * - 顶层步骤 → 引擎 start_index 映射（UUID 不进 YAML，运行仍按顶层序号）。
 *
 * 路径语法（与 commands.ts 的 resolveStepList/resolveStep 一致）：
 * - 步骤路径以数字结尾：['run', 0, 'then', 1]、['functions', 'login', 'run', 2]；
 * - 容器路径以键结尾：['run']、['run', 0, 'then']、['functions', 'login', 'run']。
 */

import type { EditorModel, Path } from './commands'
import { resolveStepList } from './commands'
import { childStepLists, type Cell, type Program, type Step } from './model'

// ---------- 路径互查 ----------

export interface StepLocation {
  step: Step
  /** 步骤路径（数字结尾）。 */
  path: Path
  /** 宿主列表（引用，就地可改）。 */
  list: Step[]
  /** 在宿主列表中的下标。 */
  index: number
  /** 宿主容器路径（= path 去掉末位下标）。 */
  containerPath: Path
  /** validation 字符串形态的 step_path（如 run[0].then[1]、login.run[2]）。 */
  stepPath: string
}

/** 按 uuid 查步骤位置（先序遍历；函数库逐函数查找）。 */
export function findStepLocation(model: EditorModel, uuid: string): StepLocation | null {
  const roots: { list: Step[]; containerPath: Path; base: string }[] =
    'functions' in model
      ? model.functions.map((fn) => ({
          list: fn.run,
          containerPath: ['functions', fn.name, 'run'] as Path,
          base: `${fn.name}.run`,
        }))
      : [{ list: model.run, containerPath: ['run'] as Path, base: 'run' }]
  for (const root of roots) {
    const found = searchList(root.list, root.containerPath, root.base, uuid)
    if (found) return found
  }
  return null
}

function searchList(list: Step[], containerPath: Path, base: string, uuid: string): StepLocation | null {
  for (let i = 0; i < list.length; i++) {
    const step = list[i]
    if (step.uuid === uuid) {
      return { step, path: [...containerPath, i], list, index: i, containerPath, stepPath: `${base}[${i}]` }
    }
    for (const child of childStepLists(step)) {
      const childContainer = childContainerPath(containerPath, i, child.key)
      const childBase = `${base}[${i}].${child.key}`
      const found = searchList(child.list, childContainer, childBase, uuid)
      if (found) return found
    }
  }
  return null
}

/** 步骤子容器的路径延续段。 */
export function childContainerPath(containerPath: Path, stepIndex: number, key: string): Path {
  return [...containerPath, stepIndex, key]
}

/** 按 uuid 取步骤（找不到返回 null）。 */
export function findStep(model: EditorModel, uuid: string): Step | null {
  return findStepLocation(model, uuid)?.step ?? null
}

/** 按 uuid 取 validation 字符串路径（错误列表定位用）。 */
export function stepPathOf(model: EditorModel, uuid: string): string | null {
  return findStepLocation(model, uuid)?.stepPath ?? null
}

// ---------- 插入锚点 ----------

/** 插入锚点：容器路径 + 容器内下标。 */
export interface InsertionAnchor {
  /** 容器路径（resolveStepList 的合法输入）。 */
  containerPath: Path
  index: number
}

/**
 * 默认插入锚点（plan §8.4：添加步骤默认插入到选中卡片之后，未选中时当前流程末尾）。
 * currentContainer 为面包屑对应的当前容器，缺省 = 根容器。
 */
export function defaultAnchor(
  model: EditorModel,
  selectedUuid: string | null,
  currentContainer?: Path,
): InsertionAnchor {
  if (selectedUuid) {
    const loc = findStepLocation(model, selectedUuid)
    if (loc) return { containerPath: loc.containerPath, index: loc.index + 1 }
  }
  const containerPath = currentContainer ?? rootContainerPath(model)
  const list = resolveStepList(model, containerPath)
  return { containerPath, index: list.length }
}

/** 根容器路径：脚本 = ['run']；函数库 = 第一个函数（无函数时为空名占位）。 */
export function rootContainerPath(model: EditorModel): Path {
  if ('functions' in model) {
    const fn = model.functions[0]
    return ['functions', fn ? fn.name : '', 'run']
  }
  return ['run']
}

// ---------- 面包屑 ----------

export interface BreadcrumbNode {
  /** 展示名：主流程 / 函数名 / 命中 test1 / 如果为真 … */
  label: string
  /** 该层容器路径（点击面包屑切换当前容器）。 */
  containerPath: Path
  /** 该层容器里最后一个步骤的 uuid（根层为 null；用于高亮/插入位置提示）。 */
  stepUuid: string | null
}

/** 步骤的祖先容器链（含根层）。返回 [] = uuid 不存在。 */
export function breadcrumb(model: EditorModel, uuid: string): BreadcrumbNode[] {
  const roots: { list: Step[]; containerPath: Path; label: string }[] =
    'functions' in model
      ? model.functions.map((fn) => ({
          list: fn.run,
          containerPath: ['functions', fn.name, 'run'] as Path,
          label: fn.name || '(未命名函数)',
        }))
      : [{ list: model.run, containerPath: ['run'] as Path, label: '主流程' }]
  for (const root of roots) {
    const trail = searchTrail(root.list, root.containerPath, null, '', uuid)
    if (trail === null) continue
    // trail[0] 是目标步骤自身所在容器的入口……链条每项代表「一个步骤 + 它所在的容器」；
    // 面包屑节点 = 根容器 + 从第 2 项起每项的容器（由其父步骤 + 键命名）。
    const nodes: BreadcrumbNode[] = [
      { label: root.label, containerPath: root.containerPath, stepUuid: null },
    ]
    for (let i = 1; i < trail.length; i++) {
      const entry = trail[i]
      nodes.push({
        label: containerLabel(entry.parent, entry.containerKey, root.label),
        containerPath: entry.containerPath,
        stepUuid: entry.lastUuid,
      })
    }
    return nodes
  }
  return []
}

interface TrailEntry {
  parent: Step | null
  containerKey: string
  containerPath: Path
  /** 容器当前末步 uuid（面包屑节点提示用）。 */
  lastUuid: string | null
}

/** 递归收集「从根容器到 uuid 所在容器」的链。 */
function searchTrail(
  list: Step[],
  containerPath: Path,
  parent: Step | null,
  containerKey: string,
  uuid: string,
): TrailEntry[] | null {
  for (let i = 0; i < list.length; i++) {
    const step = list[i]
    if (step.uuid === uuid) {
      return [{
        parent,
        containerKey,
        containerPath,
        lastUuid: step.uuid,
      }]
    }
    for (const child of childStepLists(step)) {
      const childContainer = childContainerPath(containerPath, i, child.key)
      const found = searchTrail(child.list, childContainer, step, child.key, uuid)
      if (found !== null) {
        return [{
          parent,
          containerKey,
          containerPath,
          lastUuid: step.uuid,
        }, ...found]
      }
    }
  }
  return null
}

/** 容器展示名（面包屑示例：主流程 / 如果为真 / 循环体）。 */
export function containerLabel(parent: Step | null, containerKey: string, rootFallback = ''): string {
  if (parent === null) return rootFallback || '主流程'
  switch (parent.kind) {
    case 'if':
      return containerKey === 'then' ? '如果为真' : '如果为假'
    case 'repeat':
      return '循环体'
    default:
      return containerKey
  }
}

/** Cell 展示形态（编辑器显示 $路径引用，底层存属性路径，plan §9）。 */
export function cellDisplay(cell: Cell | null | undefined): string {
  if (!cell) return ''
  if (typeof cell.ref === 'string') return `$${cell.ref}`
  return Array.isArray(cell.lit) ? `[${cell.lit.join(', ')}]` : String(cell.lit ?? '')
}

// ---------- start_index 映射（plan §8.2：顶层步骤 uuid → 引擎顶层序号） ----------

export interface StartIndexEntry {
  uuid: string
  /** 引擎 start_index（顶层 0 基序号）。 */
  index: number
}

/** 脚本主流程顶层步骤 → start_index；函数库 = 每个函数体的顶层步骤。 */
export function startIndexMap(model: EditorModel): StartIndexEntry[] {
  if ('functions' in model) {
    const entries: StartIndexEntry[] = []
    for (const fn of model.functions) {
      fn.run.forEach((step: Step, i: number) => entries.push({ uuid: step.uuid, index: i }))
    }
    return entries
  }
  return (model as Program).run.map((step: Step, i: number) => ({ uuid: step.uuid, index: i }))
}

/** 单个 uuid 的 start_index；不在顶层返回 null（嵌套步骤首版不支持直接启动，plan §3）。 */
export function startIndexOf(model: EditorModel, uuid: string): number | null {
  const entry = startIndexMap(model).find((e) => e.uuid === uuid)
  return entry ? entry.index : null
}

/** 供外部把路径转成展示串（调试/日志用）。 */
export function pathToString(path: Path): string {
  let out = ''
  for (const seg of path) {
    if (typeof seg === 'number') out += `[${seg}]`
    else out += out === '' ? seg : `.${seg}`
  }
  return out
}
