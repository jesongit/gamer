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
    <div class="card-head" @click.stop="onToggleExpand">
      <span
        class="drag-handle" title="拖动排序" draggable="true" role="button" aria-label="拖动排序"
        @dragstart.stop="onDragStart" @dragend.stop="onDragEnd" @click.stop
      >⋮⋮</span>
      <button type="button" class="expand-btn" :title="expanded ? '收起' : '展开编辑'"
        :aria-expanded="expanded" @click.stop="onToggleExpand">
      <span class="kind-icon" :title="meta.hint">{{ meta.icon }}</span>
      <span class="kind-name" :title="caption.title">{{ caption.title }}</span>
      <span class="step-no">{{ String(index + 1).padStart(2, '0') }}</span>
      <span class="summary" :title="summary">{{ caption.detail }}</span>
      <span v-if="ownErrors.length" class="err-badge" :title="ownErrors.map((d) => d.message).join('\n')">
        {{ ownErrors.length }}
      </span>
      </button>
      <span class="head-actions">
        <button v-if="canJumpToFunction" type="button" class="mini-btn jump-function"
          :disabled="functionNavigation?.busy" title="保存当前编辑并跳转到函数定义" aria-label="跳转到函数定义"
          @click.stop="jumpToFunction">跳转</button>
        <!-- 脚本/函数顶层运行入口；编辑时保留，继续沿用宿主运行校验。 -->
        <button
          v-if="testFrom" type="button" class="mini-btn test-from"
          title="从此步骤运行"
          @click.stop="emit('test-from', step.uuid)"
         aria-label="从此步骤运行"><UiIcon name="play" /><span>运行</span></button>
        <button type="button" class="mini-btn" title="复制步骤" @click.stop="duplicate" aria-label="复制步骤"><UiIcon name="copy" /><span>复制</span></button>
        <button type="button" class="mini-btn danger" title="删除步骤" aria-label="删除步骤" @click.stop="remove"><UiIcon name="trash" /><span>删除</span></button>
      </span>
    </div>

    <!-- 展开态：按类型的强类型控件（不提供任意键值编辑器） -->
    <div v-if="expanded" class="card-body" @click.stop>
      <!-- 函数调用 -->
      <template v-if="step.kind === 'call'">
        <div class="field-row call-signature">
          <span class="field-label">函数</span>
          <template v-if="targetOptions">
            <select
              class="cell-input target-select" :value="step.fn" :title="selectedHint" aria-label="函数"
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
          <div class="return-value-field">
            <label class="return-value-label">
              <span>返回值</span>
              <input class="cell-input mono" :value="step.as ?? ''" placeholder="留空不接收"
                title="接收返回值的变量名，如 hit；留空不接收" aria-label="返回值变量名"
                :aria-invalid="!!fieldError('as')" @change="setAs(($event.target as HTMLInputElement).value)" />
            </label>
            <button v-if="step.as !== null" type="button" class="mini-btn" title="清除返回值变量" aria-label="清除返回值变量" @click="setAs('')"><UiIcon name="close" /></button>
          </div>
          <span v-if="fieldError('fn')" class="cell-err-msg">{{ fieldError('fn') }}</span>
          <span v-if="fieldError('as')" class="cell-err-msg">{{ fieldError('as') }}</span>
        </div>
        <div class="field-row col">

          <span v-if="fieldError('args')" class="cell-err-msg">{{ fieldError('args') }}</span>
          <template v-if="paramSchema">
            <div v-for="name in requiredArgNames" :key="name" class="arg-row" :data-arg-name="name">
              <span class="arg-name mono" :title="paramSchema.find(p => p.name === name)?.desc">{{ name }} <span class="required-dot" aria-label="必填"></span></span>
              <CellEditor :cell="schemaArgCell(name)" :argument-name="name" :type="argType(name)" :item-type="paramSchema?.find(p => p.name === name)?.items?.type" :params="params" :templates="templates" :label="`参数 ${name}`" @change="(c) => updateSchemaArg(name, c)" />
              <button v-if="canRemoveSchemaArg(name)" type="button" class="mini-btn" :title="hasDefaultFor(name) ? '恢复默认值' : '删除实参'" :aria-label="hasDefaultFor(name) ? `恢复 ${name} 默认值` : `删除参数 ${name}`" @click.stop="removeArg(name)">✕</button>
            </div>
            <span v-if="!requiredArgNames.length" class="field-hint">无必填参数</span>
            <details class="optional-params">
              <summary>更多参数 · 已设置 {{ configuredOptionalCount }} 项</summary>
              <div class="param-buttons" role="group" aria-label="函数参数">
                <button v-for="param in paramSchema.filter(p => !p.required)" :key="param.name" type="button" class="param-button" :class="{ active: paramActive(param) }" :data-param="param.name" :aria-pressed="paramActive(param)" :title="paramButtonTitle(param)" @click.stop="toggleParam(param, $event)"><span class="mono">{{ param.name }}</span><span class="param-default">{{ paramDefaultLabel(param) }}</span></button>
              </div>
              <div v-for="name in optionalArgNames" :key="name" class="arg-row" :data-arg-name="name">
                <span class="arg-name mono">{{ name }}</span>
                <CellEditor :cell="schemaArgCell(name)" :argument-name="name" :type="argType(name)" :item-type="paramSchema?.find(p => p.name === name)?.items?.type" :params="params" :templates="templates" :label="`参数 ${name}`" :optional="isUnfilledOptional(name)" @change="(c) => updateSchemaArg(name, c)" /><span v-if="isUnfilledOptional(name)" class="field-hint">可选，留空不传</span>
                <button v-if="canRemoveSchemaArg(name)" type="button" class="mini-btn" :title="hasDefaultFor(name) ? '恢复默认值' : '删除实参'" :aria-label="hasDefaultFor(name) ? `恢复 ${name} 默认值` : `删除参数 ${name}`" @click.stop="removeArg(name)">✕</button>
              </div>
            </details>
          </template>
          <template v-else>
          <!-- 位置值形态 -->
          <div v-if="step.args.kind === 'value'" class="arg-row">
            <span class="field-label">值</span>
            <CellEditor
              :cell="step.args.cell" :type="firstParam?.type || 'expr'" :params="params" :templates="templates"
              label="参数值" @change="(c) => updateValueArg(c)"
            />
            <button type="button" class="mini-btn" title="删除实参" @click.stop="clearArgs">✕</button>
          </div>
          <!-- 命名参数形态 -->
          <template v-else-if="step.args.kind === 'map'">
            <div v-for="name in argNames" :key="name" class="arg-row">
              <span v-if="isPluginFunction" class="arg-name mono" :title="`参数 ${name}`">{{ name }}</span>
              <input v-else
                class="cell-input" :value="name" aria-label="参数名" placeholder="参数名"
                @change="renameArg(name, ($event.target as HTMLInputElement).value)"
              />
              <CellEditor
                :cell="step.args.entries[name]" :argument-name="name" :type="argType(name)" :item-type="paramSchema?.find(p => p.name === name)?.items?.type" :params="params"
                :templates="templates"
                :label="`参数 ${name}`" @change="(c) => updateArgValue(name, c)"
              />
              <button type="button" class="mini-btn" title="删除实参" @click.stop="removeArg(name)">✕</button>
            </div>
          </template>
          <div class="arg-actions">
            <button type="button" class="mini-btn add" title="添加命名参数" :disabled="isPluginFunction && !missingPluginParam" @click.stop="addArg">+ 参数</button>
            <button
              v-if="step.args.kind !== 'value'" type="button" class="mini-btn add" title="切换单值形态（单参数函数简写）"
              @click.stop="toValueArg"
            >单值</button>
            <button
              v-if="step.args.kind === 'value'" type="button" class="mini-btn add" title="切换命名参数形态"
              @click.stop="toMapArg"
            >命名参数</button>
          </div>
          </template>
          <span v-if="conversionNotice" class="field-hint args-conversion-notice" role="status">{{ conversionNotice }}</span>
        </div>
      </template>

      <template v-else-if="step.kind === 'match_templates'">
        <p class="field-hint">从上到下匹配同一帧，只执行首个命中的分支；模板不会自动点击。</p>
        <div class="field-row">
          <span class="field-label">匹配阈值</span>
          <CellEditor :cell="step.threshold" type="number" :params="params" label="匹配阈值" :error="fieldError('threshold')" @change="c => updateCell('threshold', c)" />
        </div>
        <section v-for="(branch, n) in step.cases" :key="n" class="template-case" :data-template-case="n">
          <div class="template-case-head">
            <strong>分支 {{ n + 1 }}</strong>
            <button type="button" class="mini-btn" :disabled="n === 0" :aria-label="`上移模板分支 ${n + 1}`" @click="moveTemplateCase(n, -1)">↑ 上移</button>
            <button type="button" class="mini-btn" :disabled="n === step.cases.length - 1" :aria-label="`下移模板分支 ${n + 1}`" @click="moveTemplateCase(n, 1)">↓ 下移</button>
            <button type="button" class="mini-btn danger" :disabled="step.cases.length <= 1" :aria-label="`删除模板分支 ${n + 1}`" @click="removeTemplateCase(n)">删除分支</button>
          </div>
          <div class="field-row">
            <span class="field-label">模板</span>
            <CellEditor :cell="branch.template" type="template" :params="params" :templates="templates" :label="`分支 ${n + 1} 模板`" :error="fieldError(`cases[${n}].template`)" @change="c => updateTemplateCase(n, { template: c })" />
          </div>
          <div class="field-row">
            <label class="field-label" :for="`${step.uuid}-case-${n}`">匹配结果</label>
            <input :id="`${step.uuid}-case-${n}`" class="cell-input mono" :value="branch.as ?? ''" placeholder="留空不接收，如 hit" :aria-label="`分支 ${n + 1} 匹配结果`" @change="updateTemplateCase(n, { as: ($event.target as HTMLInputElement).value.trim() || null })" />
            <span class="field-hint">变量仅在本分支内有效</span>
            <span v-if="fieldError(`cases[${n}].as`)" class="cell-err-msg">{{ fieldError(`cases[${n}].as`) }}</span>
          </div>
          <BranchContainer :model="model" :stack="stack" :container-path="subPath(`cases[${n}].do`)" :base-path="subBase(`cases[${n}].do`)"
            label="命中后执行" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid" :highlight-uuid="highlightUuid"
            :expanded-uuids="expandedUuids" :params="templateCaseParams(branch.as)" :templates="templates"
            @select="u => emit('select', u)" @toggle-expand="u => emit('toggle-expand', u)"
            @focus="p => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)" />
        </section>
        <button type="button" class="mini-btn add" :disabled="step.cases.length >= 64" aria-label="添加模板分支" @click="addTemplateCase">+ 添加模板分支</button>
        <BranchContainer :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
          label="全部未命中（可选）" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid" :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :templates="templates"
          @select="u => emit('select', u)" @toggle-expand="u => emit('toggle-expand', u)"
          @focus="p => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)" />
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

      <p v-else-if="step.kind === 'break'">退出最近一层循环，继续执行循环后面的步骤。</p>
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
import UiIcon from '../../../../../../web/src/components/ui/UiIcon.vue'
import { computed, inject, provide, ref, watch, type PropType } from 'vue'
import type { Path } from '../commands'
import { resolveStepList } from '../commands'
import type { Diagnostic } from '../diagnostics'
import { joinStepPath } from '../diagnostics'
import { childContainerPath } from '../selection'
import type { Cell, CallArgs, ParamDecl, Step } from '../model'
import { initializeArgsFromSchema } from '../factories'
import { cloneSchemaValue, hasParamDefault } from '../schema'
import { missingLit } from '../model'
import {
  postRemovalIndex,
  clearActiveStepDrag,
  getActiveStepDrag,
  readStepDragPayload,
  writeStepDragPayload,
  type StepDragPayload,
} from '../step-dnd'
import { SE_FUNCTION_NAVIGATION, SE_TARGET_OPTIONS, SE_TEMPLATE_MATCH_OPTIONS, type SeTargetOptions, type TemplateMatchOptions } from '../targets'
import { KIND_META, stepCaption } from './kinds'
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
const caption = computed(() => stepCaption(props.step, props.step.kind === 'call'
  ? paramSchema.value?.find(p => p.name === 'name')?.default as string | undefined : undefined))
const summary = computed(() => `${caption.value.title}${caption.value.detail ? ' · ' + caption.value.detail : ''}`)
const stepPath = computed(() => joinStepPath(props.basePath, props.index))
const selected = computed(() => props.selectedUuid === props.step.uuid)
const highlighted = computed(() => props.highlightUuid === props.step.uuid)
const managedExpand = computed(() => props.expandedUuids !== null)
const localExpanded = ref(false)
const expanded = computed(() =>
  managedExpand.value ? (props.expandedUuids as Set<string>).has(props.step.uuid) : localExpanded.value,
)
function onToggleExpand(): void {
  emit('select', props.step.uuid)
  emit('toggle-expand', props.step.uuid)
  if (!managedExpand.value) localExpanded.value = !localExpanded.value
}
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

const conversionNotice = ref('')

function updateStep(fields: Record<string, unknown>): boolean {
  conversionNotice.value = ''
  return props.stack.apply({ type: 'update_step', path: [...props.containerPath, props.index], fields }, `编辑 ${meta.value.label}`)
}

function updateTemplateCase(index: number, fields: Record<string, unknown>): void {
  if (props.step.kind !== 'match_templates') return
  props.stack.apply({ type: 'update_template_case', path: [...props.containerPath, props.index], index, fields }, '编辑模板分支')
}
function setTemplateCases(cases: Extract<Step, {kind: 'match_templates'}>['cases']): void {
  props.stack.apply({ type: 'set_template_cases', path: [...props.containerPath, props.index], cases }, '调整模板分支')
}
function addTemplateCase(): void {
  if (props.step.kind !== 'match_templates' || props.step.cases.length >= 64) return
  setTemplateCases([...props.step.cases, { template: { lit: '' }, as: null, body: [] }])
}
function removeTemplateCase(index: number): void {
  if (props.step.kind !== 'match_templates' || props.step.cases.length <= 1) return
  setTemplateCases(props.step.cases.filter((_, i) => i !== index))
}
function moveTemplateCase(index: number, offset: number): void {
  if (props.step.kind !== 'match_templates') return
  const cases = [...props.step.cases]
  const to = index + offset
  if (to < 0 || to >= cases.length) return
  ;[cases[index], cases[to]] = [cases[to], cases[index]]
  setTemplateCases(cases)
}
function templateCaseParams(name: string | null): ParamDecl[] {
  return name ? [...props.params.filter(p => p.name !== name), { name, type: 'object', required: false, default: null, desc: '本分支匹配结果，可用 center/score/template' }] : props.params
}

function updateCell(field: string, cell: Cell): void {
  updateStep({ [field]: cell })
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
const functionNavigation = inject(SE_FUNCTION_NAVIGATION, null)
const canJumpToFunction = computed(() => {
  if (!functionNavigation || props.step.kind !== 'call') return false
  const name = props.step.fn
  return allTargets.value.some(o => o.target === name && o.group === 'package')
    || ('functions' in props.model && props.model.functions.some(fn => fn.name === name))
})
function jumpToFunction(): void {
  if (props.step.kind === 'call' && !functionNavigation?.busy) void functionNavigation?.open(props.step.fn, props.step.uuid)
}
const targetGroups = computed(() => {
  const plugin = allTargets.value.filter((o) => o.group === 'plugin')
  const pkg = allTargets.value.filter((o) => o.group !== 'plugin')
  return [
    { id: 'plugin', label: '插件函数', options: plugin },
    { id: 'package', label: '配置包函数', options: pkg },
  ].filter((g) => g.options.length > 0)
})
const selectedTarget = computed(() => allTargets.value.find((o) => o.target === props.step.fn))
const selectedHint = computed(() => selectedTarget.value?.hint ?? '')
const isPluginFunction = computed(() => selectedTarget.value?.group === 'plugin')
const resolvedSchema = ref<{ fn: string; params: ParamDecl[] } | null>(null)
const paramSchema = computed(() => targetOptions?.resolveParamsSync?.(props.step.fn)
  ?? (resolvedSchema.value && resolvedSchema.value.fn === props.step.fn ? resolvedSchema.value.params : null))
const firstParam = computed(() => paramSchema.value?.[0])
// 打开已有步骤时只加载参数声明，不改写已经保存的值。
watch(expanded, async open => {
  const fn = props.step.fn
  if (!open || !fn || !targetOptions || paramSchema.value) return
  try {
    const params = await targetOptions.resolveParams(fn)
    if (params && props.step.fn === fn) resolvedSchema.value = { fn, params }
  } catch { /* 保留已有实参，等待宿主目录恢复。 */ }
}, { immediate: true })
const openedOptionalParams = ref(new Set<string>())
provide(SE_TEMPLATE_MATCH_OPTIONS, (argumentName): TemplateMatchOptions => {
  const step = props.step
  const args = step.kind === 'call' && step.args.kind === 'map' ? step.args.entries : {}
  const thresholdCell = step.kind === 'match_templates' ? step.threshold : args.threshold
  // Obstacles inherit the threshold, but use their own file's search region.
  const regionCell = argumentName === 'obstacles' ? undefined : args.region
  if (thresholdCell?.ref || regionCell?.ref) return { error: '匹配阈值或区域引用运行时变量，请运行该步骤验证，单次测试无法确定变量值。' }
  const threshold = thresholdCell && !thresholdCell.missing ? thresholdCell.lit
    : paramSchema.value?.find(p => p.name === 'threshold')?.default ?? 0.8
  if (typeof threshold !== 'number' || !Number.isFinite(threshold) || threshold < 0 || threshold > 1) return { error: '请先填写有效的匹配阈值（0～1）。' }
  const region = regionCell && !regionCell.missing ? regionCell.lit ?? undefined : undefined
  if (region !== undefined && (!Array.isArray(region) || region.length !== 4 || region.some(v => typeof v !== 'number' || !Number.isFinite(v) || v < 0 || v > 1) || region[2] <= 0 || region[3] <= 0)) return { error: '请先填写有效的匹配区域 [x, y, 宽, 高]（相对坐标）。' }
  return { threshold, region: region as number[] | undefined }
})
watch(() => [props.step.uuid, props.step.fn], () => { openedOptionalParams.value = new Set() })
const schemaArgNames = computed(() => {
  const current = argsRecord()
  const declared = paramSchema.value ?? []
  return [...declared.filter(paramActive).map(p => p.name),
    ...Object.keys(current).filter(name => !declared.some(p => p.name === name))]
})
const requiredArgNames = computed(() => (paramSchema.value ?? []).filter(p => p.required).map(p => p.name))
const optionalArgNames = computed(() => schemaArgNames.value.filter(name => !requiredArgNames.value.includes(name)))
const configuredOptionalCount = computed(() => Object.keys(argsRecord()).filter(name => !requiredArgNames.value.includes(name)).length)
function paramActive(param: ParamDecl): boolean {
  return (param.required && !hasParamDefault(param)) || param.name in argsRecord() || openedOptionalParams.value.has(param.name)
}
function paramDefaultLabel(param: ParamDecl): string {
  return hasParamDefault(param) ? `${param.name in argsRecord() ? '默认' : '使用默认值：'} ${JSON.stringify(param.default)}` : (param.required ? '必填' : '可选')
}
function paramButtonTitle(param: ParamDecl): string {
  const action = hasParamDefault(param)
    ? (paramActive(param) ? '恢复默认值' : '点击自定义')
    : (param.required ? '无默认值，请填写' : (paramActive(param) ? '点击收起，不传此参数' : '可选，无默认值，点击填写'))
  return [param.name, param.desc, paramDefaultLabel(param), action].filter(Boolean).join(' · ')
}
function toggleParam(param: ParamDecl, event: Event): void {
  if (!param.required && !hasParamDefault(param)) {
    if (paramActive(param)) removeArg(param.name)
    else openedOptionalParams.value = new Set([...openedOptionalParams.value, param.name])
    return
  }
  if (!hasParamDefault(param)) {
    const container = (event.currentTarget as HTMLElement).closest('.field-row')
    const row = Array.from(container?.querySelectorAll<HTMLElement>('[data-arg-name]') ?? [])
      .find(el => el.dataset.argName === param.name)
    row?.querySelector<HTMLElement>('input, select, textarea')?.focus()
    return
  }
  if (param.name in argsRecord()) removeArg(param.name)
  else updateArgValue(param.name, { lit: cloneSchemaValue(param.default) })
}
function schemaArgCell(name: string): Cell {
  const param = paramSchema.value?.find(p => p.name === name)
  return argsRecord()[name] ?? (param && hasParamDefault(param) ? { lit: cloneSchemaValue(param.default) } : missingLit(null))
}
function hasDefaultFor(name: string): boolean {
  const param = paramSchema.value?.find(p => p.name === name)
  return !!param && hasParamDefault(param)
}
function isUnfilledOptional(name: string): boolean {
  const param = paramSchema.value?.find(p => p.name === name)
  return !!param && !param.required && !hasParamDefault(param) && !(name in argsRecord())
}
function canRemoveSchemaArg(name: string): boolean {
  const param = paramSchema.value?.find(p => p.name === name)
  return (name in argsRecord() || openedOptionalParams.value.has(name)) && (!param || !param.required || hasParamDefault(param))
}
function updateSchemaArg(name: string, cell: Cell): void {
  if (props.step.kind === 'call' && props.step.args.kind === 'value' && firstParam.value?.name === name) updateValueArg(cell)
  else updateArgValue(name, cell)
}
const missingPluginParam = computed(() => {
  const current = argsRecord()
  return targetOptions?.resolveParamsSync?.(props.step.fn)?.find((d) => !(d.name in current))
})

/**
 * 下发函数名；按声明展开无默认值字段，已有自定义值按同名参数保留，
 * 单条 update_step = 一次撤销。await 期间函数若又被改动则放弃（由最新一次变更接管）。
 */
let fnRequestSeq = 0

async function applyFn(next: string): Promise<void> {
  const requestSeq = ++fnRequestSeq
  if (!next) {
    updateStep({ fn: '', args: { kind: 'none' } })
    return
  }
  // 立即记录当前选择；异步 Schema 只允许补齐最后一次选择的参数。
  if (String(props.step.fn ?? '') !== next) {
    try {
      updateStep({ fn: next })
    } catch {
      return
    }
  }
  if (!targetOptions) {
    return
  }
  let decls: ParamDecl[] | null = null
  try {
    decls = await targetOptions.resolveParams(next)
  } catch {
    decls = null // 解析失败不阻塞改函数：实参保持原样（校验层兜底）
  }
  if (requestSeq !== fnRequestSeq || props.step.kind !== 'call' || props.step.fn !== next) return
  if (!decls) return
  resolvedSchema.value = { fn: next, params: decls }
  try {
    updateStep({ args: mergeArgsWithSchema(props.step.args, decls) })
  } catch {
    // await 期间步骤已被删除（resolveStep 抛错）——放弃本次下发
  }
}

/** Schema 初始化后保留当前函数中仍有对应声明的已编辑值。 */
function mergeArgsWithSchema(current: CallArgs, decls: ParamDecl[]): CallArgs {
  const initialized = initializeArgsFromSchema(decls)
  const entries: Record<string, Cell> = initialized.kind === 'map' ? { ...initialized.entries } : {}
  const declared = new Set(decls.map((d) => d.name))

  if (current.kind === 'value') {
    if (decls.length === 1) return { kind: 'value', cell: current.cell }
    const first = decls[0]
    if (first) entries[first.name] = current.cell
  } else if (current.kind === 'map') {
    for (const [name, cell] of Object.entries(current.entries)) {
      if (declared.has(name)) entries[name] = cell
    }
  }

  return Object.keys(entries).length > 0 ? { kind: 'map', entries } : { kind: 'none' }
}

const argNames = computed<string[]>(() =>
  props.step.kind === 'call' && props.step.args.kind === 'map' ? Object.keys(props.step.args.entries) : [],
)

function argType(name: string): string {
  const decls = paramSchema.value
  return decls?.find((d) => d.name === name)?.type ?? 'text'
}
function argsRecord(): Record<string, Cell> {
  if (props.step.kind === 'call' && props.step.args.kind === 'value' && firstParam.value) {
    return { [firstParam.value.name]: props.step.args.cell }
  }
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
  const opened = new Set(openedOptionalParams.value)
  opened.delete(name)
  openedOptionalParams.value = opened
  const next = argsRecord()
  if (!(name in next)) return
  delete next[name]
  updateArgs(next)
}
function renameArg(oldName: string, raw: string): void {
  if (isPluginFunction.value) return
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
  if (isPluginFunction.value) {
    const param = missingPluginParam.value
    if (param) updateArgs({ ...current, [param.name]: { lit: param.default ?? emptyLitFor(param.type) } })
    return
  }
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
  if (props.step.kind !== 'call' || props.step.args.kind === 'value') return
  if (props.step.args.kind === 'map') {
    const names = Object.keys(props.step.args.entries)
    if (names.length > 1) {
      conversionNotice.value = '多个命名参数不能无损转换为单值，未修改当前参数。'
      return
    }
    const name = names[0]
    if (name && firstParam.value && name !== firstParam.value.name) {
      conversionNotice.value = `单值对应第一个参数 ${firstParam.value.name}，当前参数 ${name} 不能转换。`
      return
    }
    updateStep({ args: { kind: 'value', cell: name ? props.step.args.entries[name]! : { lit: '' } } })
    return
  }
  updateStep({ args: { kind: 'value', cell: { lit: '' } } })
}
function toMapArg(): void {
  if (props.step.kind !== 'call' || props.step.args.kind === 'map') return
  const decls = targetOptions?.resolveParamsSync?.(props.step.fn) ?? []
  const name = decls[0]?.name || 'value'
  const cell = props.step.args.kind === 'value' ? props.step.args.cell : { lit: '' }
  updateStep({ args: { kind: 'map', entries: { [name]: cell } } })
  if (!decls.length) conversionNotice.value = '当前函数 Schema 未加载，暂以 value 保留该单值。'
}
function emptyLitFor(type: string): unknown {
  switch (type) {
    case 'boolean': return false
    case 'integer': case 'number': return 0
    default: return ''
  }
}
function setAs(v: string): void {
  updateStep({ as: v.trim() ? v.trim() : null })
}
</script>

<style scoped>
.template-case { border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 10px; display: grid; gap: 8px; min-width: 0; }
.template-case-head { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; }
.template-case-head strong { margin-right: auto; font-size: 13px; }
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
  color: var(--text-2); cursor: grab; font-size: 12px; letter-spacing: -2px;
  user-select: none; touch-action: none;
}
.drag-handle:active { cursor: grabbing; }
.kind-icon {
  display: inline-flex; align-items: center; justify-content: center;
  width: 20px; height: 20px; border-radius: 4px;
  background: var(--bg-3); color: var(--accent); font-size: 12px; flex: none;
}
.kind-name { font-weight: 600; font-size: 13px; white-space: nowrap; }
.step-no { color: var(--text-2); font-size: 12px; font-family: var(--mono); white-space: nowrap; }
.summary {
  color: var(--text-1); font-size: 12px;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0;
}
.err-badge {
  background: var(--danger); color: #fff; font-size: 12px; line-height: 1;
  border-radius: var(--radius-sm); padding: 3px 6px; flex: none; cursor: help;
}
.head-actions { display: inline-flex; gap: 3px; flex: none; }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 12px; padding: 2px 6px; cursor: pointer; line-height: 1.3;
}
.mini-btn:hover:not(:disabled) { color: var(--accent); border-color: var(--accent); }
.mini-btn:disabled { opacity: .35; cursor: not-allowed; }
.mini-btn.danger:hover:not(:disabled) { color: var(--danger); border-color: var(--danger); }
.mini-btn.add { color: var(--accent-2); }
.mini-btn.test-from { color: var(--accent); }
.mini-btn.test-from:hover { background: var(--accent); color: #202015; }

.card-body { padding: 4px 10px 10px 32px; display: flex; flex-direction: column; gap: 4px; }
.field-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.field-row.col { flex-direction: column; align-items: flex-start; gap: 4px; }
.field-label { font-size: 12px; color: var(--text-2); min-width: 52px; flex: none; }
.field-check { display: inline-flex; align-items: center; gap: 4px; font-size: 12px; color: var(--text-1); cursor: pointer; }
.field-hint { font-size: 12px; color: var(--text-2); }
.field-hint.warn { color: var(--warn); }
.cell-err-msg { font-size: 12px; color: var(--danger); }
.arg-row { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; max-width: 100%; }
.arg-row :deep(.cell-editor) { flex-wrap: wrap; max-width: 100%; }
.arg-row :deep(.cell-mode), .arg-row :deep(.cell-tool) { flex-shrink: 0; white-space: nowrap; }
.param-buttons { display: flex; flex-wrap: wrap; gap: 6px; margin: 2px 0 8px; max-width: 100%; }
.param-button { display: inline-flex; align-items: center; gap: 6px; max-width: 100%; padding: 5px 9px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-2); color: var(--text-2); font-size: 12px; cursor: pointer; }
.param-button:hover, .param-button:focus-visible { border-color: var(--accent); color: var(--text-0); }
.param-button.active { color: var(--accent); border-color: var(--accent); background: color-mix(in srgb, var(--accent) 10%, var(--bg-1)); }
.param-button.needs-value { color: var(--warn); border-color: var(--warn); background: color-mix(in srgb, var(--warn) 8%, var(--bg-1)); }
.param-default { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 180px; font-size: 12px; }
.arg-name { min-width: 72px; color: var(--text-1); font-size: 12px; }
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
.step-card{border:1px solid var(--border);border-left:1px solid var(--border);border-radius:3px;background:var(--bg-2);overflow:visible}.step-card.expanded{background:var(--bg-3)}.card-head{background:transparent;min-height:39px;padding:5px 7px;gap:7px;align-items:center}.step-no{order:-1;display:grid;place-items:center;width:23px;height:23px;flex:0 0 23px;position:static;border:1px solid var(--control-border);border-radius:2px;font-size:12px;color:var(--text-0);align-self:center;margin:0}.kind-icon{display:none}.kind-name,.summary{font-size:13px}.head-actions{gap:2px}.head-actions .mini-btn{width:25px;height:25px;min-width:25px;padding:3px;display:inline-flex;align-items:center;justify-content:center;border-color:transparent;background:transparent}.head-actions .mini-btn:hover{border-color:var(--control-border);background:var(--bg-2)}.card-body{background:transparent;border:0;padding:4px 10px 10px 37px;gap:8px}.field-row{gap:6px}.field-label{min-width:42px}.field-row.col{align-items:stretch}.arg-row{width:100%;margin:3px 0;gap:5px}.arg-row :deep(.cell-editor){flex:1;min-width:0}.arg-name{min-width:65px;font-size:12px}.required-dot::after{content:"*";color:var(--accent)}.optional-params{width:100%;padding-top:5px;font-size:12px}.optional-params summary{color:var(--text-1);cursor:pointer;padding:4px 0}.param-buttons{gap:4px;margin:4px 0}.param-button{border-radius:3px;padding:3px 6px}.cell-input,.target-select{min-height:28px;font-size:13px;background:var(--field);border-color:var(--control-border)}.target-select{max-width:280px}.field-hint{font-size:12px}.field-row>.field-hint{flex-basis:100%;padding-left:48px}
.expand-btn {
  display: flex; align-items: center; gap: 7px; flex: 1; min-width: 0;
  min-height: 29px; padding: 0; border: 0; background: transparent;
  color: inherit; font: inherit; text-align: left; cursor: pointer;
}
.expand-btn:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.head-actions .mini-btn { width: auto; min-width: 48px; height: 27px; padding: 3px 5px; gap: 3px; white-space: nowrap; }
.call-signature > .cell-input { flex: 1 1 120px; width: 120px; min-width: 0; max-width: 220px; }
.return-value-field { display: flex; align-items: center; gap: 4px; flex: 1 1 160px; min-width: 0; max-width: 240px; }
.return-value-label { display: flex; align-items: center; gap: 6px; flex: 1; min-width: 0; font-size: 12px; color: var(--text-1); }
.return-value-label > span { flex: none; }
.return-value-label .cell-input { flex: 1; min-width: 0; width: 90px; }
.call-signature > .cell-err-msg { flex-basis: 100%; }
.kind-name { max-width: 45%; overflow: hidden; text-overflow: ellipsis; }
</style>
