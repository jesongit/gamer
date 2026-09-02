/**
 * 脚本编辑器前端 Model（阶段 0 冻结契约，见 docs/SCRIPT_EDITOR_CONTRACT.md §3）。
 *
 * 本文件是可视化编辑器的唯一编辑源形态：
 * - 字段名严格等于 __fixtures__/json/*.golden.json（五方对照中的「前端 Model」）；
 * - Step 为 19 类判别联合，分支子流程一律 Step[]（Vec 语义，空列表显式存在）；
 * - 每个步骤带浏览器内临时 uuid（选中/拖动/撤销/错误定位），**不写入 YAML**；
 * - Cell 是字段级取值单元格：{ lit: 类型化字面量 } 或 { ref: 参数名 }。
 */

// ---------- 参数 ----------

export const PARAM_TYPES = ['tmpl', 'coord', 'color', 'time', 'key', 'text', 'bool'] as const
export type ParamType = (typeof PARAM_TYPES)[number]

export type LogLevel = 'debug' | 'info' | 'warn' | 'error'
export const LOG_LEVELS: readonly LogLevel[] = ['debug', 'info', 'warn', 'error']

/** 类型化字面量：coord 为 [x, y]，bool 为布尔，其余为字符串（颜色为 6 位十六进制小写无 #）。 */
export type ParamLiteral = string | [number, number] | boolean

/** 参数声明；default === null 表示必填（没有默认值）。 */
export interface ParamDecl {
  type: ParamType
  name: string
  remark: string
  default: ParamLiteral | null
}

export interface ScriptConfig {
  interval: string
  threshold: number
  log_level: LogLevel
}

export interface ScriptModel {
  params: ParamDecl[]
  config: ScriptConfig | null
  steps: Step[]
}

export interface FunctionModel {
  name: string
  params: ParamDecl[]
  steps: Step[]
}

export interface FunctionLibraryModel {
  /** 文件短路径（不含 func/ 目录与扩展名），如 common。 */
  file: string
  functions: FunctionModel[]
}

// ---------- 取值单元格 Cell（契约 §3.4） ----------

/** coord 字面量：两个 0~1 的数字。 */
export type CoordLit = [number, number]

/** 字段级取值：lit 的具体形态由所属字段类型约束（coord→[x,y]、bool→boolean、其余→string）。 */
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

// ---------- 步骤（19 类，契约 §3.5） ----------

export const STEP_KINDS = [
  'str_app', 'cls_app', 'tap', 'swipe', 'key', 'text', 'log', 'wait',
  'find', 'match', 'check', 'color', 'if', 'loop', 'break', 'call', 'func', 'throw', 'return',
] as const

export type StepKind = (typeof STEP_KINDS)[number]

/**
 * YAML 动作键。与 kind 同名（swipe 的 YAML 键也是 swipe，仅内部键 fm ↔ Model 字段 from）。
 */
export const ACTION_KEYS: readonly string[] = STEP_KINDS

export function isStepKind(v: unknown): v is StepKind {
  return typeof v === 'string' && (STEP_KINDS as readonly string[]).includes(v)
}

/** match 候选：单模板 → 分支步骤列表（首个命中获胜）；click=true 命中后点击模板框中心并等待 interval。 */
export interface MatchCandidate {
  template: Cell
  click: boolean
  steps: Step[]
}

/** color 候选：有序列表，每项单颜色 → 分支步骤列表（不用颜色做映射键，契约 §4.2）；click=true 命中后点击取样点并等待 interval。 */
export interface ColorExpect {
  color: Cell
  click: boolean
  steps: Step[]
}

/** uuid：浏览器内分配的稳定临时 ID，仅用于编辑态定位，绝不序列化进 YAML。 */
interface StepUuid {
  uuid: string
}

export type Step =
  & StepUuid
  & (
    | { kind: 'str_app' }
    | { kind: 'cls_app' }
    | { kind: 'tap'; at: Cell }
    | { kind: 'swipe'; from: Cell; to: Cell; time: Cell }
    | { kind: 'key'; key: Cell }
    | { kind: 'text'; value: Cell }
    | { kind: 'log'; message: Cell }
    | { kind: 'wait'; duration: Cell; duration_max: Cell | null }
    | { kind: 'find'; template: Cell; block: Cell[]; verify: boolean; timeout: Cell | null; then: Step[]; else: Step[] }
    | { kind: 'match'; candidates: MatchCandidate[]; else: Step[]; timeout: Cell | null }
    | { kind: 'check'; template: Cell; timeout: Cell | null; throw: string | null }
    | { kind: 'color'; at: Cell; expect: ColorExpect[]; else: Step[] }
    | { kind: 'if'; cond: Cell; then: Step[]; else: Step[] }
    | { kind: 'loop'; times: number; steps: Step[] }
    | { kind: 'break' }
    | { kind: 'call'; target: string; args: Record<string, Cell> }
    | { kind: 'func'; target: string; args: Record<string, Cell>; then: Step[]; else: Step[] }
    | { kind: 'throw'; message: string | null }
    | { kind: 'return'; value: Cell }
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
    case 'find':
    case 'if':
    case 'func':
      return [
        { key: 'then', index: -1, list: step.then },
        { key: 'else', index: -1, list: step.else },
      ]
    case 'match':
      return [
        ...step.candidates.map((c, i) => ({ key: 'candidates', index: i, list: c.steps })),
        { key: 'else', index: -1, list: step.else },
      ]
    case 'color':
      return [
        ...step.expect.map((e, i) => ({ key: 'candidates', index: i, list: e.steps })),
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

/** 统计步骤总数（含所有分支子流程），用于防死循环 guard 的前端提示。 */
export function countSteps(steps: Step[]): number {
  let n = 0
  walkSteps(steps, () => {
    n += 1
  })
  return n
}
