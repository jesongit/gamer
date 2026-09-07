<template>
  <section class="video-draft" data-testid="video-draft">
    <div class="zone-head">
      <span class="zone-title">草稿</span>
      <span class="zone-warn-tag" title="草稿仅返回文本：不落盘、不执行">草稿不会自动执行</span>
    </div>

    <div class="rid-row">
      <input
        v-model.trim="ridInput"
        class="input rid-input mono"
        placeholder="录制会话 id（recording id）"
        data-testid="draft-recording-id"
        @change="emitRecordingId"
      />
      <button class="btn btn-sm" type="button" :disabled="!ridInput || loadingEvents" data-testid="draft-load" @click="loadEvents">
        {{ loadingEvents ? '读取中…' : '载入事件' }}
      </button>
    </div>

    <div v-if="error" class="zone-error" role="alert" data-testid="draft-error">{{ error }}</div>

    <template v-if="events.length">
      <div class="events-head">
        <span class="events-summary mono">会话事件 {{ events.length }} 条 · 已选 {{ checkedIds.length }}</span>
        <span class="events-actions">
          <button class="mini-btn" type="button" data-testid="draft-select-all" @click="selectAll">全选</button>
          <button class="mini-btn" type="button" data-testid="draft-clear" @click="checkedIds = []">清空</button>
        </span>
      </div>
      <div class="event-list" data-testid="draft-event-list">
        <label v-for="ev in events" :key="ev.event_id" class="event-row" :class="{ checked: checkedSet[ev.event_id] }">
          <input v-model="checkedIds" type="checkbox" :value="ev.event_id" class="event-check" />
          <span class="event-kind mono">{{ ev.kind }}</span>
          <span class="mono event-time">{{ fmtTime(ev.timeline_us) }}</span>
          <span class="event-source mono">{{ ev.source }}</span>
          <span class="event-payload mono" :title="payloadFull(ev)">{{ payloadSummary(ev) }}</span>
        </label>
      </div>
      <div class="generate-row">
        <button
          class="btn btn-sm btn-primary"
          type="button"
          :disabled="!checkedIds.length || generating"
          data-testid="draft-generate"
          @click="generate"
        >{{ generating ? '生成中…' : `⚡ 生成 YAML 草稿（${checkedIds.length}）` }}</button>
      </div>
    </template>
    <div v-else-if="loadedOnce" class="zone-empty">该会话没有可映射的操作事件</div>

    <template v-if="yaml">
      <div class="yaml-head">
        <span class="events-summary">草稿预览（v3 文本，请人工检查后自行保存到自动化脚本）</span>
        <button class="mini-btn" type="button" data-testid="draft-copy" @click="copyYaml">{{ copied ? '已复制 ✓' : '复制' }}</button>
      </div>
      <pre class="yaml-view" data-testid="draft-yaml">{{ yaml }}</pre>
    </template>

    <div v-if="diagnostics.length" class="diag-box" data-testid="draft-diagnostics">
      <div class="diag-title">未映射事件（{{ diagnostics.length }}）——不丢弃不猜测，需人工处理：</div>
      <div v-for="d in diagnostics" :key="d.event_id" class="diag-row">
        <span class="mono">{{ shortId(d.event_id) }}</span>
        <span class="diag-reason">{{ d.reason }}</span>
      </div>
    </div>
  </section>
</template>

<script setup>
// 草稿区：勾选录制会话的操作事件 → createVideoDraft（gamer.yaml automation.create_draft）
// → 展示 v3 YAML 文本与未映射诊断。草稿只是文本返回：不落盘、不执行（合同 §5）。
import { computed, reactive, ref, watch } from 'vue'
import { videoApi } from './videoApi'

const props = defineProps({
  recordingId: { type: String, default: '' },
})
const emit = defineEmits(['update:recordingId'])

const ridInput = ref(props.recordingId)
const events = ref([])
const checkedSet = reactive({})
const loadingEvents = ref(false)
const generating = ref(false)
const loadedOnce = ref(false)
const error = ref('')
const yaml = ref('')
const diagnostics = ref([])
const copied = ref(false)

const checkedIds = computed({
  get: () => events.value.filter(ev => checkedSet[ev.event_id]).map(ev => ev.event_id),
  set: (ids) => {
    for (const key of Object.keys(checkedSet)) delete checkedSet[key]
    for (const id of ids) checkedSet[id] = true
  },
})

watch(() => props.recordingId, (value) => {
  if (value && value !== ridInput.value) {
    ridInput.value = value
    // 录制完成自动带入会话 id：直接载入事件，减少一步操作
    if (!loadedOnce.value) void loadEvents()
  }
})

function emitRecordingId() {
  emit('update:recordingId', ridInput.value)
}

async function loadEvents() {
  if (!ridInput.value || loadingEvents.value) return
  loadingEvents.value = true
  error.value = ''
  try {
    const list = await videoApi.recordingEvents(ridInput.value)
    // 合同要求服务端时间轴升序；此处防御性重排，不依赖服务端实现细节
    events.value = [...list].sort((a, b) => Number(a.timeline_us || 0) - Number(b.timeline_us || 0))
    for (const key of Object.keys(checkedSet)) delete checkedSet[key]
    yaml.value = ''
    diagnostics.value = []
    loadedOnce.value = true
    emitRecordingId()
  } catch (e) {
    error.value = `载入事件失败：${e?.message || e}`
  } finally {
    loadingEvents.value = false
  }
}

function selectAll() {
  checkedIds.value = events.value.map(ev => ev.event_id)
}

async function generate() {
  const ids = checkedIds.value
  if (!ids.length || generating.value) return
  generating.value = true
  error.value = ''
  copied.value = false
  try {
    const result = await videoApi.createVideoDraft(ridInput.value, ids)
    yaml.value = String(result?.yaml ?? '')
    diagnostics.value = Array.isArray(result?.diagnostics) ? result.diagnostics : []
    if (!yaml.value && !diagnostics.value.length) {
      error.value = '草稿生成返回为空，请检查 gamer.yaml 扩展是否支持 automation.create_draft'
    }
  } catch (e) {
    error.value = `生成草稿失败：${e?.message || e}`
  } finally {
    generating.value = false
  }
}

async function copyYaml() {
  if (!yaml.value) return
  let ok = false
  try {
    await navigator.clipboard.writeText(yaml.value)
    ok = true
  } catch (e) {
    // 非安全上下文等场景退化为选区复制
    try {
      const ta = document.createElement('textarea')
      ta.value = yaml.value
      document.body.appendChild(ta)
      ta.select()
      ok = document.execCommand('copy')
      ta.remove()
    } catch (e2) { ok = false }
  }
  copied.value = ok
  if (ok) setTimeout(() => { copied.value = false }, 1600)
}

function fmtTime(timelineUs) {
  const us = Number(timelineUs)
  return Number.isFinite(us) ? `${(us / 1e6).toFixed(3)}s` : '—'
}

function payloadSummary(ev) {
  const p = ev?.payload
  if (!p || typeof p !== 'object') return ''
  if (p.x !== undefined && p.y !== undefined) {
    return p.x2 !== undefined && p.y2 !== undefined
      ? `(${p.x},${p.y})→(${p.x2},${p.y2})`
      : `(${p.x},${p.y})`
  }
  if (p.code !== undefined) return String(p.code)
  if (p.length !== undefined) return `${p.length} 字符`
  if (p.duration_us !== undefined) return `${(Number(p.duration_us) / 1e6).toFixed(2)}s`
  return ''
}

function payloadFull(ev) {
  const p = ev?.payload
  return p && typeof p === 'object' ? JSON.stringify(p) : ''
}

function shortId(id) {
  const s = String(id || '')
  return s.length > 12 ? `${s.slice(0, 12)}…` : s
}
</script>

<style scoped>
.video-draft { display: flex; flex-direction: column; gap: 8px; min-height: 0; }
.zone-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; flex-shrink: 0; }
.zone-title { color: var(--text-0); font-size: 13px; font-weight: 700; }
.zone-warn-tag { padding: 1px 6px; border: 1px solid rgba(251,191,36,.4); border-radius: 4px; color: var(--warn); background: rgba(251,191,36,.08); font-size: 10px; white-space: nowrap; }
.rid-row { display: flex; gap: 6px; min-width: 0; }
.rid-input { flex: 1; min-width: 0; padding: 5px 8px; font-size: 11px; }
.zone-error { padding: 5px 7px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.08); color: var(--danger); font-size: 11px; line-height: 1.5; word-break: break-all; }
.zone-empty { padding: 10px; text-align: center; color: var(--text-2); font-size: 11px; }
.events-head, .yaml-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.events-summary { color: var(--text-2); font-size: 11px; }
.events-actions { display: flex; gap: 4px; }
.event-list { display: flex; flex-direction: column; border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden auto; max-height: 150px; flex-shrink: 0; }
.event-row { display: flex; align-items: center; gap: 6px; padding: 4px 8px; font-size: 11px; color: var(--text-1); border-bottom: 1px solid rgba(80,92,119,.25); cursor: pointer; min-height: 26px; }
.event-row:last-child { border-bottom: 0; }
.event-row:hover { background: var(--bg-3); }
.event-row.checked { background: rgba(34,211,165,.06); }
.event-check { width: 13px; height: 13px; margin: 0; flex: none; }
.event-kind { min-width: 38px; color: var(--accent-2); }
.event-time { min-width: 62px; color: var(--text-2); }
.event-source { min-width: 44px; color: var(--text-1); }
.event-payload { color: var(--text-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.generate-row { display: flex; justify-content: flex-end; }
.yaml-view { margin: 0; padding: 8px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-0); color: var(--text-0); font: 11px/1.55 var(--mono); max-height: 220px; overflow: auto; white-space: pre; }
.mini-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 2px 7px; }
.mini-btn:hover { border-color: var(--accent); color: var(--accent); }
.diag-box { padding: 6px 8px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.06); display: flex; flex-direction: column; gap: 3px; max-height: 120px; overflow: auto; }
.diag-title { color: var(--danger); font-size: 11px; }
.diag-row { display: flex; gap: 8px; font-size: 10px; color: var(--text-1); }
.diag-reason { color: var(--text-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mono { font-family: var(--mono); }
</style>
