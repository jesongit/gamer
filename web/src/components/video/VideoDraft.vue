<template>
  <section class="video-draft" data-testid="video-draft">
    <div class="zone-head">
      <span class="zone-title">草稿</span>
      <span class="zone-warn-tag" title="生成/保存只是草稿工作流：不创建任务、不启动 Runner、不自动执行">草稿不会自动执行</span>
    </div>

    <div v-if="!yamlReady" class="zone-error" role="alert" data-testid="draft-dep-banner">
      需要「自动化」插件（gamer.yaml）处于运行状态：草稿生成与保存经其公开动作完成。
      请在「插件」中心安装并启动 gamer.yaml。视频导入/录制/播放/标记不受影响。
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
        <span class="events-summary mono">事件 {{ events.length }} 条 · 已选 {{ selectedIds.length }}（步骤按选择顺序生成）</span>
        <span class="events-actions">
          <button class="mini-btn" type="button" data-testid="draft-select-all" @click="selectAll">全选</button>
          <button class="mini-btn" type="button" data-testid="draft-clear" @click="clearSelection">清空</button>
        </span>
      </div>
      <div class="event-list" data-testid="draft-event-list">
        <label v-for="ev in events" :key="ev.event_id" class="event-row" :class="{ checked: checkedSet[ev.event_id] }">
          <input
            type="checkbox"
            class="event-check"
            :checked="!!checkedSet[ev.event_id]"
            :data-testid="`draft-event-check-${ev.event_id}`"
            @change="toggleEvent(ev.event_id, $event.target.checked)"
          />
          <span class="event-kind mono">{{ ev.kind }}</span>
          <span class="mono event-time">{{ fmtTime(ev.timeline_us) }}</span>
          <span class="event-source mono">{{ ev.source }}</span>
          <span class="event-payload mono" :title="payloadFull(ev)">{{ payloadSummary(ev) }}</span>
          <input
            v-model="comments[ev.event_id]"
            class="input event-comment"
            placeholder="注释…"
            :data-testid="`draft-event-comment-${ev.event_id}`"
            @click.stop
          />
          <span class="event-order" @click.stop>
            <button class="mini-btn" type="button" :disabled="!checkedSet[ev.event_id]" :data-testid="`draft-event-up-${ev.event_id}`" title="上移（重排生成顺序）" @click="moveEvent(ev.event_id, -1)">↑</button>
            <button class="mini-btn" type="button" :disabled="!checkedSet[ev.event_id]" :data-testid="`draft-event-down-${ev.event_id}`" title="下移" @click="moveEvent(ev.event_id, 1)">↓</button>
          </span>
        </label>
      </div>
      <div class="generate-row">
        <button
          class="btn btn-sm btn-primary"
          type="button"
          :disabled="!yamlReady || !selectedIds.length || generating"
          :title="yamlReady ? '' : '需要 gamer.yaml 扩展运行中'"
          data-testid="draft-generate"
          @click="generate"
        >{{ generating ? '生成中…' : `⚡ 生成 YAML 草稿（${selectedIds.length}）` }}</button>
      </div>
    </template>
    <div v-else-if="loadedOnce" class="zone-empty">该会话没有可映射的操作事件</div>

    <template v-if="yaml">
      <div class="yaml-head">
        <span class="events-summary">草稿预览（v3 文本，请人工检查后保存）<template v-if="sourceLine"> · <span class="mono" data-testid="draft-source-line">{{ sourceLine }}</span></template></span>
        <button class="mini-btn" type="button" data-testid="draft-copy" @click="copyYaml">{{ copied ? '已复制 ✓' : '复制' }}</button>
      </div>
      <pre class="yaml-view" data-testid="draft-yaml">{{ yaml }}</pre>

      <!-- 保存（§10.3）：生成文本预览 → 用户确认命名 → automation.save_draft（v3 校验） -->
      <div class="save-row" data-testid="draft-save-row">
        <input v-model.trim="saveName" class="input save-name mono" placeholder="保存为自动化脚本名" data-testid="draft-save-name" />
        <label class="check-row"><input v-model="overwrite" type="checkbox" data-testid="draft-overwrite" /> 覆盖同名</label>
        <button
          class="btn btn-sm btn-primary"
          type="button"
          :disabled="!yamlReady || !yaml || !saveName || saving"
          data-testid="draft-save"
          @click="saveDraft"
        >{{ saving ? '保存中…' : '💾 保存到当前 Package' }}</button>
      </div>

      <div v-if="diagnostics.length" class="diag-box" data-testid="draft-diagnostics">
        <div class="diag-title">未映射事件（{{ diagnostics.length }}）——不丢弃不猜测，需人工处理：</div>
        <div v-for="d in diagnostics" :key="d.event_id" class="diag-row">
          <span class="mono">{{ shortId(d.event_id) }}</span>
          <span class="diag-reason">{{ d.reason }}</span>
        </div>
      </div>
    </template>

    <div v-else-if="diagnostics.length && !yaml" class="diag-box" data-testid="draft-diagnostics-only">
      <div class="diag-title">未映射事件（{{ diagnostics.length }}）——不丢弃不猜测，需人工处理：</div>
      <div v-for="d in diagnostics" :key="d.event_id" class="diag-row">
        <span class="mono">{{ shortId(d.event_id) }}</span>
        <span class="diag-reason">{{ d.reason }}</span>
      </div>
    </div>
  </section>
</template>

<script setup>
// 草稿区（Phase 7 §10.3 可编辑工作流）：
// - 事件选择/删除（取消勾选）/重排（↑↓ 调整生成顺序）/注释（步骤上方注释行）；
// - 生成经 gamer.yaml 动作清单缝 automation.create_draft（带 comments + 回查
//   source：录制会话 id + 事件时间轴，草稿 JSON 保留供回查录像帧）；
// - 保存经 automation.save_draft（服务端 v3 保存校验 + 重名 overwrite 门禁），
//   成功后经 automation.open_editor 前端契约打开/定位 YAML 编辑器；
// - 生成/保存不创建任务、不启动 Runner、不自动执行；真机运行为显式动作
//   （现有 YAML 运行机制，Phase 9 验证）。
// gamer.yaml 未 Running：生成/保存禁用 + 依赖提示（视频其余能力不受影响）。
import { computed, reactive, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { requestAutomationEditor } from '../console/automationEditorBridge'
import { GAMER_YAML_AUTOMATION_PANEL_KEY } from '../../gamer-plugin-ids'
import { videoApi } from './videoApi'

const props = defineProps({
  recordingId: { type: String, default: '' },
  packageId: { type: String, default: '' },
  /** gamer.yaml 是否 Running（§10.1 依赖门禁）。 */
  yamlReady: { type: Boolean, default: false },
})
const emit = defineEmits(['update:recordingId'])

const router = useRouter()
const ridInput = ref(props.recordingId)
const events = ref([])
const checkedSet = reactive({})
/** 选中顺序 = 生成步骤顺序（create_draft 按请求顺序映射，§10.3 重排）。 */
const selectedIds = ref([])
const comments = reactive({})
const loadingEvents = ref(false)
const generating = ref(false)
const loadedOnce = ref(false)
const error = ref('')
const yaml = ref('')
const diagnostics = ref([])
const draftSource = ref(null) // {recording_id, events:[…]}（回查信息，草稿 JSON 的一部分）
const copied = ref(false)
const saveName = ref('')
const overwrite = ref(false)
const saving = ref(false)

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
    resetSelection()
    yaml.value = ''
    diagnostics.value = []
    draftSource.value = null
    loadedOnce.value = true
    emitRecordingId()
  } catch (e) {
    error.value = `载入事件失败：${e?.message || e}`
  } finally {
    loadingEvents.value = false
  }
}

function resetSelection() {
  for (const key of Object.keys(checkedSet)) delete checkedSet[key]
  for (const key of Object.keys(comments)) delete comments[key]
  selectedIds.value = []
}

function toggleEvent(eventId, checked) {
  if (checked) {
    if (!checkedSet[eventId]) {
      checkedSet[eventId] = true
      selectedIds.value = [...selectedIds.value, eventId]
    }
  } else {
    delete checkedSet[eventId]
    selectedIds.value = selectedIds.value.filter(id => id !== eventId)
  }
}

function moveEvent(eventId, direction) {
  const ids = [...selectedIds.value]
  const index = ids.indexOf(eventId)
  const target = index + direction
  if (index < 0 || target < 0 || target >= ids.length) return
  ;[ids[index], ids[target]] = [ids[target], ids[index]]
  selectedIds.value = ids
}

function selectAll() {
  for (const ev of events.value) checkedSet[ev.event_id] = true
  selectedIds.value = events.value.map(ev => ev.event_id)
}

function clearSelection() {
  resetSelection()
}

/** 回查信息一行（草稿 JSON 的 source 摘要）。 */
const sourceLine = computed(() => {
  const source = draftSource.value
  if (!source?.recording_id) return ''
  const rows = Array.isArray(source.events) ? source.events : []
  const picked = rows.filter(ev => ev.selected)
  if (!picked.length) return `来源 ${source.recording_id}`
  const first = picked[0]?.timeline_us ?? 0
  const last = picked[picked.length - 1]?.timeline_us ?? 0
  return `来源 ${source.recording_id} · ${picked.length} 事件 · ${(first / 1e6).toFixed(2)}s~${(last / 1e6).toFixed(2)}s`
})

async function generate() {
  if (!selectedIds.value.length || generating.value) return
  generating.value = true
  error.value = ''
  copied.value = false
  try {
    const activeComments = {}
    for (const id of selectedIds.value) {
      const text = String(comments[id] ?? '').trim()
      if (text) activeComments[id] = text
    }
    const result = await videoApi.createVideoDraft(ridInput.value, selectedIds.value, activeComments)
    yaml.value = String(result?.yaml ?? '')
    diagnostics.value = Array.isArray(result?.diagnostics) ? result.diagnostics : []
    draftSource.value = result?.source && typeof result.source === 'object' ? result.source : null
    if (!yaml.value && !diagnostics.value.length) {
      error.value = '草稿生成返回为空，请检查 gamer.yaml 扩展是否支持 automation.create_draft'
    }
  } catch (e) {
    error.value = `生成草稿失败：${e?.message || e}`
  } finally {
    generating.value = false
  }
}

async function saveDraft() {
  if (saving.value || !yaml.value || !saveName.value) return
  saving.value = true
  error.value = ''
  try {
    const result = await videoApi.saveDraft({
      packageId: props.packageId,
      name: saveName.value,
      yaml: yaml.value,
      overwrite: overwrite.value,
    })
    // automation.open_editor（前端契约）：保存成功即打开/定位 YAML 编辑器
    const scriptId = String(result?.id || '')
    if (scriptId) {
      requestAutomationEditor(props.packageId, scriptId)
      await router.push({ query: { ...(router.currentRoute.value?.query || {}), panel: GAMER_YAML_AUTOMATION_PANEL_KEY } }).catch(() => {})
    }
  } catch (e) {
    const message = String(e?.message || e)
    if (message.includes('已存在')) {
      error.value = `${message}（可勾选「覆盖同名」后重试）`
    } else {
      error.value = `保存草稿失败：${message}`
    }
  } finally {
    saving.value = false
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
.event-list { display: flex; flex-direction: column; border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden auto; max-height: 170px; flex-shrink: 0; }
.event-row { display: flex; align-items: center; gap: 5px; padding: 4px 8px; font-size: 11px; color: var(--text-1); border-bottom: 1px solid rgba(80,92,119,.25); min-height: 26px; }
.event-row:last-child { border-bottom: 0; }
.event-row:hover { background: var(--bg-3); }
.event-row.checked { background: rgba(34,211,165,.06); }
.event-check { width: 13px; height: 13px; margin: 0; flex: none; }
.event-kind { min-width: 36px; color: var(--accent-2); }
.event-time { min-width: 58px; color: var(--text-2); }
.event-source { min-width: 40px; color: var(--text-1); }
.event-payload { color: var(--text-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.event-comment { flex: 1 1 110px; min-width: 72px; max-width: 170px; padding: 2px 5px; font-size: 10px; }
.event-order { display: flex; gap: 2px; flex: none; }
.generate-row, .save-row { display: flex; justify-content: flex-end; align-items: center; gap: 8px; }
.save-row { justify-content: flex-start; }
.save-name { flex: 1; min-width: 0; padding: 4px 7px; font-size: 11px; }
.check-row { display: flex; align-items: center; gap: 4px; font-size: 11px; color: var(--text-1); white-space: nowrap; user-select: none; }
.yaml-view { margin: 0; padding: 8px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-0); color: var(--text-0); font: 11px/1.55 var(--mono); max-height: 220px; overflow: auto; white-space: pre; }
.mini-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 2px 7px; }
.mini-btn:hover { border-color: var(--accent); color: var(--accent); }
.mini-btn:disabled { opacity: .45; cursor: not-allowed; }
.diag-box { padding: 6px 8px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.06); display: flex; flex-direction: column; gap: 3px; max-height: 120px; overflow: auto; }
.diag-title { color: var(--danger); font-size: 11px; }
.diag-row { display: flex; gap: 8px; font-size: 10px; color: var(--text-1); }
.diag-reason { color: var(--text-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mono { font-family: var(--mono); }
</style>
