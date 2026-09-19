<template>
  <div class="logs-panel">
    <div class="lp-head">
      <div class="filters">
        <input v-model="search" class="input log-search" type="search" aria-label="搜索日志" placeholder="搜索当前记录…" />
        <select v-model="fDevice" aria-label="日志设备" class="select select-sm" @change="load">
          <option value="">全部设备</option>
          <option v-for="d in devices" :key="d.id" :value="d.id">{{ d.name }}</option>
        </select>
        <select v-model="fLevel" aria-label="日志级别" class="select select-sm" @change="load">
          <option value="">全部级别</option>
          <option value="info">INFO</option>
          <option value="success">SUCCESS</option>
          <option value="warn">WARN</option>
          <option value="error">ERROR</option>
        </select>
      </div>
      <div class="lp-actions">
        <button class="btn btn-ghost refresh-toggle" :aria-pressed="autoRefresh" @click="autoRefresh = !autoRefresh"><span class="live-dot" :class="{ paused: !autoRefresh }"></span>{{ autoRefresh ? '自动刷新' : '已暂停刷新' }}</button>
        <button class="btn btn-icon btn-ghost" title="刷新日志" aria-label="刷新日志" :disabled="loading" @click="load"><UiIcon name="refresh" /></button>
        <button class="btn btn-ghost" @click="clear"><UiIcon name="trash" />清空</button>
      </div>
    </div>

    <div v-if="loadError" class="log-load-error" role="alert">{{ loadError }}</div>
    <div class="log-card">
      <div class="log-columns"><span>时间</span><span>级别</span><span>事件内容</span></div>
      <div ref="streamEl" class="log-stream" @scroll="onScroll">
        <template v-for="(g, gi) in groups" :key="gi">
          <!-- 分组头：连续同一「设备+运行目标」的日志段共用一条分割线，区分不同运行 -->
          <div class="run-divider">
            <span class="rd-script">{{ g.target }}</span>
            <span class="rd-dev">{{ g.deviceLabel }}</span>
          </div>
          <div v-for="(l, i) in g.entries" :key="l.id || gi + '-' + i" class="log-line" :class="l.level">
            <span class="lg-time">{{ l.time }}</span>
            <span class="lg-level">{{ levelBadge(l.level) }}</span>
            <span class="lg-msg">{{ l.msg }}</span>
          </div>
        </template>
        <div v-if="!visibleLogs.length" class="log-empty"><UiIcon name="info" /><span>{{ loading ? '正在读取日志…' : search ? '没有匹配的记录' : '暂无日志记录' }}</span></div>
      </div>
      <div class="log-foot">
        <span>{{ visibleLogs.length }} / {{ logs.length }} 条 · 最近 200 条记录</span><span>时间正序 <span class="foot-dot">·</span> {{ autoRefresh ? '每 5 秒刷新' : '自动刷新已暂停' }}</span>
      </div>
    </div>
  </div>
</template>

<script setup>
/**
 * Console 右侧日志页签内容（gamer.core:logs，Core 自有 UI）：
 * - 服务端 ORDER BY id DESC 返回，这里反转为时间正序展示，最新日志沉底；
 * - 运行分组：按「设备 + 运行目标」连续段归组，段首显示目标 id 与设备。
 *   运行目标是 runner 私有寻址（entrypoint；日志行沿用服务端
 *   script_id 字段透传，Core 不解释其业务语义），交替/并行运行产生的交叉段落
 *   各自带组头，仍可一眼区分来源；
 * - 用户上翻查看历史时自动刷新不强制滚底（贴近底部才跟随）。
 */
import { ref, computed, nextTick, onMounted, onUnmounted, inject } from 'vue'
import { OPERATION_FEEDBACK_KEY, operationReporter } from '../workspace/operation-feedback'
import { devicesData, useToast } from '../store'
import { api } from '../api'
import UiIcon from './ui/UiIcon.vue'

const toast = useToast()
const beginReport = operationReporter(inject(OPERATION_FEEDBACK_KEY, null), '', toast)
const devices = devicesData
const logs = ref([])
const search = ref('')
const loading = ref(false), loadError = ref('')
let requestId = 0
const visibleLogs = computed(() => { const needle = search.value.trim().toLowerCase(); return logs.value.filter(log => [log.msg, log.time, log.level, runTarget(log), deviceName(log.device_id)].some(value => String(value || '').toLowerCase().includes(needle))) })
const fDevice = ref('')
const fLevel = ref('')
const autoRefresh = ref(true)
const streamEl = ref(null)
let timer = null

const LEVELS = { info: 'INFO', success: 'OK', warn: 'WARN', error: 'ERR' }
function levelBadge(l) { return LEVELS[l] || l }
function deviceName(id) { return devices.value.find(d => d.id === id)?.name || id }
/** 运行目标展示：entrypoint 为主，script_id 为服务端保留的兼容展示字段 */
function runTarget(l) { return l.entrypoint || l.script_id || '—' }

/** 时间正序 + 连续「设备+运行目标」分段。 */
const groups = computed(() => {
  const asc = [...visibleLogs.value].reverse()
  const out = []
  for (const l of asc) {
    const target = runTarget(l)
    const last = out[out.length - 1]
    if (last && last.deviceId === l.device_id && last.target === target) {
      last.entries.push(l)
    } else {
      out.push({
        deviceId: l.device_id,
        target,
        deviceLabel: deviceName(l.device_id),
        time: l.time,
        entries: [l],
      })
    }
  }
  return out
})

async function load() {
  const request = ++requestId
  loading.value = true
  try {
    const result = await api.listLogs(fDevice.value || null, fLevel.value || null, 200)
    if (request !== requestId) return
    logs.value = Array.isArray(result) ? result : []
    loadError.value = ''
    await nextTick()
    if (following && streamEl.value) streamEl.value.scrollTop = streamEl.value.scrollHeight
  } catch (error) { if (request === requestId) loadError.value = `日志读取失败：${error.message || '请重试'}` }
  finally { if (request === requestId) loading.value = false }
}

let following = true
function onScroll() {
  const el = streamEl.value
  if (el) following = el.scrollTop + el.clientHeight >= el.scrollHeight - 60
}

async function clear() {
  const toast = beginReport()
  try {
    await api.clearLogs()
    ++requestId; loading.value = false; loadError.value = ''
    logs.value = []
    toast('日志已清空', 'success')
  } catch (e) {
    toast('清空失败：' + e.message, 'error')
  }
}

async function loadDevices() {
  try { devices.value = await api.listDevices() } catch (e) {}
}

onMounted(() => {
  loadDevices(); load()
  timer = setInterval(() => { if (autoRefresh.value) load() }, 5000)
})
onUnmounted(() => { ++requestId; if (timer) clearInterval(timer) })
</script>

<style scoped>
.logs-panel { flex:1; min-height:0; display:flex; flex-direction:column; gap:12px; container-type:inline-size; }
.lp-head,.filters,.lp-actions { display:flex; align-items:center; gap:8px; flex-wrap:wrap; }
.lp-head { justify-content:space-between; flex-shrink:0; }
.filters { flex:1; }.log-search { width:280px; max-width:100%; }.select-sm { width:132px; }
.lp-actions { margin-left:auto; }.refresh-toggle { font-size:12px; }.live-dot { width:5px; height:5px; border-radius:50%; background:var(--ok); }.live-dot.paused { background:var(--text-2); }
.log-card { flex:1; min-height:0; display:flex; flex-direction:column; border:1px solid var(--border); background:var(--bg-0); border-radius:var(--radius); overflow:hidden; }
.log-columns,.log-line { display:grid; grid-template-columns:190px 64px minmax(0,1fr); gap:14px; padding:7px 16px; }
.log-columns { flex-shrink:0; background:var(--bg-1); border-bottom:1px solid var(--border); color:var(--text-2); font-size:12px; }
.log-stream { flex:1; min-height:0; overflow:auto; }
.log-line { font-size:13px; line-height:1.65; align-items:baseline; border-bottom:1px solid color-mix(in srgb,var(--border) 22%,transparent); }
.log-line:hover { background:var(--bg-1); }.lg-time { font-family:var(--mono); font-size:12px; color:var(--text-2); overflow-wrap:anywhere; font-variant-numeric:tabular-nums; }
.lg-level { font:11px/1.8 var(--mono); font-weight:600; letter-spacing:.03em; color:var(--text-1); }
.log-line.success .lg-level { color:var(--ok); }.log-line.warn .lg-level { color:var(--warn); }.log-line.error .lg-level { color:var(--danger); }
.log-line.error { background:color-mix(in srgb,var(--danger) 4%,var(--bg-0)); }.lg-msg { min-width:0; color:var(--text-0); white-space:pre-wrap; overflow-wrap:anywhere; }
.run-divider { display:flex; align-items:center; gap:12px; padding:7px 16px; background:var(--bg-1); border-bottom:1px solid color-mix(in srgb,var(--border) 60%,transparent); font-size:12px; }
.run-divider:not(:first-child) { margin-top:12px; border-top:1px solid var(--border); }.rd-script { color:var(--text-1); overflow-wrap:anywhere; }.rd-dev { color:var(--text-2); margin-left:auto; flex-shrink:0; }
.log-foot { display:flex; justify-content:space-between; gap:10px; flex-wrap:wrap; flex-shrink:0; padding:8px 16px; background:var(--bg-1); border-top:1px solid var(--border); color:var(--text-2); font-size:12px; }.foot-dot { padding:0 5px; }
.log-empty { padding:50px 16px; display:flex; gap:8px; align-items:center; justify-content:center; color:var(--text-2); font-size:13px; }.log-load-error { font-size:12px; color:var(--danger); }
@container (max-width:680px) { .log-columns,.log-line { grid-template-columns:125px 44px minmax(0,1fr); gap:8px; padding:7px 10px; }.log-search { flex:1; min-width:180px; }.filters { flex-wrap:wrap; }.rd-dev { max-width:35%; overflow-wrap:anywhere; }.log-foot { padding:7px 10px; } }
</style>
