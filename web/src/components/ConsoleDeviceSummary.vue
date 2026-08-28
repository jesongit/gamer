<template>
  <div class="dev-summary">
    <div class="ps-head">
      <span class="dot" :class="connected ? 'ok' : 'off'"></span>
      <span class="ps-title">{{ device.name }}</span>
      <span class="tag" :class="connected ? 'info' : ''">{{ connected ? '已连接' : deviceStatus }}</span>
    </div>
    <div class="sum-row">
      <span class="sum-label">接入</span>
      <span class="sum-value"><span class="kind-badge">{{ kindIcon }} {{ kindLabel }}</span></span>
    </div>
    <div class="sum-row">
      <span class="sum-label">地址</span>
      <span class="sum-value mono">{{ device.addr || '—' }}</span>
    </div>
    <div class="sum-row">
      <span class="sum-label">屏幕</span>
      <span class="sum-value">{{ screenSummary }}</span>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  device: { type: Object, required: true },
  connected: { type: Boolean, default: false },
  kindIcon: { type: String, default: '📱' },
  kindLabel: { type: String, default: '' },
  screenSummary: { type: String, default: '—' },
})

const deviceStatus = computed(() => (props.device?.status === 'online' ? '在线' : '离线'))
</script>

<style scoped>
.dev-summary { display: flex; flex-direction: column; gap: 8px; padding-bottom: 10px; border-bottom: 1px solid var(--border); }
.ps-head { display: flex; align-items: center; gap: 8px; }
.ps-title { font-size: 13px; font-weight: 600; }
.sum-row { display: flex; align-items: center; gap: 8px; font-size: 12px; }
.sum-label { width: 34px; flex-shrink: 0; font-size: 11px; color: var(--text-2); }
.sum-value { min-width: 0; word-break: break-all; color: var(--text-1); }
.kind-badge {
  display: inline-flex; align-items: center; gap: 4px; padding: 2px 8px;
  background: var(--bg-3); border: 1px solid var(--border);
  border-radius: 12px; font-size: 11px; white-space: nowrap;
}
.mono { font-family: var(--mono); font-size: 11px; color: var(--text-1); }
</style>
