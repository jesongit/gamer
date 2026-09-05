<!-- 设备占用冲突（409 device_busy）提示弹窗（RUN-003 / ADR-12）：
     逐条消费 store.runConflicts 队列，展示对方运行目标（entrypoint 为主，
     script_id 为服务端保留的兼容展示字段）、来源中文标签、开始时间
     （本地时区格式化），提供「仍要查看日志」跳控制台对应设备；
     关闭不打断当前页面其他功能。 -->
<template>
  <div v-if="cur" class="modal-mask" @click.self="close">
    <div class="modal conflict-modal">
      <div class="modal-head">
        <span class="title">⚠️ 设备正被占用</span>
        <button class="btn btn-ghost btn-sm" @click="close">✕</button>
      </div>
      <div class="modal-body">
        <div class="cf-row"><span class="cf-k">设备</span><span>{{ deviceName(cur.device_id) }}</span></div>
        <div class="cf-row"><span class="cf-k">运行目标</span><span class="mono">{{ cur.entrypoint || cur.script_id || '未知' }}</span></div>
        <div class="cf-row"><span class="cf-k">来源</span><span><span class="tag run">{{ sourceLabel(cur.source) || '未知' }}</span></span></div>
        <div class="cf-row"><span class="cf-k">开始时间</span><span class="mono">{{ formatLocalTime(cur.started_at) || '未知' }}</span></div>
        <div class="cf-tip">一个设备同时只允许一个自动化执行实例，本次启动已被服务端拒绝（409）。可前往投屏控制台查看该设备的运行日志。</div>
      </div>
      <div class="modal-foot">
        <button class="btn" @click="close">知道了</button>
        <button class="btn btn-primary" @click="viewLogs">仍要查看日志</button>
      </div>
    </div>
  </div>
</template>

<script setup>
import { computed, onMounted, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import { store, devicesData, runConflicts, shiftRunConflict } from '../store'
import { sourceLabel, formatLocalTime } from '../runs'

const router = useRouter()
// 只渲染队首，关闭后自动露出下一条
const cur = computed(() => runConflicts.value[0] || null)

const deviceName = id => devicesData.value.find(d => d.id === id)?.name || id || '未知'

function close() {
  shiftRunConflict()
}

/** 「仍要查看日志」：切到冲突设备的控制台（覆盖当前选中并跳转） */
function viewLogs() {
  const c = cur.value
  if (c?.device_id) store.deviceId = c.device_id
  close()
  if (router.currentRoute.value.path !== '/console') router.push('/console')
}

const onKey = e => { if (e.key === 'Escape' && cur.value) close() }
onMounted(() => window.addEventListener('keydown', onKey))
onUnmounted(() => window.removeEventListener('keydown', onKey))
</script>

<style scoped>
.conflict-modal { width: 460px; max-width: calc(100vw - 32px); }
.cf-row { display: flex; align-items: baseline; gap: 12px; font-size: 13px; padding: 2px 0; word-break: break-all; }
.cf-k { color: var(--text-2); flex-shrink: 0; width: 60px; }
.cf-tip { font-size: 12px; color: var(--text-2); line-height: 1.7; border-top: 1px dashed var(--border); padding-top: 10px; }
</style>
