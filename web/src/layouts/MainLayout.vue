<template>
  <div class="layout">
    <div class="main">
      <!-- WEB-006 混包告警：前端构建版本（vite 注入 __APP_VERSION__）与服务端 app.version 不一致时提示 -->
      <div v-if="versionMismatch" class="mismatch-bar" role="alert">
        <span class="mb-icon">⚠</span>
        <span>
          前端与服务端版本不一致（前端 {{ webVersion }} / 服务端 {{ systemVersion }}），可能存在混包部署，请刷新页面或重新部署后再使用。
        </span>
      </div>
      <header class="topbar">
        <div class="tb-left">
          <router-link to="/console" class="tb-device" :class="{ on: store.deviceId }">
            <span class="dot" :class="store.deviceId ? 'ok' : 'off'"></span>
            {{ store.deviceId ? currentDeviceName : '未选择设备' }}
          </router-link>
          <div class="tb-sys" :title="`${systemStateText} · ${systemVersion}`">
            <span class="dot" :class="systemStateClass"></span>
            <span class="tb-sys-text">{{ systemStateText }}</span>
            <span class="tb-sys-ver mono">{{ systemVersion }}</span>
          </div>
        </div>
        <div class="tb-right">
          <div v-if="store.running" class="run-chip">
            <span class="dot run"></span>
            <span>{{ store.runScript }}</span>
            <span class="run-step">{{ store.runStep }}</span>
            <button class="run-stop" title="停止脚本" @click="stopRunning">■</button>
          </div>
          <span v-if="session.username" class="tb-user" :title="`当前登录：${session.username}`">👤 {{ session.username }}</span>
          <button class="btn btn-sm btn-ghost" @click="onLogout">退出登录</button>
        </div>
      </header>

      <main class="content">
        <router-view />
      </main>
    </div>
  </div>
</template>

<script setup>
import { computed, ref, onMounted } from 'vue'
import { store, devicesData, tasksData, beginCancel, useToast } from '../store'
import { session, doLogout } from '../auth'
import { api } from '../api'
const toast = useToast()

const systemInfo = ref(null)
const systemVersion = computed(() => {
  const version = systemInfo.value?.app?.version
  return version === undefined || version === null || version === '' ? 'dev/unknown' : String(version)
})
const systemStateText = computed(() => {
  if (!systemInfo.value) return '服务状态未知'
  return systemInfo.value.readiness?.status === 'ready' ? '服务运行中' : '服务未就绪'
})
const systemStateClass = computed(() => (
  systemInfo.value?.readiness?.status === 'ready' ? 'ok' : 'off'
))

// 混包告警（WEB-006）：webVersion 来自构建期注入（web/package.json，CI 保证与 Cargo 同源）；
// 服务端版本以 /api/system/info 的 app.version 为准。-dev 后缀视为同版本线不算不一致。
const webVersion = typeof __APP_VERSION__ !== 'undefined' ? String(__APP_VERSION__) : ''
const normVer = (v) => String(v ?? '').replace(/-dev$/, '')
const versionMismatch = computed(() => {
  const server = normVer(systemInfo.value && systemInfo.value.app && systemInfo.value.app.version)
  return !!webVersion && !!server && normVer(webVersion) !== server
})

async function loadSystemInfo() {
  try {
    const response = await fetch('/api/system/info', { headers: { Accept: 'application/json' } })
    if (!response.ok) return
    const body = await response.json()
    if (body && typeof body === 'object') systemInfo.value = body
  } catch {
    // 系统状态仅用于展示；请求失败时保留 dev/unknown 降级文案。
  }
}

const onlineCount = computed(() => devicesData.value.filter(d => d.status === 'online').length)
const taskCount = computed(() => tasksData.value.length)

const currentDeviceName = computed(() => {
  const d = devicesData.value.find(x => x.id === store.deviceId)
  return d ? d.name : ''
})

onMounted(() => {
  loadSystemInfo()
  // 顶栏状态：在线设备数 / 定时任务数（任务数用于 title 提示）
  api.listDevices().then(d => { devicesData.value = d }).catch(() => {})
  api.listTasks().then(t => { tasksData.value = t }).catch(() => {})
})

// 退出登录：POST /api/logout（幂等，成败均清本地态）→ doLogout 内部落回 #/login
async function onLogout() {
  await doLogout()
}

/** 顶栏芯片上的停止按钮：任何页面都能按当前 run_id 取消运行。
 * POST cancel 后记录迁 stopping，终态仍以 GET /api/runs/:run_id 查询为准。 */
function stopRunning() {
  const rid = store.runId
  if (!rid) return
  beginCancel(rid)
  api.cancelRun(rid).catch(e => toast('停止失败：' + e.message, 'error'))
}
</script>

<style scoped>
.layout { display: flex; height: 100%; }
.main { flex: 1; display: flex; flex-direction: column; min-width: 0; background: var(--bg-0); }

.mismatch-bar {
  display: flex; align-items: center; gap: 8px; padding: 8px 16px;
  font-size: 12px; color: var(--warn, #fbbf24);
  background: rgba(251, 191, 36, .08);
  border-bottom: 1px solid rgba(251, 191, 36, .35);
}
.mb-icon { flex-shrink: 0; }

.topbar {
  height: 52px; flex-shrink: 0; background: var(--bg-1);
  border-bottom: 1px solid var(--border);
  display: flex; align-items: center; justify-content: space-between; padding: 0 16px;
  gap: 12px;
}
.tb-left { display: flex; align-items: center; min-width: 0; }
.tb-device { display: flex; align-items: center; gap: 8px; color: var(--text-1); text-decoration: none; font-size: 13px; white-space: nowrap; }
.tb-device.on { color: var(--text-0); font-weight: 600; }
.tb-sys { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--text-1); min-width: 0; }
.tb-sys-text { white-space: nowrap; }
.tb-sys-ver { color: var(--text-2); font-size: 11px; }
.tb-right { display: flex; align-items: center; gap: 10px; }
.tb-user { color: var(--text-2); font-size: 12px; max-width: 160px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.run-chip {
  display: flex; align-items: center; gap: 8px; padding: 5px 12px;
  border-radius: 20px; font-size: 12px; color: var(--accent);
  background: rgba(34,211,165,.08); border: 1px solid rgba(34,211,165,.3);
}
.run-step { color: var(--text-1); }
.run-stop {
  width: 20px; height: 20px; border-radius: 50%; border: 1px solid rgba(255, 80, 80, .4);
  background: rgba(255, 80, 80, .15); color: #ff6b6b; font-size: 9px; line-height: 1;
  cursor: pointer; display: flex; align-items: center; justify-content: center; padding: 0;
}
.run-stop:hover { background: rgba(255, 80, 80, .3); }

.content { flex: 1; overflow: hidden; }

@media (max-width: 900px) {
  .tb-sys { display: none; }
}
</style>
