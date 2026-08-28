<template>
  <div class="video-wrap" ref="videoWrap">
    <video
      ref="videoElement"
      autoplay
      playsinline
      :muted="props.audioMuted"
      class="video-stream"
      @mousedown="props.onMouseDown"
      @mousemove="props.onMouseMove"
      @mouseup="props.onMouseUp"
      @wheel.prevent="props.onWheel"
      @contextmenu.prevent
      @mouseleave="props.onVideoMouseLeave"
    ></video>

    <!-- 找图命中框演示（模板测试） -->
    <div v-if="props.showHit" class="hit-box" :class="{ 'hit-miss': props.hitMiss }" :style="props.hitStyle">
      <span class="hit-label">{{ props.hitLabel }}</span>
    </div>

    <!-- 框选模板 -->
    <div v-if="props.selecting" class="select-box" :style="props.selStyle"></div>

    <!-- alt 模式点击/滑动反馈 -->
    <div v-if="props.altFeedback.show && props.altFeedback.kind === 'tap'" class="alt-tap" :style="props.altTapStyle">
      <span class="alt-label">tap</span>
    </div>
    <div v-if="props.altFeedback.show && props.altFeedback.kind === 'region'" class="alt-region" :style="props.altFeedbackStyle">
      <span class="alt-label">region</span>
    </div>

    <!-- 脚本运行可视化：引擎 tap/swipe/匹配命中（服务端经 control DataChannel 推送，样式复用 alt/hit） -->
    <div v-if="props.scriptFx.tap.show" class="alt-tap" :style="props.fxTapStyle">
      <span class="alt-label">tap</span>
    </div>
    <div v-if="props.scriptFx.swipe.show" class="alt-region" :style="props.fxSwipeStyle">
      <span class="alt-label">swipe</span>
    </div>
    <div v-if="props.scriptFx.hit.show" class="hit-box" :class="{ 'hit-miss': props.scriptFx.hit.miss }" :style="props.fxHitStyle">
      <span class="hit-label">{{ props.scriptFx.hit.label }}</span>
    </div>

    <!-- 放大预览镜 -->
    <div class="loupe" v-show="props.loupe.show" :style="{ left: props.loupe.x + 'px', top: props.loupe.y + 'px' }">
      <canvas ref="loupeCanvas" width="300" height="300"></canvas>
      <span class="loupe-tag">{{ props.loupe.zoom }}×</span>
    </div>

    <div class="v-overlay" v-if="!props.connected">
      <div class="v-connecting" v-if="props.connecting">
        <span class="dot run"></span> 正在建立 WebRTC 连接…
      </div>
      <div v-else>
        <div class="v-empty-icon">📴</div>
        <div class="v-empty-text">{{ props.errorMsg || '未连接设备' }}</div>
        <button class="btn btn-primary" @click="props.flushAndConnect">连接 {{ props.currentName }}</button>
      </div>
    </div>

    <div class="v-stats" v-if="props.connected">
      <span class="st">{{ props.fps }} fps</span>
      <span class="st">延迟 {{ props.delay }}ms</span>
      <span class="st">{{ props.res }}</span>
      <span class="st">码率 {{ props.bitrate }}</span>
      <span class="st">H.264 · WebRTC</span>
    </div>

    <button class="v-fs" @click="props.fullscreen" title="全屏">⛶</button>
  </div>
</template>

<script setup>
import { onMounted, ref } from 'vue'

const props = defineProps({
  connected: { type: Boolean, default: false },
  connecting: { type: Boolean, default: false },
  errorMsg: { type: String, default: '' },
  currentName: { type: String, default: '' },
  audioMuted: { type: Boolean, default: true },
  fps: { type: [Number, String], default: 0 },
  delay: { type: [Number, String], default: 0 },
  res: { type: String, default: '—' },
  bitrate: { type: String, default: '—' },
  showHit: { type: Boolean, default: false },
  hitMiss: { type: Boolean, default: false },
  hitStyle: { type: Object, default: () => ({}) },
  hitLabel: { type: String, default: '' },
  selecting: { type: Boolean, default: false },
  selStyle: { type: Object, default: () => ({}) },
  altFeedback: { type: Object, required: true },
  altTapStyle: { type: Object, default: () => ({}) },
  altFeedbackStyle: { type: Object, default: () => ({}) },
  scriptFx: { type: Object, required: true },
  fxTapStyle: { type: Object, default: () => ({}) },
  fxSwipeStyle: { type: Object, default: () => ({}) },
  fxHitStyle: { type: Object, default: () => ({}) },
  loupe: { type: Object, required: true },
  onMouseDown: { type: Function, required: true },
  onMouseMove: { type: Function, required: true },
  onMouseUp: { type: Function, required: true },
  onWheel: { type: Function, required: true },
  onVideoMouseLeave: { type: Function, required: true },
  flushAndConnect: { type: Function, required: true },
  fullscreen: { type: Function, required: true },
})

const emit = defineEmits(['video-mounted', 'wrap-mounted', 'loupe-mounted'])
const videoWrap = ref(null)
const videoElement = ref(null)
const loupeCanvas = ref(null)

onMounted(() => {
  emit('video-mounted', videoElement.value)
  emit('wrap-mounted', videoWrap.value)
  emit('loupe-mounted', loupeCanvas.value)
})
</script>

<style scoped>
.video-wrap {
  flex: 1; position: relative; background: #000;
  border: 1px solid var(--border); border-radius: var(--radius);
  overflow: hidden; min-height: 300px;
}

.video-stream { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: contain; user-select: none; }

.hit-box {
  position: absolute; border: 2px solid var(--accent);
  box-shadow: 0 0 12px rgba(34,211,165,.5); border-radius: 4px;
  pointer-events: none; z-index: 5;
}
.hit-label {
  position: absolute; top: -22px; left: 0; background: var(--accent); color: #06251c;
  font-size: 10px; font-weight: 700; padding: 1px 6px; border-radius: 4px; white-space: nowrap;
}
.hit-miss { border-style: dashed; border-color: var(--danger); box-shadow: none; background: rgba(239,68,68,.06); }
.hit-miss .hit-label { background: var(--danger); color: #fff; }

.select-box {
  position: absolute; border: 2px dashed var(--accent-2);
  background: rgba(56,189,248,.12); pointer-events: none; z-index: 5;
}

.alt-tap {
  position: absolute; z-index: 6; width: 14px; height: 14px;
  border-radius: 50%; border: 2px solid var(--accent-2);
  background: rgba(56,189,248,.28);
  transform: translate(-50%, -50%); pointer-events: none;
  box-shadow: 0 0 8px rgba(56,189,248,.6);
}
.alt-region {
  position: absolute; z-index: 6; border: 2px dashed var(--accent-2);
  background: rgba(56,189,248,.12); pointer-events: none;
  box-shadow: 0 0 10px rgba(56,189,248,.25);
}
.alt-label {
  position: absolute; top: -20px; left: 0; font-size: 10px;
  color: var(--accent-2); background: rgba(8,10,16,.7);
  padding: 1px 5px; border-radius: 4px; white-space: nowrap;
  font-family: var(--mono);
}

.loupe {
  position: fixed; z-index: 200; width: 150px; height: 150px;
  border: 1px solid rgba(34,211,165,.5); border-radius: 10px; overflow: hidden;
  background: #000; box-shadow: 0 8px 30px rgba(0,0,0,.6);
  pointer-events: none;
}
.loupe canvas { width: 100%; height: 100%; display: block; }
.loupe-tag {
  position: absolute; right: 6px; bottom: 4px; font-size: 10px;
  color: #fff; background: rgba(0,0,0,.55); padding: 1px 5px; border-radius: 6px;
  font-family: var(--mono);
}

.v-overlay {
  position: absolute; inset: 0; z-index: 10; display: flex;
  align-items: center; justify-content: center;
  background: rgba(8,10,16,.72); backdrop-filter: blur(2px);
}
.v-connecting { display: flex; align-items: center; gap: 10px; color: var(--accent); font-size: 14px; }
.v-empty-icon { font-size: 44px; text-align: center; opacity: .6; }
.v-empty-text { color: var(--text-1); margin: 10px 0 16px; max-width: 320px; text-align: center; }

.v-stats {
  position: absolute; left: 12px; top: 12px; z-index: 6;
  display: flex; gap: 8px; background: rgba(8,10,16,.6);
  border: 1px solid rgba(255,255,255,.08); border-radius: 20px; padding: 4px 10px;
}
.st { font-size: 11px; color: var(--text-1); font-family: var(--mono); }

.v-fs {
  position: absolute; right: 12px; top: 12px; z-index: 6;
  background: rgba(8,10,16,.6); border: 1px solid rgba(255,255,255,.08);
  color: var(--text-1); border-radius: 8px; width: 30px; height: 30px; cursor: pointer;
}
.v-fs:hover { color: var(--accent); border-color: var(--accent); }
</style>
