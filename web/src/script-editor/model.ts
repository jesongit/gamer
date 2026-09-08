/**
 * 脚本可视化编辑器 Model（YAML V1，Gamer V1 简化计划 Phase 1/4）。
 *
 * 语法契约：docs/plans/gamer_v1_simplification_plan.md §Phase 1；与宿主侧
 * `server/src/extensions/gamer_yaml/syntax.rs` 语义对齐（解析/校验/序列化
 * 双端一致）。V1 只描述流程，所有实际操作都是函数调用：
 * - Program = {name?, params?, vars?, run}；无 version 字段；
 * - Step 为 4 类判别联合：函数调用（fn: args + as）/ if / repeat / return；
 *   tap、find、sleep 等不是语法关键字，而是函数；
 * - 函数文件 = {functions: {名: {description?, params?, vars?, returns?, run}}}；
 * - Cell 是字段级取值：{lit: 字面量} 或 {ref: 变量路径}（ref 不含 $，如
 *   'home.center'；V1 只支持点号字段访问，无索引）；
 * - 函数调用参数 = 无参 | 位置值（Cell）| 命名参数映射（值均为 Cell）；
 *   每个步骤带浏览器内临时 uuid（选中/拖动/撤销/错误定位），不写入 YAML。
 */

// ---------- 参数声明（函数 Schema：类型/必填/默认值/说明） ----------

/** V1 参数类型（与服务端 ParamType 对齐；别名在解析层归一）。 */
export const PARAM_TYPES = [
  'any', 'boolean', 'integer', 'number', 'string', 'list', 'object',
  'duration', 'point', 'template', 'key',
] as const
export type ParamType = (typeof PARAM_TYPES)[number]

/** 参数声明（V1 全部为映射形态，rawForm 已随 v3 字符串声明删除）。 */
export interface ParamDecl {
  name: string
  type: ParamType
  required: boolean
  /** default === null 表示未声明默认值（配合 required 表达必填）。 */
  default: unknown | null
  desc: string
}

/** 可执行脚本（automations/ 资源）。 */
export interface Program {
  name: string | null
  params: ParamDecl[]
  /** vars：字面量表（不做引用解析；键序 = 声明序）。 */
  vars: Record<string, unknown>
  run: Step[]
}

/** 函数库内单个函数。 */
export interface FunctionModel {
  name: string
  description: string
  params: ParamDecl[]
  vars: Record<string, unknown>
  /** returns 声明（文档/提示用途，V1 不做运行时校验；原样保留防丢数据）。 */
  returns: unknown | null
  run: Step[]
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
 * 字段级取值：lit 的具体形态由所属字段类型约束；ref 为变量路径
 * （`$` 后原文，如 'home.center'；仅点号段，段为小写标识符）。
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

// ---------- 函数调用参数形态 ----------

/** 函数调用实参：无参（{}）/ 位置值（标量/数组/引用）/ 命名参数映射。 */
export type CallArgs =
  | { kind: 'none' }
  | { kind: 'value'; cell: Cell }
  | { kind: 'map'; entries: Record<string, Cell> }

export function emptyArgs(): CallArgs {
  return { kind: 'none' }
}

// ---------- 步骤（4 类） ----------

/** 控制流关键字（不能作为函数名/变量名）。 */
export const RESERVED_WORDS = ['if', 'repeat', 'return'] as const

/** uuid：浏览器内分配的稳定临时 ID，仅用于编辑态定位，绝不序列化进 YAML。 */
interface StepUuid {
  uuid: string
}

export type Step =
  & StepUuid
  & (
    | { kind: 'call'; fn: string; args: CallArgs; as: string | null }
    | { kind: 'if'; cond: Cell; then: Step[]; else: Step[] }
    | { kind: 'repeat'; times: Cell; body: Step[] }
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
    case 'if':
      return [
        { key: 'then', index: -1, list: step.then },
        { key: 'else', index: -1, list: step.else },
      ]
    case 'repeat':
      return [{ key: 'body', index: -1, list: step.body }]
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

// ---------- 引用收集（校验/提示用） ----------

/** Cell 树内引用到的全部顶层变量名。 */
export function collectCellRefs(cell: Cell | null | undefined, out: Set<string>): void {
  if (isRefCell(cell)) {
    const head = cell.ref.split('.')[0] ?? ''
    if (head !== '') out.add(head)
  }
}

/** 收集一个步骤（含子步）调用的函数名与引用的变量名。 */
export function collectStepUsage(
  step: Step,
  calls: Set<string>,
  refs: Set<string>,
): void {
  switch (step.kind) {
    case 'call': {
      calls.add(step.fn)
      switch (step.args.kind) {
        case 'value':
          collectCellRefs(step.args.cell, refs)
          break
        case 'map':
          for (const cell of Object.values(step.args.entries)) collectCellRefs(cell, refs)
          break
        default:
          break
      }
      break
    }
    case 'if': {
      collectCellRefs(step.cond, refs)
      for (const child of step.then) collectStepUsage(child, calls, refs)
      for (const child of step.else) collectStepUsage(child, calls, refs)
      break
    }
    case 'repeat': {
      collectCellRefs(step.times, refs)
      for (const child of step.body) collectStepUsage(child, calls, refs)
      break
    }
    case 'return':
      collectCellRefs(step.value, refs)
      break
  }
}
