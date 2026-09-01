<template>
  <div class="config-editor" data-testid="config-editor" @click.stop>
    <div class="ce-head">
      <span class="ce-title">运行配置</span>
      <span class="ce-sub">点击后/轮询 {{ config?.interval ?? '—' }} · 阈值 {{ config ? config.threshold : '—' }} · {{ config?.log_level ?? '—' }}</span>
      <span v-if="!expanded && hasIssues" class="ce-err-badge" title="配置有误，展开查看">有误</span>
      <button v-if="!config" type="button" class="mini-btn add" @click="enable">启用配置</button>
      <button v-else type="button" class="mini-btn danger" title="清除配置（使用服务端默认值）" @click="clear">清除</button>
      <button type="button" class="mini-btn" :title="expanded ? '收起运行配置' : '展开运行配置'" @click="expanded = !expanded">
        {{ expanded ? '收起 ▴' : '展开 ▾' }}
      </button>
    </div>

    <template v-if="config">
      <template v-if="expanded">
        <div class="ce-row">
          <span class="ce-label">点击后/轮询</span>
          <input
            class="cell-input num" type="number" min="0" step="any"
            :value="intervalParts[0]" aria-label="轮询间隔数值" @input="setIntervalNum($event)"
          />
          <select
            class="cell-input" :value="intervalParts[1]" aria-label="轮询间隔单位"
            @change="setIntervalUnit(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="u in TIME_UNITS" :key="u" :value="u">{{ u }}</option>
          </select>
          <span v-if="intervalInvalid" class="ce-err">interval（轮询/点击后等待）须 &gt;0 且带单位（ms/s/m/min/h/d）</span>
        </div>

        <div class="ce-row">
          <span class="ce-label">匹配阈值</span>
          <input
            class="ce-range" type="range" min="0" max="1" step="0.01"
            :value="config.threshold" aria-label="匹配阈值滑块" @input="setThreshold(($event.target as HTMLInputElement).value)"
          />
          <input
            class="cell-input num" type="number" min="0" max="1" step="0.01"
            :value="config.threshold" aria-label="匹配阈值数值" @change="setThreshold(($event.target as HTMLInputElement).value)"
          />
          <span v-if="thresholdInvalid" class="ce-err">threshold 必须是 0~1 的数字</span>
        </div>

        <div class="ce-row">
          <span class="ce-label">日志级别</span>
          <select
            class="cell-input" :value="config.log_level" aria-label="日志级别"
            @change="setLogLevel(($event.target as HTMLSelectElement).value as LogLevel)"
          >
            <option v-for="lv in LOG_LEVELS" :key="lv" :value="lv">{{ lv }}</option>
          </select>
        </div>

        <div v-if="extraKeys.length" class="ce-err">
          存在未知配置键 {{ extraKeys.join('、') }}——仅支持 interval / threshold / log_level，保存时会被拒绝
        </div>
      </template>
    </template>
    <div v-else-if="expanded" class="ce-hint">未启用配置：interval/threshold/log_level 取服务端 config.toml 默认值</div>
  </div>
</template>

<script setup lang="ts">
/**
 * 配置编辑器（plan §9 config 行）：interval 点击后/轮询间隔数值+单位、threshold 滑块+数值、log_level 下拉。
 * config 可整体缺省（用服务端默认值），启用/清除经 set_config 提交；未知键按 schema 提示。
 */
import { computed, ref, type PropType } from 'vue'
import type { EditorModel } from '../commands'
import { LOG_LEVELS, type LogLevel, type ScriptConfig, type ScriptModel } from '../model'
import { parseTimeMs } from '../schema'

const props = defineProps({
  model: { type: Object as PropType<EditorModel>, required: true },
  stack: { type: Object as PropType<{ apply: (c: unknown, n?: string) => boolean }>, required: true },
})

const TIME_UNITS = ['ms', 's', 'm', 'min', 'h', 'd'] as const
const KNOWN_KEYS = ['interval', 'threshold', 'log_level']

const config = computed<ScriptConfig | null>(() => (props.model as ScriptModel).config ?? null)

/** 配置区默认收起（头部摘要常驻），展开/收起由头部按钮切换。 */
const expanded = ref(false)

const intervalParts = computed<[string, string]>(() => {
  const m = /^([0-9]+(?:\.[0-9]+)?)(ms|s|m|min|h|d)$/.exec(config.value?.interval ?? '')
  return m ? [m[1], m[2]] : ['500', 'ms']
})
const intervalInvalid = computed(() => config.value ? parseTimeMs(config.value.interval) === null : false)
const thresholdInvalid = computed(() => {
  const t = config.value?.threshold
  return config.value ? (typeof t !== 'number' || !Number.isFinite(t) || t < 0 || t > 1) : false
})
const extraKeys = computed<string[]>(() => {
  const c = config.value as unknown as Record<string, unknown> | null
  return c ? Object.keys(c).filter((k) => !KNOWN_KEYS.includes(k)) : []
})
/** 收起态配置错误不可见，头部以「有误」徽标提示。 */
const hasIssues = computed(() =>
  config.value !== null && (intervalInvalid.value || thresholdInvalid.value || extraKeys.value.length > 0),
)

function apply(patch: Partial<ScriptConfig>): boolean {
  const cur = config.value
  if (!cur) return false
  return props.stack.apply({ type: 'set_config', config: { ...cur, ...patch } }, '修改运行配置')
}

function enable(): void {
  expanded.value = true
  props.stack.apply(
    { type: 'set_config', config: { interval: '500ms', threshold: 0.85, log_level: 'info' } },
    '启用运行配置',
  )
}
function clear(): void {
  props.stack.apply({ type: 'set_config', config: null }, '清除运行配置')
}
function setIntervalNum(e: Event): void {
  const raw = (e.target as HTMLInputElement).value
  if (raw === '' || !Number.isFinite(Number(raw))) return
  apply({ interval: `${raw}${intervalParts.value[1]}` })
}
function setIntervalUnit(unit: string): void {
  apply({ interval: `${intervalParts.value[0]}${unit}` })
}
function setThreshold(raw: string): void {
  const n = Number(raw)
  if (!Number.isFinite(n)) return
  apply({ threshold: Math.min(1, Math.max(0, n)) })
}
function setLogLevel(level: LogLevel): void {
  apply({ log_level: level })
}
</script>

<style scoped>
.config-editor {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  padding: 8px 10px;
  margin: 6px 0;
}
.ce-head { display: flex; align-items: center; gap: 8px; }
.ce-title { font-weight: 600; font-size: 13px; }
.ce-sub { font-size: 12px; color: var(--text-2); flex: 1; }
.ce-row { display: flex; align-items: center; gap: 8px; margin-top: 8px; flex-wrap: wrap; }
.ce-label { font-size: 12px; color: var(--text-2); min-width: 56px; }
.ce-range { width: 160px; accent-color: var(--accent); }
.ce-err { font-size: 11px; color: var(--danger); }
.ce-err-badge {
  flex: none; font-size: 11px; color: var(--danger);
  border: 1px solid var(--danger); border-radius: 4px; padding: 0 5px;
}
.ce-hint { font-size: 12px; color: var(--text-2); }
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
.mini-btn:hover { color: var(--accent); border-color: var(--accent); }
.mini-btn.danger:hover { color: var(--danger); border-color: var(--danger); }
.mini-btn.add { color: var(--accent-2); }
</style>
