<template>
  <div class="cell-editor" :class="{ 'cell-error': !!error, 'is-ref': isRef }" :title="error || undefined">
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
      <!-- tmpl：模板短名（缩略图选择器由页面外壳注入，这里提供短名输入 + 候选 datalist） -->
      <template v-if="type === 'tmpl'">
        <input
          class="cell-input"
          list="se-tmpl-options"
          :value="litString"
          :placeholder="placeholder || '模板短名，如 account.png'"
          :aria-label="label"
          @input.stop="onText($event, (v) => emitLit(v))"
        />
        <datalist id="se-tmpl-options">
          <option v-for="t in templates" :key="t" :value="t" />
        </datalist>
      </template>

      <!-- coord：X/Y 双数字 -->
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
      </template>

      <!-- color：色板 + hex 输入 + 取色占位 -->
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
        <button type="button" class="cell-tool" disabled title="从投屏取色（页面接入后可用）">取色</button>
      </template>

      <!-- time：数值 + 单位 -->
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

      <!-- bool：开关 -->
      <template v-else-if="type === 'bool'">
        <label class="cell-switch">
          <input
            type="checkbox" :checked="cell.lit === true" :aria-label="label"
            @change.stop="emitLit(($event.target as HTMLInputElement).checked)"
          />
          <span>{{ cell.lit === true ? '真' : '假' }}</span>
        </label>
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
    <span v-if="error" class="cell-err-msg">{{ error }}</span>
  </div>
</template>

<script setup lang="ts">
/**
 * 取值单元格编辑器：字面量 ↔ 同类型 $参数 切换（plan §9 底注）。
 * 纯受控组件——不直接写模型，change 事件携带新 Cell，由宿主（StepCard/ParamEditor）
 * 构造 update_step / update_param 命令经 CommandStack 提交。
 * 七类字面量控件：模板短名 / 坐标双数字 / hex 色+取色占位 / 数值+单位 / 按键枚举 / 文本 / 开关。
 */
import { computed } from 'vue'
import type { PropType } from 'vue'
import { isRefCell, type Cell, type ParamDecl, type ParamType } from '../model'
import { KEY_ENUM } from '../schema'

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

const TYPE_LABELS: Record<ParamType, string> = {
  tmpl: '模板', coord: '坐标', color: '颜色', time: '时间', key: '按键', text: '文本', bool: '布尔',
}
const typeLabel = computed(() => TYPE_LABELS[props.type])

const isRef = computed(() => props.allowRef && isRefCell(props.cell))
const sameTypeParams = computed(() => props.params.filter((p) => p.type === props.type))

const litString = computed(() => (isRefCell(props.cell) ? '' : String(props.cell.lit ?? '')))
const coordLit = computed<[number, number]>(() => {
  const v = props.cell.lit
  return Array.isArray(v) && v.length === 2 && v.every((n) => Number.isFinite(n)) ? [v[0], v[1]] : [0.5, 0.5]
})
const timeParts = computed<[string, string]>(() => {
  const m = /^([0-9]+(?:\.[0-9]+)?)(ms|s|m|min|h|d)$/.exec(litString.value.trim())
  return m ? [m[1], m[2]] : ['1', 's']
})
const hexLit = computed(() => (/^[0-9a-fA-F]{6}$/.test(litString.value) ? litString.value : '888888'))

function defaultLiteral(type: ParamType): unknown {
  switch (type) {
    case 'coord': return [0.5, 0.5]
    case 'bool': return true
    case 'color': return 'ff8800'
    case 'time': return '1s'
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
.cell-switch { display: inline-flex; align-items: center; gap: 4px; font-size: 12px; color: var(--text-1); cursor: pointer; }
.cell-mini { display: inline-flex; align-items: center; gap: 3px; font-size: 11px; color: var(--text-2); }
.cell-hint { font-size: 11px; color: var(--text-2); }
.cell-err-msg { font-size: 11px; color: var(--danger); }
</style>
