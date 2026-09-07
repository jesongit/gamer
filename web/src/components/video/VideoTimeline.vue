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
        <span class="time-actions">
          <button class="mini-btn" type="button" data-testid="frame-prev" title="后退一帧（约 33ms）后取精确帧" @click="stepFrame(-1)">− 帧</button>
          <button class="mini-btn" type="button" data-testid="frame-next" title="前进一帧（约 33ms）后取精确帧" @click="stepFrame(1)">+ 帧</button>
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
    </template>
  </section>
</template>

<script setup>
// 时间轴区：纯只读离线预览（不触达设备）。浏览器 <video> 只做流畅预览，
// 精确帧一律取服务端确定帧端点（同一请求逐字节可重复）——
// 计划 §4.3：媒体播放器定位与精确帧提取分离，避免 seek 后拿到旧画面。
import { computed, ref, watch } from 'vue'
import { FRAME_STEP_SECONDS, ptsFromTime, videoApi } from './videoApi'

const props = defineProps({
  media: { type: Object, default: null },
})

const videoEl = ref(null)
const currentTime = ref(0)
const frameUrl = ref('')
const frameCaption = ref('')
const frameBusy = ref(false)
const frameError = ref('')
// 逐帧步进：seek 完成后再取精确帧（seeked 未触发时用兜底定时器取当前时间）
let pendingStep = 0
let stepFallbackTimer = null

const fileUrl = computed(() => (props.media ? videoApi.mediaFileUrl(props.media.id) : ''))
const totalSeconds = computed(() => {
  const us = Number(props.media?.duration_us)
  return Number.isFinite(us) && us > 0 ? us / 1e6 : 0
})

watch(() => props.media?.id, () => {
  currentTime.value = 0
  clearFrame()
  pendingStep = 0
  if (stepFallbackTimer) clearTimeout(stepFallbackTimer)
  stepFallbackTimer = null
})

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
  if (stepFallbackTimer) {
    clearTimeout(stepFallbackTimer)
    stepFallbackTimer = null
  }
  const t = Number(videoEl.value?.currentTime)
  if (Number.isFinite(t)) currentTime.value = t
  if (pendingStep !== 0) {
    pendingStep = 0
    grabExactFrame()
  }
}

/** 取当前预览时间的精确帧；ptsUs 传入时直接按该值请求（同一请求逐字节可重复）。 */
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
}

/** 逐帧 ± ：预览近似步进（±33ms）→ seek 完成（或兜底超时）后取服务端精确帧。 */
function stepFrame(direction) {
  const el = videoEl.value
  if (!props.media || !el) return
  const from = Number.isFinite(Number(el.currentTime)) ? Number(el.currentTime) : currentTime.value
  const to = Math.min(Math.max(from + direction * FRAME_STEP_SECONDS, 0), totalSeconds.value || Number.MAX_VALUE)
  pendingStep = direction
  frameError.value = ''
  try {
    el.currentTime = to
  } catch (e) {
    // 元数据未加载等场景无法 seek：退化为按当前显示时间取帧
    pendingStep = 0
  }
  if (stepFallbackTimer) clearTimeout(stepFallbackTimer)
  stepFallbackTimer = setTimeout(() => {
    stepFallbackTimer = null
    if (pendingStep !== 0) {
      pendingStep = 0
      grabExactFrame(ptsFromTime(currentTime.value))
    }
  }, 800)
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
.frame-box { display: flex; flex-direction: column; gap: 4px; padding: 6px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-0); }
.frame-shot { max-width: 100%; max-height: 200px; object-fit: contain; align-self: center; image-rendering: pixelated; }
.frame-caption { color: var(--text-2); font-size: 10px; text-align: center; }
.frame-hint { color: var(--text-2); font-size: 10px; line-height: 1.5; }
.zone-error { padding: 5px 7px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.08); color: var(--danger); font-size: 11px; line-height: 1.5; }
.mono { font-family: var(--mono); }
</style>
