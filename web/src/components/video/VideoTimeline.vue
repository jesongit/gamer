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
            :disabled="!framesMeta || stepBusy"
            @click="stepFrame(-1)"
          >− 帧</button>
          <button
            class="mini-btn"
            type="button"
            data-testid="frame-next"
            title="下一展示帧（服务端真实帧表定位）"
            :disabled="!framesMeta || stepBusy"
            @click="stepFrame(1)"
          >+ 帧</button>
          <button
            class="mini-btn"
            type="button"
            :disabled="frameBusy"
            data-testid="frame-exact"
            @click="grabExactFrame()"
          >{{ frameBusy ? '取帧中…' : '◎ 精确帧' }}</button>
        </span>
      </div>

      <div v-if="frameError" class="zone-error" role="alert" data-testid="frame-error">{{ frameError }}</div>

      <div v-if="frameUrl" class="frame-box" data-testid="frame-box">
        <img
          :src="frameUrl"
          class="frame-shot"
          alt="服务端精确帧"
          data-testid="frame-image"
          @load="onFrameLoad"
          @error="onFrameError"
        />
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
import { describeCalibration } from './calibration'
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
})

const emit = defineEmits(['add-marker', 'remove-marker', 'update-marker', 'save-calibration'])

const videoEl = ref(null)
const currentTime = ref(0)
const frameUrl = ref('')
const frameCaption = ref('')
const frameBusy = ref(false)
const frameError = ref('')
// 真实展示帧表元信息（帧总数；加载失败 → 逐帧按钮禁用，不做时间近似降级）
const framesMeta = ref(null)
// 当前锁定帧身份 {index, pts_us}（null = 未锁定，按预览时间重新解析）
const currentFrame = ref(null)
const stepBusy = ref(false)
const markerBusy = ref(false)
const newMarkerLabel = ref('')

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

const markersAvailable = computed(() => !!props.media && !!framesMeta.value)
const staleCount = computed(() => (props.calibration
  ? props.markers.filter(marker => Number(marker.frame?.calibration_version) !== Number(props.calibration.version)).length
  : 0))
const calibrationText = computed(() => {
  if (!props.calibration || !props.media) return ''
  const encoded = { width: props.media.width, height: props.media.height }
  try { return describeCalibration(props.calibration, encoded) } catch { return '' }
})

watch(() => props.media?.id, () => {
  currentTime.value = 0
  clearFrame()
  framesMeta.value = null
  currentFrame.value = null
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
  const rectGiven = calForm.rectW !== '' && calForm.rectH !== ''
  const refW = Math.round(Number(calForm.refW))
  const refH = Math.round(Number(calForm.refH))
  // 显式范围校验（不做 max(1,·) 静默钳制——0/负数是输入错误，必须报给用户）
  if (!(refW >= 1) || !(refH >= 1)) {
    calibrationError.value = '参考分辨率宽高必须 ≥ 1'
    return
  }
  const next = {
    rotation: Number(calForm.rotation) || 0,
    pixel_aspect: {
      num: Math.max(1, Math.round(Number(calForm.paNum) || 1)),
      den: Math.max(1, Math.round(Number(calForm.paDen) || 1)),
    },
    reference_size: { width: refW, height: refH },
    content_rect: rectGiven
      ? {
        x: Math.max(0, Math.round(Number(calForm.rectX) || 0)),
        y: Math.max(0, Math.round(Number(calForm.rectY) || 0)),
        w: Math.max(1, Math.round(Number(calForm.rectW) || 0)),
        h: Math.max(1, Math.round(Number(calForm.rectH) || 0)),
      }
      : null,
  }
  emit('save-calibration', next)
}

async function loadFramesMeta() {
  const id = props.media?.id
  if (!id) return
  try {
    framesMeta.value = await videoApi.mediaFrames(id)
  } catch (e) {
    framesMeta.value = null
    frameError.value = '展示帧表加载失败：逐帧步进不可用（' + (e?.message || e) + '）'
  }
}

function clearFrame() {
  frameUrl.value = ''
  frameCaption.value = ''
  frameError.value = ''
  frameBusy.value = false
}

function onTimeUpdate() {
  const t = Number(videoEl.value?.currentTime)
  if (Number.isFinite(t)) currentTime.value = t
}

// frameBusy 由 <img> 的 load/error 事件驱动复位（请求本身是 URL 赋值，无 await 点）；
// 切换素材时 clearFrame 兜底复位，避免按钮卡在禁用态。
function onFrameLoad() {
  frameBusy.value = false
}

function onFrameError() {
  frameBusy.value = false
  frameError.value = '精确帧获取失败：素材文件缺失或帧参数越界'
}

function onSeeked() {
  const t = Number(videoEl.value?.currentTime)
  if (Number.isFinite(t)) currentTime.value = t
}

/** 把帧身份渲染到精确帧区：按展示序索引寻址（同一请求逐字节可重复），
 *  并把预览 <video> seek 到该帧时刻保持两者同步。 */
function showFrameByIndex(position) {
  if (!props.media) return
  const nextUrl = videoApi.mediaFrameUrl(props.media.id, { index: position.index, maxWidth: 640 })
  frameError.value = ''
  currentFrame.value = position
  if (nextUrl === frameUrl.value) return
  frameBusy.value = true
  frameUrl.value = nextUrl
  frameCaption.value = `帧 ${position.index} · pts_us=${position.pts_us}（t=${(position.pts_us / 1e6).toFixed(3)}s）`
  const el = videoEl.value
  if (el) {
    try { el.currentTime = position.pts_us / 1e6 } catch { /* 元数据未就绪时静默 */ }
  }
}

/** 取当前预览时间的精确帧；ptsUs 传入时直接按该值请求（同一请求逐字节可重复）。
 *  时间寻址的帧身份由服务端解析（首个 pts ≥ 目标的展示帧），本地不估算。 */
function grabExactFrame(ptsUs) {
  if (!props.media || frameBusy.value) return
  const pts = Number.isFinite(Number(ptsUs)) ? Math.max(0, Math.round(Number(ptsUs))) : ptsFromTime(currentTime.value)
  // 面板宽度有限，取 640px 上限；加载态由 img load/error 收口
  const nextUrl = videoApi.mediaFrameUrl(props.media.id, { ptsUs: pts, maxWidth: 640 })
  // 同一帧重复请求：src 不变则 img 不会再触发 load，busy 会卡死 → 直接 no-op（画面已在）
  if (nextUrl === frameUrl.value) return
  frameError.value = ''
  frameBusy.value = true
  frameUrl.value = nextUrl
  frameCaption.value = `pts_us=${pts}（t=${(pts / 1e6).toFixed(3)}s）`
  // 时间寻址结果的身份（解析后帧）未知：清锁，下一次逐帧按当前时间重新解析
  currentFrame.value = null
}

/** 逐帧 ±：服务端真实展示帧表相邻定位（prev/next），不按固定时长估算。
 *  首步先按预览时间解析当前帧，之后沿相邻帧链走；边界（首/末帧）为 no-op。 */
async function stepFrame(direction) {
  if (!props.media || !framesMeta.value || stepBusy.value) return
  const id = props.media.id
  stepBusy.value = true
  try {
    let position = currentFrame.value
    if (!position) {
      const meta = await videoApi.mediaFrames(id, { ptsUs: ptsFromTime(currentTime.value) })
      position = meta?.current || null
      framesMeta.value = meta || framesMeta.value
    }
    if (!position) {
      // 空素材（0 帧）：无相邻可言
      return
    }
    const neighbors = await videoApi.mediaFrameNeighbors(id, position.index)
    const target = direction < 0 ? neighbors?.prev : neighbors?.next
    if (!target) return // 首/末帧边界：不动
    showFrameByIndex(target)
  } catch (e) {
    frameError.value = '逐帧定位失败：' + (e?.message || e)
  } finally {
    stepBusy.value = false
  }
}

// ---- 标记 ----

/** 在当前帧加标记：先解析当前帧身份（未锁定时按预览时间服务端解析），再上抛
 *  帧身份（frame_index+pts_us+当前校准版本）——绝不存浏览器浮点秒。 */
async function addMarkerAtCurrentFrame() {
  if (!props.media || !framesMeta.value || markerBusy.value) return
  markerBusy.value = true
  try {
    let position = currentFrame.value
    if (!position) {
      const meta = await videoApi.mediaFrames(props.media.id, { ptsUs: ptsFromTime(currentTime.value) })
      position = meta?.current || null
      framesMeta.value = meta || framesMeta.value
    }
    if (!position) {
      frameError.value = '当前没有可标记的展示帧'
      return
    }
    showFrameByIndex(position)
    emit('add-marker', {
      label: newMarkerLabel.value.trim(),
      frame: {
        media_id: props.media.id,
        frame_index: position.index,
        pts_us: position.pts_us,
        calibration_version: Number(props.calibration?.version) || 1,
      },
    })
    newMarkerLabel.value = ''
  } catch (e) {
    frameError.value = '标记定位失败：' + (e?.message || e)
  } finally {
    markerBusy.value = false
  }
}

function isStale(marker) {
  return Number(marker?.frame?.calibration_version) !== Number(props.calibration?.version)
}

async function jumpToMarker(marker) {
  if (!props.media || marker.frame.media_id !== props.media.id) {
    frameError.value = `该标记在其他素材上（${marker.frame.media_id}），请先在素材库切换`
    return
  }
  showFrameByIndex({ index: marker.frame.frame_index, pts_us: marker.frame.pts_us })
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
  try {
    // 事件 PTS → 展示帧身份（服务端解析首个 pts ≥ 目标的展示帧）
    const meta = await videoApi.mediaFrames(props.media.id, { ptsUs: view.ptsUs })
    const position = meta?.current || null
    if (!position) return
    showFrameByIndex(position)
  } catch (e) {
    frameError.value = '事件跳转失败：' + (e?.message || e)
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
.time-actions { display: flex; gap: 5px; margin-left: auto; }
.mini-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 3px 7px; }
.mini-btn:hover { border-color: var(--accent); color: var(--accent); }
.mini-btn:disabled { opacity: .45; cursor: not-allowed; }
.mini-btn.danger:hover { border-color: var(--danger); color: var(--danger); }
.frame-box { display: flex; flex-direction: column; gap: 4px; padding: 6px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-0); }
.frame-shot { max-width: 100%; max-height: 200px; object-fit: contain; align-self: center; image-rendering: pixelated; }
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
