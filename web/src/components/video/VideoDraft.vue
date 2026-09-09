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

    <div class="draft-context" data-testid="draft-context">
      <span>录制：<span class="mono">{{ activeRecordingId || '未选择' }}</span></span>
      <span>Package：<span class="mono">{{ packageId || '未选择' }}</span></span>
      <span>设备：<span class="mono">{{ deviceId || '未返回' }}</span></span>
      <span>Android：<span class="mono">{{ androidPackageName || '未返回' }}</span></span>
    </div>

    <div class="history-not-verified" role="note" data-testid="draft-history-not-verified">
      NOT_VERIFIED：后端暂未提供录制会话历史列表；当前会话由素材库/项目上下文带入，下面只读取该会话真实事件，不推断其他会话或设备。
    </div>

    <div class="rid-row">
      <input
        v-model.trim="ridInput"
        class="input rid-input mono"
        aria-label="录制会话来源"
        placeholder="由录制历史或项目带入 recording id"
        data-testid="draft-recording-id"
        @change="emitRecordingId"
      />
      <button class="btn btn-sm" type="button" :disabled="!ridInput || loadingEvents" data-testid="draft-load" @click="loadEvents">
        {{ loadingEvents ? '读取中…' : '载入事件' }}
      </button>
    </div>

    <div v-if="pendingSwitch" class="switch-protect" role="dialog" aria-live="polite" data-testid="draft-switch-protect">
      <div class="switch-protect-title">当前草稿有未保存修改</div>
      <div class="switch-protect-text">
        {{ pendingSwitch.kind === 'package' ? 'Package 正在切换。' : pendingSwitch.kind === 'device' ? '设备上下文正在切换。' : pendingSwitch.kind === 'android' ? 'Android 运行目标正在切换。' : (pendingSwitch.reload ? '当前录制会话正在重新载入。' : '录制会话正在切换。') }}
        请决定如何处理当前草稿。
      </div>
      <div class="switch-protect-actions">
        <button class="mini-btn" type="button" data-testid="draft-switch-retain" @click="resolveSwitch('retain')">保留草稿</button>
        <button class="mini-btn danger-btn" type="button" data-testid="draft-switch-discard" @click="resolveSwitch('discard')">放弃并切换</button>
        <button class="mini-btn" type="button" data-testid="draft-switch-cancel" @click="resolveSwitch('cancel')">取消</button>
      </div>
    </div>

    <div v-if="error" class="zone-error" role="alert" data-testid="draft-error">{{ error }}</div>

    <template v-if="events.length">
      <div class="events-head">
        <span class="events-summary mono">事件 {{ filteredEvents.length }}/{{ events.length }} 条 · 已选 {{ selectedIds.length }}（步骤按选择顺序生成）</span>
        <span class="events-actions">
          <button class="mini-btn" type="button" data-testid="draft-select-all" @click="selectAll">全选当前</button>
          <button class="mini-btn" type="button" data-testid="draft-clear" @click="clearSelection">清空选择</button>
        </span>
      </div>
      <div class="event-filters" aria-label="录制事件筛选" data-testid="draft-event-filters">
        <input v-model.trim="eventFilters.search" class="input" type="search" placeholder="搜索事件、类型或负载" data-testid="draft-event-search" />
        <select v-model="eventFilters.kind" class="select" data-testid="draft-event-kind">
          <option value="">全部类型</option>
          <option v-for="kind in eventKinds" :key="kind" :value="kind">{{ kind }}</option>
        </select>
        <select v-model="eventFilters.source" class="select" data-testid="draft-event-source">
          <option value="">全部来源</option>
          <option v-for="source in eventSources" :key="source" :value="source">{{ source }}</option>
        </select>
        <select v-model="eventFilters.status" class="select" data-testid="draft-event-status">
          <option value="">全部状态</option>
          <option v-for="status in eventStatuses" :key="status" :value="status">{{ status }}</option>
        </select>
        <input v-model="eventFilters.from" class="input" type="number" min="0" step="0.001" placeholder="起始秒" aria-label="起始秒" data-testid="draft-event-from" />
        <input v-model="eventFilters.to" class="input" type="number" min="0" step="0.001" placeholder="结束秒" aria-label="结束秒" data-testid="draft-event-to" />
        <button class="mini-btn" type="button" data-testid="draft-event-filter-clear" @click="clearEventFilters">清除筛选</button>
      </div>
      <div class="event-list" data-testid="draft-event-list">
        <label v-for="ev in filteredEvents" :key="ev.event_key" class="event-row" :class="{ checked: checkedSet[ev.event_id], unsupported: !!ev.support_reason }">
          <input
            type="checkbox"
            class="event-check"
            :disabled="!ev.event_id"
            :checked="!!checkedSet[ev.event_id]"
            :data-testid="`draft-event-check-${ev.event_id}`"
            @change="toggleEvent(ev.event_id, $event.target.checked)"
          />
          <span class="event-kind mono">{{ ev.kind }}</span>
          <span class="mono event-time">{{ fmtTime(ev.timeline_us) }}</span>
          <span class="event-source mono">{{ ev.source }}</span>
          <span class="event-payload mono" :title="payloadFull(ev)">{{ payloadSummary(ev) }}</span>
          <span v-if="ev.support_reason" class="event-support" :title="ev.support_reason">不支持</span>
          <input
            v-model="comments[ev.event_id]"
            class="input event-comment"
            placeholder="注释…"
            :data-testid="`draft-event-comment-${ev.event_id}`"
            @click.stop
            @input="markDraftDirty"
          />
          <span class="event-order" @click.stop>
            <button class="mini-btn" type="button" :disabled="!checkedSet[ev.event_id]" :data-testid="`draft-event-up-${ev.event_id}`" title="上移（重排生成顺序）" @click="moveEvent(ev.event_id, -1)">↑</button>
            <button class="mini-btn" type="button" :disabled="!checkedSet[ev.event_id]" :data-testid="`draft-event-down-${ev.event_id}`" title="下移" @click="moveEvent(ev.event_id, 1)">↓</button>
          </span>
        </label>
        <div v-if="!filteredEvents.length" class="list-empty" data-testid="draft-event-filter-empty">没有符合条件的事件</div>
      </div>
      <div class="generate-row">
        <button
          class="btn btn-sm btn-primary"
          type="button"
          :disabled="!yamlReady || !selectedIds.length || generating || eventLoadState !== 'ready'"
          :title="yamlReady ? '' : '需要 gamer.yaml 扩展运行中'"
          data-testid="draft-generate"
          @click="generate"
        >{{ generating ? '生成中…' : `⚡ 生成 YAML 草稿（${selectedIds.length}）` }}</button>
      </div>
    </template>
    <div v-else-if="loadedOnce" class="zone-empty">该会话没有可映射的操作事件</div>

    <template v-if="yaml">
      <div class="yaml-head">
        <span class="events-summary">草稿预览（YAML V1 文本，请人工检查后保存）<template v-if="sourceLine"> · <span class="mono" data-testid="draft-source-line">{{ sourceLine }}</span></template><template v-if="draftPackageId"> · Package <span class="mono" data-testid="draft-bound-package">{{ draftPackageId }}</span></template><template v-if="draftContext?.deviceId"> · 设备 <span class="mono" data-testid="draft-bound-device">{{ draftContext.deviceId }}</span></template></span>
        <button class="mini-btn" type="button" data-testid="draft-copy" @click="copyYaml">{{ copied ? '已复制 ✓' : '复制' }}</button>
      </div>
      <pre class="yaml-view" data-testid="draft-yaml">{{ yaml }}</pre>

      <!-- 保存（§10.3）：生成文本预览 → 用户确认命名 → automation.save_draft（YAML V1 校验） -->
      <div class="save-row" data-testid="draft-save-row">
        <input v-model.trim="saveName" class="input save-name mono" placeholder="保存为自动化脚本名" data-testid="draft-save-name" @input="markDraftDirty" />
        <label class="check-row"><input v-model="overwrite" type="checkbox" data-testid="draft-overwrite" @change="markDraftDirty" /> 覆盖同名</label>
        <button
          class="btn btn-sm btn-primary"
          type="button"
          :disabled="!yamlReady || !yaml || !saveName || saving"
          data-testid="draft-save"
          @click="saveDraft"
        >{{ saving ? '保存中…' : (saveState === 'failed' ? '重试保存' : '💾 保存到当前 Package') }}</button>
      </div>

      <div v-if="saveState === 'failed'" class="save-failed" role="status" data-testid="draft-save-failed">
        保存失败，当前草稿和编辑上下文仍已保留；修正名称或依赖后可以重试。
      </div>
      <div v-else-if="saveState === 'saved'" class="save-success" role="status" data-testid="draft-save-success">
        已保存到 Package {{ draftPackageId }}，正在打开自动化编辑器；不会创建任务或执行设备输入。
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
import {
  draftEventDiagnostics,
  filterDraftEvents,
  normalizeDraftEvents,
  uniqueDraftValues,
} from './videoDraftWorkflow'

const props = defineProps({
  recordingId: { type: String, default: '' },
  packageId: { type: String, default: '' },
  deviceId: { type: String, default: '' },
  androidPackageName: { type: String, default: '' },
  /** gamer.yaml 是否 Running（§10.1 依赖门禁）。 */
  yamlReady: { type: Boolean, default: false },
})
const emit = defineEmits(['update:recordingId'])

const router = useRouter()
const ridInput = ref(props.recordingId)
/** 当前事件列表所属的录制会话；不能直接用输入框值作为异步请求上下文。 */
const activeRecordingId = ref(String(props.recordingId || '').trim())
const events = ref([])
const checkedSet = reactive({})
/** 选中顺序 = 生成步骤顺序（create_draft 按请求顺序映射，§10.3 重排）。 */
const selectedIds = ref([])
const comments = reactive({})
const loadingEvents = ref(false)
const eventLoadState = ref('idle') // idle | loading | ready | error
const generating = ref(false)
const loadedOnce = ref(false)
const error = ref('')
const yaml = ref('')
const diagnostics = ref([])
const loadDiagnostics = ref([])
const draftDiagnostics = ref([])
const draftSource = ref(null) // {recording_id, events:[…]}（回查信息，草稿 JSON 的一部分）
/** 生成时冻结的上下文；保存不得改用后来切换到的 Package。 */
const draftPackageId = ref('')
const draftRecordingId = ref('')
const draftDirty = ref(false)
const draftRevision = ref(0)
const pendingSwitch = ref(null)
const copied = ref(false)
const saveName = ref('')
const overwrite = ref(false)
const saving = ref(false)
const saveState = ref('idle') // idle | saving | saved | failed
const eventFilters = reactive({ search: '', kind: '', source: '', status: '', from: '', to: '' })
const draftContext = ref(null)

// 每次会话切换都递增；所有异步工作都必须带着这份上下文回来。
let contextVersion = 0
let eventsRequestSeq = 0
let generationRequestSeq = 0
let saveRequestSeq = 0

const hasUnsavedDraft = computed(() => draftDirty.value)
const packageId = computed(() => normalizedId(props.packageId))
const deviceId = computed(() => normalizedId(props.deviceId))
const androidPackageName = computed(() => normalizedId(props.androidPackageName))
const normalizedEvents = computed(() => normalizeDraftEvents(events.value))
const filteredEvents = computed(() => filterDraftEvents(events.value, eventFilters))
const eventKinds = computed(() => uniqueDraftValues(events.value, 'kind'))
const eventSources = computed(() => uniqueDraftValues(events.value, 'source'))
const eventStatuses = computed(() => uniqueDraftValues(events.value, 'status'))

function normalizedId(value) {
  return String(value || '').trim()
}

function currentContext() {
  return {
    version: contextVersion,
    recordingId: activeRecordingId.value,
    packageId: packageId.value,
    deviceId: deviceId.value,
    androidPackageName: androidPackageName.value,
    revision: draftRevision.value,
  }
}

function isCurrentContext(context) {
  return context.version === contextVersion
    && context.recordingId === activeRecordingId.value
    && context.packageId === packageId.value
    && context.deviceId === deviceId.value
    && context.androidPackageName === androidPackageName.value
    && context.revision === draftRevision.value
}

function refreshDiagnostics() {
  const seen = new Set()
  diagnostics.value = [...loadDiagnostics.value, ...draftDiagnostics.value].filter(item => {
    const key = `${item?.event_id || 'unknown'}:${item?.reason || item?.code || ''}`
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })
}

function markDraftDirty() {
  draftDirty.value = true
  draftRevision.value += 1
  if (saveState.value !== 'saving') saveState.value = 'idle'
}

function invalidateAsyncWork() {
  contextVersion += 1
  eventsRequestSeq += 1
  generationRequestSeq += 1
  saveRequestSeq += 1
  // 旧请求的 finally 也不能结束新上下文的 loading 状态。
  loadingEvents.value = false
  generating.value = false
  saving.value = false
}

function clearDraftState() {
  yaml.value = ''
  draftSource.value = null
  draftPackageId.value = ''
  draftRecordingId.value = ''
  draftDiagnostics.value = []
  saveName.value = ''
  overwrite.value = false
  draftDirty.value = false
  copied.value = false
  draftContext.value = null
  saveState.value = 'idle'
  refreshDiagnostics()
}

function clearEventFilters() {
  Object.assign(eventFilters, { search: '', kind: '', source: '', status: '', from: '', to: '' })
}

function temporalDiagnostics(list) {
  const result = []
  let previous = null
  for (const event of Array.isArray(list) ? list : []) {
    const eventId = String(event?.event_id || 'unknown')
    const timeline = Number(event?.timeline_us)
    if (!Number.isFinite(timeline) || timeline < 0) {
      result.push({ event_id: eventId, reason: 'timeline_us 无效或为负数，无法可靠确定事件时序' })
      continue
    }
    if (previous && timeline < previous.timeline) {
      result.push({
        event_id: eventId,
        reason: `事件时序逆序（${timeline} < ${previous.timeline}），已排序但请人工确认`,
      })
    }
    previous = { timeline, eventId }
  }
  return result
}

function compareEventTime(a, b) {
  const left = Number(a?.timeline_us)
  const right = Number(b?.timeline_us)
  const leftValid = Number.isFinite(left) && left >= 0
  const rightValid = Number.isFinite(right) && right >= 0
  if (leftValid && rightValid) return left - right
  if (leftValid) return -1
  if (rightValid) return 1
  return 0
}

function resetSessionState({ preserveDraft = false } = {}) {
  // 事件选择、顺序和注释永远属于旧会话，即使用户选择保留已生成的草稿也不能复用。
  events.value = []
  resetSelection()
  loadedOnce.value = false
  eventLoadState.value = 'idle'
  loadDiagnostics.value = []
  clearEventFilters()
  if (!preserveDraft) clearDraftState()
  else refreshDiagnostics()
}

function startEventLoad(recordingId, { preserveDraft = false } = {}) {
  const id = normalizedId(recordingId)
  if (!id) return

  const requestSeq = ++eventsRequestSeq
  const requestVersion = contextVersion
  loadingEvents.value = true
  eventLoadState.value = 'loading'
  loadedOnce.value = false
  error.value = ''
  events.value = []
  resetSelection()
  loadDiagnostics.value = []
  if (!preserveDraft) clearDraftState()
  else refreshDiagnostics()

  void (async () => {
    try {
      const list = await videoApi.recordingEvents(id)
      if (requestSeq !== eventsRequestSeq || requestVersion !== contextVersion || id !== activeRecordingId.value) return
      const sourceList = Array.isArray(list) ? list : []
      // 合同要求服务端时间轴升序；防御性排序同时保留逆序/非法时间诊断。
      loadDiagnostics.value = [
        ...temporalDiagnostics(sourceList),
        ...draftEventDiagnostics(sourceList),
      ]
      events.value = [...sourceList].sort(compareEventTime)
      eventLoadState.value = 'ready'
      loadedOnce.value = true
      refreshDiagnostics()
      emitRecordingId()
    } catch (e) {
      if (requestSeq !== eventsRequestSeq || requestVersion !== contextVersion || id !== activeRecordingId.value) return
      // 失败和空列表都必须清除旧事件，避免再次提交上一会话的 event_id。
      events.value = []
      resetSelection()
      eventLoadState.value = 'error'
      loadedOnce.value = true
      loadDiagnostics.value = []
      refreshDiagnostics()
      error.value = `载入事件失败：${e?.message || e}`
    } finally {
      if (requestSeq === eventsRequestSeq && requestVersion === contextVersion && id === activeRecordingId.value) {
        loadingEvents.value = false
      }
    }
  })()
}

function applyRecordingSwitch(recordingId, { preserveDraft = false, emitUpdate = false, reload = false } = {}) {
  const id = normalizedId(recordingId)
  pendingSwitch.value = null
  invalidateAsyncWork()
  activeRecordingId.value = id
  ridInput.value = id
  draftRevision.value += 1
  resetSessionState({ preserveDraft })
  error.value = ''
  if (emitUpdate) emit('update:recordingId', id)
  if (id) startEventLoad(id, { preserveDraft })
  else if (reload) eventLoadState.value = 'idle'
}

function requestRecordingSwitch(recordingId, { emitUpdate = false, reload = false } = {}) {
  const id = normalizedId(recordingId)
  if (id === activeRecordingId.value && !reload) {
    if (emitUpdate) emit('update:recordingId', id)
    return
  }
  if (hasUnsavedDraft.value) {
    pendingSwitch.value = { kind: 'recording', recordingId: id, emitUpdate, reload }
    return
  }
  applyRecordingSwitch(id, { emitUpdate, reload })
}

function requestPackageSwitch(packageId) {
  const id = normalizedId(packageId)
  if (!hasUnsavedDraft.value) {
    invalidateAsyncWork()
    draftRevision.value += 1
    clearDraftState()
    return
  }
  pendingSwitch.value = { kind: 'package', packageId: id }
}

function requestDeviceSwitch(kind, value) {
  if (!hasUnsavedDraft.value) {
    invalidateAsyncWork()
    draftRevision.value += 1
    clearDraftState()
    return
  }
  pendingSwitch.value = { kind, value: normalizedId(value) }
}

function resolveSwitch(action) {
  const pending = pendingSwitch.value
  if (!pending) return
  pendingSwitch.value = null
  if (action === 'cancel') {
    if (pending.kind === 'recording') ridInput.value = activeRecordingId.value
    return
  }
  if (pending.kind === 'package') {
    if (action === 'discard') {
      invalidateAsyncWork()
      draftRevision.value += 1
      resetSelection()
      clearDraftState()
    }
    // retain：草稿继续绑定生成时的 Package；后续保存仍使用 draftPackageId。
    return
  }
  if (pending.kind === 'device' || pending.kind === 'android') {
    if (action === 'discard') {
      invalidateAsyncWork()
      draftRevision.value += 1
      clearDraftState()
    }
    // 保留：draftContext 继续绑定生成时的设备/Android 上下文。
    return
  }
  applyRecordingSwitch(pending.recordingId, {
    preserveDraft: action === 'retain',
    emitUpdate: pending.emitUpdate,
    reload: pending.reload,
  })
}

watch(() => props.recordingId, (value) => {
  const id = normalizedId(value)
  if (id === ridInput.value && id === activeRecordingId.value) return
  ridInput.value = id
  // 录制完成自动带入会话 id；不再用 loadedOnce 阻止新会话读取。
  requestRecordingSwitch(id)
})

watch(() => props.packageId, (value, oldValue) => {
  const next = normalizedId(value)
  if (next === normalizedId(oldValue)) return
  requestPackageSwitch(next)
})

watch(() => props.deviceId, (value, oldValue) => {
  const next = normalizedId(value)
  if (next === normalizedId(oldValue)) return
  requestDeviceSwitch('device', next)
})

watch(() => props.androidPackageName, (value, oldValue) => {
  const next = normalizedId(value)
  if (next === normalizedId(oldValue)) return
  requestDeviceSwitch('android', next)
})

function emitRecordingId() {
  const id = normalizedId(ridInput.value)
  ridInput.value = id
  requestRecordingSwitch(id, { emitUpdate: true })
}

async function loadEvents() {
  const id = normalizedId(ridInput.value)
  if (!id || loadingEvents.value) return
  if (id !== activeRecordingId.value) {
    requestRecordingSwitch(id, { emitUpdate: true })
    return
  }
  if (hasUnsavedDraft.value) {
    pendingSwitch.value = { kind: 'recording', recordingId: id, emitUpdate: false, reload: true }
    return
  }
  invalidateAsyncWork()
  startEventLoad(id)
}

function resetSelection() {
  for (const key of Object.keys(checkedSet)) delete checkedSet[key]
  for (const key of Object.keys(comments)) delete comments[key]
  selectedIds.value = []
}

function toggleEvent(eventId, checked) {
  if (!eventId) return
  if (checked) {
    if (!checkedSet[eventId]) {
      checkedSet[eventId] = true
      selectedIds.value = [...selectedIds.value, eventId]
      markDraftDirty()
    }
  } else {
    delete checkedSet[eventId]
    selectedIds.value = selectedIds.value.filter(id => id !== eventId)
    markDraftDirty()
  }
}

function moveEvent(eventId, direction) {
  const ids = [...selectedIds.value]
  const index = ids.indexOf(eventId)
  const target = index + direction
  if (index < 0 || target < 0 || target >= ids.length) return
  ;[ids[index], ids[target]] = [ids[target], ids[index]]
  selectedIds.value = ids
  markDraftDirty()
}

function selectAll() {
  if (!filteredEvents.value.length) return
  const visibleIds = filteredEvents.value.map(ev => ev.event_id).filter(Boolean)
  for (const eventId of visibleIds) checkedSet[eventId] = true
  selectedIds.value = [...new Set([...selectedIds.value, ...visibleIds])]
  markDraftDirty()
}

function clearSelection() {
  const visibleIds = new Set(filteredEvents.value.map(ev => ev.event_id).filter(Boolean))
  const next = selectedIds.value.filter(eventId => !visibleIds.has(eventId))
  const changed = next.length !== selectedIds.value.length || [...visibleIds].some(eventId => comments[eventId] !== undefined)
  for (const eventId of visibleIds) delete checkedSet[eventId]
  selectedIds.value = next
  if (!selectedIds.value.length) {
    for (const key of Object.keys(comments)) delete comments[key]
  }
  if (changed) markDraftDirty()
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
  if (!selectedIds.value.length || generating.value || eventLoadState.value !== 'ready') return
  const context = currentContext()
  if (!context.recordingId) return
  const availableIds = new Set(normalizedEvents.value.map(event => event.event_id).filter(Boolean))
  const eventIds = selectedIds.value.filter(eventId => availableIds.has(eventId))
  if (!eventIds.length) {
    error.value = '没有可提交的有效事件 ID；无效事件已保留在列表中并显示诊断'
    return
  }
  const requestSeq = ++generationRequestSeq
  generating.value = true
  error.value = ''
  copied.value = false
  saveState.value = 'idle'
  try {
    const activeComments = {}
    for (const id of eventIds) {
      const text = String(comments[id] ?? '').trim()
      if (text) activeComments[id] = text
    }
    const result = await videoApi.createVideoDraft(context.recordingId, eventIds, activeComments)
    if (requestSeq !== generationRequestSeq || !isCurrentContext(context) || eventLoadState.value !== 'ready') return
    const sourceRecordingId = normalizedId(result?.source?.recording_id)
    if (sourceRecordingId && sourceRecordingId !== context.recordingId) {
      error.value = `生成草稿来源异常：返回会话 ${sourceRecordingId}，当前为 ${context.recordingId}`
      return
    }
    yaml.value = String(result?.yaml ?? '')
    draftDiagnostics.value = Array.isArray(result?.diagnostics) ? result.diagnostics : []
    refreshDiagnostics()
    draftSource.value = result?.source && typeof result.source === 'object' ? result.source : null
    draftPackageId.value = context.packageId
    draftRecordingId.value = context.recordingId
    draftContext.value = { ...context }
    draftDirty.value = true
    if (!yaml.value && !diagnostics.value.length) {
      error.value = '草稿生成返回为空，请检查 gamer.yaml 扩展是否支持 automation.create_draft'
    }
  } catch (e) {
    if (requestSeq !== generationRequestSeq || !isCurrentContext(context)) return
    error.value = `生成草稿失败：${e?.message || e}`
  } finally {
    if (requestSeq === generationRequestSeq) generating.value = false
  }
}

async function saveDraft() {
  if (saving.value || !yaml.value || !saveName.value || !draftPackageId.value) return
  const context = currentContext()
  const packageId = draftPackageId.value
  const requestRevision = draftRevision.value
  const requestSeq = ++saveRequestSeq
  saving.value = true
  saveState.value = 'saving'
  error.value = ''
  try {
    const result = await videoApi.saveDraft({
      packageId,
      name: saveName.value,
      yaml: yaml.value,
      overwrite: overwrite.value,
    })
    if (requestSeq !== saveRequestSeq || !isCurrentContext(context) || requestRevision !== draftRevision.value) {
      if (requestSeq === saveRequestSeq) {
        saveState.value = 'failed'
        error.value = '保存响应未应用：草稿上下文已变化，当前草稿仍保留，请重新保存'
      }
      return
    }
    const resultPackageId = normalizedId(result?.package_id)
    if (resultPackageId && resultPackageId !== packageId) {
      saveState.value = 'failed'
      error.value = `保存草稿返回了错误的 Package：${resultPackageId}`
      return
    }
    // automation.open_editor（前端契约）：保存成功即打开/定位 YAML 编辑器
    const scriptId = String(result?.id || '')
    if (!scriptId) {
      saveState.value = 'failed'
      error.value = '保存草稿响应缺少资源 ID，当前草稿已保留，请稍后重试'
      return
    }
    draftDirty.value = false
    saveState.value = 'saved'
    requestAutomationEditor(packageId, scriptId)
    await router.push({ query: { ...(router.currentRoute.value?.query || {}), panel: GAMER_YAML_AUTOMATION_PANEL_KEY } }).catch(() => {})
  } catch (e) {
    if (requestSeq !== saveRequestSeq || !isCurrentContext(context) || requestRevision !== draftRevision.value) return
    const message = String(e?.message || e)
    if (message.includes('已存在')) {
      error.value = `${message}（可勾选「覆盖同名」后重试）`
    } else {
      error.value = `保存草稿失败：${message}`
    }
    saveState.value = 'failed'
  } finally {
    if (requestSeq === saveRequestSeq) saving.value = false
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
.draft-context { display: flex; flex-wrap: wrap; gap: 4px 12px; color: var(--text-2); font-size: 10px; }
.history-not-verified { color: var(--text-2); font-size: 10px; line-height: 1.45; }
.switch-protect { padding: 7px 8px; border: 1px solid rgba(251,191,36,.45); border-radius: var(--radius-sm); background: rgba(251,191,36,.08); }
.switch-protect-title { color: var(--warn); font-size: 11px; font-weight: 700; }
.switch-protect-text { margin-top: 3px; color: var(--text-1); font-size: 10px; line-height: 1.45; }
.switch-protect-actions { display: flex; justify-content: flex-end; gap: 4px; margin-top: 6px; }
.danger-btn { border-color: rgba(248,113,113,.45); color: var(--danger); }
.zone-error { padding: 5px 7px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.08); color: var(--danger); font-size: 11px; line-height: 1.5; word-break: break-all; }
.zone-empty { padding: 10px; text-align: center; color: var(--text-2); font-size: 11px; }
.events-head, .yaml-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.events-summary { color: var(--text-2); font-size: 11px; }
.events-actions { display: flex; gap: 4px; }
.event-filters { display: grid; grid-template-columns: minmax(0, 1.4fr) repeat(3, minmax(74px, .7fr)) repeat(2, minmax(70px, .55fr)) auto; gap: 5px; }
.event-filters .input, .event-filters .select { min-width: 0; padding: 4px 6px; font-size: 11px; }
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
.event-row.unsupported { background: rgba(251,191,36,.045); }
.event-support { flex: none; color: var(--warn); font-size: 10px; }
.event-comment { flex: 1 1 110px; min-width: 72px; max-width: 170px; padding: 2px 5px; font-size: 10px; }
.event-order { display: flex; gap: 2px; flex: none; }
.generate-row, .save-row { display: flex; justify-content: flex-end; align-items: center; gap: 8px; }
.save-row { justify-content: flex-start; }
.save-name { flex: 1; min-width: 0; padding: 4px 7px; font-size: 11px; }
.save-failed, .save-success { padding: 5px 7px; border-radius: var(--radius-sm); font-size: 10px; line-height: 1.4; }
.save-failed { border: 1px solid rgba(248,113,113,.35); color: var(--danger); background: rgba(248,113,113,.08); }
.save-success { border: 1px solid rgba(34,211,165,.35); color: var(--accent); background: rgba(34,211,165,.06); }
@media (max-width: 760px) {
  .event-filters { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .event-filters .input[type='search'] { grid-column: 1 / -1; }
  .event-filters .mini-btn { grid-column: 1 / -1; }
}
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
