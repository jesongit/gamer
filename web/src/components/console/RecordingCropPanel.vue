<template>
  <div class="rec-crop">
    <div class="ps-head">
      <span class="ps-title">⏺ 录制裁切 · {{ draft.kind === 'swipe' ? '滑动起点' : '点击' }}</span>
      <span class="ps-sub mono">{{ zoomPct }}</span>
    </div>
    <div v-if="!draft.frameDataUrl" class="rec-crop-err">冻结帧不可用（画面未就绪），可改用坐标或丢弃。</div>
    <div class="crop-stage rec-crop-stage" ref="stageEl">
      <canvas
        ref="canvasEl"
        class="crop-canvas"
        @mousedown="onDown"
        @mousemove="onMove"
        @mouseup="onUp"
        @mouseleave="onLeave"
        @wheel="onWheel"
      ></canvas>
    </div>
    <div class="rec-crop-legend mono">
      <span class="lg lg-a">A 自动框</span><span class="lg lg-m">M 模板框{{ adjusted ? '（已调整）' : '' }}</span><span class="lg lg-s">S 搜索区域</span>
      <span class="rec-crop-sizes">M {{ Math.round(mRect.w) }}×{{ Math.round(mRect.h) }} · S {{ Math.round(sRect.w) }}×{{ Math.round(sRect.h) }} px</span>
    </div>
    <input v-model="name" class="input mono" placeholder="模板短名（record_click_….png）" @keydown.enter="confirm" />
    <div v-if="nameTaken" class="rec-crop-err">短名已存在，请改名（不会覆盖）。</div>
    <div v-else-if="draft.status === 'failed'" class="rec-crop-err">上传失败：{{ draft.reason }}</div>
    <div class="crop-actions">
      <button class="btn btn-sm" @click="ctx.downgrade(draft)">⌖ 只使用坐标</button>
      <button class="btn btn-sm" @click="ctx.discard(draft)">✕ 丢弃</button>
      <button class="btn btn-sm btn-primary" :disabled="!canConfirm" @click="confirm">{{ confirmLabel }}</button>
    </div>
  </div>
</template>

<script setup>
/**
 * 录制二次裁切面板（阶段 6，plan §11.4-11.7）：复用「二次裁切」画布交互语言——
 * 底图 = 冻结帧（加载 searchRect 对应范围，绝不止 50×50 小图），叠加自动框 A、
 * 当前模板框 M（默认 = A，可拖/缩）、搜索区域框 S 与实际像素尺寸；短名可编辑
 * （冲突要求改名不覆盖）。确认 → 上传（服务端灰度重编码）→ find/match 步骤定稿；
 * 「只使用坐标」→ tap 降级；失败保留草稿可重试/丢弃。
 * 纯展示+画布交互组件：状态与上传流程在 useRecording（ctx 提供），本组件不直接发请求。
 */
import { computed, nextTick, reactive, ref, watch } from 'vue'

const props = defineProps({ context: { type: Object, required: true } })
const ctx = reactive(props.context)

const canvasEl = ref(null)
const stageEl = ref(null)
const name = ref('')
const adjusted = ref(false)
const zoom = ref(1)
const mRect = reactive({ x: 0, y: 0, w: 0, h: 0 })
const imgW = ref(0)
const imgH = ref(0)
let frameImg = null
let frameImgOk = false
let drag = { mode: null, sx: 0, sy: 0, orig: null }

const draft = computed(() => ctx.draft)

/** 当前搜索区域（像素）：未调整 = A 外扩自动框；已调整 = union(A,M)+25px 裁剪。 */
const sRect = computed(() => (ctx.computeSearchRect(draft.value, mRect, adjusted.value)) || { x: 0, y: 0, w: 0, h: 0 })

const nameTaken = computed(() => {
  const n = normalizeName(name.value)
  return !!n && !!ctx.shortNameTaken && ctx.shortNameTaken(n)
})
const canConfirm = computed(() => !!draft.value && draft.value.frameDataUrl && !!normalizeName(name.value) && !nameTaken.value)
const confirmLabel = computed(() => (draft.value && draft.value.status === 'failed' ? '↻ 重试上传' : '✓ 确认上传'))
const zoomPct = computed(() => `${Math.round(scale() * 100)}%`)

function normalizeName(n) {
  const t = String(n || '').trim()
  if (!t) return ''
  return /\.(png|jpe?g)$/i.test(t) ? t : `${t}.png`
}

/** 显示缩放：fit 整帧 × zoom（初始让搜索区域约 200px 宽，便于拖动小模板）。 */
function baseFit() {
  const fw = imgW.value || 1
  const fh = imgH.value || 1
  return Math.min(300 / fw, 240 / fh)
}
function scale() { return baseFit() * zoom.value }

function loadFrame(d) {
  frameImg = null
  frameImgOk = false
  imgW.value = d?.frameW || 0
  imgH.value = d?.frameH || 0
  if (!d?.frameDataUrl) return
  const img = new Image()
  img.onload = () => {
    frameImg = img
    frameImgOk = true
    render()
  }
  img.onerror = () => { frameImgOk = false }
  img.src = d.frameDataUrl
}

function resetFor(d) {
  if (!d) return
  mRect.x = d.aRect.x
  mRect.y = d.aRect.y
  mRect.w = d.aRect.w
  mRect.h = d.aRect.h
  adjusted.value = false
  name.value = d.name || ctx.defaultNameFor(d.kind, d.draft && d.draft.shortName) || ''
  const s = d.searchRect || { w: 100, h: 100 }
  zoom.value = Math.max(1, Math.min(24, 200 / Math.max(s.w, s.h, 1) / baseFit()))
  loadFrame(d)
  nextTick(() => {
    // 初始视野滚到模板框附近
    const stage = stageEl.value
    if (stage) {
      const sc = scale()
      stage.scrollLeft = Math.max(0, d.aRect.x * sc - stage.clientWidth / 2)
      stage.scrollTop = Math.max(0, d.aRect.y * sc - stage.clientHeight / 2)
    }
    render()
  })
}

watch(() => draft.value && draft.value.uuid, () => { if (draft.value) resetFor(draft.value) }, { immediate: true })
watch(() => draft.value && draft.value.status, () => render())

function render() {
  const canvas = canvasEl.value
  const d = draft.value
  if (!canvas || !d) return
  const fw = imgW.value || d.frameW || 1
  const fh = imgH.value || d.frameH || 1
  const sc = scale()
  const W = Math.max(1, Math.round(fw * sc))
  const H = Math.max(1, Math.round(fh * sc))
  canvas.width = W
  canvas.height = H
  canvas.style.width = W + 'px'
  canvas.style.height = H + 'px'
  const g = canvas.getContext('2d')
  g.clearRect(0, 0, W, H)
  if (frameImgOk && frameImg) g.drawImage(frameImg, 0, 0, W, H)

  const box = (r, stroke, dash, fill) => {
    g.save()
    if (dash) g.setLineDash([5, 4])
    g.strokeStyle = stroke
    g.lineWidth = 1.5
    if (fill) { g.fillStyle = fill; g.fillRect(r.x * sc, r.y * sc, r.w * sc, r.h * sc) }
    g.strokeRect(r.x * sc, r.y * sc, r.w * sc, r.h * sc)
    g.restore()
  }
  const s = sRect.value
  box(s, 'rgba(96,165,250,.95)', true, 'rgba(96,165,250,.10)')          // S 搜索区域
  box(d.aRect, 'rgba(34,211,165,.9)', true)                              // A 自动框
  box(mRect, 'rgba(250,204,21,.95)', false, 'rgba(250,204,21,.08)')      // M 模板框
  // M 角点手柄
  g.fillStyle = '#fff'
  const hs = 5
  const sc2 = sc
  for (const [hx, hy] of [[mRect.x, mRect.y], [mRect.x + mRect.w, mRect.y], [mRect.x, mRect.y + mRect.h], [mRect.x + mRect.w, mRect.y + mRect.h]]) {
    g.fillRect(hx * sc2 - hs / 2, hy * sc2 - hs / 2, hs, hs)
  }
  g.fillStyle = 'rgba(250,204,21,.95)'
  g.font = '10px monospace'
  g.fillText(`${Math.round(mRect.w)}×${Math.round(mRect.h)}`, mRect.x * sc + 4, Math.max(10, mRect.y * sc - 4))
}

/** 鼠标 → 冻结帧像素坐标。 */
function evPt(e) {
  const canvas = canvasEl.value
  const rect = canvas.getBoundingClientRect()
  const sc = scale()
  return { x: (e.clientX - rect.left) / sc, y: (e.clientY - rect.top) / sc }
}

function hitMode(p) {
  const r = mRect
  const HIT = 10 / scale()
  const corners = { nw: [r.x, r.y], ne: [r.x + r.w, r.y], sw: [r.x, r.y + r.h], se: [r.x + r.w, r.y + r.h] }
  for (const [k, [hx, hy]] of Object.entries(corners)) {
    if (Math.hypot(p.x - hx, p.y - hy) <= HIT) return k
  }
  const edges = { n: [r.x + r.w / 2, r.y], s: [r.x + r.w / 2, r.y + r.h], w: [r.x, r.y + r.h / 2], e: [r.x + r.w, r.y + r.h / 2] }
  for (const [k, [hx, hy]] of Object.entries(edges)) {
    const onSeg = (k === 'n' || k === 's') ? (p.x >= r.x && p.x <= r.x + r.w) : (p.y >= r.y && p.y <= r.y + r.h)
    if (Math.hypot(p.x - hx, p.y - hy) <= HIT && onSeg) return k
  }
  if (p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h) return 'move'
  return null
}

function onDown(e) {
  if (!draft.value) return
  const p = evPt(e)
  const mode = hitMode(p)
  if (!mode) return
  drag = { mode, sx: p.x, sy: p.y, orig: { ...mRect } }
  e.preventDefault()
}

function onMove(e) {
  const p = evPt(e)
  if (!drag.mode) return
  const d = draft.value
  const o = drag.orig
  const MIN = 4
  const fw = imgW.value || d.frameW
  const fh = imgH.value || d.frameH
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v))
  const dx = p.x - drag.sx
  const dy = p.y - drag.sy
  switch (drag.mode) {
    case 'move':
      mRect.x = clamp(o.x + dx, 0, fw - o.w)
      mRect.y = clamp(o.y + dy, 0, fh - o.h)
      break
    case 'nw':
      mRect.x = clamp(o.x + dx, 0, o.x + o.w - MIN); mRect.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      mRect.w = o.x + o.w - mRect.x; mRect.h = o.y + o.h - mRect.y
      break
    case 'ne':
      mRect.w = clamp(o.w + dx, MIN, fw - o.x); mRect.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      mRect.h = o.y + o.h - mRect.y
      break
    case 'sw':
      mRect.x = clamp(o.x + dx, 0, o.x + o.w - MIN); mRect.w = o.x + o.w - mRect.x
      mRect.h = clamp(o.h + dy, MIN, fh - o.y)
      break
    case 'se':
      mRect.w = clamp(o.w + dx, MIN, fw - o.x); mRect.h = clamp(o.h + dy, MIN, fh - o.y)
      break
    case 'n':
      mRect.y = clamp(o.y + dy, 0, o.y + o.h - MIN); mRect.h = o.y + o.h - mRect.y
      break
    case 's':
      mRect.h = clamp(o.h + dy, MIN, fh - o.y)
      break
    case 'w':
      mRect.x = clamp(o.x + dx, 0, o.x + o.w - MIN); mRect.w = o.x + o.w - mRect.x
      break
    case 'e':
      mRect.w = clamp(o.w + dx, MIN, fw - o.x)
      break
  }
  render()
}

function endDrag() {
  if (!drag.mode) return
  drag = { mode: null, sx: 0, sy: 0, orig: null }
  const d = draft.value
  adjusted.value = !!d && (Math.round(mRect.x) !== Math.round(d.aRect.x) || Math.round(mRect.y) !== Math.round(d.aRect.y)
    || Math.round(mRect.w) !== Math.round(d.aRect.w) || Math.round(mRect.h) !== Math.round(d.aRect.h))
  render()
}

function onUp() { endDrag() }
function onLeave() { endDrag() }

function onWheel(e) {
  e.preventDefault()
  const next = Math.max(1, Math.min(24, zoom.value * (e.deltaY < 0 ? 1.2 : 1 / 1.2)))
  if (next !== zoom.value) {
    zoom.value = next
    render()
  }
}

function confirm() {
  const d = draft.value
  if (!d) return
  if (d.status === 'failed') { ctx.retry(d); return }
  ctx.confirm(d, { name: normalizeName(name.value), rect: { ...mRect }, adjusted: adjusted.value })
}
</script>

<style scoped>
.rec-crop { display: flex; flex-direction: column; gap: 8px; border: 1px solid rgba(250,204,21,.4); border-radius: var(--radius-sm); padding: 10px; background: var(--bg-1); flex-shrink: 0; }
.ps-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.ps-title { font-size: 13px; font-weight: 600; color: var(--warn); }
.ps-sub { font-size: 11px; color: var(--text-2); }
.crop-stage { display: flex; overflow: auto; border: 1px solid var(--border); border-radius: var(--radius-sm); background: #000; max-height: 260px; }
.crop-stage .crop-canvas { margin: auto; }
.crop-canvas { border-radius: var(--radius-sm); cursor: crosshair; background: #000; touch-action: none; }
.rec-crop-legend { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 10px; color: var(--text-2); }
.lg { display: inline-flex; align-items: center; gap: 4px; }
.lg::before { content: ''; width: 10px; height: 10px; border: 1.5px solid; display: inline-block; }
.lg-a::before { border-color: rgba(34,211,165,.9); border-style: dashed; }
.lg-m::before { border-color: rgba(250,204,21,.95); }
.lg-s::before { border-color: rgba(96,165,250,.95); border-style: dashed; }
.rec-crop-sizes { margin-left: auto; }
.rec-crop-err { font-size: 11px; color: var(--danger); }
.crop-actions { display: flex; gap: 8px; }
.crop-actions .btn-primary { margin-left: auto; }
.mono { font-family: var(--mono); font-size: 11px; color: var(--text-1); }
</style>
