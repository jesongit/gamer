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
