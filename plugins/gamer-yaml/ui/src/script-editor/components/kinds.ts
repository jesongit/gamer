/**
 * 卡片层共享元数据与定位辅助（YAML V1）。
 *
 * - KIND_META：6 类步骤（call/if/repeat/return/match_templates/break）的中文名 + 单字图标；
 * - stepCaption / stepSummary：动作名称 + 关键参数，同供编辑卡片与运行摘要；
 * - breadcrumbForContainer / basePathOfContainer：容器路径 → 面包屑节点 / step_path 字符串基；
 * - parseStepPath / locateDiagnostic：诊断 step_path（如 run[0].then[1]、login.run[2]）
 *   → 命令路径 → 目标卡片 uuid 与祖先链（ErrorSummary 点击定位用）。
 */

import type { Path } from '../commands'
import { resolveStep } from '../commands'
import type { Diagnostic } from '../diagnostics'
import { isRefCell, type Cell, type Step } from '../model'
import { containerLabel, type BreadcrumbNode } from '../selection'
import { NATIVE_CALL_NAMES } from '../call-names'

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
  break: { kind: 'break', label: '跳出循环', icon: '断', hint: '退出最近一层 repeat，继续执行循环后面的步骤' },
  match_templates: { kind: 'match_templates', label: '模板分支', icon: '图', hint: '同一帧按顺序匹配，只执行首个命中的分支' },
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
  if (Array.isArray(cell.lit) && cell.lit.length === 2 && cell.lit.every(v => typeof v === 'number')) return `${cell.lit[0]}, ${cell.lit[1]}`
  if (cell.lit !== null && typeof cell.lit === 'object') return JSON.stringify(cell.lit)
  if (cell.lit === true) return 'true'
  if (cell.lit === false) return 'false'
  return String(cell.lit ?? '')
}

/** 保留 0/false/null 与引用；文本换行以转义形式展示，摘要不修改实参。 */
function valuePreview(cell: Cell | undefined, missing = '未填写', quoted = false): string {
  if (!cell || cell.missing) return missing
  if (isRefCell(cell)) return `$${cell.ref}`
  if (cell.lit === null) return 'null'
  if (typeof cell.lit === 'string') return quoted || !cell.lit || /[\r\n\t]/.test(cell.lit) ? JSON.stringify(cell.lit) : cell.lit
  return cellShort(cell)
}
function pointPreview(cell: Cell | undefined): string {
  if (!cell || cell.missing || isRefCell(cell)) return valuePreview(cell, '未设置坐标')
  const point = cell.lit as { x?: unknown; y?: unknown } | null
  if (Array.isArray(point) && point.length === 2) return `(${point.join(', ')})`
  if (point && typeof point === 'object' && 'x' in point && 'y' in point) return `(${point.x}, ${point.y})`
  return valuePreview(cell)
}

export function stepCaption(step: Step, defaultName?: string): { title: string; detail: string } {
  switch (step.kind) {
    case 'call': {
      const entries = step.args.kind === 'map' ? step.args.entries : {}
      const arg = (key: string) => step.args.kind === 'value' ? step.args.cell : entries[key]
      const title = (entries.name && cellShort(entries.name).trim())
        || (typeof defaultName === 'string' && defaultName.trim()) || NATIVE_CALL_NAMES[step.fn] || step.fn || '未选择函数'
      let detail = ''
      switch (step.fn) {
        case 'find': case 'wait_find': case 'tap_template': case 'wait_disappear':
          detail = valuePreview(arg('template'), '未选择模板'); break
        case 'tap': detail = pointPreview(arg('position')); break
        case 'swipe': detail = `${pointPreview(entries.from)} → ${pointPreview(entries.to)}`; break
        case 'sleep': {
          const cell = arg('duration')
          detail = cell && !cell.missing && !isRefCell(cell) && typeof cell.lit === 'number'
            ? `${cell.lit}ms` : valuePreview(cell, '未设置时长')
          break
        }
        case 'key': detail = valuePreview(arg('key'), '未选择按键')
          + (entries.action ? ` · ${valuePreview(entries.action)}` : ''); break
        case 'input_text': detail = valuePreview(arg('text'), '未填写文本', true); break
        case 'log': detail = valuePreview(arg('message'), '未填写内容', true); break
        case 'launch': case 'stop_app': detail = valuePreview(arg('package'), '设备配置的应用'); break
        case 'eq': case 'ne': case 'gt': case 'ge': case 'lt': case 'le':
          detail = `${valuePreview(entries.a)} 与 ${valuePreview(entries.b)}`; break
        default:
          detail = step.args.kind === 'value' ? valuePreview(step.args.cell)
            : Object.entries(entries).filter(([key]) => key !== 'name').map(([key, cell]) => `${key}=${valuePreview(cell)}`).join(' · ')
      }
      if (step.as) detail += `${detail ? ' ' : ''}→ ${step.as}`
      return { title, detail }
    }
    case 'match_templates': return { title: '模板分支', detail: step.cases.map(c => valuePreview(c.template, '未选择模板')).join(' → ') }
    case 'if': return { title: '如果', detail: valuePreview(step.cond, '未填写条件') }
    case 'repeat': return { title: '重复', detail: `${valuePreview(step.times, '未填写次数')} 次` }
    case 'break': return { title: '跳出循环', detail: '退出最近一层 repeat' }
    case 'return': return { title: '返回', detail: valuePreview(step.value, '未填写返回值') }
  }
}

export function stepSummary(step: Step, defaultName?: string): string {
  const { title, detail } = stepCaption(step, defaultName)
  return detail ? `${title} · ${detail}` : title
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
  const segments = stepPath.split('.')
  if (segments[0] === 'functions' && segments[2]?.startsWith('run[')) segments.shift()
  const toks = segments.map((t) => {
    const m = /^([\p{L}\p{N}_]+)(?:\[(\d+)\])?$/u.exec(t)
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
    if (t.name === 'cases' && toks[i + 1]?.name === 'do' && toks[i + 1]?.idx !== null) {
      path.push(`cases[${t.idx}].do`, toks[++i]!.idx!)
    } else path.push(t.name, t.idx)
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
