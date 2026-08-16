<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">运行日志</div>
        <div class="page-sub">脚本执行轨迹 · 自动刷新</div>
      </div>
      <div class="head-actions">
        <div class="filters">
          <select v-model="fDevice" class="select select-sm" @change="load">
            <option value="">全部设备</option>
            <option v-for="d in devices" :key="d.id" :value="d.id">{{ d.name }}</option>
          </select>
          <select v-model="fLevel" class="select select-sm" @change="load">
            <option value="">全部级别</option>
            <option value="info">INFO</option>
            <option value="success">SUCCESS</option>
            <option value="warn">WARN</option>
            <option value="error">ERROR</option>
          </select>
        </div>
        <button class="btn" @click="load">刷新</button>
        <button class="btn" @click="clear">清空</button>
      </div>
    </div>

    <div class="card log-card">
      <div class="log-stream mono">
        <div v-for="(l, i) in logs" :key="l.id || i" class="log-line" :class="l.level">
          <span class="lg-time">{{ l.time }}</span>
          <span class="lg-dev">{{ deviceName(l.device_id) }}</span>
          <span class="lg-script">{{ scriptName(l.script_id) }}</span>
          <span class="lg-level">{{ levelBadge(l.level) }}</span>
          <span class="lg-msg">{{ l.msg }}</span>
        </div>
        <div v-if="!logs.length" class="empty">
          <span class="icon">📭</span>
          <span>没有日志记录</span>
        </div>
      </div>
      <div class="log-foot">
        <span>共 {{ logs.length }} 条</span>
        <span v-if="autoRefresh" class="refresh-hint">每 5 秒自动刷新</span>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, onMounted, onUnmounted } from 'vue'
import { devicesData, scriptsData, useToast } from '../store'
import { api } from '../api'

const toast = useToast()
const devices = devicesData
const scripts = scriptsData
const logs = ref([])
const fDevice = ref('')
const fLevel = ref('')
const autoRefresh = ref(true)
let timer = null

const LEVELS = { info: 'INFO', success: 'OK', warn: 'WARN', error: 'ERR' }
function levelBadge(l) { return LEVELS[l] || l }
function deviceName(id) { return devices.value.find(d => d.id === id)?.name || id }
function scriptName(id) { return scripts.value.find(s => s.id === id)?.name || id }

async function load() {
  try {
    logs.value = await api.listLogs(fDevice.value || null, fLevel.value || null, 200)
  } catch (e) {}
}

async function clear() {
  try {
    await api.clearLogs()
    logs.value = []
    toast('日志已清空', 'success')
  } catch (e) {
    toast('清空失败：' + e.message, 'error')
  }
}

async function loadDevices() {
  try { devices.value = await api.listDevices() } catch (e) {}
}
async function loadScripts() {
  try { scripts.value = await api.listScripts() } catch (e) {}
}

onMounted(() => {
  loadDevices(); loadScripts(); load()
  timer = setInterval(() => { if (autoRefresh.value) load() }, 5000)
})
onUnmounted(() => { if (timer) clearInterval(timer) })
</script>

<style scoped>
.head-actions { display: flex; gap: 10px; align-items: center; }
.filters { display: flex; gap: 8px; }
.select-sm { width: 130px; padding: 6px 10px; font-size: 12px; }

.log-card { flex: 1; display: flex; flex-direction: column; min-height: 0; padding: 0; overflow: hidden; }
.log-stream { flex: 1; overflow: auto; padding: 12px 14px; display: flex; flex-direction: column; gap: 2px; font-size: 12px; }
.log-line { display: flex; gap: 12px; padding: 3px 0; line-height: 1.6; align-items: baseline; white-space: nowrap; }
.log-line:hover { background: rgba(30,36,52,.4); }
.lg-time { color: var(--text-2); flex-shrink: 0; }
.lg-dev { color: var(--text-1); min-width: 110px; flex-shrink: 0; overflow: hidden; text-overflow: ellipsis; }
.lg-script { color: var(--accent-2); min-width: 140px; flex-shrink: 0; overflow: hidden; text-overflow: ellipsis; }
.lg-level { width: 36px; flex-shrink: 0; font-weight: 700; }
.log-line.info .lg-level { color: var(--text-2); }
.log-line.success .lg-level { color: var(--ok); }
.log-line.warn .lg-level { color: var(--warn); }
.log-line.error .lg-level { color: var(--danger); }
.log-line.error .lg-msg { color: var(--danger); }
.log-line.success .lg-msg { color: var(--ok); }
.lg-msg { color: var(--text-1); }

.log-foot { border-top: 1px solid var(--border); padding: 10px 14px; display: flex; align-items: center; justify-content: space-between; font-size: 12px; color: var(--text-2); }
.refresh-hint { color: var(--text-2); font-size: 11px; }
</style>
