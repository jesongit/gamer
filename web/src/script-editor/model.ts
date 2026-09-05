/**
 * 脚本可视化编辑器 Model（YAML v3，Phase 12 P12.1 重写）。
 *
 * 语法契约：docs/plans/phase12_v3_dsl_contract.md（§1-§6）；语义裁决见
 * docs/reference/adr/ADR-YAML-01~04。编辑器只读写 v3：
 * - Program = {version: 3, params, defaults, steps}；函数库 = bare-map {名: {params, steps}}
 *   的 Model 包装（functions 数组保序，codec 负责与 YAML 映射互转）；
 * - Step 为 19 类判别联合，分支子流程一律 Step[]；每个步骤带浏览器内临时 uuid
 *   （选中/拖动/撤销/错误定位），**不写入 YAML**；
 * - Cell 是字段级取值单元格：{lit: 类型化字面量} 或 {ref: 属性路径}
 *   （ref 不含前导 $，如 'reward.center'、'list[0]'，ADR-YAML-03 match 上下文）。
 */

// ---------- 参数声明（契约 §1 / §7） ----------

/** v3 规范参数类型（服务端 schema 五类；v2 ty 名由服务端映射到这五类）。 */
export const PARAM_TYPES = ['string', 'number', 'integer', 'boolean', 'enum'] as const
export type ParamType = (typeof PARAM_TYPES)[number]

/** 参数默认值字面量：标量（字符串/数字/布尔）。 */
export type ParamLiteral = string | number | boolean

/**
 * 参数声明。type 保留声明原文（rawForm 的串可以是 int/text 等 v2 ty 名，
 * 序列化按原文保真）；default === null 表示必填（map 形态无 default 键）。
 * rawForm = true 时该声明以整条字符串形态序列化：'type:name:remark[:default]'。
 */
export interface ParamDecl {
  type: string
  name: string
  remark: string
  /** coord 参数的默认值为 [x, y] 元组（schema descriptor / 字面量表双形态）。 */
  default: ParamLiteral | [number, number] | null
  rawForm: boolean
}

/** Program 级 defaults（契约 §1/§4，T45）：threshold step 值 > defaults > Runtime 兜底。 */
export interface DefaultsModel {
  vision_threshold: number | null
  after_tap: string | number | null
  after_match: string | number | null
  poll_interval: string | number | null
}

/** 可执行脚本（scripts/ 资源）。version 恒为 3（codec 保证）。 */
export interface Program {
  version: 3
  params: ParamDecl[]
  defaults: DefaultsModel | null
  steps: Step[]
}

/** 函数库内单个函数（bare-map 的一项）。 */
export interface FunctionModel {
  name: string
  params: ParamDecl[]
  steps: Step[]
}

export interface FunctionLibraryModel {
  /** 文件短路径（functions/ 下相对路径去扩展名），编辑态元数据，不序列化。 */
  file: string
  functions: FunctionModel[]
}

// ---------- 取值单元格 Cell ----------

/** coord 字面量：两个数字（相对坐标 0~1 由校验层把关）。 */
export type CoordLit = [number, number]

/**
 * 字段级取值：lit 的具体形态由所属字段类型约束；ref 为属性路径
 * （`$` 后原文，如 'reward.center' / 'match.score' / 'list[0]'）。
 */
export type Cell = { lit: unknown; ref?: undefined } | { ref: string; lit?: undefined }

export function lit(value: unknown): Cell {
  return { lit: value }
}
export function ref(name: string): Cell {
  return { ref: name }
}
export function isRefCell(cell: Cell | null | undefined): cell is { ref: string } {
  return cell !== null && cell !== undefined && typeof (cell as Cell).ref === 'string'
}

/** 步骤字段类型（CellEditor 控件选择与字面量校验的依据）。 */
export const CELL_TYPES = ['tmpl', 'coord', 'time', 'key', 'text', 'bool', 'expr', 'number'] as const
export type CellType = (typeof CELL_TYPES)[number]

// ---------- 步骤（19 类，契约 §1-§4） ----------

/** 步骤 kind（编辑器内部标识）。YAML 动作键经 yamlKeyOf 互转（app.start 等点号键）。 */
export const STEP_KINDS = [
  'app_start', 'app_stop', 'tap', 'swipe', 'key', 'text', 'wait',
  'log', 'set', 'if', 'loop', 'break', 'call', 'return', 'throw',
  'find', 'match_first', 'check', 'invoke',
] as const

export type StepKind = (typeof STEP_KINDS)[number]

const YAML_KEYS: Record<StepKind, string> = {
  app_start: 'app.start',
  app_stop: 'app.stop',
  tap: 'tap', swipe: 'swipe', key: 'key', text: 'text', wait: 'wait',
  log: 'log', set: 'set', if: 'if', loop: 'loop', break: 'break',
  call: 'call', return: 'return', throw: 'throw',
  find: 'find', match_first: 'match_first', check: 'check', invoke: 'invoke',
}

/** kind → YAML 动作键（app_start ↔ 'app.start'）。 */
export function yamlKeyOf(kind: StepKind): string {
  return YAML_KEYS[kind]
}

/** YAML 动作键全集（解析与序列化共用）。 */
export const ACTION_KEYS: readonly string[] = STEP_KINDS.map((k) => YAML_KEYS[k])

export function isStepKind(v: unknown): v is StepKind {
  return typeof v === 'string' && (STEP_KINDS as readonly string[]).includes(v)
}

/** find 二次验证（ADR-YAML-03：then 执行完后在 timeout 内二次验证模板）。 */
export interface FindVerify {
  template: Cell
  timeout: Cell | null
}

/** match_first 候选：单模板（+ 可选 threshold）→ 命中后步骤组（首个命中获胜）。 */
export interface MatchFirstCandidate {
  template: Cell
  threshold: number | null
  steps: Step[]
}

/** uuid：浏览器内分配的稳定临时 ID，仅用于编辑态定位，绝不序列化进 YAML。 */
interface StepUuid {
  uuid: string
}

export type Step =
  & StepUuid
  & (
    | { kind: 'app_start'; package: Cell | null }
    | { kind: 'app_stop'; package: Cell | null }
    | { kind: 'tap'; at: Cell }
    | { kind: 'swipe'; from: Cell; to: Cell; duration: Cell }
    | { kind: 'key'; key: Cell; action: 'down' | 'up' | 'press' | null }
    | { kind: 'text'; value: Cell }
    | { kind: 'wait'; min: Cell; max: Cell | null }
    | { kind: 'log'; message: Cell; level: string | null }
    | { kind: 'set'; name: string; value: Cell }
    | { kind: 'if'; cond: Cell; then: Step[]; else: Step[] }
    | { kind: 'loop'; times: Cell | null; steps: Step[] }
    | { kind: 'break' }
    | { kind: 'call'; target: string; with: Record<string, Cell>; save: string | null }
    | { kind: 'return'; value: Cell }
    | { kind: 'throw'; message: Cell }
    | {
      kind: 'find'
      template: Cell
      timeout: Cell | null
      threshold: number | null
      region: unknown | null
      save: string | null
      then: Step[]
      else: Step[]
      verify: FindVerify | null
    }
    | { kind: 'match_first'; candidates: MatchFirstCandidate[]; else: Step[] }
    | { kind: 'check'; template: Cell; timeout: Cell | null; threshold: number | null; throw: Cell | null }
    | { kind: 'invoke'; capability: string; with: Record<string, Cell>; save: string | null }
  )

// ---------- uuid 分配与步骤树工具 ----------

let uuidSeq = 0

/** 生成步骤 uuid：优先 crypto.randomUUID，不可用时退回计数器 + 随机数。 */
export function newStepUuid(): string {
  const c = typeof crypto !== 'undefined' ? crypto : undefined
  if (c && typeof c.randomUUID === 'function') return c.randomUUID()
  uuidSeq += 1
  return `step-${Date.now().toString(36)}-${uuidSeq}-${Math.random().toString(36).slice(2, 8)}`
}

/** 为一棵步骤树补齐 uuid（已有 uuid 的步骤保持不变）。返回传入引用本身，便于链式使用。 */
export function allocateUuids(steps: Step[]): Step[] {
  for (const step of steps) {
    if (!step.uuid) step.uuid = newStepUuid()
    for (const list of childStepLists(step)) allocateUuids(list.list)
  }
  return steps
}

/** 深拷贝步骤并重发全部 uuid（复制/粘贴用：副本必须与原步骤 uuid 不同）。 */
export function cloneStepWithNewUuids(step: Step): Step {
  const clone = structuredClone(step) as Step
  reassignUuids(clone)
  return clone
}

function reassignUuids(step: Step): void {
  step.uuid = newStepUuid()
  for (const child of childStepLists(step)) {
    for (const s of child.list) reassignUuids(s)
  }
}

/**
 * 枚举一个步骤携带的全部子流程列表（Vec 语义）。
 * list 引用是步骤对象上的原数组，就地修改即可被步骤持有。
 */
export function childStepLists(step: Step): { key: string; index: number; list: Step[] }[] {
  switch (step.kind) {
    case 'if':
    case 'find':
      return [
        { key: 'then', index: -1, list: step.then },
        { key: 'else', index: -1, list: step.else },
      ]
    case 'match_first':
      return [
        ...step.candidates.map((c, i) => ({ key: 'candidates', index: i, list: c.steps })),
        { key: 'else', index: -1, list: step.else },
      ]
    case 'loop':
      return [{ key: 'steps', index: -1, list: step.steps }]
    default:
      return []
  }
}

/** 先序遍历步骤树；visit 返回 false 时跳过该步骤的子流程。 */
export function walkSteps(
  steps: Step[],
  visit: (step: Step, parent: Step | null, containerKey: string | null) => boolean | void,
  parent: Step | null = null,
  containerKey: string | null = null,
): void {
  for (const step of steps) {
    if (visit(step, parent, containerKey) === false) continue
    for (const child of childStepLists(step)) walkSteps(child.list, visit, step, child.key)
  }
}

/** 统计步骤总数（含所有分支子流程）。 */
export function countSteps(steps: Step[]): number {
  let n = 0
  walkSteps(steps, () => {
    n += 1
  })
  return n
}
