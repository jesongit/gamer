<template>
  <div v-if="recovery.message" class="update-recovery" role="status">
    <span>{{ recovery.message }}</span>
    <button v-if="recovery.ready" class="btn btn-sm" @click="refreshUpdatedPage">刷新到新版</button>
  </div>
</template>
<script setup>
import { onMounted, onUnmounted } from 'vue'
import { recovery, refreshUpdatedPage, startUpdateRecovery } from '../system/updateRecovery'
let stop
onMounted(() => { stop = startUpdateRecovery() })
onUnmounted(() => stop?.())
</script>
<style scoped>
.update-recovery{position:fixed;bottom:18px;left:50%;transform:translateX(-50%);z-index:10000;display:flex;align-items:center;gap:12px;max-width:calc(100vw - 32px);padding:12px 16px;border:1px solid var(--border);border-radius:8px;background:var(--bg-card,#20242c);color:var(--text,#fff);box-shadow:0 4px 20px #0004;font-size:13px}
</style>
