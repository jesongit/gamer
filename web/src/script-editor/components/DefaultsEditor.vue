<template>
  <div class="defaults-editor" data-testid="defaults-editor" @click.stop>
    <div class="de-head">
      <span class="de-title">脚本默认值</span>
      <span class="de-sub">
        阈值 {{ defaults?.vision_threshold ?? '默认' }}
        · 点击后 {{ defaults?.after_tap ?? '默认' }} · 命中后 {{ defaults?.after_match ?? '默认' }} · 轮询 {{ defaults?.poll_interval ?? '默认' }}
      </span>
      <span v-if="!expanded && hasIssues" class="de-err-badge" title="默认值有误，展开查看">有误</span>
      <button v-if="!defaults" type="button" class="mini-btn add" @click="enable">启用默认值</button>
      <button v-else type="button" class="mini-btn danger" title="清除 defaults（各项用引擎内置默认）" @click="clear">清除</button>
      <button type="button" class="mini-btn" :title="expanded ? '收起默认值' : '展开默认值'" @click="expanded = !expanded">
        {{ expanded ? '收起 ▴' : '展开 ▾' }}
      </button>
    </div>

    <template v-if="defaults && expanded">
      <div class="de-row">
        <span class="de-label">视觉阈值</span>
        <input
          class="de-range" type="range" min="0" max="1" step="0.01"
          :value="threshold" aria-label="视觉阈值滑块" @input="setThreshold(($event.target as HTMLInputElement).value)"
        />
        <input
          class="cell-input num" type="number" min="0" max="1" step="0.01"
          :value="threshold" aria-label="视觉阈值数值" @change="setThreshold(($event.target as HTMLInputElement).value)"
        />
        <span v-if="thresholdInvalid" class="de-err">threshold 必须是 0~1 的数字</span>
        <span class="de-hint">defaults.vision.threshold（步骤 threshold 可覆盖）</span>
      </div>

      <div v-for="item in TIMING_ITEMS" :key="item.key" class="de-row">
        <span class="de-label">{{ item.label }}</span>
        <input
          class="cell-input num" type="number" min="0" step="any"
          :value="timingParts(item.key)[0]" :aria-label="`${item.label}数值`"
          @input="setTimingNum(item.key, $event)"
        />
        <select
          class="cell-input" :value="timingParts(item.key)[1]" :aria-label="`${item.label}单位`"
          @change="setTimingUnit(item.key, ($event.target as HTMLSelectElement).value)"
        >
          <option v-for="u in TIME_UNITS" :key="u" :value="u">{{ u }}</option>
        </select>
        <span v-if="timingInvalid(item.key)" class="de-err">须 ≥0 且带单位（ms/s/m/h）</span>
        <span class="de-hint">{{ item.hint }}</span>
      </div>
    </template>
    <div v-else-if="expanded && !defaults" class="de-hint">
      未启用 defaults：阈值/时序取引擎内置值（threshold 0.80 等）
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 脚本默认值编辑器（v3 defaults，契约 §1/§4，取代 v2 config{interval,threshold,log_level}）：
 * vision.threshold + timing{after_tap, after_match, poll_interval}。
 * defaults 可整体缺省（引擎内置默认），启用/清除经 set_defaults 提交。
 */
import { computed, ref, type PropType } from 'vue'
import type { EditorModel } from '../commands'
import type { DefaultsModel, Program } from '../model'
import { parseTimeMs, TIME_UNITS } from '../schema'

const props = defineProps({
  model: { type: Object as PropType<EditorModel>, required: true },
  stack: { type: Object as PropType<{ apply: (c: unknown, n?: string) => boolean }>, required: true },
})

const EMPTY_DEFAULTS: DefaultsModel = {
  vision_threshold: null,
  after_tap: null,
  after_match: null,
  poll_interval: null,
}

const TIMING_ITEMS = [
  { key: 'after_tap', label: '点击后', hint: 'defaults.timing.after_tap（每次 tap 后等待）' },
  { key: 'after_match', label: '命中后', hint: 'defaults.timing.after_match（匹配命中后等待）' },
  { key: 'poll_interval', label: '轮询', hint: 'defaults.timing.poll_interval（find/check 轮询间隔）' },
] as const

type TimingKey = (typeof TIMING_ITEMS)[number]['key']

const defaults = computed<DefaultsModel | null>(() => (props.model as Program).defaults ?? null)

/** 默认值区默认收起（头部摘要常驻），展开/收起由头部按钮切换。 */
const expanded = ref(false)

const threshold = computed(() => {
  const t = defaults.value?.vision_threshold
  return typeof t === 'number' && Number.isFinite(t) ? t : 0.8
})
const thresholdInvalid = computed(() => {
  const d = defaults.value
  if (!d) return false
  const t = d.vision_threshold
  return d.vision_threshold !== null && (typeof t !== 'number' || !Number.isFinite(t) || t < 0 || t > 1)
})

function timingParts(key: TimingKey): [string, string] {
  const raw = defaults.value?.[key]
  if (typeof raw === 'number') return [String(raw), 'ms']
  const m = /^([0-9]+(?:\.[0-9]+)?)(ms|s|m|h)$/.exec(String(raw ?? ''))
  return m ? [m[1], m[2]] : ['100', 'ms']
}
function timingInvalid(key: TimingKey): boolean {
  const raw = defaults.value?.[key]
  if (raw === null || raw === undefined) return false
  return typeof raw === 'number' ? !(Number.isFinite(raw) && raw >= 0) : parseTimeMs(raw) === null
}

const hasIssues = computed(() =>
  defaults.value !== null && (thresholdInvalid.value || TIMING_ITEMS.some((it) => timingInvalid(it.key))),
)

function apply(patch: Partial<DefaultsModel>): boolean {
  const cur = defaults.value
  if (!cur) return false
  return props.stack.apply({ type: 'set_defaults', defaults: { ...cur, ...patch } }, '修改脚本默认值')
}

function enable(): void {
  expanded.value = true
  props.stack.apply({ type: 'set_defaults', defaults: { ...EMPTY_DEFAULTS, vision_threshold: 0.85 } }, '启用脚本默认值')
}
function clear(): void {
  props.stack.apply({ type: 'set_defaults', defaults: null }, '清除脚本默认值')
}
function setThreshold(raw: string): void {
  const n = Number(raw)
  if (!Number.isFinite(n)) return
  apply({ vision_threshold: Math.min(1, Math.max(0, n)) })
}
function setTimingNum(key: TimingKey, e: Event): void {
  const raw = (e.target as HTMLInputElement).value
  if (raw === '' || !Number.isFinite(Number(raw))) return
  apply({ [key]: `${raw}${timingParts(key)[1]}` } as Partial<DefaultsModel>)
}
function setTimingUnit(key: TimingKey, unit: string): void {
  apply({ [key]: `${timingParts(key)[0]}${unit}` } as Partial<DefaultsModel>)
}
</script>

<style scoped>
.defaults-editor {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  padding: 8px 10px;
  margin: 6px 0;
}
.de-head { display: flex; align-items: center; gap: 8px; }
.de-title { font-weight: 600; font-size: 13px; }
.de-sub { font-size: 12px; color: var(--text-2); flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.de-row { display: flex; align-items: center; gap: 8px; margin-top: 8px; flex-wrap: wrap; }
.de-label { font-size: 12px; color: var(--text-2); min-width: 56px; }
.de-hint { font-size: 11px; color: var(--text-2); }
.de-range { width: 160px; accent-color: var(--accent); }
.de-err { font-size: 11px; color: var(--danger); }
.de-err-badge {
  flex: none; font-size: 11px; color: var(--danger);
  border: 1px solid var(--danger); border-radius: 4px; padding: 0 5px;
}
.cell-input {
  background: var(--bg-2); color: var(--text-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 3px 6px; font-size: 12px; min-width: 60px;
}
.cell-input:focus { outline: none; border-color: var(--accent); }
.cell-input.num { width: 76px; }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 11px; padding: 2px 6px; cursor: pointer;
}
.mini-btn:hover:not(:disabled) { color: var(--accent); border-color: var(--accent); }
.mini-btn.danger:hover:not(:disabled) { color: var(--danger); border-color: var(--danger); }
.mini-btn.add { color: var(--accent-2); }
</style>
