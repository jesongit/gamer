/**
 * 命令栈（plan §8.2 / §8.4）：可视化编辑器的唯一写入口。
 *
 * - 每个命令在应用时记录逆操作，undo/redo 依赖对象引用保持不变（删除步骤原对象
 *   原样放回，uuid 天然稳定）；
 * - 支持事务合并（录制用）：begin→多次 apply→commit，一次事务 = 一条历史 = 一次 undo；
 *   abort 回滚本事务已应用的部分且不进历史；
 * - 命令路径用数组寻址（如 ['steps', 0, 'then', 1]），函数库支持 ['functions', 'login', 'steps', 0]。
 */

import {
  cloneStepWithNewUuids,
  type FunctionLibraryModel,
  type FunctionModel,
  type ParamDecl,
  type ScriptConfig,
  type ScriptModel,
  type Step,
} from './model'

// ---------- 路径与文档 ----------

/** 路径段：对象键（'steps'/'then'/'functions'/函数名）或数组下标。 */
export type PathSeg = string | number
export type Path = PathSeg[]

/** 路径终点的宿主对象类型。 */
export type EditorModel = ScriptModel | FunctionLibraryModel

export function clonePath(path: Path): Path {
  return [...path]
}

/**
 * 步骤子列表键 → 对应数组；不存在的键返回 undefined。
 * 'candidates' 与候选下标成对出现（['…', 'candidates', n]），由 resolveStepList 特判。
 */
function stepChildList(step: Step, key: string, candidateIndex = -1): Step[] | undefined {
  switch (key) {
    case 'then':
      return 'then' in step ? (step as { then: Step[] }).then : undefined
    case 'else':
      return 'else' in step ? (step as { else: Step[] }).else : undefined
    case 'steps':
      return step.kind === 'loop' ? step.steps : undefined
    case 'candidates':
      if (step.kind === 'match') return step.candidates[candidateIndex]?.steps
      if (step.kind === 'color') return step.expect[candidateIndex]?.steps
      return undefined
    default:
      return undefined
  }
}

/**
 * 解析路径到 Step[] 容器。路径语法（以键或 ('candidates', n) 对结尾）：
 * - ['steps']                                 脚本主流程
 * - ['steps', 0, 'then']                      steps[0].then
 * - ['steps', 0, 'candidates', 1]             steps[0] 的 match 候选 1 分支（或 color expect 1）
 * - ['functions', <名|序号>, 'steps']         函数库函数体
 * - ['functions', 'login', 'steps', 0, 'else'] 函数体内嵌套分支
 */
export function resolveStepList(model: EditorModel, path: Path): Step[] {
  if (path.length === 0) throw new Error('路径为空')
  const last = path[path.length - 1]
  if (typeof last === 'number') {
    // ('candidates', n) 形态：倒数第二段必须是 'candidates'
    if (path.length >= 2 && path[path.length - 2] === 'candidates') {
      const step = resolveStep(model, path.slice(0, -2))
      const child = stepChildList(step, 'candidates', last)
      if (!child) throw new Error(`步骤 ${step.kind} 没有候选分支 ${last}`)
      return child
    }
    throw new Error(`路径以数字结尾指向步骤而非列表：${path.map(String).join('.')}`)
  }
  // 以键结尾
  if (path.length === 1) {
    if (last === 'steps' && 'steps' in model) return model.steps
    throw new Error(`路径首段非法：${String(last)}`)
  }
  if (last === 'steps' && path.length === 3 && path[0] === 'functions' && 'functions' in model) {
    return resolveFunction(model, path[1]).steps
  }
  const step = resolveStep(model, path.slice(0, -1))
  const child = stepChildList(step, last)
  if (!child) throw new Error(`步骤 ${step.kind} 没有子列表 ${last}（路径 ${path.map(String).join('.')}）`)
  return child
}

/** 解析路径到单个步骤（路径以数字下标结尾，且倒数第二段不是 'candidates'）。 */
export function resolveStep(model: EditorModel, path: Path): Step {
  if (path.length === 0) throw new Error('路径为空')
  const index = path[path.length - 1]
  if (typeof index !== 'number') {
    throw new Error(`步骤路径必须以数字下标结尾：${path.map(String).join('.')}`)
  }
  if (path.length >= 2 && path[path.length - 2] === 'candidates') {
    throw new Error(`['candidates', n] 结尾的路径是容器而非步骤：${path.map(String).join('.')}`)
  }
  const list = resolveStepList(model, path.slice(0, -1))
  const step = list[index]
  if (!step) throw new Error(`路径越界：${path.map(String).join('.')}`)
  return step
}

function resolveFunction(model: FunctionLibraryModel, seg: PathSeg): FunctionModel {
  if (typeof seg === 'number') {
    const fn = model.functions[seg]
    if (!fn) throw new Error(`函数序号越界：${seg}`)
    return fn
  }
  const fn = model.functions.find((f) => f.name === seg)
  if (!fn) throw new Error(`函数不存在：${seg}`)
  return fn
}

/**
 * 解析 params 命令目标宿主（持有 params 数组的对象）：
 * - path 缺省/空 = 文件级：脚本模型自身（函数库模型没有文件级 params → 报错）；
 * - ['functions', <函数名|序号>, 'params'] = 函数级：返回该函数记录（阶段 4）。
 * 返回宿主对象引用而非数组快照：redo/undo 经同一引用整体替换或就地增删，
 * Vue reactive 场景下宿主是代理，写入天然触发更新。
 */
function resolveParamsHost(model: EditorModel, path?: Path): { params: ParamDecl[] } {
  if (!path || path.length === 0) {
    if (!('params' in model)) {
      throw new Error('函数库模型没有文件级 params；请用 [\'functions\', 函数名, \'params\'] 路径编辑具体函数')
    }
    return model as ScriptModel
  }
  if ('functions' in model && path.length === 3 && path[0] === 'functions' && path[2] === 'params') {
    return resolveFunction(model, path[1])
  }
  throw new Error(`非法 params 路径：${path.map(String).join('.')}（缺省=文件级，或 ['functions', 函数名, 'params']）`)
}

// ---------- 命令 ----------

/**
 * 深度解包 Vue reactive 代理（duck-typed __v_raw，与 vue 的 toRaw 同一内部标记）。
 * UI 层会传 reactive(model) 进来，且组件常从代理对象展开构造新对象（{...proxy} 的字段值
 * 仍是嵌套代理）——structuredClone 无法克隆任何 Proxy（DataCloneError），快照/复制前
 * 必须递归解包。返回全新纯对象（快照语义，与 structuredClone 一致）。
 */
function unwrap<T>(value: T): T {
  if (value === null || typeof value !== 'object') return value
  const raw = (value as { __v_raw?: unknown }).__v_raw
  if (raw !== undefined && raw !== null) return unwrap(raw as T)
  if (Array.isArray(value)) {
    return (value as unknown[]).map((v) => unwrap(v)) as unknown as T
  }
  const out: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) out[k] = unwrap(v)
  return out as T
}

export type Command =
  | { type: 'insert_step'; path: Path; index: number; step: Step }
  | { type: 'remove_step'; path: Path; index: number }
  | { type: 'move_step'; from: { path: Path; index: number }; to: { path: Path; index: number } }
  | { type: 'duplicate_step'; path: Path; index: number }
  | { type: 'update_step'; path: Path; fields: Record<string, unknown> }
  /** params 命令：path 缺省 = 文件级（脚本）；['functions', <函数名|序号>, 'params'] = 函数级（阶段 4）。 */
  | { type: 'set_params'; path?: Path; params: ParamDecl[] }
  | { type: 'insert_param'; path?: Path; index: number; decl: ParamDecl }
  | { type: 'remove_param'; path?: Path; index: number }
  | { type: 'update_param'; path?: Path; index: number; decl: ParamDecl }
  | { type: 'set_config'; config: ScriptConfig | null }

interface HistoryEntry {
  name: string
  undo(): void
  redo(): void
}

/** 移动目标位于被移动步骤自己的子树内 → 拒绝（避免结构损坏）。 */
function isDescendantPath(ancestor: Path, descendant: Path): boolean {
  if (descendant.length <= ancestor.length) return false
  for (let i = 0; i < ancestor.length; i++) {
    if (ancestor[i] !== descendant[i]) return false
  }
  return true
}

export class CommandStack {
  private readonly model: EditorModel
  private readonly entries: HistoryEntry[] = []
  private pointer = -1
  private transactionStack: { name: string; applied: HistoryEntry[] }[] | null = null
  private changeListeners = new Set<() => void>()

  constructor(model: EditorModel) {
    this.model = model
  }

  /** 订阅变更（UI 绑定）；返回取消函数。 */
  onChange(fn: () => void): () => void {
    this.changeListeners.add(fn)
    return () => this.changeListeners.delete(fn)
  }

  private emitChange(): void {
    for (const fn of this.changeListeners) fn()
  }

  get canUndo(): boolean {
    return this.pointer >= 0
  }

  get canRedo(): boolean {
    return this.pointer < this.entries.length - 1
  }

  /** 可一步撤销的历史条数（不含进行中的事务）。 */
  get depth(): number {
    return this.pointer + 1
  }

  /** 应用命令并记录历史。返回 false 表示命令被拒绝（未改变模型、不进历史）。 */
  apply(command: Command, name = command.type): boolean {
    const entry = this.execute(command, name)
    if (entry === null) return false
    entry.redo() // 立即执行；undo/redo 此后通过闭包重放
    this.record(entry)
    this.emitChange()
    return true
  }

  undo(): boolean {
    if (!this.canUndo || this.inTransaction) return false
    this.entries[this.pointer].undo()
    this.pointer -= 1
    this.emitChange()
    return true
  }

  redo(): boolean {
    if (!this.canRedo || this.inTransaction) return false
    this.pointer += 1
    this.entries[this.pointer].redo()
    this.emitChange()
    return true
  }

  /** 事务期间禁止裸 undo/redo（必须先 commit/abort）。 */
  get inTransaction(): boolean {
    return this.transactionStack !== null
  }

  begin(name = 'transaction'): void {
    if (this.transactionStack === null) this.transactionStack = []
    this.transactionStack.push({ name, applied: [] })
  }

  /** 提交事务：本事务内全部命令合并为一条历史记录。 */
  commit(): void {
    const tx = this.popTransaction()
    if (tx.applied.length === 0) return
    this.record({
      name: tx.name,
      undo: () => {
        for (let i = tx.applied.length - 1; i >= 0; i--) tx.applied[i].undo()
      },
      redo: () => {
        for (const e of tx.applied) e.redo()
      },
    })
    this.emitChange()
  }

  /** 放弃事务：回滚已应用部分，不进历史。 */
  abort(): void {
    const tx = this.popTransaction()
    for (let i = tx.applied.length - 1; i >= 0; i--) tx.applied[i].undo()
    this.emitChange()
  }

  /** 事务便捷封装：fn 内 apply 的全部命令合并为一次撤销。fn 内不得自行 begin/commit/abort。 */
  transaction<T>(fn: () => T, name = 'transaction'): T {
    this.begin(name)
    try {
      const result = fn()
      this.commit()
      return result
    } catch (e) {
      if (this.inTransaction) this.abort()
      throw e
    }
  }

  private popTransaction(): { name: string; applied: HistoryEntry[] } {
    if (this.transactionStack === null || this.transactionStack.length === 0) {
      throw new Error('没有进行中的事务')
    }
    const tx = this.transactionStack.pop()
    if (this.transactionStack.length === 0) this.transactionStack = null
    return tx as { name: string; applied: HistoryEntry[] }
  }

  private record(entry: HistoryEntry): void {
    if (this.transactionStack !== null) {
      this.transactionStack[this.transactionStack.length - 1].applied.push(entry)
      return
    }
    // 丢弃 pointer 之后的重做分支
    this.entries.splice(this.pointer + 1)
    this.entries.push(entry)
    this.pointer = this.entries.length - 1
  }

  /** 执行命令并返回 {undo, redo}；被拒绝返回 null。 */
  private execute(command: Command, name: string): HistoryEntry | null {
    switch (command.type) {
      case 'insert_step': {
        const list = resolveStepList(this.model, command.path)
        if (command.index < 0 || command.index > list.length) return null
        return {
          name,
          redo: () => list.splice(command.index, 0, command.step),
          undo: () => list.splice(command.index, 1),
        }
      }
      case 'remove_step': {
        const list = resolveStepList(this.model, command.path)
        if (command.index < 0 || command.index >= list.length) return null
        const removed = list[command.index]
        const position = { path: clonePath(command.path), index: command.index }
        return {
          name,
          redo: () => {
            const l = resolveStepList(this.model, position.path)
            if (l[position.index] === removed) l.splice(position.index, 1)
          },
          undo: () => {
            const l = resolveStepList(this.model, position.path)
            l.splice(position.index, 0, removed)
          },
        }
      }
      case 'move_step': {
        const fromList = resolveStepList(this.model, command.from.path)
        const step = fromList[command.from.index]
        if (!step) return null
        const fromPathAbs = [...command.from.path, command.from.index]
        const toPathAbs = [...command.to.path, command.to.index]
        if (isDescendantPath(fromPathAbs, toPathAbs)) return null
        const from = { path: clonePath(command.from.path), index: command.from.index }
        const to = { path: clonePath(command.to.path), index: command.to.index }
        // to.index 语义：源元素删除后目标列表中的插入下标（post-removal）。
        // redo 后元素实际位于 min(to.index, 目标列表长度)；undo 按该位置取回并放回 from.index。
        // 该推导对同列表/跨列表统一成立，无需区分。
        // to.index 语义：源元素删除后目标列表中的插入下标（post-removal）。
        // redo 后元素实际位于 min(to.index, 目标列表长度)；undo 按该位置取回并放回 from.index。
        const moveForward = (): void => {
          const fl = resolveStepList(this.model, from.path)
          if (from.index < 0 || from.index >= fl.length) return
          const s = fl[from.index]
          fl.splice(from.index, 1)
          const tl = resolveStepList(this.model, to.path)
          const insertAt = Math.max(0, Math.min(to.index, tl.length))
          tl.splice(insertAt, 0, s)
        }
        const moveBack = (): void => {
          const tl = resolveStepList(this.model, to.path)
          if (tl.length === 0) return
          const pos = Math.min(to.index, tl.length - 1)
          const s = tl[pos]
          tl.splice(pos, 1)
          const fl = resolveStepList(this.model, from.path)
          fl.splice(from.index, 0, s)
        }
        return {
          name,
          redo: moveForward,
          undo: moveBack,
        }
      }
      case 'duplicate_step': {
        const list = resolveStepList(this.model, command.path)
        const source = list[command.index]
        if (!source) return null
        const copy = cloneStepWithNewUuids(unwrap(source))
        const at = command.index + 1
        const position = { path: clonePath(command.path), index: at }
        return {
          name,
          redo: () => {
            const l = resolveStepList(this.model, position.path)
            l.splice(position.index, 0, copy)
          },
          undo: () => {
            const l = resolveStepList(this.model, position.path)
            l.splice(position.index, 1)
          },
        }
      }
      case 'update_step': {
        const step = resolveStep(this.model, command.path)
        const pathStr = command.path.map(String).join('.')
        const oldValues: Record<string, unknown> = {}
        for (const key of Object.keys(command.fields)) {
          if (!(key in (step as unknown as Record<string, unknown>))) {
            throw new Error(`步骤 ${step.kind} 没有字段 ${key}（路径 ${pathStr}）`)
          }
          oldValues[key] = structuredClone(unwrap((step as unknown as Record<string, unknown>)[key]))
        }
        const newValues = structuredClone(unwrap(command.fields))
        const target = step as unknown as Record<string, unknown>
        return {
          name,
          redo: () => Object.assign(target, newValues),
          undo: () => Object.assign(target, oldValues),
        }
      }
      case 'set_params': {
        const host = resolveParamsHost(this.model, command.path)
        const oldParams = host.params
        const newParams = structuredClone(unwrap(command.params))
        return {
          name,
          redo: () => {
            host.params = newParams
          },
          undo: () => {
            host.params = oldParams
          },
        }
      }
      case 'insert_param': {
        const host = resolveParamsHost(this.model, command.path)
        const at = Math.max(0, Math.min(command.index, host.params.length + 1))
        return {
          name,
          redo: () => host.params.splice(at, 0, command.decl),
          undo: () => host.params.splice(at, 1),
        }
      }
      case 'remove_param': {
        const host = resolveParamsHost(this.model, command.path)
        const removed = host.params[command.index]
        if (!removed) return null
        return {
          name,
          redo: () => host.params.splice(command.index, 1),
          undo: () => host.params.splice(command.index, 0, removed),
        }
      }
      case 'update_param': {
        const host = resolveParamsHost(this.model, command.path)
        const old = host.params[command.index]
        if (!old) return null
        const updated = structuredClone(unwrap(command.decl))
        return {
          name,
          redo: () => {
            host.params[command.index] = updated
          },
          undo: () => {
            host.params[command.index] = old
          },
        }
      }
      case 'set_config': {
        if (!('config' in this.model)) throw new Error('函数库模型没有文件级 config')
        const m = this.model as ScriptModel
        const oldConfig = m.config
        const newConfig = command.config === null ? null : structuredClone(unwrap(command.config))
        return {
          name,
          redo: () => {
            m.config = newConfig
          },
          undo: () => {
            m.config = oldConfig
          },
        }
      }
    }
  }
}

// ---------- 便捷路径构造 ----------

export const paths = {
  steps: (): Path => ['steps'],
  functionSteps: (name: string): Path => ['functions', name, 'steps'],
  /** 函数级 params 容器（阶段 4：params 命令带 path 时使用）。 */
  functionParams: (name: string): Path => ['functions', name, 'params'],
  child: (path: Path, key: string, index: number): Path => [...path, key, index],
  /** 校验路径（validation 字符串形态）与命令路径互转所需：['steps', 0, 'then', 1]。 */
  join: (...segs: PathSeg[]): Path => segs,
}
