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
          <router-link to="/console" class="gamer-brand" aria-label="Gamer 首页"><span class="gamer-mark">G</span><span>amer</span></router-link>
        </div>
        <div id="gamer-main-navigation" class="tb-navigation"></div>
        <div class="tb-right">
          <div v-if="store.running && globalView" class="run-chip">
            <span class="dot run"></span>
            <span>{{ store.runScript }}</span>
            <span class="run-step">{{ store.runStep }}</span>
            <button class="run-stop" title="停止脚本" @click="stopRunning">■</button>
          </div>
          <span v-if="session.username" class="tb-user" :title="`当前登录：${session.username}`">{{ session.username }}</span>
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
import { useRoute } from 'vue-router'
import { store, devicesData, tasksData, beginCancel, useToast } from '../store'
import { session, doLogout } from '../auth'
import { api } from '../api'
const toast = useToast()
const route = useRoute()
const globalView = computed(() => { const key = String(route.query.panel || ''); return !key || key.startsWith('gamer.core:') || ['packages', 'market', 'plugins'].includes(key) })

const systemInfo = ref(null)
const systemVersion = computed(() => {
  const version = systemInfo.value?.app?.version
  return version === undefined || version === null || version === '' ? 'dev/unknown' : String(version)
})

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
    // 无法获取服务端版本时不显示混包告警。
  }
}

onMounted(() => {
  loadSystemInfo()
  // 初始化共享设备与任务数据。
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
  height: 48px; flex-shrink: 0; background: var(--chrome);
  border-bottom: 1px solid var(--border);
  display: flex; align-items: center; padding: 0 18px;
  gap: 12px;
}
.tb-left { display: flex; align-items: center; flex: 0 0 auto; }
.tb-navigation { flex: 1; min-width: 0; align-self: stretch; }
.tb-navigation :deep(.workspace-tabs) { height: 100%; padding: 0; border: 0; gap: 4px; }
.tb-navigation :deep(.workspace-tab-add) { margin-left: 0; }
.tb-right { display: flex; align-items: center; gap: 10px; flex: 0 0 auto; white-space: nowrap; }
.tb-right .btn { height: 28px; }
.tb-user { color: var(--text-2); font-size: 12px; max-width: 160px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.run-chip {
  display: flex; align-items: center; gap: 8px; padding: 5px 12px;
  border-radius: 20px; font-size: 12px; color: var(--accent);
  background: color-mix(in srgb, var(--accent) 8%, transparent); border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
}
.run-step { color: var(--text-1); }
.run-stop {
  width: 20px; height: 20px; border-radius: 50%; border: 1px solid rgba(255, 80, 80, .4);
  background: rgba(255, 80, 80, .15); color: #ff6b6b; font-size: 9px; line-height: 1;
  cursor: pointer; display: flex; align-items: center; justify-content: center; padding: 0;
}
.run-stop:hover { background: rgba(255, 80, 80, .3); }

.content { flex: 1; min-height: 0; overflow: hidden; }

@media (max-width: 1100px) {
  .topbar { padding: 0 10px; gap: 8px; }
  .tb-navigation :deep(.workspace-tabs) { gap: 0; }
  .tb-navigation :deep(.workspace-tab) { min-width: 0; padding-inline: 9px; font-size: 12px; }
  .tb-right { gap: 6px; }
  .tb-user { max-width: 80px; }
  .run-chip { gap: 4px; padding-inline: 6px; }
  .run-chip > span:not(.dot) { max-width: 60px; overflow: hidden; text-overflow: ellipsis; }
}
.gamer-brand{display:flex;align-items:center;text-decoration:none;color:var(--text-0);font-size:22px;font-weight:750;letter-spacing:-1px}.gamer-mark{display:grid;place-items:center;width:27px;height:27px;margin-right:2px;background:var(--text-0);color:var(--chrome);clip-path:polygon(0 0,100% 0,100% 72%,72% 100%,0 100%);font-size:22px}
</style>
