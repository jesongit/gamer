<template>
  <section class="video-timeline" data-testid="video-timeline">
    <div class="zone-head">
      <span class="zone-title">时间轴</span>
      <span v-if="media" class="zone-sub" :title="media.name">{{ media.name }}</span>
    </div>

    <div v-if="!media" class="zone-empty">在素材库中选择一个素材进行预览</div>
    <template v-else>
      <video
        :key="media.id"
        ref="videoEl"
        class="preview"
        :src="fileUrl"
        controls
        preload="metadata"
        data-testid="video-preview"
        @play="onPlay"
        @seeking="onSeeking"
        @timeupdate="onTimeUpdate"
        @seeked="onSeeked"
      ></video>

      <div class="time-row">
        <span class="mono time-readout" data-testid="video-time">{{ currentTime.toFixed(3) }}s</span>
        <span class="mono time-total">/ {{ totalSeconds.toFixed(3) }}s</span>
        <span v-if="framesMeta" class="mono frame-count" data-testid="frame-count">{{ framesMeta.frame_count }} 帧</span>
        <span class="time-actions">
          <button
            class="mini-btn"
            type="button"
            data-testid="frame-prev"
            title="上一展示帧（服务端真实帧表定位）"
            :disabled="!hasFrames || stepBusy || frameBusy"
            @click="stepFrame(-1)"
          >− 帧</button>
          <button
            class="mini-btn"
            type="button"
            data-testid="frame-next"
            title="下一展示帧（服务端真实帧表定位）"
            :disabled="!hasFrames || stepBusy || frameBusy"
            @click="stepFrame(1)"
          >+ 帧</button>
          <label class="frame-index-control">
            <span class="mono">#</span>
            <input
              v-model="requestedFrameIndex"
              class="input frame-index-input"
              type="number"
              min="0"
              step="1"
              placeholder="帧索引"
              aria-label="指定展示帧索引"
              data-testid="frame-index-input"
              :disabled="!hasFrames || stepBusy || frameBusy"
              @keyup.enter="goToFrameByIndex"
            />
            <button
              class="mini-btn"
              type="button"
              data-testid="frame-index-go"
              :disabled="!hasFrames || stepBusy || frameBusy"
              @click="goToFrameByIndex"
            >定位</button>
          </label>
          <button
            class="mini-btn"
            type="button"
            :disabled="!hasFrames || frameBusy || stepBusy"
            data-testid="frame-exact"
            @click="grabExactFrame()"
          >{{ frameBusy ? '取帧中…' : '◎ 精确帧' }}</button>
        </span>
      </div>

      <div v-if="frameError" class="zone-error" role="alert" data-testid="frame-error">{{ frameError }}</div>
      <div v-if="emptyFrameTable" class="zone-note" role="status" data-testid="frame-empty">
        当前素材没有可用的展示帧，逐帧、指定帧和模板制作均不可用。
      </div>

      <div v-if="frameUrl" class="frame-box" data-testid="frame-box">
        <img
          :key="frameRequest?.requestGeneration || frameUrl"
          :src="frameUrl"
          :data-frame-request="frameRequest?.requestGeneration || ''"
          class="frame-shot"
          alt="服务端精确帧"
          data-testid="frame-image"
          @load="onFrameLoad"
          @error="onFrameError"
        />
        <div class="frame-actions">
          <button
            class="mini-btn"
            type="button"
            :disabled="!yamlReady || !frameReady"
            :title="!yamlReady ? '需要「自动化」插件（gamer.yaml）处于运行状态' : (frameReady ? '在当前确定帧上框选创建模板并离线测试' : '等待当前确定帧图加载完成')"
            data-testid="frame-to-template"
            @click="emitCreateTemplate"
          >✂️ 帧上做模板</button>
          <span v-if="!yamlReady" class="frame-dep-hint">需自动化插件运行中</span>
        </div>
        <div class="frame-caption mono">{{ frameCaption }}</div>
      </div>
      <div class="frame-hint">预览进度（浏览器解码）与服务端精确帧可能有小偏差，制作模板请以精确帧图为准。</div>

      <!-- 标记（Phase 6）：帧身份 = frame_index + pts_us + calibration_version -->
      <div class="markers-box" data-testid="markers-box">
        <div class="sub-head">
          <span class="sub-title">标记</span>
          <span class="marker-add">
            <input
              v-model="newMarkerLabel"
              class="input marker-label-input"
              type="text"
              placeholder="标记名（可空）"
              data-testid="marker-label-input"
              @keyup.enter="addMarkerAtCurrentFrame"
            />
            <button
              class="mini-btn"
              type="button"
              :disabled="!markersAvailable || markerBusy"
              :title="markersAvailable ? '在当前锁定帧添加标记（帧身份寻址）' : '先定位到一个展示帧（逐帧或精确帧）'"
              data-testid="marker-add"
              @click="addMarkerAtCurrentFrame"
            >{{ markerBusy ? '定位中…' : '⚑ 在当前帧加标记' }}</button>
          </span>
        </div>
        <div v-if="staleCount" class="zone-note warn" role="status" data-testid="marker-stale-banner">
          {{ staleCount }} 个标记基于旧校准（当前校准 v{{ calibration?.version }}）——请重新确认后再用于制作
        </div>
        <div v-if="!markers.length" class="list-empty">暂无标记：定位到展示帧后「在当前帧加标记」</div>
        <div v-for="marker in markers" :key="marker.id" class="marker-row" :class="{ stale: isStale(marker) }" data-testid="marker-row">
          <input
            class="input marker-name"
            type="text"
            :value="marker.label"
            :title="marker.frame ? `帧 ${marker.frame.frame_index} · pts_us=${marker.frame.pts_us} · 校准 v${marker.frame.calibration_version}` : ''"
            :data-testid="`marker-name-${marker.id}`"
            @change="emit('update-marker', marker.id, { label: $event.target.value })"
          />
          <span v-if="isStale(marker)" class="tag err" data-testid="marker-stale">旧校准</span>
          <button class="mini-btn" type="button" :data-testid="`marker-jump-${marker.id}`" @click="jumpToMarker(marker)">跳转</button>
          <button class="mini-btn danger" type="button" :data-testid="`marker-del-${marker.id}`" @click="emit('remove-marker', marker.id)">删</button>
          <input
            class="input marker-note"
            type="text"
            :value="marker.note"
            placeholder="注释…"
            :data-testid="`marker-note-${marker.id}`"
            @change="emit('update-marker', marker.id, { note: $event.target.value })"
          />
        </div>
      </div>

      <!-- 校准（Phase 6）：旋转/像素比例/有效画面区域/参考分辨率；应用后版本递增 -->
      <details class="calibration-box" data-testid="calibration-box">
        <summary class="sub-title" data-testid="calibration-summary">
          校准 <span class="mono cal-version">v{{ calibration?.version ?? '?' }}</span>
          <span v-if="calibrationText" class="cal-desc">{{ calibrationText }}</span>
        </summary>
        <div class="cal-grid">
          <label class="form-row"><span class="form-label">旋转</span>
            <select v-model.number="calForm.rotation" class="input" data-testid="calibration-rotation">
              <option :value="0">0°</option><option :value="90">90°</option>
              <option :value="180">180°</option><option :value="270">270°</option>
            </select>
          </label>
          <label class="form-row"><span class="form-label">像素比 x:y</span>
            <input v-model="calForm.paNum" class="input num" type="number" min="1" data-testid="calibration-pa-num" />
            <input v-model="calForm.paDen" class="input num" type="number" min="1" data-testid="calibration-pa-den" />
          </label>
          <label class="form-row"><span class="form-label">参考宽×高</span>
            <input v-model="calForm.refW" class="input num" type="number" min="1" data-testid="calibration-ref-w" />
            <input v-model="calForm.refH" class="input num" type="number" min="1" data-testid="calibration-ref-h" />
          </label>
          <label class="form-row"><span class="form-label">有效区域 x,y,w,h</span>
            <input v-model="calForm.rectX" class="input num" type="number" min="0" placeholder="x" data-testid="calibration-rect-x" />
            <input v-model="calForm.rectY" class="input num" type="number" min="0" placeholder="y" data-testid="calibration-rect-y" />
            <input v-model="calForm.rectW" class="input num" type="number" min="0" placeholder="w" data-testid="calibration-rect-w" />
            <input v-model="calForm.rectH" class="input num" type="number" min="0" placeholder="h" data-testid="calibration-rect-h" />
          </label>
          <div class="form-actions">
            <button class="btn btn-sm btn-primary" type="button" data-testid="calibration-apply" @click="applyCalibration">应用校准（版本 +1）</button>
            <button class="btn btn-sm" type="button" data-testid="calibration-reset" @click="syncCalForm()">还原</button>
          </div>
          <div v-if="calibrationError" class="zone-error" role="alert" data-testid="calibration-error">{{ calibrationError }}</div>
          <div class="frame-hint">校准变化后旧标记/模板区域不悄悄变形：它们保留旧校准版本并标脏，需重新确认。</div>
        </div>
      </details>

      <!-- 自录事件叠加（Phase 6）：base_pts_us 整数映射对齐；外部视频无事件不伪造 -->
      <div v-if="recordingId" class="events-box" data-testid="events-box">
        <div class="sub-head">
          <span class="sub-title">操作事件 <span class="mono">{{ recordingId }}</span></span>
          <button class="mini-btn" type="button" :disabled="eventsBusy" data-testid="events-load" @click="loadEvents">
            {{ eventsBusy ? '载入中…' : (eventsView.length ? '↻ 重载' : '载入') }}
          </button>
        </div>
        <div v-if="eventsError" class="zone-error" role="alert" data-testid="events-error">{{ eventsError }}</div>
        <div v-if="eventsLoaded && !eventsView.length" class="list-empty">该录制会话没有已接受的操作事件</div>
        <div v-for="(view, index) in eventsView" :key="view.event.event_id || index" class="event-row" data-testid="timeline-event-row" @click="jumpToEvent(view)">
          <span class="mono event-time">{{ fmtUs(view.event.timeline_us) }}</span>
          <span class="tag" :class="`src-${view.event.source}`">{{ view.sourceLabel }}</span>
          <span class="event-kind mono">{{ view.event.kind }}</span>
          <span class="event-summary">{{ view.eventSummaryText }}</span>
          <span v-if="view.unmapped" class="tag err" title="事件时间不在任何录制分段内">未对齐</span>
        </div>
      </div>
    </template>
  </section>
</template>

<script setup>
// 时间轴区（Phase 6 重构）：浏览器 <video> 只做流畅预览，精确帧一律取服务端
// 确定帧端点（展示序索引寻址，可重复）。在 V1 只读预览之上叠加制作能力：
// - 标记：帧身份（frame_index+pts_us+校准版本）引用，注释可编辑；校准变化标脏
// - 校准：旋转/像素比例/有效画面区域/参考分辨率，应用后由宿主递增版本
// - 自录事件：会话分段 base_pts_us 整数映射到媒体 PTS（recordingEvents.js），
//   外部素材无 recordingId 时不渲染事件区（不伪造操作日志）
import { computed, reactive, ref, watch } from 'vue'
import { calibrationDiagnostics, describeCalibration } from './calibration'
import { alignEvents, eventSummary } from './recordingEvents'
import { ptsFromTime, videoApi } from './videoApi'

const props = defineProps({
  media: { type: Object, default: null },
  /** 当前项目标记集合（帧身份引用）。 */
  markers: { type: Array, default: () => [] },
  /** 当前项目校准（含 version）。 */
  calibration: { type: Object, default: null },
  /** 项目关联的录制会话 id（外部素材为空 → 不显示事件区）。 */
  recordingId: { type: String, default: '' },
  /** gamer.yaml 是否 Running（§10.1 依赖门禁：模板制作入口禁用态）。 */
  yamlReady: { type: Boolean, default: false },
})

const emit = defineEmits(['add-marker', 'remove-marker', 'update-marker', 'save-calibration', 'create-template'])

const videoEl = ref(null)
const currentTime = ref(0)
const frameUrl = ref('')
const frameCaption = ref('')
const frameBusy = ref(false)
const frameError = ref('')
// 真实展示帧表元信息（帧总数；加载失败 → 逐帧按钮禁用，不做时间近似降级）
const framesMeta = ref(null)
// 当前制作帧身份。它与浏览器预览时间分离，且附带内部上下文用于防止旧响应串入。
const currentFrame = ref(null)
// 当前精确帧 PNG 请求：requestGeneration 用于丢弃旧 <img> 的 load/error 回调。
const frameRequest = ref(null)
const stepBusy = ref(false)
const markerBusy = ref(false)
const newMarkerLabel = ref('')
const requestedFrameIndex = ref('')

// 帧上下文代次：素材/校准/用户预览移动都会使旧制作帧失效；
// mediaGeneration 另行保留给帧表加载，预览移动不应让同一素材的帧表失效。
let mediaGeneration = 0
let frameContextGeneration = 0
let frameRequestGeneration = 0
let frameResolveGeneration = 0
let frameMetaRequestGeneration = 0
let frameResolutionInFlight = 0
let pendingProgrammaticSeek = null

// ---- 事件叠加状态 ----
const eventsBusy = ref(false)
const eventsLoaded = ref(false)
const eventsError = ref('')
const eventsView = ref([])
// 事件加载按录制会话代次防串台（切换项目/素材后旧响应不应用）
let eventsLoadSeq = 0

const fileUrl = computed(() => (props.media ? videoApi.mediaFileUrl(props.media.id) : ''))
const totalSeconds = computed(() => {
  const us = Number(props.media?.duration_us)
  return Number.isFinite(us) && us > 0 ? us / 1e6 : 0
})

const hasFrames = computed(() => Number(framesMeta.value?.frame_count) > 0)
const emptyFrameTable = computed(() => !!framesMeta.value && Number(framesMeta.value.frame_count) === 0)
const markersAvailable = computed(() => !!props.media && hasFrames.value)
const frameReady = computed(() => !!validLockedFrame({ requireImage: true }))
const staleCount = computed(() => (props.calibration
  ? props.markers.filter(marker => Number(marker.frame?.calibration_version) !== Number(props.calibration.version)).length
  : 0))
const calibrationText = computed(() => {
  if (!props.calibration || !props.media) return ''
  const encoded = { width: props.media.width, height: props.media.height }
  try { return describeCalibration(props.calibration, encoded) } catch { return '' }
})

watch(() => props.media?.id, () => {
  mediaGeneration += 1
  currentTime.value = 0
  invalidateProductionFrame()
  framesMeta.value = null
  requestedFrameIndex.value = ''
  stepBusy.value = false
  markerBusy.value = false
  eventsView.value = []
  eventsLoaded.value = false
  eventsError.value = ''
  void loadFramesMeta()
}, { immediate: true })

watch(() => props.recordingId, () => {
  // 会话变化：清空旧事件（不自动拉取，避免打开项目就打两三个请求）
  eventsView.value = []
  eventsLoaded.value = false
  eventsError.value = ''
})

watch(() => props.calibration?.version, (version, previous) => {
  // 校准版本是制作坐标的身份组成部分；同一帧图不能被新校准静默复用。
  if (previous !== undefined && version !== previous) invalidateProductionFrame()
})

// ---- 校准表单（本地草稿；应用时才上抛并递增版本） ----
const calForm = reactive({ rotation: 0, paNum: '1', paDen: '1', refW: '', refH: '', rectX: '', rectY: '', rectW: '', rectH: '' })
const calibrationError = ref('')

watch(() => props.calibration, () => syncCalForm(), { immediate: true })

function syncCalForm() {
  const calibration = props.calibration
  calibrationError.value = ''
  if (!calibration) return
  calForm.rotation = Number(calibration.rotation) || 0
  calForm.paNum = String(calibration.pixel_aspect?.num ?? 1)
  calForm.paDen = String(calibration.pixel_aspect?.den ?? 1)
  calForm.refW = String(calibration.reference_size?.width ?? '')
  calForm.refH = String(calibration.reference_size?.height ?? '')
  const rect = calibration.content_rect
  calForm.rectX = rect ? String(rect.x) : ''
  calForm.rectY = rect ? String(rect.y) : ''
  calForm.rectW = rect ? String(rect.w) : ''
  calForm.rectH = rect ? String(rect.h) : ''
}

function applyCalibration() {
  calibrationError.value = ''
  const parseInteger = (raw, label, minimum) => {
    const text = String(raw ?? '').trim()
    const value = Number(text)
    if (!text || !Number.isSafeInteger(value) || value < minimum) {
      if (!calibrationError.value) calibrationError.value = `${label}必须是 ≥ ${minimum} 的整数`
      return null
    }
    return value
  }
  const rotation = parseInteger(calForm.rotation, '旋转', 0)
  const paNum = parseInteger(calForm.paNum, '像素比例分子', 1)
  const paDen = parseInteger(calForm.paDen, '像素比例分母', 1)
  const refW = parseInteger(calForm.refW, '参考分辨率宽度', 1)
  const refH = parseInteger(calForm.refH, '参考分辨率高度', 1)
  if ([rotation, paNum, paDen, refW, refH].some(value => value === null)) return
  if (![0, 90, 180, 270].includes(rotation)) {
    calibrationError.value = '旋转只支持 0°、90°、180°、270°'
    return
  }
  const rectValues = [calForm.rectX, calForm.rectY, calForm.rectW, calForm.rectH]
  const rectGiven = rectValues.some(value => String(value ?? '').trim() !== '')
  let contentRect = null
  if (rectGiven) {
    const rectX = parseInteger(calForm.rectX, '有效区域 x', 0)
    const rectY = parseInteger(calForm.rectY, '有效区域 y', 0)
    const rectW = parseInteger(calForm.rectW, '有效区域宽度', 1)
    const rectH = parseInteger(calForm.rectH, '有效区域高度', 1)
    if ([rectX, rectY, rectW, rectH].some(value => value === null)) return
    contentRect = { x: rectX, y: rectY, w: rectW, h: rectH }
  }
  const next = {
    rotation,
    pixel_aspect: {
      num: paNum,
      den: paDen,
    },
    reference_size: { width: refW, height: refH },
    content_rect: contentRect,
  }
  const diagnostics = calibrationDiagnostics({ version: calibrationVersion(), ...next })
  if (diagnostics.length) {
    calibrationError.value = diagnostics[0].message
    return
  }
  emit('save-calibration', next)
}

async function loadFramesMeta() {
  const id = String(props.media?.id || '')
  if (!id) return
  const generation = mediaGeneration
  const requestGeneration = ++frameMetaRequestGeneration
  try {
    const meta = await videoApi.mediaFrames(id)
    if (generation !== mediaGeneration || requestGeneration !== frameMetaRequestGeneration || String(props.media?.id || '') !== id) return
    if (!responseMatchesMedia(meta, id)) throw new Error('帧表响应与当前素材不一致')
    framesMeta.value = meta
    if (Number(meta?.frame_count) === 0) {
      invalidateProductionFrame()
      frameError.value = ''
    }
  } catch (e) {
    if (generation !== mediaGeneration || requestGeneration !== frameMetaRequestGeneration || String(props.media?.id || '') !== id) return
    framesMeta.value = null
    frameError.value = '展示帧表加载失败：逐帧步进不可用（' + (e?.message || e) + '）'
  }
}

function calibrationVersion() {
  const value = Number(props.calibration?.version)
  return Number.isFinite(value) ? value : 1
}

function captureFrameContext() {
  return {
    mediaId: String(props.media?.id || ''),
    mediaGeneration,
    frameContextGeneration,
    calibrationVersion: calibrationVersion(),
  }
}

function isCurrentFrameContext(context) {
  return !!context
    && context.mediaId !== ''
    && context.mediaId === String(props.media?.id || '')
    && context.mediaGeneration === mediaGeneration
    && context.frameContextGeneration === frameContextGeneration
    && context.calibrationVersion === calibrationVersion()
}

function responseMatchesMedia(response, mediaId) {
  const responseId = response?.media_id ?? response?.mediaId
  return responseId === undefined || responseId === null || String(responseId) === String(mediaId)
}

function normalizeFramePosition(position, mediaId) {
  if (!position || !responseMatchesMedia(position, mediaId)) return null
  const index = Number(position.index)
  const ptsUs = Number(position.pts_us ?? position.ptsUs)
  if (!Number.isSafeInteger(index) || index < 0 || !Number.isSafeInteger(ptsUs) || ptsUs < 0) return null
  return { index, pts_us: ptsUs }
}

function staleFrameResponse() {
  const error = new Error('stale frame response')
  error.code = 'stale_frame_response'
  return error
}

function isStaleFrameResponse(error) {
  return error?.code === 'stale_frame_response'
}

function assertFrameRequestCurrent(context, requestGeneration) {
  if (!isCurrentFrameContext(context) || requestGeneration !== frameResolveGeneration) throw staleFrameResponse()
}

/** 使制作帧、PNG 请求和所有未完成的帧解析失效；预览时钟本身不清零。 */
function invalidateProductionFrame() {
  frameContextGeneration += 1
  frameResolveGeneration += 1
  pendingProgrammaticSeek = null
  currentFrame.value = null
  frameRequest.value = null
  frameUrl.value = ''
  frameCaption.value = ''
  frameError.value = ''
  frameBusy.value = false
}

function invalidateFromPreview() {
  if (currentFrame.value || frameRequest.value || pendingProgrammaticSeek || frameResolutionInFlight > 0) {
    invalidateProductionFrame()
  }
}

function onTimeUpdate() {
  const t = Number(videoEl.value?.currentTime)
  if (Number.isFinite(t)) currentTime.value = t
  if (Number.isFinite(t) && isProgrammaticSeekPosition(t)) return
  invalidateFromPreview()
}

// frameBusy 由 <img> 的 load/error 事件驱动复位；事件必须绑定到当前请求代次。
function frameRequestFromEvent(event) {
  const target = event?.currentTarget || event?.target
  const requestGeneration = target?.dataset?.frameRequest
  const request = frameRequest.value
  if (!request || !requestGeneration || String(request.requestGeneration) !== String(requestGeneration)) return null
  return request
}

function isCurrentFrameRequest(request) {
  return !!request
    && isCurrentFrameContext(request)
    && frameRequest.value?.requestGeneration === request.requestGeneration
    && currentFrame.value?.requestGeneration === request.requestGeneration
}

function onFrameLoad(event) {
  const request = frameRequestFromEvent(event)
  if (!isCurrentFrameRequest(request)) return
  request.status = 'ready'
  frameBusy.value = false
}

function onFrameError(event) {
  const request = frameRequestFromEvent(event)
  if (!isCurrentFrameRequest(request)) return
  invalidateProductionFrame()
  frameBusy.value = false
  frameError.value = '精确帧获取失败：素材文件缺失或帧参数越界'
}

function onPlay() {
  invalidateFromPreview()
}

function isProgrammaticSeekPosition(time) {
  const pending = pendingProgrammaticSeek
  if (!pending) return false
  if (!isCurrentFrameRequest(pending)) {
    pendingProgrammaticSeek = null
    return false
  }
  return Math.abs(time - pending.pts_us / 1e6) <= 0.01
}

function onSeeking() {
  const t = Number(videoEl.value?.currentTime)
  if (Number.isFinite(t) && isProgrammaticSeekPosition(t)) return
  invalidateFromPreview()
}

function onSeeked() {
  const t = Number(videoEl.value?.currentTime)
  if (!Number.isFinite(t)) return
  currentTime.value = t
  if (isProgrammaticSeekPosition(t)) {
    // 保留哨兵到后续 timeupdate：浏览器在不同实现中可能在 seeked 后再补一次回显。
    pendingProgrammaticSeek = { ...pendingProgrammaticSeek, settled: true }
    return
  }
  invalidateFromPreview()
}

function validLockedFrame({ requireImage = false } = {}) {
  const frame = currentFrame.value
  const request = frameRequest.value
  const mediaId = String(props.media?.id || '')
  if (!frame || !request || !mediaId) return null
  if (!isCurrentFrameRequest(request)
      || frame.media_id !== mediaId
      || frame.calibration_version !== calibrationVersion()
      || frame.index !== request.index
      || frame.pts_us !== request.pts_us) return null
  if (requireImage && request.status !== 'ready') return null
  return { index: frame.index, pts_us: frame.pts_us }
}

/** 按预览位置让服务端解析展示帧；不做本地帧率/时间步长估算。 */
async function resolveFrameAtTime(context, ptsUs) {
  const requestGeneration = ++frameResolveGeneration
  frameResolutionInFlight += 1
  try {
    const meta = await videoApi.mediaFrames(context.mediaId, { ptsUs })
    assertFrameRequestCurrent(context, requestGeneration)
    if (!responseMatchesMedia(meta, context.mediaId)) throw new Error('帧响应与当前素材不一致')
    framesMeta.value = meta || framesMeta.value
    const position = normalizeFramePosition(meta?.current, context.mediaId)
    if (!position) return null
    return position
  } finally {
    frameResolutionInFlight -= 1
  }
}

function beginFrameRequest(context) {
  const requestGeneration = ++frameResolveGeneration
  frameResolutionInFlight += 1
  return { context, requestGeneration }
}

async function getFrameNeighbors(context, position) {
  const operation = beginFrameRequest(context)
  try {
    const response = await videoApi.mediaFrameNeighbors(context.mediaId, position.index)
    assertFrameRequestCurrent(context, operation.requestGeneration)
    if (!responseMatchesMedia(response, context.mediaId)) throw new Error('相邻帧响应与当前素材不一致')
    if (Number(response?.index) !== position.index) throw new Error('相邻帧响应与请求索引不一致')
    return response
  } finally {
    frameResolutionInFlight -= 1
  }
}

/** 按用户指定的展示序索引定位；PTS 必须取邻帧端点返回值，不能由前端估算。 */
async function resolveFrameByIndex(context, index) {
  const response = await getFrameNeighbors(context, { index, pts_us: 0 })
  const position = normalizeFramePosition(response, context.mediaId)
  if (!position) throw new Error('指定帧响应缺少有效的 index/pts_us')
  return position
}

/** 把帧身份渲染到精确帧区：按展示序索引寻址（同一请求逐字节可重复），
 *  并把预览 <video> seek 到该帧时刻保持两者同步。 */
function showFrameByIndex(position, context = captureFrameContext()) {
  const mediaId = String(props.media?.id || '')
  const normalized = normalizeFramePosition(position, mediaId)
  if (!normalized || !isCurrentFrameContext(context)) return null
  const nextUrl = videoApi.mediaFrameUrl(mediaId, { index: normalized.index, maxWidth: 640 })
  frameError.value = ''
  const existing = frameRequest.value
  if (existing && existing.url === nextUrl && isCurrentFrameRequest(existing)) {
    currentFrame.value = {
      ...normalized,
      media_id: mediaId,
      calibration_version: context.calibrationVersion,
      requestGeneration: existing.requestGeneration,
    }
    return existing
  }
  const request = {
    ...context,
    index: normalized.index,
    pts_us: normalized.pts_us,
    url: nextUrl,
    requestGeneration: ++frameRequestGeneration,
    status: 'loading',
  }
  currentFrame.value = {
    ...normalized,
    media_id: mediaId,
    calibration_version: context.calibrationVersion,
    requestGeneration: request.requestGeneration,
  }
  frameRequest.value = request
  frameBusy.value = true
  frameUrl.value = nextUrl
  frameCaption.value = `帧 ${normalized.index} · pts_us=${normalized.pts_us}（t=${(normalized.pts_us / 1e6).toFixed(3)}s）`
  pendingProgrammaticSeek = { ...request }
  const el = videoEl.value
  if (el) {
    try { el.currentTime = normalized.pts_us / 1e6 } catch { /* 元数据未就绪时静默 */ }
  }
  return request
}

/** 取当前预览时间的精确帧；ptsUs 传入时直接按该值请求（同一请求逐字节可重复）。
 *  时间寻址的帧身份由服务端解析（首个 pts ≥ 目标的展示帧），本地不估算。 */
async function grabExactFrame(ptsUs) {
  if (!props.media || frameBusy.value) return
  const pts = Number.isFinite(Number(ptsUs)) ? Math.max(0, Math.round(Number(ptsUs))) : ptsFromTime(currentTime.value)
  // 先由服务端按真实帧表解析身份，再以 index 取图；制作区永远不保留“只有时间、没有身份”的锁帧。
  invalidateProductionFrame()
  const context = captureFrameContext()
  frameError.value = ''
  frameBusy.value = true
  let shown = false
  try {
    const position = await resolveFrameAtTime(context, pts)
    if (!position || !isCurrentFrameContext(context)) {
      if (isCurrentFrameContext(context)) frameError.value = '当前没有可用于制作的展示帧'
      return
    }
    shown = !!showFrameByIndex(position, context)
  } catch (e) {
    if (isCurrentFrameContext(context) && !isStaleFrameResponse(e)) {
      frameError.value = '精确帧解析失败：' + (e?.message || e)
    }
  } finally {
    if (!shown && isCurrentFrameContext(context)) frameBusy.value = false
  }
}

/** 逐帧 ±：服务端真实展示帧表相邻定位（prev/next），不按固定时长估算。
 *  首步先按预览时间解析当前帧，之后沿相邻帧链走；边界（首/末帧）为 no-op。 */
async function stepFrame(direction) {
  if (!props.media || !framesMeta.value || stepBusy.value) return
  const context = captureFrameContext()
  stepBusy.value = true
  try {
    let position = validLockedFrame()
    if (!position) {
      position = await resolveFrameAtTime(context, ptsFromTime(currentTime.value))
    }
    if (!position) {
      // 空素材（0 帧）：无相邻可言
      return
    }
    if (!isCurrentFrameContext(context)) return
    const neighbors = await getFrameNeighbors(context, position)
    const rawTarget = direction < 0 ? neighbors?.prev : neighbors?.next
    if (!rawTarget) return // 首/末帧边界：不动
    const target = normalizeFramePosition(rawTarget, context.mediaId)
    if (!target) throw new Error('相邻帧响应与当前素材不一致')
    if (!isCurrentFrameContext(context)) return
    showFrameByIndex(target, context)
  } catch (e) {
    if (isCurrentFrameContext(context) && !isStaleFrameResponse(e)) {
      frameError.value = '逐帧定位失败：' + (e?.message || e)
    }
  } finally {
    stepBusy.value = false
  }
}

/** 指定展示帧入口：索引只用于服务端查表，实际 PTS 由服务端返回。 */
async function goToFrameByIndex() {
  if (!props.media || !hasFrames.value || stepBusy.value || frameBusy.value) return
  const raw = String(requestedFrameIndex.value ?? '').trim()
  const index = Number(raw)
  if (!raw || !Number.isSafeInteger(index) || index < 0) {
    frameError.value = '指定帧索引必须是非负整数'
    return
  }
  const count = Number(framesMeta.value?.frame_count)
  if (Number.isSafeInteger(count) && index >= count) {
    frameError.value = `指定帧索引越界：有效范围为 0–${Math.max(0, count - 1)}`
    return
  }
  const context = captureFrameContext()
  stepBusy.value = true
  frameError.value = ''
  try {
    const position = await resolveFrameByIndex(context, index)
    if (!isCurrentFrameContext(context)) return
    showFrameByIndex(position, context)
  } catch (e) {
    if (isCurrentFrameContext(context) && !isStaleFrameResponse(e)) {
      frameError.value = '指定帧定位失败：' + (e?.message || e)
    }
  } finally {
    stepBusy.value = false
  }
}

// ---- 标记 ----

/** 在当前帧加标记：先解析当前帧身份（未锁定时按预览时间服务端解析），再上抛
 *  帧身份（frame_index+pts_us+当前校准版本）——绝不存浏览器浮点秒。 */
async function addMarkerAtCurrentFrame() {
  if (!props.media || !framesMeta.value || markerBusy.value) return
  const context = captureFrameContext()
  markerBusy.value = true
  try {
    let position = validLockedFrame()
    if (!position) {
      position = await resolveFrameAtTime(context, ptsFromTime(currentTime.value))
    }
    if (!position) {
      frameError.value = '当前没有可标记的展示帧'
      return
    }
    if (!isCurrentFrameContext(context)) return
    if (!showFrameByIndex(position, context)) return
    emit('add-marker', {
      label: newMarkerLabel.value.trim(),
      frame: {
        media_id: context.mediaId,
        frame_index: position.index,
        pts_us: position.pts_us,
        calibration_version: context.calibrationVersion,
      },
    })
    newMarkerLabel.value = ''
  } catch (e) {
    if (isCurrentFrameContext(context) && !isStaleFrameResponse(e)) {
      frameError.value = '标记定位失败：' + (e?.message || e)
    }
  } finally {
    markerBusy.value = false
  }
}

/** 正向入口（§10.2）：把当前确定帧身份交给模板工作台（缺身份时先按预览时间
 *  解析；解析失败提示，不猜测帧身份）。 */
async function emitCreateTemplate() {
  if (!props.media) return
  const context = captureFrameContext()
  let position = validLockedFrame({ requireImage: true })
  if (!position) {
    try {
      position = await resolveFrameAtTime(context, ptsFromTime(currentTime.value))
    } catch (e) {
      if (isCurrentFrameContext(context) && !isStaleFrameResponse(e)) {
        frameError.value = '帧解析失败：' + (e?.message || e)
      }
      return
    }
  }
  if (!position) {
    if (isCurrentFrameContext(context)) frameError.value = '当前没有可用于制作模板的展示帧'
    return
  }
  if (!isCurrentFrameContext(context)) return
  const locked = showFrameByIndex(position, context)
  // showFrameByIndex 新建 PNG 请求后仍在 loading 时，不允许把新身份伪装成已就绪的制作帧。
  if (!locked || frameRequest.value?.status !== 'ready' || !validLockedFrame({ requireImage: true })) {
    if (isCurrentFrameContext(context)) frameError.value = '确定帧图尚未就绪，暂不能制作模板'
    return
  }
  emit('create-template', { mediaId: context.mediaId, frameIndex: position.index, ptsUs: position.pts_us })
}

function isStale(marker) {
  return Number(marker?.frame?.calibration_version) !== Number(props.calibration?.version)
}

async function jumpToMarker(marker) {
  const frame = marker?.frame
  if (!frame) {
    frameError.value = '该标记缺少确定帧身份，无法跳转'
    return
  }
  if (!props.media || String(frame.media_id) !== String(props.media.id)) {
    frameError.value = `该标记在其他素材上（${frame.media_id || '未知'}），请先在素材库切换`
    return
  }
  const index = Number(frame.frame_index)
  const ptsUs = Number(frame.pts_us)
  if (!Number.isSafeInteger(index) || index < 0 || !Number.isSafeInteger(ptsUs) || ptsUs < 0) {
    frameError.value = '该标记的帧身份无效，无法跳转'
    return
  }
  // 标记本身已经保存服务端确认过的 index + PTS；按 index 取图，禁止退回到浮点时间。
  showFrameByIndex({ index, pts_us: ptsUs })
}

// ---- 操作事件叠加 ----

async function loadEvents() {
  const recordingId = props.recordingId
  if (!recordingId || eventsBusy.value) return
  const seq = ++eventsLoadSeq
  eventsBusy.value = true
  eventsError.value = ''
  try {
    // 会话元数据（分段 + base_pts_us）与事件流分开拉取；纯函数对齐
    const [session, events] = await Promise.all([
      videoApi.recordingStatus(recordingId),
      videoApi.recordingEvents(recordingId),
    ])
    if (seq !== eventsLoadSeq) return // 已切换会话/素材：丢弃过期响应
    eventsView.value = alignEvents(events, session?.segments || []).map(view => ({
      ...view,
      eventSummaryText: eventSummary(view.event),
    }))
    eventsLoaded.value = true
  } catch (e) {
    if (seq === eventsLoadSeq) eventsError.value = '操作事件载入失败：' + (e?.message || e)
  } finally {
    if (seq === eventsLoadSeq) eventsBusy.value = false
  }
}

async function jumpToEvent(view) {
  if (view.unmapped || view.ptsUs === null) return
  if (view.mediaId !== props.media?.id) {
    frameError.value = `该事件在其他分段素材上（${view.mediaId}），请先在素材库切换`
    return
  }
  const context = captureFrameContext()
  try {
    // 事件 PTS → 展示帧身份（服务端解析首个 pts ≥ 目标的展示帧）
    const position = await resolveFrameAtTime(context, view.ptsUs)
    if (!position) return
    if (!isCurrentFrameContext(context)) return
    showFrameByIndex(position, context)
  } catch (e) {
    if (isCurrentFrameContext(context) && !isStaleFrameResponse(e)) {
      frameError.value = '事件跳转失败：' + (e?.message || e)
    }
  }
}

function fmtUs(us) {
  const value = Number(us)
  return Number.isFinite(value) ? `${(value / 1e6).toFixed(3)}s` : '—'
}
</script>

<style scoped>
.video-timeline { display: flex; flex-direction: column; gap: 8px; min-height: 0; }
.zone-head { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; flex-shrink: 0; }
.zone-title { color: var(--text-0); font-size: 13px; font-weight: 700; }
.zone-sub { color: var(--text-2); font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.zone-empty { padding: 18px 10px; text-align: center; color: var(--text-2); font-size: 12px; }
.preview { width: 100%; max-height: 220px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: #000; }
.time-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.time-readout { color: var(--accent-2); font-size: 12px; }
.time-total { color: var(--text-2); font-size: 11px; }
.time-actions { display: flex; gap: 5px; margin-left: auto; align-items: center; flex-wrap: wrap; }
.frame-index-control { display: inline-flex; align-items: center; gap: 3px; }
.frame-index-input { width: 72px; padding: 3px 5px; font: 11px var(--mono); }
.mini-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 3px 7px; }
.mini-btn:hover { border-color: var(--accent); color: var(--accent); }
.mini-btn:disabled { opacity: .45; cursor: not-allowed; }
.mini-btn.danger:hover { border-color: var(--danger); color: var(--danger); }
.frame-box { display: flex; flex-direction: column; gap: 4px; padding: 6px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-0); }
.frame-shot { max-width: 100%; max-height: 200px; object-fit: contain; align-self: center; image-rendering: pixelated; }
.frame-actions { display: flex; align-items: center; gap: 6px; justify-content: center; }
.frame-dep-hint { color: var(--warn); font-size: 10px; }
.frame-caption { color: var(--text-2); font-size: 10px; text-align: center; }
.frame-hint { color: var(--text-2); font-size: 10px; line-height: 1.5; }
.zone-error { padding: 5px 7px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.08); color: var(--danger); font-size: 11px; line-height: 1.5; }
.zone-note { padding: 5px 7px; border-radius: var(--radius-sm); font-size: 11px; line-height: 1.5; }
.zone-note.warn { border: 1px solid rgba(245,180,80,.4); background: rgba(245,180,80,.08); color: var(--warning, #d9a13c); }
.markers-box, .calibration-box, .events-box { display: flex; flex-direction: column; gap: 6px; padding: 8px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-0); }
.sub-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.sub-title { color: var(--text-0); font-size: 12px; font-weight: 700; cursor: default; }
summary.sub-title { cursor: pointer; }
.marker-add { display: flex; gap: 5px; align-items: center; }
.input { padding: 3px 6px; font-size: 11px; border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); min-width: 0; }
.input.num { width: 52px; }
.marker-label-input { width: 110px; }
.marker-row { display: grid; grid-template-columns: minmax(0, 1fr) auto auto auto; gap: 4px; align-items: center; }
.marker-row.stale { opacity: .7; }
.marker-row .marker-note { grid-column: 1 / -1; }
.marker-name { font-size: 11px; }
.marker-note { font-size: 10px; color: var(--text-2); }
.tag { display: inline-block; padding: 1px 5px; border-radius: 8px; font-size: 10px; background: var(--bg-3); color: var(--text-2); }
.tag.err { background: rgba(248,113,113,.12); color: var(--danger); }
.tag.src-manual { background: rgba(96,165,250,.12); color: var(--accent-2, #60a5fa); }
.tag.src-keymap { background: rgba(52,211,153,.12); color: #34d399; }
.tag.src-runner { background: rgba(192,132,252,.12); color: #c084fc; }
.tag.src-plugin { background: rgba(251,191,36,.12); color: #fbbf24; }
.cal-version { color: var(--accent); font-size: 10px; margin-left: 4px; }
.cal-desc { color: var(--text-2); font-size: 10px; font-weight: 400; margin-left: 6px; }
.cal-grid { display: flex; flex-direction: column; gap: 6px; }
.form-row { display: flex; align-items: center; gap: 4px; font-size: 11px; }
.form-label { color: var(--text-2); white-space: nowrap; min-width: 88px; }
.form-actions { display: flex; gap: 6px; }
.event-row { display: flex; align-items: center; gap: 6px; font-size: 11px; padding: 2px 0; cursor: pointer; min-width: 0; }
.event-row:hover { color: var(--accent); }
.event-time { color: var(--text-2); flex-shrink: 0; }
.event-kind { color: var(--text-1); flex-shrink: 0; }
.event-summary { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-2); min-width: 0; flex: 1; }
.list-empty { padding: 8px 6px; text-align: center; color: var(--text-2); font-size: 11px; }
.mono { font-family: var(--mono); }
.frame-count { color: var(--text-2); font-size: 11px; }
</style>
