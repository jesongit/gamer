/**
 * 卡片层共享元数据与定位辅助（YAML V1）。
 *
 * - KIND_META：4 类步骤（call/if/repeat/return）的中文名 + 单字图标；
 * - stepSummary：卡片收起态自然语言摘要（call 摘要 = 函数名 + 实参）；
 * - breadcrumbForContainer / basePathOfContainer：容器路径 → 面包屑节点 / step_path 字符串基；
 * - parseStepPath / locateDiagnostic：诊断 step_path（如 run[0].then[1]、login.run[2]）
 *   → 命令路径 → 目标卡片 uuid 与祖先链（ErrorSummary 点击定位用）。
 */

import type { Path } from '../commands'
import { resolveStep } from '../commands'
import type { Diagnostic } from '../diagnostics'
import { isRefCell, type Cell, type Step } from '../model'
import { containerLabel, type BreadcrumbNode } from '../selection'

// ---------- 动作元数据 ----------

export interface KindMeta {
  kind: Step['kind']
  /** 中文名（添加面板同源文案）。 */
  label: string
  /** 单字图标（字体安全的中文单字，不依赖图标字体）。 */
  icon: string
  /** 一句话动作语义（展开态提示）。 */
  hint: string
}

export const KIND_META: Record<Step['kind'], KindMeta> = {
  call: { kind: 'call', label: '函数调用', icon: '调', hint: '调用一个函数（原生插件函数或当前 Package 函数），as 接收返回值' },
  if: { kind: 'if', label: '条件分支', icon: '判', hint: 'false/null 为假、非空结果为真，走 then/else 分支' },
  repeat: { kind: 'repeat', label: '固定循环', icon: '循', hint: '按固定次数执行 do 循环体（受执行预算约束）' },
  return: { kind: 'return', label: '返回值', icon: '返', hint: '结束并返回一个值（脚本顶层返回即运行结果）' },
}

// ---------- 摘要 ----------

/** Cell 摘要：引用 → $路径；坐标 → x, y；其余原值。 */
export function cellShort(cell: Cell | null | undefined): string {
  if (!cell) return ''
  if (isRefCell(cell)) return `$${cell.ref}`
  if (Array.isArray(cell.lit)) return `${cell.lit[0]}, ${cell.lit[1]}`
  if (cell.lit === true) return 'true'
  if (cell.lit === false) return 'false'
  return String(cell.lit ?? '')
}

/** 卡片收起态摘要；空占位字段按新建未完成态显示基础文案。 */
export function stepSummary(step: Step): string {
  switch (step.kind) {
    case 'call': {
      const name = step.fn || '（未填函数）'
      if (step.args.kind === 'none') {
        return step.as ? `调用 ${name} → ${step.as}` : `调用 ${name}`
      }
      if (step.args.kind === 'value') {
        const v = cellShort(step.args.cell) || '?'
        return step.as ? `调用 ${name} ${v} → ${step.as}` : `调用 ${name} ${v}`
      }
      const pairs = Object.entries(step.args.entries)
        .map(([key, cell]) => `${key}: ${cellShort(cell) || '?'}`)
        .join(', ')
      const tail = step.as ? ` → ${step.as}` : ''
      return pairs ? `调用 ${name} ${pairs}${tail}` : `调用 ${name}${tail}`
    }
    case 'if': {
      const c = step.cond
      return `如果 ${isRefCell(c) ? `$${c.ref}` : String(c.lit ?? '?')}`
    }
    case 'repeat': return `重复 ${cellShort(step.times)} 次`
    case 'return': return `返回 ${cellShort(step.value) || '?'}`
  }
}

// ---------- 容器路径辅助 ----------

/** 容器路径嵌套深度（根容器 = 0；一层分支 = 1；用于内嵌/专注分界）。 */
export function containerNesting(containerPath: Path): number {
  const rootLen = containerPath[0] === 'functions' ? 3 : 1
  return Math.max(0, (containerPath.length - rootLen) / 2)
}

/** 容器路径 → step_path 字符串基（'run' / 'login.run' / 'run[0].then'）。 */
export function basePathOfContainer(containerPath: Path): string {
  let out: string
  let i: number
  if (containerPath[0] === 'functions') {
    out = `${String(containerPath[1])}.run`
    i = 3
  } else {
    out = 'run'
    i = 1
  }
  while (i < containerPath.length) {
    out += `[${String(containerPath[i])}]`
    out += `.${String(containerPath[i + 1])}`
    i += 2
  }
  return out
}

/** 容器路径 → 面包屑节点链（含根层；无效路径返回已收集部分 + 根兜底）。 */
export function breadcrumbForContainer(model: Parameters<typeof resolveStep>[0], containerPath: Path): BreadcrumbNode[] {
  const isFn = 'functions' in model
  const nodes: BreadcrumbNode[] = []
  if (isFn) {
    const name = containerPath[0] === 'functions' ? String(containerPath[1]) : ''
    nodes.push({ label: name || '(未命名函数)', containerPath: ['functions', name, 'run'], stepUuid: null })
  } else {
    nodes.push({ label: '主流程', containerPath: ['run'], stepUuid: null })
  }
  const rootLen = isFn ? 3 : 1
  let i = rootLen
  try {
    while (i < containerPath.length) {
      const step = resolveStep(model, containerPath.slice(0, i + 1))
      const key = String(containerPath[i + 1])
      nodes.push({
        label: containerLabel(step, key),
        containerPath: containerPath.slice(0, i + 2),
        stepUuid: null,
      })
      i += 2
    }
  } catch {
    // 路径失效（步骤被删/重命名）：返回已收集部分 + 根兜底
  }
  return nodes
}

// ---------- 诊断 step_path 解析与定位 ----------

/**
 * validation/服务端 step_path 字符串 → 命令路径。
 * 支持：run[0]、run[0].then[1]、login.run[2]；functions.<名>.run[N]；
 * params/vars/yaml 等非步骤路径返回 null。
 */
export function parseStepPath(stepPath: string): Path | null {
  if (!stepPath) return null
  const toks = stepPath.split('.').map((t) => {
    const m = /^(\w+)(?:\[(\d+)\])?$/.exec(t)
    return m ? { name: m[1], idx: m[2] === undefined ? null : Number(m[2]) } : null
  })
  if (toks.length === 0 || toks.some((t) => t === null)) return null
  const path: Path = []
  let i = 0
  const first = toks[0] as { name: string; idx: number | null }
  if (first.name === 'run') {
    path.push('run')
    if (first.idx !== null) path.push(first.idx)
    i = 1
  } else {
    // 函数库：<函数名>.run[N]（params/vars 等其余顶层不是步骤）
    const second = toks[1]
    if (first.idx !== null || !second || second.name !== 'run' || second.idx === null) return null
    path.push('functions', first.name, 'run', second.idx)
    i = 2
  }
  for (; i < toks.length; i++) {
    const t = toks[i] as { name: string; idx: number | null }
    if (t.idx === null) return null
    path.push(t.name, t.idx)
  }
  return path
}

export interface LocateResult {
  /** 目标步骤 uuid（卡片高亮/选中）。 */
  uuid: string
  /** 目标宿主容器路径（决定是否需要专注视图）。 */
  containerPath: Path
  /** 祖先步骤 uuid 链（逐层展开卡片用，不含目标自身）。 */
  ancestorUuids: string[]
}

/** 诊断 → 卡片定位信息；非步骤路径或路径失效返回 null。 */
export function locateDiagnostic(model: Parameters<typeof resolveStep>[0], diag: Diagnostic): LocateResult | null {
  const path = parseStepPath(diag.step_path)
  if (!path || typeof path[path.length - 1] !== 'number') return null
  try {
    const step = resolveStep(model, path)
    const rootLen = path[0] === 'functions' ? 3 : 1
    const ancestorUuids: string[] = []
    for (let end = rootLen + 1; end < path.length; end += 2) {
      try {
        ancestorUuids.push(resolveStep(model, path.slice(0, end)).uuid)
      } catch {
        // 祖先失效不影响目标定位
      }
    }
    return { uuid: step.uuid, containerPath: path.slice(0, -1), ancestorUuids }
  } catch {
    return null
  }
}
