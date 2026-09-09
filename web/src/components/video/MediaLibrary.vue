<template>
  <section class="media-library" data-testid="media-library">
    <div class="zone-head">
      <span class="zone-title">素材库</span>
      <span class="zone-actions">
        <button class="btn btn-sm" type="button" :disabled="loading" data-testid="media-refresh" @click="$emit('refresh')">↻ 刷新</button>
        <button class="btn btn-sm btn-primary" type="button" :disabled="importing" data-testid="media-import" @click="pickFile">
          {{ importing ? '上传中…' : '⬆ 导入视频' }}
        </button>
        <input
          ref="fileInput"
          type="file"
          accept="video/*"
          class="file-hidden"
          data-testid="media-file-input"
          @change="onFileChosen"
        />
      </span>
    </div>

    <!-- 录制入口：设备下拉 + 3s 轮询活动会话驱动 开始/停止/取消 -->
    <div class="record-box">
      <div class="record-row">
        <select v-model="deviceId" class="select record-device" data-testid="record-device" :disabled="!devices.length">
          <option v-if="!devices.length" :value="0">暂无设备（先在投屏页添加）</option>
          <option v-for="d in devices" :key="d.id" :value="d.id">{{ d.name || d.id }}</option>
        </select>
      </div>
      <div class="record-row">
        <button
          v-if="!session"
          class="btn btn-sm btn-primary"
          type="button"
          :disabled="!deviceId || busy"
          data-testid="record-start"
          @click="start"
        >● 开始录制</button>
        <template v-else>
          <button class="btn btn-sm btn-primary" type="button" :disabled="busy" data-testid="record-stop" @click="stop">■ 停止并保存</button>
          <button class="btn btn-sm btn-danger" type="button" :disabled="busy" data-testid="record-cancel" @click="cancel">取消</button>
        </template>
        <span v-if="session" class="record-state" data-testid="record-state">
          <span class="dot run"></span>
          {{ stateLabel(session.state) }} · 事件 {{ session.event_count ?? 0 }}
        </span>
      </div>
    </div>

    <div v-if="importing" class="zone-note" role="status" data-testid="media-import-progress">
      正在上传并探测「{{ importingName }}」…（当前导入接口不提供取消操作）
    </div>
    <div v-if="error" class="zone-error" role="alert" data-testid="media-error">{{ error }}</div>

    <div v-if="loadError" class="zone-error" role="alert" data-testid="media-load-error">
      素材读取失败：{{ loadError }}
      <button class="mini-btn" type="button" data-testid="media-retry" @click="$emit('refresh')">重试</button>
    </div>

    <div class="media-list" data-testid="media-list">
      <div class="list-head"><span>名称</span><span>时长</span><span>尺寸</span><span>来源</span><span>状态</span><span></span></div>
      <div v-if="loading && !mediaList.length" class="list-empty">读取中…</div>
      <div v-else-if="loadError" class="list-empty">素材读取失败，请点击“重试”</div>
      <div v-else-if="!mediaList.length" class="list-empty">暂无素材：导入视频或开始一次录制</div>
      <div
        v-for="m in mediaList"
        :key="m.id"
        class="media-row"
        :class="{ selected: m.id === selectedId }"
        data-testid="media-row"
        @click="$emit('select', m.id)"
      >
        <span class="media-name" :title="m.name">{{ m.name }}</span>
        <span class="mono">{{ fmtDuration(m.duration_us) }}</span>
        <span class="mono">{{ m.width }}×{{ m.height }}</span>
        <span class="mono">{{ sourceLabel(m.source) }}</span>
        <span class="tag" :class="stateTagClass(m.state)">{{ stateLabel(m.state) }}</span>
        <span class="row-actions">
          <button class="mini-btn danger" type="button" :class="{ armed: armedId === m.id }" @click.stop="remove(m)">
            {{ armedId === m.id ? '确认删除' : '删除' }}
          </button>
        </span>
      </div>
    </div>

    <section class="history-box" data-testid="recording-history">
      <div class="history-head">
        <span class="zone-title">录制历史</span>
        <span class="history-source-note">按素材库中真实录制元数据展示</span>
      </div>
      <div class="history-not-verified" role="note" data-testid="recording-history-not-verified">
        NOT_VERIFIED：后端暂未提供录制会话历史列表；当前仅展示 /api/media 返回的录制素材，未返回的会话 ID、设备和分段关系不会被推断。
      </div>
      <div class="history-filters" aria-label="录制历史筛选">
        <input v-model="historySearch" class="input" type="search" placeholder="名称" data-testid="recording-history-search" />
        <input v-model="historyDate" class="input" type="date" aria-label="时间" data-testid="recording-history-date" />
        <select v-model="historyDevice" class="select" data-testid="recording-history-device">
          <option value="">全部设备</option>
          <option v-if="!historyDevices.length" value="" disabled>设备字段未返回</option>
          <option v-for="device in historyDevices" :key="device" :value="device">{{ device }}</option>
        </select>
        <select v-model="historyDuration" class="select" data-testid="recording-history-duration">
          <option value="">全部时长</option>
          <option value="short">少于 1 分钟</option>
          <option value="medium">1–5 分钟</option>
          <option value="long">超过 5 分钟</option>
        </select>
        <select v-model="historyStatus" class="select" data-testid="recording-history-status">
          <option value="">全部状态</option>
          <option v-for="state in historyStates" :key="state" :value="state">{{ stateLabel(state) }}</option>
        </select>
      </div>

      <div v-if="historyLoading" class="list-empty" data-testid="recording-history-loading">读取录制历史…</div>
      <div v-else-if="loadError" class="list-empty" data-testid="recording-history-failed">录制历史读取失败，请点击“重试”</div>
      <div v-else-if="!recordingHistory.length" class="list-empty" data-testid="recording-history-empty">暂无录制历史</div>
      <div v-else-if="!filteredRecordingHistory.length" class="list-empty" data-testid="recording-history-no-match">没有符合条件的录制记录</div>
      <div v-else class="history-list">
        <div
          v-for="record in filteredRecordingHistory"
          :key="record.media.id"
          class="history-row"
          :class="{ selected: record.media.id === selectedId }"
          data-testid="recording-history-row"
          @click="selectHistory(record)"
        >
          <span class="history-name" :title="record.name">{{ record.name }}</span>
          <span class="mono">{{ record.dateLabel }}</span>
          <span class="mono">{{ record.deviceLabel }}</span>
          <span class="mono">{{ fmtDuration(record.durationUs) }}</span>
          <span class="tag" :class="stateTagClass(record.state)">{{ stateLabel(record.state) }}</span>
          <button class="mini-btn" type="button" data-testid="recording-history-select" @click.stop="selectHistory(record)">查看</button>
        </div>
      </div>
    </section>

    <section v-if="currentRecording" class="recording-detail" data-testid="recording-detail">
      <div class="history-head">
        <span class="zone-title">当前录制</span>
        <span class="tag" :class="stateTagClass(currentRecording.state)">{{ stateLabel(currentRecording.state) }}</span>
      </div>
      <div class="recording-meta">
        <span>{{ currentRecording.started_at ? formatDate(currentRecording.started_at) : '开始时间未知' }}</span>
        <span>设备：{{ currentRecording.device_id || '未返回' }}</span>
        <span>事件：{{ currentRecording.event_count ?? 0 }}</span>
        <span v-if="currentRecording.segments">分段：{{ currentRecording.segments.length }}</span>
      </div>
      <div v-if="currentRecording.error" class="history-event-error" data-testid="recording-terminal-error">{{ currentRecording.error }}</div>
      <div v-if="eventLoading" class="list-empty" data-testid="recording-events-loading">读取事件…</div>
      <div v-else-if="eventError" class="history-event-error" role="alert" data-testid="recording-events-error">{{ eventError }}</div>
      <div v-else-if="eventsLoaded && !eventRows.length" class="list-empty" data-testid="recording-events-empty">该录制没有可展示的事件</div>
      <div v-else-if="eventsLoaded" class="event-list" data-testid="recording-events-list">
        <div v-for="item in eventRows" :key="item.eventId" class="event-row">
          <span class="mono">{{ item.timelineLabel }}</span>
          <span>{{ item.sourceLabel }}</span>
          <span>{{ item.summary }}</span>
          <span class="tag" :class="item.status === 'accepted' ? 'ok' : 'info'">{{ item.statusLabel }}</span>
        </div>
      </div>
    </section>
  </section>
</template>

<script setup>
// 素材库区：列表（listMedia 数据由宿主注入）/导入（1GiB 内字节直传）/删除（409 = 被引用）/
// 录制入口（设备下拉复用全局 devicesData，activeRecording 3s 轮询驱动按钮态）。
// 本组件不解释素材内容语义；预览与精确帧在时间轴区。
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { devicesData } from '../../store'
import { videoApi } from './videoApi'
import { filterRecordingEvents } from './recordingEvents'

const props = defineProps({
  mediaList: { type: Array, default: () => [] },
  loading: { type: Boolean, default: false },
  selectedId: { type: String, default: '' },
  // 父层当前会吞掉 listMedia 错误；保留显式错误入口，避免把失败误报为空。
  loadError: { type: String, default: '' },
})
const emit = defineEmits(['select', 'refresh', 'changed', 'recording-finished', 'recording-selected'])

const devices = devicesData
// 设备 id 是 UUID 字符串；不能使用 v-model.number，否则以数字开头的 UUID
// 会被 Vue 的 parseFloat 截断（例如 "831d..." → 831）。
const deviceId = ref('')
const fileInput = ref(null)
const importing = ref(false)
const importingName = ref('')
const error = ref('')
const armedId = ref('')
const historySearch = ref('')
const historyDate = ref('')
const historyDevice = ref('')
const historyDuration = ref('')
const historyStatus = ref('')
const historyLoading = computed(() => props.loading && !props.mediaList.length)
const currentRecording = ref(null)
const eventRows = ref([])
const eventLoading = ref(false)
const eventError = ref('')
const eventsLoaded = ref(false)

const historyStates = ['recording', 'finalizing', 'completed', 'interrupted', 'failed', 'cancelled', 'ready', 'missing']

function recordingInfo(media) {
  const recording = media?.recording && typeof media.recording === 'object' ? media.recording : {}
  const durationUs = media?.duration_us ?? recording.duration_us
  const state = String(recording.state || media?.state || '—')
  const deviceId = recording.device_id ?? media?.device_id ?? media?.deviceId ?? ''
  const sessionId = recording.id ?? media?.recording_id ?? media?.recordingId ?? ''
  const date = recording.started_at || media?.created_at || recording.created_at || ''
  return {
    media,
    name: String(media?.name || media?.id || '未命名录制'),
    date,
    dateLabel: formatDate(date),
    deviceId: String(deviceId || ''),
    deviceLabel: deviceId ? String(deviceId) : '未返回',
    durationUs,
    state,
    sessionId: sessionId ? String(sessionId) : '',
  }
}

const recordingHistory = computed(() => (Array.isArray(props.mediaList) ? props.mediaList : [])
  .filter(media => String(media?.source || '').toLowerCase() === 'recording')
  .map(recordingInfo))

const historyDevices = computed(() => [...new Set(recordingHistory.value.map(item => item.deviceId).filter(Boolean))])

const filteredRecordingHistory = computed(() => recordingHistory.value.filter(record => {
  const search = historySearch.value.trim().toLowerCase()
  if (search && !`${record.name} ${record.media.id} ${record.sessionId}`.toLowerCase().includes(search)) return false
  if (historyDate.value && !String(record.date).startsWith(historyDate.value)) return false
  if (historyDevice.value && record.deviceId !== historyDevice.value) return false
  if (historyStatus.value && record.state !== historyStatus.value) return false
  const duration = Number(record.durationUs)
  if (historyDuration.value === 'short' && (!Number.isFinite(duration) || duration >= 60e6)) return false
  if (historyDuration.value === 'medium' && (!Number.isFinite(duration) || duration < 60e6 || duration > 300e6)) return false
  if (historyDuration.value === 'long' && (!Number.isFinite(duration) || duration <= 300e6)) return false
  return true
}))

// ---- 录制会话轮询（3s；无选中设备不轮询）----
const session = ref(null)
const busy = ref(false)
let pollTimer = null
let pollSeq = 0

async function pollActive() {
  const id = deviceId.value
  if (!id) {
    session.value = null
    return
  }
  const seq = ++pollSeq
  try {
    const active = await videoApi.activeRecording(id)
    if (seq === pollSeq) {
      session.value = active
      if (active && !currentRecording.value) currentRecording.value = active
    }
  } catch (e) {
    // 轮询失败不打断面板：保留上次结果，仅首次失败提示
    if (seq === pollSeq && !session.value) error.value = describe(e, '查询录制状态失败')
  }
}

function startPolling() {
  stopPolling()
  if (!deviceId.value) {
    session.value = null
    return
  }
  error.value = ''
  void pollActive()
  pollTimer = setInterval(() => { void pollActive() }, 3000)
}

function stopPolling() {
  if (pollTimer) clearInterval(pollTimer)
  pollTimer = null
}

watch(deviceId, startPolling, { immediate: true })
watch(() => props.mediaList.length, () => { error.value = '' })
onBeforeUnmount(stopPolling)

// ---- 录制动作 ----

async function start() {
  if (!deviceId.value || busy.value) return
  busy.value = true
  error.value = ''
  try {
    session.value = await videoApi.recordingStart(deviceId.value)
    currentRecording.value = session.value
    eventsLoaded.value = false
    eventRows.value = []
    eventError.value = ''
  } catch (e) {
    error.value = e?.status === 409 ? '该设备已有活动录制会话' : describe(e, '启动录制失败')
  } finally {
    busy.value = false
  }
}

async function stop() {
  const current = session.value
  if (!current || busy.value) return
  busy.value = true
  error.value = ''
  try {
    const finished = await videoApi.recordingStop(current.id)
    session.value = null
    currentRecording.value = finished || current
    emit('recording-finished', finished)
    emit('changed')
    void loadEvents(currentRecording.value)
  } catch (e) {
    error.value = describe(e, '停止录制失败')
  } finally {
    busy.value = false
    void pollActive()
  }
}

async function cancel() {
  const current = session.value
  if (!current || busy.value) return
  busy.value = true
  error.value = ''
  try {
    const cancelled = await videoApi.recordingCancel(current.id)
    session.value = null
    currentRecording.value = cancelled || current
    emit('changed') // 已落盘部分保留为 interrupted 素材，媒体列表可能变化
    void loadEvents(currentRecording.value)
  } catch (e) {
    error.value = describe(e, '取消录制失败')
  } finally {
    busy.value = false
    void pollActive()
  }
}

// ---- 导入 / 删除 ----

function pickFile() {
  if (importing.value) return
  fileInput.value?.click()
}

async function onFileChosen(event) {
  const file = event.target?.files?.[0]
  event.target.value = ''
  if (!file || importing.value) return
  importing.value = true
  importingName.value = file.name
  error.value = ''
  try {
    const bytes = new Uint8Array(await file.arrayBuffer())
    await videoApi.importMedia(bytes, file.name)
    emit('changed')
  } catch (e) {
    error.value = describe(e, '导入失败')
  } finally {
    importing.value = false
    importingName.value = ''
  }
}

async function remove(media) {
  if (armedId.value !== media.id) {
    armedId.value = media.id
    error.value = ''
    return
  }
  armedId.value = ''
  error.value = ''
  try {
    await videoApi.deleteMedia(media.id)
    emit('changed')
  } catch (e) {
    error.value = e?.status === 409 || e?.code === 'media_referenced'
      ? referenceReason(media, e)
      : describe(e, '删除失败')
  }
}

function referenceReason(media, e) {
  const refs = Array.isArray(media?.refs) ? media.refs : []
  const detail = refs.map(ref => [ref.package_id, ref.plugin_id, ref.kind].filter(Boolean).join(' / ')).filter(Boolean)
  if (detail.length) return `素材被视频项目引用，无法删除：${detail.join('；')}。请先移除对应项目引用。`
  const code = e?.code || e?.data?.error || 'media_referenced'
  return `素材被视频项目引用，无法删除（服务端：${code}）。请先移除对应项目引用。`
}

function selectHistory(record) {
  emit('select', record.media.id)
  if (record.sessionId) {
    emit('recording-selected', { ...record, id: record.sessionId })
    void loadEvents({ id: record.sessionId, state: record.state, device_id: record.deviceId })
  }
}

async function loadEvents(recording) {
  const id = String(recording?.id || '').trim()
  if (!id) {
    eventsLoaded.value = false
    eventRows.value = []
    eventError.value = ''
    return
  }
  eventLoading.value = true
  eventError.value = ''
  eventsLoaded.value = false
  try {
    const events = await videoApi.recordingEvents(id)
    eventRows.value = filterRecordingEvents(events)
    eventsLoaded.value = true
  } catch (e) {
    eventError.value = describe(e, '读取录制事件失败')
  } finally {
    eventLoading.value = false
  }
}

// ---- 展示辅助 ----

function describe(e, fallback) {
  return `${fallback}：${e?.message || e}`
}

function formatDate(value) {
  const date = new Date(value)
  if (!value || Number.isNaN(date.getTime())) return '时间未知'
  return date.toLocaleString()
}

function fmtDuration(durationUs) {
  const us = Number(durationUs)
  if (!Number.isFinite(us) || us <= 0) return '—'
  const totalSeconds = us / 1e6
  const minutes = Math.floor(totalSeconds / 60)
  const seconds = totalSeconds - minutes * 60
  return minutes > 0 ? `${minutes}:${seconds.toFixed(1).padStart(4, '0')}` : `${seconds.toFixed(1)}s`
}

function sourceLabel(source) {
  return source === 'recording' ? '录制' : source === 'import' ? '导入' : String(source || '—')
}

function stateLabel(state) {
  const labels = {
    recording: '录制中',
    finalizing: '收尾中',
    completed: '已完成',
    interrupted: '已中断',
    failed: '失败',
    cancelled: '已取消',
    ready: '就绪',
    importing: '导入中',
    missing: '缺失',
  }
  return labels[state] || String(state || '—')
}

function stateTagClass(state) {
  if (state === 'ready' || state === 'completed') return 'ok'
  if (state === 'missing' || state === 'failed') return 'err'
  if (state === 'importing' || state === 'recording' || state === 'finalizing') return 'run'
  return 'info'
}
</script>

<style scoped>
.media-library { display: flex; flex-direction: column; gap: 8px; min-height: 0; }
.zone-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; flex-shrink: 0; }
.zone-title { color: var(--text-0); font-size: 13px; font-weight: 700; }
.zone-actions { display: flex; gap: 6px; align-items: center; }
.file-hidden { display: none; }
.record-box { display: flex; flex-direction: column; gap: 6px; padding: 8px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-0); }
.record-row { display: flex; align-items: center; gap: 8px; min-width: 0; }
.record-device { min-width: 0; flex: 1; padding: 5px 8px; font-size: 12px; }
.record-state { display: inline-flex; align-items: center; gap: 5px; color: var(--accent); font-size: 11px; white-space: nowrap; }
.zone-note { color: var(--accent-2); font-size: 11px; }
.zone-error { padding: 5px 7px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.08); color: var(--danger); font-size: 11px; line-height: 1.5; word-break: break-all; }
.media-list { border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden auto; max-height: 240px; flex-shrink: 0; }
.list-head, .media-row { display: grid; grid-template-columns: minmax(0, 1fr) 56px 62px 38px 44px auto; align-items: center; gap: 6px; padding: 5px 8px; font-size: 11px; }
.list-head { color: var(--text-2); border-bottom: 1px solid var(--border); background: var(--bg-2); }
.media-row { min-height: 28px; color: var(--text-1); border-bottom: 1px solid rgba(80,92,119,.25); cursor: pointer; }
.media-row:last-child { border-bottom: 0; }
.media-row:hover, .media-row.selected { background: var(--bg-3); }
.media-row.selected { box-shadow: inset 2px 0 var(--accent); }
.media-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-0); }
.list-empty { padding: 16px 10px; text-align: center; color: var(--text-2); font-size: 12px; }
.row-actions { display: flex; gap: 4px; }
.mini-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 2px 6px; }
.mini-btn:hover { border-color: var(--accent); color: var(--accent); }
.mini-btn.danger:hover, .mini-btn.danger.armed { border-color: var(--danger); color: var(--danger); }
.mono { font-family: var(--mono); font-size: 10px; }
.history-box, .recording-detail { display: flex; flex-direction: column; gap: 6px; padding: 8px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-0); }
.history-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.history-source-note { color: var(--text-2); font-size: 10px; }
.history-not-verified { color: var(--text-2); font-size: 10px; line-height: 1.45; }
.history-filters { display: grid; grid-template-columns: minmax(0, 1.4fr) minmax(100px, 1fr) minmax(100px, 1fr); gap: 5px; }
.history-filters .input, .history-filters .select { min-width: 0; padding: 4px 6px; font-size: 11px; }
.history-list { border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden auto; max-height: 190px; }
.history-row { display: grid; grid-template-columns: minmax(0, 1.4fr) minmax(90px, 1fr) minmax(70px, .8fr) 58px 48px auto; align-items: center; gap: 5px; min-height: 28px; padding: 5px 7px; border-bottom: 1px solid rgba(80,92,119,.25); color: var(--text-1); font-size: 10px; cursor: pointer; }
.history-row:last-child { border-bottom: 0; }
.history-row:hover, .history-row.selected { background: var(--bg-3); }
.history-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-0); }
.recording-meta { display: flex; flex-wrap: wrap; gap: 4px 10px; color: var(--text-2); font-size: 10px; }
.history-event-error { padding: 5px 7px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); color: var(--danger); font-size: 11px; line-height: 1.4; }
.event-list { display: flex; flex-direction: column; gap: 3px; max-height: 150px; overflow: auto; }
.event-row { display: grid; grid-template-columns: 52px 42px minmax(0, 1fr) auto; align-items: center; gap: 5px; color: var(--text-1); font-size: 10px; }
@media (max-width: 600px) {
  .history-filters { grid-template-columns: 1fr 1fr; }
  .history-filters .input:first-child { grid-column: 1 / -1; }
  .history-row { grid-template-columns: minmax(0, 1fr) 68px auto; }
  .history-row > :nth-child(2), .history-row > :nth-child(3), .history-row > :nth-child(4) { display: none; }
}
</style>
