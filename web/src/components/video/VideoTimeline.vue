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
    </template>
  </section>
</template>

<script setup>
// 时间轴区：纯只读离线预览（不触达设备）。浏览器 <video> 只做流畅预览，
// 精确帧一律取服务端确定帧端点（同一请求逐字节可重复）——
// 计划 §4.3：媒体播放器定位与精确帧提取分离，避免 seek 后拿到旧画面。
// 逐帧 ±（Phase 5）：走服务端真实展示帧表（/frames/:index 相邻帧，VFR/B 帧
// 展示序由服务端归一），前端没有任何固定步长（33ms）假设。
import { computed, ref, watch } from 'vue'
import { ptsFromTime, videoApi } from './videoApi'

const props = defineProps({
  media: { type: Object, default: null },
})

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

const fileUrl = computed(() => (props.media ? videoApi.mediaFileUrl(props.media.id) : ''))
const totalSeconds = computed(() => {
  const us = Number(props.media?.duration_us)
  return Number.isFinite(us) && us > 0 ? us / 1e6 : 0
})

watch(() => props.media?.id, () => {
  currentTime.value = 0
  clearFrame()
  framesMeta.value = null
  currentFrame.value = null
  stepBusy.value = false
  void loadFramesMeta()
}, { immediate: true })

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
.frame-count { color: var(--text-2); font-size: 11px; }
</style>
