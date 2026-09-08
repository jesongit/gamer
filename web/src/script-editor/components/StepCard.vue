<template>
  <div
    class="step-card"
    :class="{
      selected, expanded, dragging, 'has-error': ownErrors.length > 0, 'card-highlight': highlighted,
      'drop-before': dropPosition === 'before', 'drop-after': dropPosition === 'after',
      [`kind-${step.kind}`]: true,
    }"
    :data-step-uuid="step.uuid"
    :data-step-path="stepPath"
    @click.stop="emit('select', step.uuid)"
    @dragover.prevent.stop="onDragOver"
    @dragleave.stop="onDragLeave"
    @drop.prevent.stop="onDrop"
  >
    <!-- 卡头：拖动手柄 + 图标 + 中文名 + 序号 + 摘要 + 动作按钮 -->
    <div class="card-head">
      <span
        class="drag-handle" title="拖动排序" draggable="true" role="button" aria-label="拖动排序"
        @dragstart.stop="onDragStart" @dragend.stop="onDragEnd" @click.stop
      >⋮⋮</span>
      <span class="kind-icon" :title="meta.hint">{{ meta.icon }}</span>
      <span class="kind-name">{{ meta.label }}</span>
      <span class="step-no">#{{ index + 1 }}</span>
      <span class="summary" :title="summary">{{ summary }}</span>
      <span v-if="ownErrors.length" class="err-badge" :title="ownErrors.map((d) => d.message).join('\n')">
        {{ ownErrors.length }}
      </span>
      <span class="head-actions">
        <!-- 函数测试入口：仅宿主开启 testFrom 时显示（函数库页签的函数体顶层卡片） -->
        <button
          v-if="testFrom" type="button" class="mini-btn test-from"
          title="从此步骤测试函数"
          @click.stop="emit('test-from', step.uuid)"
        >▶测试</button>
        <button
          type="button" class="mini-btn expand-btn"
          :title="expanded ? '收起' : '展开编辑'"
          @click.stop="onToggleExpand"
        >{{ expanded ? '▾' : '▸' }}</button>
        <button type="button" class="mini-btn" title="上移" :disabled="index === 0" @click.stop="moveBy(-1)">↑</button>
        <button type="button" class="mini-btn" title="下移" :disabled="index >= listLength - 1" @click.stop="moveBy(1)">↓</button>
        <button type="button" class="mini-btn" title="复制步骤" @click.stop="duplicate">⧉</button>
        <button type="button" class="mini-btn danger" title="删除步骤" @click.stop="remove">✕</button>
      </span>
    </div>

    <!-- 展开态：按类型的强类型控件（不提供任意键值编辑器） -->
    <div v-if="expanded" class="card-body" @click.stop>
      <!-- 函数调用 -->
      <template v-if="step.kind === 'call'">
        <div class="field-row">
          <span class="field-label">函数</span>
          <template v-if="targetOptions">
            <select
              class="cell-input target-select" :value="step.fn" aria-label="函数"
              @change="applyFn(($event.target as HTMLSelectElement).value)"
            >
              <option value="">（选择函数）</option>
              <optgroup v-for="g in targetGroups" :key="g.id" :label="g.label">
                <option v-for="o in g.options" :key="o.target" :value="o.target">{{ o.label || o.target }}</option>
              </optgroup>
              <option v-if="step.fn && !allTargets.some((o) => o.target === step.fn)" :value="step.fn">{{ step.fn }}（已失效）</option>
            </select>
          </template>
          <input
            v-else
            class="cell-input mono" :value="step.fn"
            placeholder="函数名，如 wait_find"
            aria-label="函数名"
            @change="applyFn(($event.target as HTMLInputElement).value)"
          />
          <span v-if="fieldError('fn')" class="cell-err-msg">{{ fieldError('fn') }}</span>
          <span v-if="selectedHint" class="field-hint">{{ selectedHint }}</span>
        </div>
        <div class="field-row col">
          <span class="field-label">参数</span>
          <span v-if="targetOptions" class="field-hint">选定函数后按其 Schema 自动生成（默认值已预填，必填项需补齐）；位置值可直接填单值</span>
          <span v-if="fieldError('args')" class="cell-err-msg">{{ fieldError('args') }}</span>
          <!-- 位置值形态 -->
          <div v-if="step.args.kind === 'value'" class="arg-row">
            <span class="field-label">值</span>
            <CellEditor
              :cell="step.args.cell" type="expr" :params="params" :templates="templates"
              label="参数值" @change="(c) => updateValueArg(c)"
            />
            <button type="button" class="mini-btn" title="删除实参" @click.stop="clearArgs">✕</button>
          </div>
          <!-- 命名参数形态 -->
          <template v-else-if="step.args.kind === 'map'">
            <div v-for="name in argNames" :key="name" class="arg-row">
              <input
                class="cell-input" :value="name" aria-label="参数名" placeholder="参数名"
                @change="renameArg(name, ($event.target as HTMLInputElement).value)"
              />
              <CellEditor
                :cell="step.args.entries[name]" :type="argType(name)" :params="params"
                :templates="templates"
                :label="`参数 ${name}`" @change="(c) => updateArgValue(name, c)"
              />
              <button type="button" class="mini-btn" title="删除实参" @click.stop="removeArg(name)">✕</button>
            </div>
          </template>
          <div class="arg-actions">
            <button type="button" class="mini-btn add" title="添加命名参数" @click.stop="addArg">+ 参数</button>
            <button
              v-if="step.args.kind !== 'value'" type="button" class="mini-btn add" title="切换单值形态（单参数函数简写）"
              @click.stop="toValueArg"
            >单值</button>
            <button
              v-if="step.args.kind === 'value'" type="button" class="mini-btn add" title="切换命名参数形态"
              @click.stop="toMapArg"
            >命名参数</button>
          </div>
        </div>
        <div class="field-row">
          <label class="field-check" title="把函数返回值存入变量（无返回值函数存 null）">
            <input type="checkbox" :checked="step.as !== null" @change="toggleAs" />
            接收返回值
          </label>
          <input
            v-if="step.as !== null" class="cell-input" :value="step.as ?? ''" placeholder="变量名，如 home"
            aria-label="返回值变量名" @change="setAs(($event.target as HTMLInputElement).value)"
          />
        </div>
      </template>

      <!-- if -->
      <template v-else-if="step.kind === 'if'">
        <div class="field-row">
          <span class="field-label">条件</span>
          <CellEditor :cell="step.cond" type="expr" :params="params" label="条件" :error="fieldError('cond')" @change="(c) => updateCell('cond', c)" />
          <span class="field-hint">false/null 为假，非空结果为真；比较用 eq/gt 等函数</span>
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('then')" :base-path="subBase('then')"
          label="如果为真" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
          label="如果为假" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
      </template>

      <!-- repeat -->
      <template v-else-if="step.kind === 'repeat'">
        <div class="field-row">
          <span class="field-label">次数</span>
          <CellEditor :cell="step.times" type="expr" :params="params" label="次数" :error="fieldError('times')" @change="(c) => updateCell('times', c)" />
          <span class="field-hint">零或正整数；可用 $count 引用参数</span>
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('body')" :base-path="subBase('body')"
          label="循环体" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
      </template>

      <!-- return -->
      <template v-else-if="step.kind === 'return'">
        <div class="field-row">
          <span class="field-label">返回值</span>
          <CellEditor :cell="step.value" type="expr" :params="params" label="返回值" :error="fieldError('value')" @change="(c) => updateCell('value', c)" />
          <span class="field-hint">结束当前脚本/函数并返回该值</span>
        </div>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 步骤卡片（V1）：函数调用 / if / repeat / return 四类。
 * - 收起态 = 自然语言摘要（kinds.stepSummary）；
 * - 展开态 = 该类型强类型控件；字段错误按 Diagnostic.field 标红定位；
 * - 函数调用卡：函数下拉（宿主注入 SE_TARGET_OPTIONS）+ Schema 驱动的命名参数 +
 *   as 接收返回值；
 * - if/repeat 的分支子流程内嵌 BranchContainer（一层内嵌、更深专注）。
 * 纯受控组件：所有写操作构造 Command 提交 stack，自身不改模型。
 */
import { computed, inject, ref, type PropType } from 'vue'
import type { Path } from '../commands'
import { resolveStepList } from '../commands'
import type { Diagnostic } from '../diagnostics'
import { joinStepPath } from '../diagnostics'
import { childContainerPath } from '../selection'
import type { Cell, CallArgs, ParamDecl, Step } from '../model'
import {
  postRemovalIndex,
  clearActiveStepDrag,
  getActiveStepDrag,
  readStepDragPayload,
  writeStepDragPayload,
  type StepDragPayload,
} from '../step-dnd'
import { SE_TARGET_OPTIONS, type SeTargetOptions } from '../targets'
import { KIND_META, stepSummary } from './kinds'
import CellEditor from './CellEditor.vue'
import BranchContainer from './BranchContainer.vue'

const props = defineProps({
  model: { type: Object as PropType<Parameters<typeof resolveStepList>[0]>, required: true },
  stack: { type: Object as PropType<{ apply: (c: unknown, n?: string) => boolean }>, required: true },
  step: { type: Object as PropType<Step>, required: true },
  /** 宿主容器路径（move/duplicate/delete 按此寻址）。 */
  containerPath: { type: Array as PropType<Path>, required: true },
  /** step_path 字符串基（诊断定位）。 */
  basePath: { type: String, required: true },
  index: { type: Number, required: true },
  /** 0 = 根层卡片；1 = 一层内嵌分支内卡片。 */
  depth: { type: Number, default: 0 },
  diagnostics: { type: Array as PropType<Diagnostic[]>, default: () => [] },
  selectedUuid: { type: String, default: null },
  /** 诊断定位瞬态高亮的目标 uuid（区别于选中态；非本卡片时无效果）。 */
  highlightUuid: { type: String, default: null },
  /** 画布托管的展开集合；null（独立挂载）时用组件内部状态。 */
  expandedUuids: { type: Object as PropType<Set<string> | null>, default: null },
  params: { type: Array as PropType<ParamDecl[]>, default: () => [] },
  templates: { type: Array as PropType<string[]>, default: () => [] },
  /** 显示「从此步骤测试函数」入口（函数库测试；仅函数体顶层容器由宿主开启）。 */
  testFrom: { type: Boolean, default: false },
})

const emit = defineEmits(['select', 'toggle-expand', 'focus', 'add-here', 'test-from'])

const meta = computed(() => KIND_META[props.step.kind])
const summary = computed(() => stepSummary(props.step))
const stepPath = computed(() => joinStepPath(props.basePath, props.index))
const selected = computed(() => props.selectedUuid === props.step.uuid)
const highlighted = computed(() => props.highlightUuid === props.step.uuid)
const managedExpand = computed(() => props.expandedUuids !== null)
const localExpanded = ref(false)
const expanded = computed(() =>
  managedExpand.value ? (props.expandedUuids as Set<string>).has(props.step.uuid) : localExpanded.value,
)
function onToggleExpand(): void {
  emit('toggle-expand', props.step.uuid)
  if (!managedExpand.value) localExpanded.value = !localExpanded.value
}
const listLength = computed(() => resolveStepList(props.model, props.containerPath).length)
const ownErrors = computed(() => props.diagnostics.filter((d) => d.step_path === stepPath.value))

// ---------- 步骤拖放排序 ----------

const dragging = ref(false)
const dropPosition = ref<'before' | 'after' | null>(null)

function clearDropPosition(): void {
  dropPosition.value = null
}

function onDragStart(event: DragEvent): void {
  if (!event.dataTransfer) return
  const payload: StepDragPayload = {
    uuid: props.step.uuid,
    path: [...props.containerPath],
    index: props.index,
  }
  writeStepDragPayload(event.dataTransfer, payload)
  dragging.value = true
}

function onDragEnd(): void {
  dragging.value = false
  clearDropPosition()
  clearActiveStepDrag()
}

function dragPayload(event: DragEvent): StepDragPayload | null {
  return readStepDragPayload(event.dataTransfer) ?? getActiveStepDrag()
}

function onDragOver(event: DragEvent): void {
  const source = dragPayload(event)
  if (!source || source.uuid === props.step.uuid) {
    clearDropPosition()
    return
  }
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
  dropPosition.value = event.clientY < rect.top + rect.height / 2 ? 'before' : 'after'
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'move'
}

function onDragLeave(event: DragEvent): void {
  const current = event.currentTarget as HTMLElement
  const next = event.relatedTarget
  if (next instanceof Node && current.contains(next)) return
  clearDropPosition()
}

function onDrop(event: DragEvent): void {
  const source = dragPayload(event)
  const position = dropPosition.value
  clearDropPosition()
  if (!source || !position || source.uuid === props.step.uuid) return
  const toIndex = postRemovalIndex(source, props.containerPath, props.index, position === 'before')
  props.stack.apply(
    {
      type: 'move_step',
      from: { path: source.path, index: source.index },
      to: { path: [...props.containerPath], index: toIndex },
    },
    '拖动步骤',
  )
}

function fieldError(field: string): string {
  return ownErrors.value.find((d) => d.field === field)?.message ?? ''
}

// ---------- 命令提交 ----------

function updateStep(fields: Record<string, unknown>): boolean {
  return props.stack.apply({ type: 'update_step', path: [...props.containerPath, props.index], fields }, `编辑 ${meta.value.label}`)
}

function updateCell(field: string, cell: Cell): void {
  updateStep({ [field]: cell })
}

function moveBy(dir: -1 | 1): void {
  props.stack.apply(
    { type: 'move_step', from: { path: props.containerPath, index: props.index }, to: { path: props.containerPath, index: props.index + dir } },
    dir < 0 ? '上移步骤' : '下移步骤',
  )
}
function duplicate(): void {
  props.stack.apply({ type: 'duplicate_step', path: props.containerPath, index: props.index }, '复制步骤')
}
function remove(): void {
  const wasSelected = selected.value
  props.stack.apply({ type: 'remove_step', path: props.containerPath, index: props.index }, '删除步骤')
  if (wasSelected) emit('select', null)
}

// ---------- 分支子容器 ----------

function subPath(key: string): Path {
  return childContainerPath(props.containerPath, props.index, key)
}
function subBase(key: string): string {
  return `${stepPath.value}.${key}`
}

// ---------- 函数调用：函数下拉 + Schema 驱动参数 + as ----------

const targetOptions = inject<SeTargetOptions | null>(SE_TARGET_OPTIONS, null)
const allTargets = computed(() => targetOptions?.targets ?? [])
const targetGroups = computed(() => {
  const plugin = allTargets.value.filter((o) => o.group === 'plugin')
  const pkg = allTargets.value.filter((o) => o.group !== 'plugin')
  return [
    { id: 'plugin', label: '插件函数', options: plugin },
    { id: 'package', label: '配置包函数', options: pkg },
  ].filter((g) => g.options.length > 0)
})
const selectedHint = computed(() => allTargets.value.find((o) => o.target === props.step.fn)?.hint ?? '')

/**
 * 下发函数名；宿主注入了解析器时一并按 Schema 重生成实参（默认值预填），
 * 单条 update_step = 一次撤销。await 期间函数若又被改动则放弃（由最新一次变更接管）。
 */
async function applyFn(next: string): Promise<void> {
  if (!next) {
    updateStep({ fn: '', args: { kind: 'none' } })
    return
  }
  if (!targetOptions) {
    updateStep({ fn: next })
    return
  }
  const prev = String(props.step.fn ?? '')
  let decls: ParamDecl[] | null = null
  try {
    decls = await targetOptions.resolveParams(next)
  } catch {
    decls = null // 解析失败不阻塞改函数：实参保持原样（校验层兜底）
  }
  if (String(props.step.fn ?? '') !== prev) return
  try {
    updateStep(decls ? { fn: next, args: argsFromDecls(decls) } : { fn: next })
  } catch {
    // await 期间步骤已被删除（resolveStep 抛错）——放弃本次下发
  }
}

/** 按函数 Schema 生成实参：有默认值填默认值；必填且无默认的参数留待用户补。 */
function argsFromDecls(decls: ParamDecl[]): CallArgs {
  const entries: Record<string, Cell> = {}
  for (const d of decls) {
    if (d.default !== null && d.default !== undefined) entries[d.name] = { lit: d.default }
  }
  return Object.keys(entries).length > 0 ? { kind: 'map', entries } : { kind: 'none' }
}

const argNames = computed<string[]>(() =>
  props.step.kind === 'call' && props.step.args.kind === 'map' ? Object.keys(props.step.args.entries) : [],
)

function argType(name: string): string {
  if (!targetOptions?.resolveParamsSync) return 'text'
  const decls = targetOptions.resolveParamsSync(props.step.fn)
  return decls?.find((d) => d.name === name)?.type ?? 'text'
}
function argsRecord(): Record<string, Cell> {
  return props.step.kind === 'call' && props.step.args.kind === 'map' ? { ...props.step.args.entries } : {}
}
function updateArgs(entries: Record<string, Cell>): void {
  updateStep({ args: { kind: 'map', entries } })
}
function updateValueArg(cell: Cell): void {
  updateStep({ args: { kind: 'value', cell } })
}
function updateArgValue(name: string, cell: Cell): void {
  updateArgs({ ...argsRecord(), [name]: cell })
}
function removeArg(name: string): void {
  const next = argsRecord()
  delete next[name]
  updateArgs(next)
}
function renameArg(oldName: string, raw: string): void {
  const name = raw.trim()
  if (!name || name === oldName) return
  const current = argsRecord()
  if (name in current) return // 重复键直接忽略
  const next: Record<string, Cell> = {}
  for (const [k, v] of Object.entries(current)) next[k === oldName ? name : k] = v as Cell
  updateArgs(next)
}
function addArg(): void {
  const current = argsRecord()
  const decls = targetOptions?.resolveParamsSync?.(props.step.fn)
  // 优先补 Schema 中尚未填写的参数；否则用 paramN 占位
  const missing = decls?.find((d) => !(d.name in current) && d.default === null)
  if (missing) {
    updateArgs({ ...current, [missing.name]: { lit: emptyLitFor(missing.type) } })
    return
  }
  let i = 1
  while (`param${i}` in current) i++
  updateArgs({ ...current, [`param${i}`]: { lit: '' } })
}
function clearArgs(): void {
  updateStep({ args: { kind: 'none' } })
}
function toValueArg(): void {
  updateStep({ args: { kind: 'value', cell: { lit: '' } } })
}
function toMapArg(): void {
  updateStep({ args: { kind: 'map', entries: {} } })
}
function emptyLitFor(type: string): unknown {
  switch (type) {
    case 'boolean': return false
    case 'integer': case 'number': return 0
    default: return ''
  }
}
function toggleAs(e: Event): void {
  updateStep({ as: (e.target as HTMLInputElement).checked ? '' : null })
}
function setAs(v: string): void {
  updateStep({ as: v.trim() ? v.trim() : null })
}
</script>

<style scoped>
.step-card {
  position: relative;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  margin: 6px 0;
  overflow: visible;
}
.step-card.dragging { opacity: .45; }
.step-card.drop-before::before,
.step-card.drop-after::after {
  content: '';
  position: absolute;
  left: 6px;
  right: 6px;
  height: 3px;
  border-radius: 3px;
  background: var(--accent);
  box-shadow: 0 0 6px color-mix(in srgb, var(--accent) 70%, transparent);
  pointer-events: none;
  z-index: 2;
}
.step-card.drop-before::before { top: -5px; }
.step-card.drop-after::after { bottom: -5px; }
.step-card.selected { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
.step-card.has-error { border-color: var(--danger); }
.step-card.card-highlight { border-color: var(--warn); box-shadow: 0 0 0 2px var(--warn); }

.card-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  cursor: pointer;
  min-height: 32px;
}
.card-head:hover { background: var(--bg-3); }
.drag-handle {
  color: var(--text-2); cursor: grab; font-size: 11px; letter-spacing: -2px;
  user-select: none; touch-action: none;
}
.drag-handle:active { cursor: grabbing; }
.kind-icon {
  display: inline-flex; align-items: center; justify-content: center;
  width: 20px; height: 20px; border-radius: 4px;
  background: var(--bg-3); color: var(--accent); font-size: 12px; flex: none;
}
.kind-name { font-weight: 600; font-size: 13px; white-space: nowrap; }
.step-no { color: var(--text-2); font-size: 11px; font-family: var(--mono); white-space: nowrap; }
.summary {
  color: var(--text-1); font-size: 12px;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0;
}
.err-badge {
  background: var(--danger); color: #fff; font-size: 10px; line-height: 1;
  border-radius: 8px; padding: 3px 6px; flex: none; cursor: help;
}
.head-actions { display: inline-flex; gap: 3px; flex: none; }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 11px; padding: 2px 6px; cursor: pointer; line-height: 1.3;
}
.mini-btn:hover:not(:disabled) { color: var(--accent); border-color: var(--accent); }
.mini-btn:disabled { opacity: .35; cursor: not-allowed; }
.mini-btn.danger:hover:not(:disabled) { color: var(--danger); border-color: var(--danger); }
.mini-btn.add { color: var(--accent-2); }
.mini-btn.test-from { color: var(--accent); }
.mini-btn.test-from:hover { background: var(--accent); color: #06251c; }

.card-body { padding: 4px 10px 10px 32px; display: flex; flex-direction: column; gap: 4px; }
.field-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.field-row.col { flex-direction: column; align-items: flex-start; gap: 4px; }
.field-label { font-size: 12px; color: var(--text-2); min-width: 52px; flex: none; }
.field-check { display: inline-flex; align-items: center; gap: 4px; font-size: 12px; color: var(--text-1); cursor: pointer; }
.field-hint { font-size: 12px; color: var(--text-2); }
.field-hint.warn { color: var(--warn); }
.cell-err-msg { font-size: 11px; color: var(--danger); }
.arg-row { display: flex; align-items: center; gap: 6px; }
.arg-actions { display: flex; gap: 6px; }
.cell-input.num { width: 74px; }
.target-select {
  background: var(--bg-2); color: var(--text-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 3px 6px; font-size: 12px; min-width: 60px; max-width: 200px;
}
.target-select:focus { outline: none; border-color: var(--accent); }
.target-select option { background: var(--bg-1); color: var(--text-0); }
.mono { font-family: var(--mono); }
</style>
