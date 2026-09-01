/**
 * 步骤卡片原生拖放所需的数据与下标换算。
 *
 * 拖放只传递路径和 uuid，不把 Step 对象塞进 DataTransfer；真正的移动仍由
 * CommandStack 执行，因此跨容器移动、撤销和重做都沿用同一套命令语义。
 */
import type { Path } from './commands'

export const STEP_DRAG_MIME = 'application/x-gamer-step'

export interface StepDragPayload {
  uuid: string
  path: Path
  index: number
}

// 部分浏览器在 dragover 阶段限制 getData() 的读取；同页拖放时用内存中的活动源定位兜底。
let activeDrag: StepDragPayload | null = null

function isPath(value: unknown): value is Path {
  return Array.isArray(value) && value.every((segment) =>
    (typeof segment === 'string' && segment.length > 0)
    || (typeof segment === 'number' && Number.isInteger(segment) && segment >= 0),
  )
}

function decode(raw: string): StepDragPayload | null {
  if (!raw) return null
  try {
    const value: unknown = JSON.parse(raw)
    if (!value || typeof value !== 'object') return null
    const candidate = value as Partial<StepDragPayload>
    if (typeof candidate.uuid !== 'string' || !candidate.uuid) return null
    if (!isPath(candidate.path)) return null
    if (typeof candidate.index !== 'number' || !Number.isInteger(candidate.index) || candidate.index < 0) return null
    return { uuid: candidate.uuid, path: [...candidate.path], index: candidate.index }
  } catch {
    return null
  }
}

/** 把步骤定位信息写入拖放数据，text/plain 作为浏览器兼容兜底。 */
export function writeStepDragPayload(dataTransfer: DataTransfer, payload: StepDragPayload): void {
  const raw = JSON.stringify(payload)
  dataTransfer.setData(STEP_DRAG_MIME, raw)
  dataTransfer.setData('text/plain', raw)
  dataTransfer.effectAllowed = 'move'
  activeDrag = { uuid: payload.uuid, path: [...payload.path], index: payload.index }
}

/** 从拖放数据读取步骤定位信息；外部拖入的普通文本会被忽略。 */
export function readStepDragPayload(dataTransfer: DataTransfer | null): StepDragPayload | null {
  if (!dataTransfer) return null
  return decode(dataTransfer.getData(STEP_DRAG_MIME)) ?? decode(dataTransfer.getData('text/plain'))
}

export function getActiveStepDrag(): StepDragPayload | null {
  return activeDrag ? { uuid: activeDrag.uuid, path: [...activeDrag.path], index: activeDrag.index } : null
}

export function clearActiveStepDrag(): void {
  activeDrag = null
}

export function samePath(a: Path, b: Path): boolean {
  return a.length === b.length && a.every((segment, index) => segment === b[index])
}

/**
 * move_step 的 to.index 是“源元素删除后”的目标下标。
 * targetIndex/before 使用拖放时看到的原列表位置，先换算为命令所需语义。
 */
export function postRemovalIndex(
  source: StepDragPayload,
  targetPath: Path,
  targetIndex: number,
  before: boolean,
): number {
  const rawIndex = targetIndex + (before ? 0 : 1)
  if (samePath(source.path, targetPath) && source.index < rawIndex) return rawIndex - 1
  return rawIndex
}
