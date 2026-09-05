<template>
  <div class="cell-editor" :class="{ 'cell-error': !!(error || selfError), 'is-ref': isRef }" :title="error || selfError || undefined">
    <div v-if="allowRef" class="cell-mode" role="group" aria-label="取值方式">
      <button
        type="button"
        class="mode-btn"
        :class="{ active: !isRef }"
        :title="isRef ? '切换为字面量' : '当前为字面量'"
        @click.stop="switchToLit"
      >值</button>
      <button
        type="button"
        class="mode-btn"
        :class="{ active: isRef }"
        :title="!isRef ? '切换为 $引用（支持属性路径，如 $reward.center）' : '当前为 $引用'"
        @click.stop="switchToRef"
      >引用</button>
    </div>

    <template v-if="isRef">
      <!-- v3 表达式引用：自由属性路径（$reward.center / $list[0]）+ 声明参数联想 -->
      <input
        class="cell-input ref-input mono" :value="refPath" :list="listId"
        :placeholder="placeholder || '变量路径，如 reward.center'" :aria-label="`${label}引用`"
        spellcheck="false" autocomplete="off"
        @input.stop="onRefInput(($event.target as HTMLInputElement).value)"
      />
      <datalist :id="listId">
        <option v-for="p in params" :key="p.name" :value="p.name">{{ p.remark || p.type }}</option>
      </datalist>
    </template>

    <template v-else>
      <!-- tmpl：模板短名（自定义下拉，悬停行内预览缩略图；候选由页面外壳注入）
           + 框选（宿主注入 seCellTools 时可用：投屏框选生成新模板，保存后自动填入） -->
      <template v-if="type === 'tmpl'">
        <span class="tmpl-wrap">
          <input
            class="cell-input"
            :value="litString"
            :placeholder="placeholder || '模板短名，如 account.png'"
            :aria-label="label"
            autocomplete="off"
            @input.stop="onText($event, (v) => { emitLit(v); open = true })"
            @focus="open = true"
            @blur="open = false"
            @keydown.esc.stop="open = false"
          />
          <button
            type="button" class="cell-tool tpl-toggle" :class="{ active: open }"
            title="选择模板（悬停预览缩略图）"
            @mousedown.prevent @click="open = !open"
          >▾</button>
          <div v-if="open" class="tpl-drop">
            <div v-if="!filteredTemplates.length" class="tpl-drop-empty">无匹配模板</div>
            <div
              v-for="t in filteredTemplates" :key="t" class="tpl-drop-row"
              @mousedown.prevent @click="pick(t)"
              @mouseenter="hovered = t" @mouseleave="hovered = ''"
            >
              <span class="tpl-drop-thumb">
                <img v-if="hovered === t && thumbUrl(t)" :src="thumbUrl(t)!" alt="" loading="lazy" />
              </span>
              <span class="tpl-drop-name mono">{{ t }}</span>
            </div>
          </div>
        </span>
        <button
          v-if="tools" type="button" class="cell-tool live"
          :class="{ active: capturing }"
          title="在投屏画面框选新模板，保存后自动填入此处"
          @click.stop="onCaptureTemplate"
        >{{ capturing ? '框选中…' : '框选' }}</button>
        <button
          v-if="tools" type="button" class="cell-tool live"
          :class="{ active: matching }"
          :disabled="matching || isRef || !litString.trim()"
          :title="isRef ? '引用需要运行时值，不能在编辑态预览匹配' : '按步骤实际匹配规则预览当前模板（只匹配，不点击）'"
          @click.stop="onMatchTemplate"
        >{{ matching ? '匹配中…' : '匹配' }}</button>
      </template>

      <!-- coord：X/Y 双数字 + 投屏选点（宿主注入 seCellTools 时可用） -->
      <template v-else-if="type === 'coord'">
        <label class="cell-mini">X
          <input
            class="cell-input num" type="number" step="0.01" min="0" max="1"
            :value="coordLit[0]" :aria-label="`${label}X`"
            @input.stop="onCoord(0, $event)"
          />
        </label>
        <label class="cell-mini">Y
          <input
            class="cell-input num" type="number" step="0.01" min="0" max="1"
            :value="coordLit[1]" :aria-label="`${label}Y`"
            @input.stop="onCoord(1, $event)"
          />
        </label>
        <button
          v-if="tools" type="button" class="cell-tool live"
          :class="{ active: picking }"
          :title="`在投屏画面上点击选点，自动填入 ${label} 坐标`"
          @click.stop="onPickCoord"
        >{{ picking ? '点击画面选取…' : '选坐标' }}</button>
      </template>

      <!-- time：数值 + 单位（默认 ms；v3 亦接受裸毫秒数字） -->
      <template v-else-if="type === 'time'">
        <input
          class="cell-input num" type="number" min="0" step="any"
          :value="timeParts[0]" :aria-label="`${label}数值`"
          @input.stop="onTimeNum($event)"
        />
        <select class="cell-select unit" :value="timeParts[1]" :aria-label="`${label}单位`" @change.stop="onTimeUnit($event)">
          <option v-for="u in TIME_UNITS" :key="u" :value="u">{{ u }}</option>
        </select>
      </template>

      <!-- number：数值输入（loop.times 等） -->
      <template v-else-if="type === 'number'">
        <input
          class="cell-input num" type="number" step="any"
          :value="numLit" :aria-label="label"
          @input.stop="onNum($event)"
        />
      </template>

      <!-- key：枚举下拉 -->
      <template v-else-if="type === 'key'">
        <select class="cell-select" :value="litString" :aria-label="label" @change.stop="emitLit(($event.target as HTMLSelectElement).value)">
          <option v-for="k in KEY_ENUM" :key="k" :value="k">{{ k }}</option>
        </select>
      </template>

      <!-- bool：下拉 真/假 -->
      <template v-else-if="type === 'bool'">
        <select
          class="cell-select" :value="cell.lit === true ? 'true' : 'false'" :aria-label="label"
          @change.stop="emitLit(($event.target as HTMLSelectElement).value === 'true')"
        >
          <option value="true">真</option>
          <option value="false">假</option>
        </select>
      </template>

      <!-- expr：通用表达式字面量（true/false/数字自动识别，其余按字符串） -->
      <template v-else-if="type === 'expr'">
        <input
          class="cell-input mono" :value="litString" :list="listId"
          :placeholder="placeholder || '字面量或 $引用'" :aria-label="label"
          spellcheck="false" autocomplete="off"
          @input.stop="onText($event, (v) => emitLit(parseExprText(v)))"
        />
      </template>

      <!-- text：单行/多行 -->
      <template v-else>
        <textarea
          v-if="multiline"
          class="cell-input area" rows="2" :value="litString"
          :placeholder="placeholder || '文本'" :aria-label="label"
          @input.stop="onText($event, (v) => emitLit(v))"
        />
        <input
          v-else
          class="cell-input" :value="litString"
          :placeholder="placeholder || '文本'" :aria-label="label"
          @input.stop="onText($event, (v) => emitLit(v))"
        />
      </template>
    </template>
    <span v-if="error || selfError" class="cell-err-msg">{{ error || selfError }}</span>
  </div>
</template>

<script lang="ts">
/** 模块级实例计数（datalist id 唯一化；script setup 内变量每实例重置，须放这里）。 */
let listIdSeq = 0
function nextListId(): number {
  return ++listIdSeq
}
</script>
<script setup lang="ts">
/**
 * 取值单元格编辑器（v3）：字面量 ↔ $属性路径引用 切换（$reward.center、$list[0]）。
 * 纯受控组件——不直接写模型，change 事件携带新 Cell，由宿主（StepCard/ParamEditor）
 * 构造 update_step / update_param 命令经 CommandStack 提交。
 * 字面量控件：模板短名 / 坐标双数字 / 数值+单位 / 按键枚举 / 布尔 / 通用表达式 / 数字 / 文本。
 * v3 表达式动态类型：引用下拉不再按参数类型过滤，全部声明仅作联想。
 */
import { computed, inject, ref } from 'vue'
import type { PropType } from 'vue'
import { pinyin } from 'pinyin-pro'
import { isRefCell, type Cell, type CellType, type ParamDecl } from '../model'
import { checkCellLiteral, isRefPath, KEY_ENUM, TIME_UNITS } from '../schema'

const props = defineProps({
  cell: { type: Object as PropType<Cell>, required: true },
  type: { type: String as PropType<CellType>, required: true },
  /** 可引用的参数声明（引用联想；v3 不按类型过滤）。 */
  params: { type: Array as PropType<ParamDecl[]>, default: () => [] },
  /** 默认值编辑等场景禁止切引用。 */
  allowRef: { type: Boolean, default: true },
  /** 字段错误消息（红框 + 提示）。 */
  error: { type: String, default: '' },
  label: { type: String, default: '值' },
  placeholder: { type: String, default: '' },
  multiline: { type: Boolean, default: false },
  /** tmpl 字段的可选模板短名候选。 */
  templates: { type: Array as PropType<string[]>, default: () => [] },
})

const emit = defineEmits(['change'])

/** datalist id 每实例唯一，避免多实例互相覆盖联想列表。 */
const listId = `se-params-${nextListId()}`

/**
 * 投屏取值工具（Console 编辑态 provide('seCellTools')）：选点/框选生成模板/匹配预览。
 * 未注入（独立脚本页无投屏、单测）时按钮不渲染。
 */
interface CellTools {
  pickCoord(): Promise<{ x: number; y: number } | null>
  pickColor(): Promise<{ hex: string; x: number; y: number } | null>
  captureTemplate(): Promise<string | null>
  matchTemplate(name: string): Promise<unknown>
}
const tools = inject<CellTools | null>('seCellTools', null)
const picking = ref(false)
const matching = ref(false)

async function onPickCoord(): Promise<void> {
  if (!tools || picking.value) return
  picking.value = true
  try {
    const hit = await tools.pickCoord()
    if (hit) emitLit([hit.x, hit.y])
  } finally {
    picking.value = false
  }
}
/** 框选生成新模板：等待宿主裁切保存完成，成功则以模板短名自动填入本字段 */
const capturing = ref(false)
async function onCaptureTemplate(): Promise<void> {
  if (!tools || capturing.value) return
  capturing.value = true
  try {
    const name = await tools.captureTemplate()
    if (name) emitLit(name)
  } finally {
    capturing.value = false
  }
}

async function onMatchTemplate(): Promise<void> {
  const name = litString.value.trim()
  if (!tools || matching.value || isRef.value || !name) return
  matching.value = true
  try {
    await tools.matchTemplate(name)
  } finally {
    matching.value = false
  }
}

const isRef = computed(() => props.allowRef && isRefCell(props.cell))

/** 即时自校验：字面量按类型规则当场校验；引用态只提示路径语法。 */
const selfError = computed(() => {
  if (isRef.value) {
    return isRefPath(props.cell.ref) ? '' : `引用 $${props.cell.ref} 不是合法属性路径`
  }
  return checkCellLiteral(props.type, props.cell.lit)?.message ?? ''
})

// ---- tmpl 自定义下拉（悬停行内预览缩略图，缩略图 URL 由页面外壳 provide('tplPreviewUrl') 注入） ----
const open = ref(false)
const hovered = ref('')
const tplPreviewUrl = inject<((short: string) => string | null) | null>('tplPreviewUrl', null)

// 与模板列表保持同一搜索口径：名称子串 + 中文名拼音首字母；匹配位置越靠前越优先。
const tplPinyinCache = new Map<string, string>()
const PY_OFFSET = 1e4
function tplPinyinInitials(name: string): string {
  let initials = tplPinyinCache.get(name)
  if (initials === undefined) {
    initials = pinyin(name, { pattern: 'first', toneType: 'none', type: 'array' })
      .join('').replace(/\s+/g, '').toLowerCase()
    tplPinyinCache.set(name, initials)
  }
  return initials
}
function templateMatchIndex(name: string, query: string): number {
  const direct = name.toLowerCase().indexOf(query)
  if (direct !== -1) return direct
  // 拼音首字母串不含中文，查询词含中文时跳过该口径。
  if (!/[\u4e00-\u9fff]/.test(query)) {
    const initial = tplPinyinInitials(name).indexOf(query)
    if (initial !== -1) return PY_OFFSET + initial
  }
  return -1
}
const filteredTemplates = computed(() => {
  const q = litString.value.trim().toLowerCase()
  if (!q) return props.templates
  return props.templates
    .map((name, index) => ({ name, index, match: templateMatchIndex(name, q) }))
    .filter((item) => item.match !== -1)
    .sort((a, b) => a.match - b.match || a.index - b.index)
    .map((item) => item.name)
})
function thumbUrl(t: string): string | null {
  return tplPreviewUrl ? tplPreviewUrl(t) : null
}
function pick(t: string): void {
  emitLit(t)
  open.value = false
}

const litString = computed(() => (isRefCell(props.cell) ? '' : String(props.cell.lit ?? '')))
const refPath = computed(() => (isRefCell(props.cell) ? props.cell.ref : ''))
const coordLit = computed<[number, number]>(() => {
  const v = props.cell.lit
  return Array.isArray(v) && v.length === 2 && v.every((n) => Number.isFinite(n)) ? [v[0], v[1]] : [0.5, 0.5]
})
const timeParts = computed<[string, string]>(() => {
  const raw = litString.value.trim()
  if (typeof props.cell.lit === 'number') return [String(props.cell.lit), 'ms']
  const m = /^([0-9]+(?:\.[0-9]+)?)(ms|s|m|h)$/.exec(raw)
  // 解析失败/空值回退 1ms（默认单位 ms）
  return m ? [m[1], m[2]] : ['1', 'ms']
})
const numLit = computed<string>(() => {
  const v = props.cell.lit
  return typeof v === 'number' ? String(v) : (typeof v === 'string' && v !== '' ? v : '0')
})

function defaultLiteral(type: CellType): unknown {
  switch (type) {
    case 'coord': return [0.5, 0.5]
    case 'bool': return true
    case 'time': return '500ms'
    case 'number': return 0
    case 'expr': return ''
    case 'key': return 'BACK'
    case 'tmpl': return ''
    case 'text': return ''
  }
}

/** expr 输入串 → 字面量：true/false → 布尔；数字串 → 数值；其余按字符串。 */
function parseExprText(v: string): unknown {
  const t = v.trim()
  if (t === 'true') return true
  if (t === 'false') return false
  if (t !== '' && Number.isFinite(Number(t)) && /^-?(?:\d+\.?\d*|\.\d+)$/.test(t)) return Number(t)
  return v
}

function emitLit(value: unknown): void {
  emit('change', { lit: value } satisfies Cell)
}
function emitRef(name: string): void {
  emit('change', { ref: name } satisfies Cell)
}

function switchToLit(): void {
  if (!isRef.value) return
  emitLit(defaultLiteral(props.type))
}
function switchToRef(): void {
  if (isRef.value) return
  // 默认取第一个声明参数；无声明给空路径（编辑中间态，校验层提示）
  const first = props.params[0]
  emitRef(first ? first.name : '')
}
function onRefInput(raw: string): void {
  // 剥离前导 $（用户可直接粘贴 $reward.center）；空串保留为编辑中间态
  const path = raw.trim().replace(/^\$/, '')
  emitRef(path)
}

function onText(e: Event, fn: (v: string) => void): void {
  fn((e.target as HTMLInputElement).value)
}
function onCoord(axis: 0 | 1, e: Event): void {
  const raw = (e.target as HTMLInputElement).value
  const n = raw === '' ? 0 : Number(raw)
  if (!Number.isFinite(n)) return
  const next: [number, number] = [coordLit.value[0], coordLit.value[1]]
  next[axis] = n
  emitLit(next)
}
function onTimeNum(e: Event): void {
  const raw = (e.target as HTMLInputElement).value
  if (raw === '' || !Number.isFinite(Number(raw))) return
  emitLit(`${raw}${timeParts.value[1]}`)
}
function onTimeUnit(e: Event): void {
  emitLit(`${timeParts.value[0]}${(e.target as HTMLSelectElement).value}`)
}
function onNum(e: Event): void {
  const raw = (e.target as HTMLInputElement).value
  if (raw === '' || !Number.isFinite(Number(raw))) return
  emitLit(Number(raw))
}
</script>

<style scoped>
.cell-editor {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-height: 28px;
  padding: 2px 4px;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
}
.cell-editor.cell-error { border-color: var(--danger); background: rgba(248, 113, 113, .08); }
.cell-mode { display: inline-flex; border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden; }
.mode-btn {
  border: none; background: var(--bg-2); color: var(--text-2);
  font-size: 11px; padding: 2px 8px; cursor: pointer;
}
.mode-btn.active { background: var(--accent); color: #06251c; font-weight: 600; }
.cell-input, .cell-select {
  background: var(--bg-2); color: var(--text-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 3px 6px; font-size: 12px; min-width: 60px;
}
.cell-input:focus, .cell-select:focus { outline: none; border-color: var(--accent); }
.cell-input.num { width: 74px; }
.cell-input.ref-input { width: 150px; }
.cell-input.area { min-width: 180px; resize: vertical; }
.cell-select.unit { width: 64px; }
.cell-tool { font-size: 11px; padding: 2px 8px; border-radius: var(--radius-sm); border: 1px dashed var(--border); background: transparent; color: var(--text-2); cursor: not-allowed; }
.cell-tool.live { cursor: pointer; border-style: solid; }
.cell-tool.live:hover:not(.active) { color: var(--accent); border-color: var(--accent); }
/* 取点/框选进行中的激活态：按钮保持"被按下"样式，填入数据后自动恢复 */
.cell-tool.live.active { background: var(--accent); color: #06251c; border-color: var(--accent); font-weight: 600; }
.cell-mini { display: inline-flex; align-items: center; gap: 3px; font-size: 11px; color: var(--text-2); }
.tmpl-wrap { display: inline-flex; align-items: center; gap: 4px; position: relative; }
.tpl-toggle { cursor: pointer; border: 1px solid var(--border); background: var(--bg-2); color: var(--text-2); border-radius: var(--radius-sm); padding: 2px 7px; font-size: 10px; }
.tpl-toggle.active { border-color: var(--accent); color: var(--accent); }
.tpl-drop {
  position: absolute; top: calc(100% + 4px); left: 0; z-index: 60;
  min-width: 210px; max-width: 320px; max-height: 260px; overflow: auto;
  background: var(--bg-2); border: 1px solid var(--border); border-radius: var(--radius-sm);
  box-shadow: 0 8px 24px rgba(0, 0, 0, .45); display: flex; flex-direction: column;
}
.tpl-drop-row { display: flex; align-items: center; gap: 8px; padding: 5px 8px; cursor: pointer; }
.tpl-drop-row:hover { background: var(--bg-3); }
.tpl-drop-thumb {
  width: 42px; height: 28px; flex-shrink: 0; display: flex; align-items: center; justify-content: center;
  background: var(--bg-0); border: 1px solid var(--border); border-radius: 4px; overflow: hidden;
}
.tpl-drop-thumb img { max-width: 100%; max-height: 100%; object-fit: contain; }
.tpl-drop-name { font-size: 11px; color: var(--text-0); word-break: break-all; }
.tpl-drop-empty { padding: 10px; font-size: 11px; color: var(--text-2); text-align: center; }
.cell-err-msg { font-size: 11px; color: var(--danger); }
.mono { font-family: var(--mono); }
</style>
