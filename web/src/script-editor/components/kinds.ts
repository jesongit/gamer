/**
 * 卡片层共享元数据与定位辅助（v3）。
 *
 * - KIND_META：19 类动作的中文名 + 单字图标（卡片左侧固定列）；
 * - stepSummary：卡片收起态自然语言摘要；
 * - breadcrumbForContainer / basePathOfContainer：容器路径 → 面包屑节点 / step_path 字符串基；
 * - parseStepPath / locateDiagnostic：诊断 step_path（如 steps[0].candidates[1].steps[0].then[0]）
 *   → 命令路径 → 目标卡片 uuid 与祖先链（ErrorSummary 点击定位用）。
 */

import type { Path } from '../commands'
import { resolveStep } from '../commands'
import type { Diagnostic } from '../diagnostics'
import { isRefCell, type Cell, type Step, type StepKind } from '../model'
import { containerLabel, type BreadcrumbNode } from '../selection'

// ---------- 动作元数据 ----------

export interface KindMeta {
  kind: StepKind
  /** 中文动作名（添加面板 PANEL_GROUPS 同源文案）。 */
  label: string
  /** 单字图标（字体安全的中文单字，不依赖图标字体）。 */
  icon: string
  /** 一句话动作语义（展开态提示）。 */
  hint: string
}

export const KIND_META: Record<StepKind, KindMeta> = {
  app_start: { kind: 'app_start', label: '启动应用', icon: '启', hint: '启动应用（缺省为设备当前配置的应用）' },
  app_stop: { kind: 'app_stop', label: '关闭应用', icon: '关', hint: '关闭应用（缺省为设备当前配置的应用）' },
  tap: { kind: 'tap', label: '点击坐标', icon: '点', hint: '点击屏幕相对坐标（0~1）或 $引用（如 $reward.center）' },
  swipe: { kind: 'swipe', label: '滑动', icon: '滑', hint: '从起点滑到终点，时长必须带单位' },
  key: { kind: 'key', label: '按键', icon: '键', hint: '发送单个按键（服务端按键枚举；action 可选 down/up/press）' },
  text: { kind: 'text', label: '输入文本', icon: '文', hint: '向设备输入文本' },
  wait: { kind: 'wait', label: '等待', icon: '等', hint: '固定等待或 {min, max} 随机区间，时间须带单位' },
  log: { kind: 'log', label: '记录日志', icon: '志', hint: '写一条运行日志（level 可选 debug/info/warn/error）' },
  set: { kind: 'set', label: '设置变量', icon: '值', hint: '把表达式取值存入变量，供后续 $变量名 引用' },
  if: { kind: 'if', label: '条件分支', icon: '判', hint: '按表达式真值走 then/else 分支（$flag、布尔字面量等）' },
  loop: { kind: 'loop', label: '循环', icon: '循', hint: '按次数执行子流程；省略 times 表示无限循环（体内需有 break）' },
  break: { kind: 'break', label: '跳出循环', icon: '跳', hint: '跳出最近一层循环，仅能放在 loop 子流程内' },
  call: { kind: 'call', label: '调用脚本/函数', icon: '调', hint: '调用可调用资源：script:<资源id> 或 function:<文件短路径>/<函数名>，with 传参、save 存返回值' },
  invoke: { kind: 'invoke', label: '调用能力', icon: '能', hint: '调用宿主能力（如 vision.match / input.tap），with 传参、save 存结果' },
  throw: { kind: 'throw', label: '抛出错误', icon: '抛', hint: '结束整个运行（含调用链），原因可为表达式' },
  return: { kind: 'return', label: '返回值', icon: '返', hint: '仅函数体内合法；返回任意值供 call 的 save 接收' },
  find: { kind: 'find', label: '等待模板', icon: '找', hint: '轮询等待模板出现；命中执行 then（配合 $match.center 或 save 引用），超时走 else；verify 可二次验证' },
  match_first: { kind: 'match_first', label: '多模板匹配', icon: '匹', hint: '按序检测候选模板，首个命中候选执行自己的 steps；全未命中走 else' },
  check: { kind: 'check', label: '检查模板', icon: '检', hint: '在 timeout 内轮询匹配模板做界面断言（不点击），超时抛错结束运行' },
}

// ---------- 摘要 ----------

/** Cell 摘要：引用 → $路径；坐标 → x, y；其余原值。 */
export function cellShort(cell: Cell | null | undefined, type: string): string {
  if (!cell) return ''
  if (isRefCell(cell)) return `$${cell.ref}`
  if (type === 'coord' && Array.isArray(cell.lit)) return `${cell.lit[0]}, ${cell.lit[1]}`
  if (cell.lit === true) return 'true'
  if (cell.lit === false) return 'false'
  return String(cell.lit ?? '')
}

/** 卡片收起态摘要；空占位字段按新建未完成态显示基础文案。 */
export function stepSummary(step: Step): string {
  switch (step.kind) {
    case 'app_start': return step.package ? `启动应用 ${cellShort(step.package, 'expr')}` : '启动当前应用'
    case 'app_stop': return step.package ? `关闭应用 ${cellShort(step.package, 'expr')}` : '关闭当前应用'
    case 'tap': return `点击坐标 ${cellShort(step.at, 'coord') || '?, ?'}`
    case 'swipe': return `从 ${cellShort(step.from, 'coord') || '?'} 滑到 ${cellShort(step.to, 'coord') || '?'} · ${cellShort(step.duration, 'time') || '?'}`
    case 'key': return `按键 ${cellShort(step.key, 'key') || '?'}${step.action && step.action !== 'press' ? `（${step.action}）` : ''}`
    case 'text': {
      const v = cellShort(step.value, 'text')
      return v ? `输入文本 ${v}` : '输入文本'
    }
    case 'log': {
      const v = cellShort(step.message, 'text')
      return v ? `记录日志 ${v}` : '记录日志'
    }
    case 'wait': {
      const base = cellShort(step.min, 'time') || '?'
      return step.max ? `随机等待 ${base}～${cellShort(step.max, 'time')}` : `等待 ${base}`
    }
    case 'set': return `设置 ${step.name || '（未命名）'} = ${cellShort(step.value, 'expr') || '?'}`
    case 'if': {
      const c = step.cond
      return `如果 ${isRefCell(c) ? `$${c.ref}` : String(c.lit ?? '?')}`
    }
    case 'loop': return step.times === null ? '无限循环' : `循环 ${cellShort(step.times, 'number')} 次`
    case 'break': return '跳出循环'
    case 'call': return `调用 ${step.target || '（未填目标）'}`
    case 'invoke': return `调用能力 ${step.capability || '（未填能力）'}`
    case 'throw': return `终止：${cellShort(step.message, 'expr') || '（无原因）'}`
    case 'return': return `返回 ${cellShort(step.value, 'expr') || '?'}`
    case 'find': {
      const t = cellShort(step.template, 'tmpl')
      return `等待 ${t || '（未选模板）'} 并执行命中后步骤`
    }
    case 'match_first': return `按顺序匹配 ${step.candidates.length} 个模板（首个命中获胜）`
    case 'check': {
      const t = cellShort(step.template, 'tmpl')
      return `检查 ${t || '（未选模板）'}`
    }
  }
}

// ---------- 容器路径辅助 ----------

/** 容器路径嵌套深度（根容器 = 0；一层分支 = 1；用于内嵌/专注分界）。 */
export function containerNesting(containerPath: Path): number {
  const rootLen = containerPath[0] === 'functions' ? 3 : 1
  return Math.max(0, (containerPath.length - rootLen) / 2)
}

/** 容器路径 → step_path 字符串基（'steps' / 'login.steps' / 'steps[0].then' / 'steps[0].candidates[1].steps'）。 */
export function basePathOfContainer(containerPath: Path): string {
  let out: string
  let i: number
  if (containerPath[0] === 'functions') {
    out = `${String(containerPath[1])}.steps`
    i = 3
  } else {
    out = 'steps'
    i = 1
  }
  while (i < containerPath.length) {
    out += `[${String(containerPath[i])}]`
    if (containerPath[i + 1] === 'candidates') {
      out += `.candidates[${String(containerPath[i + 2])}].steps`
      i += 3
    } else {
      out += `.${String(containerPath[i + 1])}`
      i += 2
    }
  }
  return out
}

/** 容器路径 → 面包屑节点链（含根层；无效路径返回已收集部分 + 根兜底）。 */
export function breadcrumbForContainer(model: Parameters<typeof resolveStep>[0], containerPath: Path): BreadcrumbNode[] {
  const isFn = 'functions' in model
  const nodes: BreadcrumbNode[] = []
  if (isFn) {
    const name = containerPath[0] === 'functions' ? String(containerPath[1]) : ''
    nodes.push({ label: name || '(未命名函数)', containerPath: ['functions', name, 'steps'], stepUuid: null })
  } else {
    nodes.push({ label: '主流程', containerPath: ['steps'], stepUuid: null })
  }
  const rootLen = isFn ? 3 : 1
  let i = rootLen
  try {
    while (i < containerPath.length) {
      const step = resolveStep(model, containerPath.slice(0, i + 1))
      const key = containerPath[i + 1]
      if (key === 'candidates') {
        const candIdx = Number(containerPath[i + 2])
        nodes.push({
          label: containerLabel(step, 'candidates', candIdx),
          containerPath: containerPath.slice(0, i + 3),
          stepUuid: null,
        })
        i += 3
      } else {
        nodes.push({
          label: containerLabel(step, String(key), -1),
          containerPath: containerPath.slice(0, i + 2),
          stepUuid: null,
        })
        i += 2
      }
    }
  } catch {
    // 路径失效（步骤被删/重命名）：返回已收集部分 + 根兜底
  }
  return nodes
}

// ---------- 诊断 step_path 解析与定位 ----------

/**
 * validation/服务端 step_path 字符串 → 命令路径。
 * 支持：steps[0]、steps[0].then[1]、steps[0].candidates[1].steps[0]、login.steps[2]、login.params[1]。
 * params[N] / defaults / yaml 等非步骤路径返回 null；candidates 结尾返回容器路径（步骤定位需再 resolve）。
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
  if (first.name === 'steps') {
    path.push('steps')
    if (first.idx !== null) path.push(first.idx)
    i = 1
  } else {
    // 函数库：<函数名>.steps[N]（params 等其余顶层不是步骤）
    const second = toks[1]
    if (first.idx !== null || !second || second.name !== 'steps' || second.idx === null) return null
    path.push('functions', first.name, 'steps', second.idx)
    i = 2
  }
  for (; i < toks.length; i++) {
    const t = toks[i] as { name: string; idx: number | null }
    if (t.name === 'steps') {
      if (t.idx === null) return null
      // candidates[N].steps[M]：steps 是候选分支内的步骤下标（容器本身是 ['…','candidates',N]）
      if (path.length >= 2 && path[path.length - 2] === 'candidates') path.push(t.idx)
      // 其余（loop 循环体 steps[N] 等）：按普通容器键处理
      else path.push('steps', t.idx)
      continue
    }
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
      const sub = path.slice(0, end)
      if (sub[sub.length - 2] === 'candidates') continue
      try {
        ancestorUuids.push(resolveStep(model, sub).uuid)
      } catch {
        // 祖先失效不影响目标定位
      }
    }
    return { uuid: step.uuid, containerPath: path.slice(0, -1), ancestorUuids }
  } catch {
    return null
  }
}
