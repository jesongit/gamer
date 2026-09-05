/**
 * 步骤工厂与添加面板分组（v3）。
 *
 * 手动添加全部走这里的强类型工厂；面板按任务分组
 * （应用/操作/识别/流程/复用/函数专用），return 仅函数上下文可见。
 */

import type { Cell, Step, StepKind } from './model'
import { lit, newStepUuid } from './model'

// ---------- 工厂 ----------

/** Omit 对联合类型不可分发，先分发再去 uuid，保证 overrides 按各分支字段校验。 */
type DistributedOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never
export type StepOverrides = Partial<DistributedOmit<Step, 'uuid'>>

/** 创建步骤：填默认字段 + 覆盖项 + 分配 uuid。未知 kind 抛错。 */
export function createStep(kind: StepKind, overrides: StepOverrides = {}): Step {
  const base = { uuid: newStepUuid(), kind, ...overrides } as Step
  return base
}

/** 复制上下文里的默认值：坐标取屏幕中心，时间 1s。 */
const CENTER: Cell = lit([0.5, 0.5])

export const DEFAULT_FACTORIES: Record<StepKind, () => Step> = {
  app_start: () => createStep('app_start', { package: null }),
  app_stop: () => createStep('app_stop', { package: null }),
  tap: () => createStep('tap', { at: CENTER }),
  swipe: () => createStep('swipe', { from: lit([0.3, 0.5]), to: lit([0.7, 0.5]), duration: lit('500ms') }),
  key: () => createStep('key', { key: lit('BACK'), action: null }),
  text: () => createStep('text', { value: lit('') }),
  log: () => createStep('log', { message: lit(''), level: null }),
  wait: () => createStep('wait', { min: lit('1s'), max: null }),
  set: () => createStep('set', { name: '', value: lit('') }),
  if: () => createStep('if', { cond: lit(true), then: [], else: [] }),
  loop: () => createStep('loop', { times: lit(3), steps: [] }),
  break: () => createStep('break'),
  call: () => createStep('call', { target: '', with: {}, save: null }),
  invoke: () => createStep('invoke', { capability: '', with: {}, save: null }),
  return: () => createStep('return', { value: lit(null) }),
  throw: () => createStep('throw', { message: lit('') }),
  find: () => createStep('find', {
    template: lit(''),
    timeout: null,
    threshold: null,
    region: null,
    save: null,
    then: [],
    else: [],
    verify: null,
  }),
  match_first: () => createStep('match_first', {
    candidates: [{ template: lit(''), threshold: null, steps: [] }],
    else: [],
  }),
  check: () => createStep('check', { template: lit(''), timeout: null, threshold: null }),
}

/** 便捷入口：按 kind 创建默认步骤。 */
export function makeStep(kind: StepKind): Step {
  const factory = DEFAULT_FACTORIES[kind]
  if (!factory) throw new Error(`未知步骤类型 ${kind}`)
  return factory()
}

// ---------- 添加面板分组 ----------

export type PanelGroupId = 'app' | 'action' | 'recognition' | 'flow' | 'reuse' | 'function'

export interface PanelEntry {
  kind: StepKind
  /** 面板展示名（中文动作名，卡片收起摘要的基调）。 */
  label: string
  group: PanelGroupId
}

export const PANEL_GROUPS: { id: PanelGroupId; label: string; entries: PanelEntry[] }[] = [
  {
    id: 'app',
    label: '应用',
    entries: [
      { kind: 'app_start', label: '启动应用', group: 'app' },
      { kind: 'app_stop', label: '关闭应用', group: 'app' },
    ],
  },
  {
    id: 'action',
    label: '操作',
    entries: [
      { kind: 'tap', label: '点击坐标', group: 'action' },
      { kind: 'swipe', label: '滑动', group: 'action' },
      { kind: 'key', label: '按键', group: 'action' },
      { kind: 'text', label: '输入文本', group: 'action' },
      { kind: 'wait', label: '等待', group: 'action' },
    ],
  },
  {
    id: 'recognition',
    label: '识别',
    entries: [
      { kind: 'find', label: '等待模板', group: 'recognition' },
      { kind: 'match_first', label: '多模板匹配', group: 'recognition' },
      { kind: 'check', label: '检查模板', group: 'recognition' },
    ],
  },
  {
    id: 'flow',
    label: '流程',
    entries: [
      { kind: 'if', label: '条件分支', group: 'flow' },
      { kind: 'loop', label: '循环', group: 'flow' },
      { kind: 'break', label: '跳出循环', group: 'flow' },
      { kind: 'set', label: '设置变量', group: 'flow' },
      { kind: 'throw', label: '抛出错误', group: 'flow' },
      { kind: 'log', label: '记录日志', group: 'flow' },
    ],
  },
  {
    id: 'reuse',
    label: '复用',
    entries: [
      { kind: 'call', label: '调用脚本/函数', group: 'reuse' },
      { kind: 'invoke', label: '调用能力', group: 'reuse' },
    ],
  },
  {
    id: 'function',
    label: '函数专用',
    entries: [
      { kind: 'return', label: '返回值', group: 'function' },
    ],
  },
]

/** 按编辑上下文取面板条目：脚本上下文隐藏「函数专用」（return 仅函数）。 */
export function panelEntries(context: 'script' | 'function'): PanelEntry[] {
  return PANEL_GROUPS.flatMap((g) => g.entries).filter((e) => context === 'function' || e.group !== 'function')
}
