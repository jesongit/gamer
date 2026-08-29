<template>
  <div
    class="step-card"
    :class="{ selected, expanded, 'has-error': ownErrors.length > 0, 'card-highlight': highlighted, [`kind-${step.kind}`]: true }"
    :data-step-uuid="step.uuid"
    :data-step-path="stepPath"
    @click.stop="emit('select', step.uuid)"
  >
    <!-- 卡头：拖动手柄（占位）+ 图标 + 中文名 + 序号 + 摘要 + 动作按钮 -->
    <div class="card-head">
      <span class="drag-handle" title="拖动排序（占位，排序请用上移/下移）" @pointerdown.stop @click.stop>⋮⋮</span>
      <span class="kind-icon" :title="meta.hint">{{ meta.icon }}</span>
      <span class="kind-name">{{ meta.label }}</span>
      <span class="step-no">#{{ index + 1 }}</span>
      <span class="summary" :title="summary">{{ summary }}</span>
      <span v-if="ownErrors.length" class="err-badge" :title="ownErrors.map((d) => d.message).join('\n')">
        {{ ownErrors.length }}
      </span>
      <span class="head-actions">
        <!-- 函数测试入口（阶段 5）：仅宿主开启 testFrom 时显示（函数库页签的函数体顶层卡片） -->
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
      <div v-if="step.kind === 'str_app' || step.kind === 'cls_app'" class="field-hint">{{ meta.hint }}</div>

      <!-- tap -->
      <template v-else-if="step.kind === 'tap'">
        <div class="field-row">
          <span class="field-label">坐标</span>
          <CellEditor :cell="step.at" type="coord" :params="params" label="坐标" :error="fieldError('at')" @change="(c) => updateCell('at', c)" />
        </div>
      </template>

      <!-- swipe -->
      <template v-else-if="step.kind === 'swipe'">
        <div class="field-row">
          <span class="field-label">起点</span>
          <CellEditor :cell="step.from" type="coord" :params="params" label="起点" :error="fieldError('from')" @change="(c) => updateCell('from', c)" />
        </div>
        <div class="field-row">
          <span class="field-label">终点</span>
          <CellEditor :cell="step.to" type="coord" :params="params" label="终点" :error="fieldError('to')" @change="(c) => updateCell('to', c)" />
        </div>
        <div class="field-row">
          <span class="field-label">时长</span>
          <CellEditor :cell="step.time" type="time" :params="params" label="时长" :error="fieldError('time')" @change="(c) => updateCell('time', c)" />
        </div>
      </template>

      <!-- key -->
      <template v-else-if="step.kind === 'key'">
        <div class="field-row">
          <span class="field-label">按键</span>
          <CellEditor :cell="step.key" type="key" :params="params" label="按键" :error="fieldError('key')" @change="(c) => updateCell('key', c)" />
        </div>
      </template>

      <!-- text -->
      <template v-else-if="step.kind === 'text'">
        <div class="field-row">
          <span class="field-label">文本</span>
          <CellEditor :cell="step.value" type="text" :params="params" label="文本" :error="fieldError('value')" @change="(c) => updateCell('value', c)" />
        </div>
      </template>

      <!-- log -->
      <template v-else-if="step.kind === 'log'">
        <div class="field-row">
          <span class="field-label">日志</span>
          <CellEditor :cell="step.message" type="text" :params="params" label="日志内容" multiline :error="fieldError('message')" @change="(c) => updateCell('message', c)" />
        </div>
      </template>

      <!-- wait -->
      <template v-else-if="step.kind === 'wait'">
        <div class="field-row">
          <span class="field-label">时长</span>
          <CellEditor :cell="step.duration" type="time" :params="params" label="时长" :error="fieldError('duration')" @change="(c) => updateCell('duration', c)" />
          <label class="field-check" title="启用后等待 [时长, 上限] 内的随机值">
            <input type="checkbox" :checked="step.duration_max !== null" @change="toggleRandom" />
            随机区间
          </label>
          <CellEditor
            v-if="step.duration_max" :cell="step.duration_max" type="time" :params="params"
            label="随机上限" :error="fieldError('duration_max')" @change="(c) => updateCell('duration_max', c)"
          />
        </div>
      </template>

      <!-- find -->
      <template v-else-if="step.kind === 'find'">
        <div class="field-row">
          <span class="field-label">主模板</span>
          <CellEditor :cell="step.template" type="tmpl" :params="params" :templates="templates" label="主模板" :error="fieldError('template')" @change="(c) => updateCell('template', c)" />
          <label class="field-check" title="命中点击后等 interval 重匹配，仍命中补一击">
            <input type="checkbox" :checked="step.verify" @change="setVerify(($event.target as HTMLInputElement).checked)" />
            二次确认
          </label>
        </div>
        <div v-for="(b, bi) in step.block" :key="bi" class="field-row">
          <span class="field-label">障碍 {{ bi + 1 }}</span>
          <CellEditor :cell="b" type="tmpl" :params="params" :templates="templates" :label="`障碍${bi + 1}`" :error="fieldError(`block[${bi}]`)" @change="(c) => updateBlockAt(bi, c)" />
          <button type="button" class="mini-btn" title="删除障碍" @click.stop="removeBlock(bi)">✕</button>
        </div>
        <div class="field-row">
          <button type="button" class="mini-btn add" title="添加障碍" @click.stop="addBlock">+ 添加障碍</button>
          <label class="field-check">
            <input type="checkbox" :checked="step.timeout !== null" @change="toggleFindTimeout" />
            等待超时
          </label>
          <CellEditor
            v-if="step.timeout" :cell="step.timeout" type="time" :params="params"
            label="超时" :error="fieldError('timeout')" @change="(c) => updateCell('timeout', c)"
          />
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('then')" :base-path="subBase('then')"
          label="命中后" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
        />
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
          label="未命中" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
        />
      </template>

      <!-- match -->
      <template v-else-if="step.kind === 'match'">
        <div class="field-hint warn">仅检测不点击；按序首个命中分支获胜</div>
        <div v-for="(cand, ci) in step.candidates" :key="ci" class="cand-block" :class="{ 'cell-error': !!fieldError(`candidates[${ci}].template`) || !!fieldError('candidates') }">
          <div class="field-row">
            <span class="field-label">候选 {{ ci + 1 }}</span>
            <CellEditor
              :cell="cand.template" type="tmpl" :params="params" :templates="templates"
              :label="`候选${ci + 1}`" :error="fieldError(`candidates[${ci}].template`) || fieldError('candidates')"
              @change="(c) => updateCandidateTemplate(ci, c)"
            />
            <button v-if="step.candidates.length > 1" type="button" class="mini-btn" title="删除候选" @click.stop="removeCandidate(ci)">✕</button>
          </div>
          <BranchContainer
            :model="model" :stack="stack" :container-path="candPath(ci)" :base-path="candBase(ci)"
            :label="candLabel(ci)" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
            :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
            :templates="templates"
            @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
            @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
          />
        </div>
        <div class="field-row">
          <button type="button" class="mini-btn add" title="添加候选" @click.stop="addCandidate">+ 添加候选</button>
          <label class="field-check">
            <input type="checkbox" :checked="step.timeout !== null" @change="toggleMatchTimeout" />
            轮询超时
          </label>
          <CellEditor
            v-if="step.timeout" :cell="step.timeout" type="time" :params="params"
            label="超时" :error="fieldError('timeout')" @change="(c) => updateCell('timeout', c)"
          />
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
          label="都未命中" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
        />
      </template>

      <!-- color -->
      <template v-else-if="step.kind === 'color'">
        <div class="field-row">
          <span class="field-label">坐标</span>
          <CellEditor :cell="step.at" type="coord" :params="params" label="取色坐标" :error="fieldError('at')" @change="(c) => updateCell('at', c)" />
          <span class="field-hint warn">仅检测不点击</span>
        </div>
        <div v-for="(exp, ei) in step.expect" :key="ei" class="cand-block" :class="{ 'cell-error': !!fieldError(`expect[${ei}].color`) || !!fieldError('expect') }">
          <div class="field-row">
            <span class="field-label">颜色 {{ ei + 1 }}</span>
            <CellEditor
              :cell="exp.color" type="color" :params="params" :label="`颜色${ei + 1}`"
              :error="fieldError(`expect[${ei}].color`) || fieldError('expect')"
              @change="(c) => updateExpectColor(ei, c)"
            />
            <button v-if="step.expect.length > 1" type="button" class="mini-btn" title="删除颜色候选" @click.stop="removeExpect(ei)">✕</button>
          </div>
          <BranchContainer
            :model="model" :stack="stack" :container-path="candPath(ei)" :base-path="candBase(ei)"
            :label="candLabel(ei)" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
            :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
            :templates="templates"
            @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
            @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
          />
        </div>
        <div class="field-row">
          <button type="button" class="mini-btn add" title="添加颜色候选" @click.stop="addExpect">+ 添加颜色候选</button>
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
          label="颜色未命中" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
        />
      </template>

      <!-- if -->
      <template v-else-if="step.kind === 'if'">
        <div class="field-row">
          <span class="field-label">条件</span>
          <CellEditor :cell="step.cond" type="bool" :params="params" label="条件" :error="fieldError('cond')" @change="(c) => updateCell('cond', c)" />
          <span class="field-hint">只接受布尔字面量或布尔参数</span>
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('then')" :base-path="subBase('then')"
          label="如果为真" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
        />
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
          label="如果为假" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
        />
      </template>

      <!-- loop -->
      <template v-else-if="step.kind === 'loop'">
        <div class="field-row">
          <span class="field-label">次数</span>
          <input
            v-if="step.times !== null" class="cell-input num" type="number" min="0"
            :value="step.times" aria-label="循环次数" @change="setTimes(($event.target as HTMLInputElement).value)"
          />
          <label class="field-check">
            <input type="checkbox" :checked="step.times === null" @change="toggleInfinite(($event.target as HTMLInputElement).checked)" />
            无限循环
          </label>
          <span v-if="step.times === null" class="field-hint warn">无限循环——请确保体内有退出条件</span>
          <span v-if="fieldError('steps')" class="cell-err-msg">{{ fieldError('steps') }}</span>
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('steps')" :base-path="subBase('steps')"
          label="循环体" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
        />
      </template>

      <!-- call / func -->
      <template v-else-if="step.kind === 'call' || step.kind === 'func'">
        <div class="field-row">
          <span class="field-label">{{ step.kind === 'call' ? '目标脚本' : '目标函数' }}</span>
          <input
            class="cell-input" :value="step.target"
            :placeholder="step.kind === 'call' ? 'sub_task.yaml' : 'common/login'"
            :aria-label="step.kind === 'call' ? '目标脚本' : '目标函数'"
            @input="setTarget(($event.target as HTMLInputElement).value)"
          />
          <span v-if="fieldError('target')" class="cell-err-msg">{{ fieldError('target') }}</span>
        </div>
        <div class="field-row col">
          <span class="field-label">参数 args</span>
          <span v-if="fieldError('args')" class="cell-err-msg">{{ fieldError('args') }}</span>
          <div v-for="name in argNames" :key="name" class="arg-row">
            <input
              class="cell-input" :value="name" aria-label="参数名" placeholder="参数名"
              @change="renameArg(name, ($event.target as HTMLInputElement).value)"
            />
            <CellEditor
              :cell="step.args[name]" :type="argType(name)" :params="params"
              :label="`args ${name}`" @change="(c) => updateArgValue(name, c)"
            />
            <button type="button" class="mini-btn" title="删除实参" @click.stop="removeArg(name)">✕</button>
          </div>
          <button type="button" class="mini-btn add" title="添加实参" @click.stop="addArg">+ 添加实参</button>
        </div>
        <template v-if="step.kind === 'func'">
          <BranchContainer
            :model="model" :stack="stack" :container-path="subPath('then')" :base-path="subBase('then')"
            label="成功时" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
            :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
            :templates="templates"
            @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
            @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
          />
          <BranchContainer
            :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
            label="失败时" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
            :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
            :templates="templates"
            @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
            @focus="(p) => emit('focus', p)" @add-here="(p) => emit('add-here', p)"
          />
        </template>
      </template>

      <!-- throw -->
      <template v-else-if="step.kind === 'throw'">
        <div class="field-row">
          <span class="field-label">原因</span>
          <input
            class="cell-input grow" :value="step.message ?? ''" placeholder="可空"
            aria-label="终止原因" @input="setMessage(($event.target as HTMLInputElement).value)"
          />
          <span class="field-hint warn">会结束整个任务（含调用链）</span>
        </div>
      </template>

      <!-- return -->
      <template v-else-if="step.kind === 'return'">
        <div v-if="context !== 'function'" class="field-hint warn">return 在脚本中非法——只能出现在函数库（func/）的函数体内</div>
        <div class="field-row">
          <span class="field-label">返回值</span>
          <CellEditor :cell="step.value" type="bool" :params="params" label="返回值" :error="fieldError('value')" @change="(c) => updateCell('value', c)" />
        </div>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 步骤卡片（plan §8.4 / §9）：17 类全覆盖。
 * - 收起态 = 自然语言摘要（kinds.stepSummary，§9 表文案）；
 * - 展开态 = 该类型强类型控件（无任意键值编辑器）；字段错误按 Diagnostic.field 标红定位；
 * - 左侧动作图标 + 中文名 + 序号 + 上移/下移/复制/删除（全部经 CommandStack）；
 * - find/match/color/if/loop/func 的分支子流程内嵌 BranchContainer（一层内嵌、更深专注）。
 * 纯受控组件：所有写操作构造 Command 提交 stack，自身不改模型。
 */
import { computed, ref, type PropType } from 'vue'
import type { Path } from '../commands'
import { resolveStepList } from '../commands'
import type { Diagnostic } from '../diagnostics'
import { joinStepPath } from '../diagnostics'
import { childContainerPath, containerLabel } from '../selection'
import type { Cell, ParamDecl, Step, StepKind } from '../model'
import { KIND_META, stepSummary } from './kinds'
import CellEditor from './CellEditor.vue'
import BranchContainer from './BranchContainer.vue'

/** 联合类型放宽：模板内按 kind 分支后访问各自字段；运行时由 v-if 保证形态。 */
type AnyStep = Step & Record<string, any>

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
  context: { type: String as PropType<'script' | 'function'>, default: 'script' },
  params: { type: Array as PropType<ParamDecl[]>, default: () => [] },
  resolveTarget: {
    type: Function as PropType<((kind: 'call' | 'func', target: string) => { params: ParamDecl[] } | null) | undefined>,
    default: undefined,
  },
  templates: { type: Array as PropType<string[]>, default: () => [] },
  /** 显示「从此步骤测试函数」入口（函数库测试；仅函数体顶层容器由宿主开启）。 */
  testFrom: { type: Boolean, default: false },
})

const emit = defineEmits(['select', 'toggle-expand', 'focus', 'add-here', 'test-from'])

const meta = computed(() => KIND_META[props.step.kind as StepKind])
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

function fieldError(field: string): string {
  return ownErrors.value.find((d) => d.field === field)?.message ?? ''
}

const s = computed(() => props.step as AnyStep)

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
  return childContainerPath(props.containerPath, props.index, key, -1)
}
function subBase(key: string): string {
  return `${stepPath.value}.${key}`
}
function candPath(ci: number): Path {
  return childContainerPath(props.containerPath, props.index, 'candidates', ci)
}
function candBase(ci: number): string {
  return `${stepPath.value}.candidates[${ci}].steps`
}
function candLabel(ci: number): string {
  return containerLabel(props.step, 'candidates', ci)
}

// ---------- 各类型字段操作 ----------

function toggleRandom(e: Event): void {
  const on = (e.target as HTMLInputElement).checked
  if (on) {
    const cur = s.value.duration
    updateStep({ duration_max: cur && typeof cur.lit === 'string' ? { lit: cur.lit } : { lit: '1s' } })
  } else {
    updateStep({ duration_max: null })
  }
}
function setVerify(v: boolean): void {
  updateStep({ verify: v })
}
function toggleFindTimeout(e: Event): void {
  updateStep({ timeout: (e.target as HTMLInputElement).checked ? { lit: '30s' } : null })
}
function toggleMatchTimeout(e: Event): void {
  updateStep({ timeout: (e.target as HTMLInputElement).checked ? { lit: '30s' } : null })
}
function updateBlockAt(i: number, cell: Cell): void {
  updateStep({ block: s.value.block.map((c: Cell, j: number) => (j === i ? cell : c)) })
}
function addBlock(): void {
  updateStep({ block: [...s.value.block, { lit: '' }] })
}
function removeBlock(i: number): void {
  updateStep({ block: s.value.block.filter((_: Cell, j: number) => j !== i) })
}
function addCandidate(): void {
  updateStep({ candidates: [...s.value.candidates, { template: { lit: '' }, steps: [] }] })
}
function removeCandidate(i: number): void {
  updateStep({ candidates: s.value.candidates.filter((_: unknown, j: number) => j !== i) })
}
function updateCandidateTemplate(i: number, cell: Cell): void {
  updateStep({ candidates: s.value.candidates.map((c: unknown, j: number) => (j === i ? { ...(c as object), template: cell } : c)) })
}
function addExpect(): void {
  updateStep({ expect: [...s.value.expect, { color: { lit: '' }, steps: [] }] })
}
function removeExpect(i: number): void {
  updateStep({ expect: s.value.expect.filter((_: unknown, j: number) => j !== i) })
}
function updateExpectColor(i: number, cell: Cell): void {
  updateStep({ expect: s.value.expect.map((c: unknown, j: number) => (j === i ? { ...(c as object), color: cell } : c)) })
}
function setTimes(raw: string): void {
  const n = Number(raw)
  updateStep({ times: Number.isFinite(n) && n >= 0 ? Math.floor(n) : 1 })
}
function toggleInfinite(on: boolean): void {
  updateStep({ times: on ? null : 1 })
}
function setTarget(v: string): void {
  updateStep({ target: v })
}
function setMessage(v: string): void {
  updateStep({ message: v === '' ? null : v })
}

// ---------- args（call/func） ----------

const argNames = computed<string[]>(() => Object.keys(s.value.args ?? {}))

function argType(name: string): ParamDecl['type'] {
  if (!props.resolveTarget) return 'text'
  const decls = props.resolveTarget(props.step.kind === 'call' ? 'call' : 'func', s.value.target)?.params
  return decls?.find((d) => d.name === name)?.type ?? 'text'
}
function updateArgValue(name: string, cell: Cell): void {
  updateStep({ args: { ...s.value.args, [name]: cell } })
}
function removeArg(name: string): void {
  const next = { ...s.value.args }
  delete next[name]
  updateStep({ args: next })
}
function renameArg(oldName: string, raw: string): void {
  const name = raw.trim()
  if (!name || name === oldName) return
  if (name in s.value.args) return // 重复键直接忽略（校验层另有 args.unknown 提示）
  const next: Record<string, Cell> = {}
  for (const [k, v] of Object.entries(s.value.args)) next[k === oldName ? name : k] = v as Cell
  updateStep({ args: next })
}
function addArg(): void {
  const args = s.value.args ?? {}
  let i = 1
  while (`param${i}` in args) i++
  updateStep({ args: { ...args, [`param${i}`]: { lit: '' } } })
}
</script>

<style scoped>
.step-card {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  margin: 6px 0;
  overflow: visible;
}
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
.drag-handle { color: var(--text-2); cursor: grab; font-size: 11px; letter-spacing: -2px; user-select: none; }
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
.cand-block {
  border-left: 2px solid var(--border);
  padding-left: 8px;
  margin: 2px 0;
  display: flex; flex-direction: column; gap: 4px;
}
.cand-block.cell-error { border-left-color: var(--danger); }
.arg-row { display: flex; align-items: center; gap: 6px; }
.cell-input.grow { flex: 1; }
.cell-input.num { width: 74px; }
</style>
