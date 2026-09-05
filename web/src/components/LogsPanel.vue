<template>
  <div class="logs-panel">
    <div class="lp-head">
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
      <div class="lp-actions">
        <span v-if="autoRefresh" class="refresh-hint">每 5 秒自动刷新</span>
        <button class="btn btn-sm" @click="load">刷新</button>
        <button class="btn btn-sm" @click="clear">清空</button>
      </div>
    </div>

    <div class="card log-card">
      <div ref="streamEl" class="log-stream mono" @scroll="onScroll">
        <template v-for="(g, gi) in groups" :key="gi">
          <!-- 分组头：连续同一「设备+运行目标」的日志段共用一条分割线，区分不同运行 -->
          <div class="run-divider">
            <span class="rd-script">{{ g.target }}</span>
            <span class="rd-dev">{{ g.deviceLabel }}</span>
            <span class="rd-time mono">{{ g.time }}</span>
            <span class="rd-line"></span>
          </div>
          <div v-for="(l, i) in g.entries" :key="l.id || gi + '-' + i" class="log-line" :class="l.level">
            <span class="lg-time">{{ l.time }}</span>
            <span class="lg-level">{{ levelBadge(l.level) }}</span>
            <span class="lg-msg">{{ l.msg }}</span>
          </div>
        </template>
        <div v-if="!logs.length" class="empty">
          <span class="icon">📭</span>
          <span>没有日志记录</span>
        </div>
      </div>
      <div class="log-foot">
        <span>共 {{ logs.length }} 条（按时间正序，最近在最下）</span>
      </div>
    </div>
  </div>
</template>

<script setup>
/**
 * Console 右侧日志页签内容（gamer.core:logs，Core 自有 UI）：
 * - 服务端 ORDER BY id DESC 返回，这里反转为时间正序展示，最新日志沉底；
 * - 运行分组：按「设备 + 运行目标」连续段归组，段首渲染分割线（目标 id + 设备 +
 *   起始时间）。运行目标是 runner 私有寻址（entrypoint；日志行沿用服务端
 *   script_id 字段透传，Core 不解释其业务语义），交替/并行运行产生的交叉段落
 *   各自带组头，仍可一眼区分来源；
 * - 用户上翻查看历史时自动刷新不强制滚底（贴近底部才跟随）。
 */
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { devicesData, useToast } from '../store'
import { api } from '../api'

const toast = useToast()
const devices = devicesData
const logs = ref([])
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
  const asc = [...logs.value].reverse()
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
  try {
    logs.value = await api.listLogs(fDevice.value || null, fLevel.value || null, 200)
    scrollBottomIfFollowing()
  } catch (e) {}
}

/** 贴近底部（或首次加载）时刷新后滚到最新；用户上翻历史则不打扰。 */
function scrollBottomIfFollowing() {
  const el = streamEl.value
  if (!el) return
  const nearBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 60
  if (nearBottom) requestAnimationFrame(() => { el.scrollTop = el.scrollHeight })
}
function onScroll() { scrollBottomIfFollowing() }

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

onMounted(() => {
  loadDevices(); load()
  timer = setInterval(() => { if (autoRefresh.value) load() }, 5000)
})
onUnmounted(() => { if (timer) clearInterval(timer) })
</script>

<style scoped>
.logs-panel { display: flex; flex-direction: column; gap: 10px; flex: 1; min-height: 0; }
.lp-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; flex-wrap: wrap; }
.filters { display: flex; gap: 8px; }
.select-sm { width: 140px; padding: 6px 10px; font-size: 12px; }
.lp-actions { display: flex; gap: 8px; align-items: center; }
.refresh-hint { color: var(--text-2); font-size: 11px; }

.log-card { flex: 1; display: flex; flex-direction: column; min-height: 0; padding: 0; overflow: hidden; }
.log-stream { flex: 1; overflow: auto; padding: 10px 14px; display: flex; flex-direction: column; gap: 2px; font-size: 12px; }
.log-line { display: flex; gap: 12px; padding: 2px 0 2px 14px; line-height: 1.6; align-items: baseline; white-space: nowrap; }
.log-line:hover { background: rgba(30,36,52,.4); }
.lg-time { color: var(--text-2); flex-shrink: 0; }
.lg-level { width: 36px; flex-shrink: 0; font-weight: 700; }
.log-line.info .lg-level { color: var(--text-2); }
.log-line.success .lg-level { color: var(--ok); }
.log-line.warn .lg-level { color: var(--warn); }
.log-line.error .lg-level { color: var(--danger); }
.log-line.error .lg-msg { color: var(--danger); }
.log-line.success .lg-msg { color: var(--ok); }
.lg-msg { color: var(--text-1); }

/* 运行分组分割线：目标 id + 设备 + 起始时间 + 延伸线 */
.run-divider {
  display: flex; align-items: center; gap: 8px;
  margin: 10px 0 4px; font-size: 12px;
}
.run-divider:first-child { margin-top: 2px; }
.rd-script { color: var(--accent-2); font-weight: 600; }
.rd-dev { color: var(--text-1); font-size: 11px; }
.rd-time { color: var(--text-2); font-size: 11px; }
.rd-line { flex: 1; height: 1px; background: var(--border); }

.log-foot { border-top: 1px solid var(--border); padding: 8px 14px; display: flex; align-items: center; justify-content: space-between; font-size: 12px; color: var(--text-2); }
</style>
