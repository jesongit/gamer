<template>
  <div
    ref="videoWrap"
    class="video-wrap"
    :class="{ 'keyboard-active': props.keyboardFocused }"
  >
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

    <!-- 脚本运行可视化：引擎 tap/swipe/匹配命中（服务端经 control DataChannel 推送） -->
    <div v-if="props.scriptFx.tap.show" class="alt-tap" :style="props.fxTapStyle">
      <span class="alt-label">tap</span>
    </div>
    <div v-if="props.scriptFx.swipe.show" class="alt-region" :style="props.fxSwipeStyle">
      <span class="alt-label">swipe</span>
    </div>
    <div v-if="props.scriptFx.hit.show" class="hit-box" :class="{ 'hit-miss': props.scriptFx.hit.miss }" :style="props.fxHitStyle">
      <span class="hit-label">{{ props.scriptFx.hit.label }}</span>
    </div>

    <!-- 当前按键映射：纯展示层，pointer-events:none，绝不拦截既有投屏鼠标/框选操作。 -->
    <div v-if="props.keymapOverlay.length" class="keymap-overlay" aria-hidden="true">
      <div
        v-for="item in props.keymapOverlay"
        :key="item.id"
        class="keymap-mark"
        :class="[`keymap-mark-${item.type || 'tap'}`, { active: item.active }]"
        :style="item.style"
      >
        <span v-if="item.type === 'swipe'" class="keymap-arrow">➜</span>
        <span class="keymap-label">{{ item.label }}</span>
      </div>
    </div>
    <div v-if="props.keymapStatus.name" class="keymap-status" role="status">
      ⌨ {{ props.keymapStatus.name }}<span v-if="props.keymapStatus.inactive"> · 文本模式下映射不生效</span>
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

    <div v-if="props.connected && props.keyboardFocused" class="keyboard-focus-badge" role="status">
      ⌨ 键盘控制已启用
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
  scriptFx: { type: Object, required: true },
  keymapOverlay: { type: Array, default: () => [] },
  keymapStatus: { type: Object, default: () => ({}) },
  fxTapStyle: { type: Object, default: () => ({}) },
  fxSwipeStyle: { type: Object, default: () => ({}) },
  fxHitStyle: { type: Object, default: () => ({}) },
  loupe: { type: Object, required: true },
  onMouseDown: { type: Function, required: true },
  onMouseMove: { type: Function, required: true },
  onMouseUp: { type: Function, required: true },
  onWheel: { type: Function, required: true },
  onVideoMouseLeave: { type: Function, required: true },
  keyboardFocused: { type: Boolean, default: false },
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
  border: 0; border-radius: 0;
  overflow: hidden; min-height: 300px;
}
.video-wrap.keyboard-active { outline: none; }
.keyboard-focus-badge {
  position: absolute; left: 12px; bottom: 12px; z-index: 12;
  padding: 5px 9px; border: 1px solid rgba(34,211,165,.7); border-radius: 7px;
  background: rgba(4, 22, 18, .9); color: var(--accent); font-size: 11px;
  box-shadow: 0 0 12px rgba(34,211,165,.22); pointer-events: none;
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

.keymap-overlay { position: absolute; inset: 0; z-index: 7; pointer-events: none; }
.keymap-mark {
  position: absolute; transform: translate(-50%, -50%); min-width: 18px; min-height: 18px;
  display: flex; align-items: center; justify-content: center; border: 1px solid rgba(251,191,36,.8);
  background: rgba(251,191,36,.18); color: #fde68a; border-radius: 50%;
  box-shadow: 0 0 10px rgba(251,191,36,.25); transition: opacity .12s, filter .12s;
}
.keymap-mark-swipe { transform: none; width: 0; height: 0; min-width: 0; min-height: 0; border: 0; background: none; box-shadow: none; }
.keymap-mark-swipe::before { content: ''; position: absolute; left: 0; top: 0; width: var(--keymap-w, 0px); height: 2px; transform-origin: left center; transform: rotate(var(--keymap-angle, 0deg)); background: rgba(251,191,36,.8); box-shadow: 0 0 8px rgba(251,191,36,.35); }
.keymap-arrow { position: absolute; left: var(--keymap-w, 0px); top: 0; transform: translate(-50%, -50%) rotate(var(--keymap-angle, 0deg)); color: #fde68a; font-size: 18px; line-height: 1; }
.keymap-label { position: absolute; left: 50%; top: -23px; transform: translateX(-50%); white-space: nowrap; padding: 2px 6px; border: 1px solid rgba(251,191,36,.55); border-radius: 5px; background: rgba(8,10,16,.82); color: #fde68a; font: 600 10px var(--mono); }
.keymap-mark.active { filter: brightness(1.8); background: rgba(251,191,36,.44); }
.keymap-status { position: absolute; left: 12px; top: 42px; z-index: 8; pointer-events: none; padding: 4px 8px; border: 1px solid rgba(251,191,36,.5); border-radius: 6px; background: rgba(8,10,16,.78); color: #fde68a; font: 11px var(--mono); }

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
