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
        :title="!isRef ? '切换为脚本/函数参数引用' : '当前为参数引用'"
        @click.stop="switchToRef"
      >参数</button>
    </div>

    <template v-if="isRef">
      <select class="cell-select" :value="cell.ref" :aria-label="`${label}参数`" @change.stop="onRefChange">
        <option value="">— 选择 {{ typeLabel }} 参数 —</option>
        <option v-for="p in sameTypeParams" :key="p.name" :value="p.name">
          ${{ p.name }}{{ p.remark ? `（${p.remark}）` : '' }}
        </option>
      </select>
      <span v-if="!sameTypeParams.length" class="cell-hint">当前没有 {{ typeLabel }} 型参数</span>
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
          :title="isRef ? '参数引用需要运行时值，不能在编辑态预览匹配' : '按步骤实际匹配规则预览当前模板（只匹配，不点击）'"
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

      <!-- color：色板 + hex 输入 + 投屏选色（带放大镜） -->
      <template v-else-if="type === 'color'">
        <input
          class="cell-color" type="color"
          :value="`#${hexLit}`" :aria-label="`${label}色板`"
          @input.stop="onColorPick($event)"
        />
        <input
          class="cell-input hex" :value="litString" maxlength="6"
          placeholder="6 位十六进制" :aria-label="`${label}hex`"
          @input.stop="onText($event, (v) => emitLit(v.toLowerCase()))"
        />
        <button
          v-if="tools" type="button" class="cell-tool live"
          :class="{ active: picking }"
          title="在投屏画面上点击取色（带放大镜），自动填入颜色"
          @click.stop="onPickColor"
        >{{ picking ? '点击画面取色…' : '屏幕选色' }}</button>
      </template>

      <!-- time：数值 + 单位（默认 ms） -->
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

<script setup lang="ts">
/**
 * 取值单元格编辑器：字面量 ↔ 同类型 $参数 切换（plan §9 底注）。
 * 纯受控组件——不直接写模型，change 事件携带新 Cell，由宿主（StepCard/ParamEditor）
 * 构造 update_step / update_param 命令经 CommandStack 提交。
 * 七类字面量控件：模板短名 / 坐标双数字 / hex 色+取色占位 / 数值+单位 / 按键枚举 / 文本 / 开关。
 */
import { computed, inject, ref } from 'vue'
import type { PropType } from 'vue'
import { isRefCell, type Cell, type ParamDecl, type ParamType } from '../model'
import { checkCellLiteral, KEY_ENUM } from '../schema'

const props = defineProps({
  cell: { type: Object as PropType<Cell>, required: true },
  type: { type: String as PropType<ParamType>, required: true },
  /** 可引用的参数声明（同类型过滤后进下拉）。 */
  params: { type: Array as PropType<ParamDecl[]>, default: () => [] },
  /** 默认值编辑等场景禁止切参数。 */
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

const TIME_UNITS = ['ms', 's', 'm', 'min', 'h', 'd'] as const

/**
 * 投屏取值工具（Console 编辑态 provide('seCellTools')）：选点/选色/框选生成模板/匹配预览。
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
async function onPickColor(): Promise<void> {
  if (!tools || picking.value) return
  picking.value = true
  try {
    const hit = await tools.pickColor()
    if (hit?.hex) emitLit(hit.hex)
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

const TYPE_LABELS: Record<ParamType, string> = {
  tmpl: '模板', coord: '坐标', color: '颜色', time: '时间', key: '按键', text: '文本', bool: '布尔',
}
const typeLabel = computed(() => TYPE_LABELS[props.type])

const isRef = computed(() => props.allowRef && isRefCell(props.cell))
const sameTypeParams = computed(() => props.params.filter((p) => p.type === props.type))

/** 即时自校验：字面量按类型规则（coord 0~1 / time 带单位>0 / color hex / key 枚举 /
 *  tmpl 非空…）当场校验，不等保存或父级诊断；参数引用态由父级校验覆盖 */
const selfError = computed(() => {
  if (isRef.value) return ''
  return checkCellLiteral(props.type, props.cell.lit)?.message ?? ''
})

// ---- tmpl 自定义下拉（替代原生 datalist）：悬停行内预览缩略图，缩略图 URL 由
// 页面外壳 provide('tplPreviewUrl') 注入（短名 → 当前分区图片 URL）。 ----
const open = ref(false)
const hovered = ref('')
const tplPreviewUrl = inject<((short: string) => string | null) | null>('tplPreviewUrl', null)
const filteredTemplates = computed(() => {
  const q = litString.value.trim().toLowerCase()
  if (!q) return props.templates
  return props.templates.filter((t) => t.toLowerCase().includes(q))
})
function thumbUrl(t: string): string | null {
  return tplPreviewUrl ? tplPreviewUrl(t) : null
}
function pick(t: string): void {
  emitLit(t)
  open.value = false
}

const litString = computed(() => (isRefCell(props.cell) ? '' : String(props.cell.lit ?? '')))
const coordLit = computed<[number, number]>(() => {
  const v = props.cell.lit
  return Array.isArray(v) && v.length === 2 && v.every((n) => Number.isFinite(n)) ? [v[0], v[1]] : [0.5, 0.5]
})
const timeParts = computed<[string, string]>(() => {
  const m = /^([0-9]+(?:\.[0-9]+)?)(ms|s|m|min|h|d)$/.exec(litString.value.trim())
  // 解析失败/空值回退 1ms（默认单位 ms）
  return m ? [m[1], m[2]] : ['1', 'ms']
})
const hexLit = computed(() => (/^[0-9a-fA-F]{6}$/.test(litString.value) ? litString.value : '888888'))

function defaultLiteral(type: ParamType): unknown {
  switch (type) {
    case 'coord': return [0.5, 0.5]
    case 'bool': return true
    case 'color': return 'ff8800'
    case 'time': return '500ms'
    case 'key': return 'BACK'
    case 'tmpl': return ''
    case 'text': return ''
  }
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
  const first = sameTypeParams.value[0]
  if (first) emitRef(first.name)
}
function onRefChange(e: Event): void {
  const name = (e.target as HTMLSelectElement).value
  if (name) emitRef(name)
  else emitLit(defaultLiteral(props.type))
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
function onColorPick(e: Event): void {
  const hex = (e.target as HTMLInputElement).value.replace('#', '').toLowerCase()
  emitLit(hex)
}
function onTimeNum(e: Event): void {
  const raw = (e.target as HTMLInputElement).value
  if (raw === '' || !Number.isFinite(Number(raw))) return
  emitLit(`${raw}${timeParts.value[1]}`)
}
function onTimeUnit(e: Event): void {
  emitLit(`${timeParts.value[0]}${(e.target as HTMLSelectElement).value}`)
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
.cell-input.hex { width: 84px; font-family: var(--mono); }
.cell-input.area { min-width: 180px; resize: vertical; }
.cell-select.unit { width: 64px; }
.cell-color { width: 32px; height: 24px; padding: 0; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-2); }
.cell-tool { font-size: 11px; padding: 2px 8px; border-radius: var(--radius-sm); border: 1px dashed var(--border); background: transparent; color: var(--text-2); cursor: not-allowed; }
.cell-tool.live { cursor: pointer; border-style: solid; }
.cell-tool.live:hover:not(.active) { color: var(--accent); border-color: var(--accent); }
/* 取点/取色进行中的激活态：按钮保持"被按下"样式，填入数据后自动恢复 */
.cell-tool.live.active { background: var(--accent); color: #06251c; border-color: var(--accent); font-weight: 600; }
.cell-mini { display: inline-flex; align-items: center; gap: 3px; font-size: 11px; color: var(--text-2); }
.cell-hint { font-size: 11px; color: var(--text-2); }
.cell-err-msg { font-size: 11px; color: var(--danger); }
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
</style>
