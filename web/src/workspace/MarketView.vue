<template>
  <div class="market-view">
    <section class="market-section" aria-label="配置市场">
      <div class="market-section-head">
        <h3>发现配置 <span class="market-hint">{{ remotePackages.length }} 个</span></h3>
        <div class="market-head-actions"><button class="btn btn-sm" :disabled="remoteLoading" @click="loadRemoteRegistry">刷新</button></div>
      </div>
      <p v-if="remoteLoading" class="market-hint">正在读取远端源…</p>
      <p v-else-if="remoteError" class="market-error">配置市场读取失败：{{ remoteError }}</p>
      <p v-else-if="!remotePackages.length" class="market-hint">市场暂无可用配置包。</p>
      <div v-else class="market-remote-list">
        <article v-for="entry in remotePackages" :key="entry.id" class="market-remote-card">
          <div class="market-remote-main">
            <div class="market-remote-title">
              <strong>{{ entry.name || entry.id }}</strong>
              <span class="mono">v{{ entry.version || '?' }}</span>
              <span v-if="installedPackageVersion(entry.id)" class="market-state ok">已装 v{{ installedPackageVersion(entry.id) }}</span>
            </div>
            <div class="mono market-remote-id">{{ entry.id }}</div>
            <div class="market-remote-facts">
              <span>适用应用：{{ formatList(entry.android_targets) }}</span>
              <span>必需插件：{{ formatList(entry.required_plugins) }}</span>
              <span v-if="hasList(entry.optional_plugins)">可选插件：{{ formatList(entry.optional_plugins) }}</span>
              <span>作者：{{ entry.author || entry.publisher || '未声明' }}</span>
            </div>
          </div>
          <div class="market-remote-actions">
            <button
              class="btn btn-sm btn-primary"
              :disabled="installBusyId === entry.id || !entry.download_url"
              :title="entry.download_url ? '' : '该条目缺少 download_url，无法安装'"
              @click="installRemotePackage(entry)"
            >{{ installBusyId === entry.id ? '安装中…' : (installedPackageVersion(entry.id) ? '覆盖安装' : '安装') }}</button>
          </div>
        </article>
      </div>

    </section>
  </div>
</template>

<script setup>
const confirmDialog = useConfirmDialog()
// 两类市场直接展示可安装内容；本地管理由各自主导航页面承载。
import { useConfirmDialog } from '../components/ui/useConfirmDialog'
import { computed, onMounted, ref, inject } from 'vue'
import { OPERATION_FEEDBACK_KEY, operationReporter } from './operation-feedback'
import { api } from '../api'
import { useToast } from '../store'
import { loadPackages, packageStore, refreshPackages } from '../package-store'

const toast = useToast()
const beginReport = operationReporter(inject(OPERATION_FEEDBACK_KEY, null), '', toast)
const packages = computed(() => packageStore.packages)

// ---------- 远端 Package 源（registry.json 相对路径裸 fetch；禁改 registry.json 本身） ----------
const REMOTE_REGISTRY_URL = 'registry.json'
const remotePackages = ref([])
const remoteLoading = ref(false)
const remoteError = ref('')
const installBusyId = ref('')

async function loadRemoteRegistry() {
  if (remoteLoading.value) return
  remoteLoading.value = true
  remoteError.value = ''
  try {
    const r = await fetch(REMOTE_REGISTRY_URL, { cache: 'no-store' })
    // 404 = 无远端源；生产环境 ServeDir 把缺失静态文件 SPA-fallback 成 index.html
    //（200 + HTML），非 JSON 响应同样按「无远端源」空态处理而非报错
    if (r.status === 404) {
      remotePackages.value = []
      return
    }
    if (!r.ok) throw new Error(`HTTP ${r.status}`)
    const ct = (r.headers.get('content-type') || '').toLowerCase()
    if (ct && !ct.includes('json')) {
      remotePackages.value = []
      return
    }
    const data = await r.json()
    remotePackages.value = Array.isArray(data?.packages) ? data.packages : []
  } catch (e) {
    remotePackages.value = []
    remoteError.value = e?.message || '请重试'
  } finally {
    remoteLoading.value = false
  }
}

/** 已装同 id 包版本（卡片上的「已装 vX」标记 + 安装按钮文案切换） */
function installedPackageVersion(id) {
  return packages.value.find(p => p.id === id)?.version || ''
}

function hasList(v) { return Array.isArray(v) ? v.length > 0 : !!v }
function formatList(v) {
  if (!hasList(v)) return '未声明'
  return Array.isArray(v) ? v.join('、') : String(v)
}

/** 相对 download_url 解析到 registry.json 所在目录（远端源天然支持子目录托管） */
function resolveRemoteUrl(u) {
  try {
    return new URL(String(u), new URL(REMOTE_REGISTRY_URL, window.location.href)).href
  } catch {
    return String(u || '')
  }
}

async function installRemotePackage(entry) {
  const toast = beginReport()
  if (installBusyId.value || !entry?.download_url) return
  installBusyId.value = entry.id
  try {
    const r = await fetch(resolveRemoteUrl(entry.download_url))
    if (!r.ok) throw new Error(`归档下载失败：HTTP ${r.status}`)
    const bytes = new Uint8Array(await r.arrayBuffer())
    const expectedSha256 = entry.sha256 ? String(entry.sha256) : undefined
    try {
      await api.importPackageArchive(bytes, { expectedSha256 })
    } catch (e) {
      // 409 = 同 id Package 已存在 → 明确提示覆盖（§9；与 usePackageContext 语义一致）
      if (e?.status !== 409) throw e
      const existing = e?.data?.existing
      const detail = existing?.version ? `（当前 v${existing.version}）` : ''
      if (!await confirmDialog(`配置 ${entry.id} 已存在${detail}，覆盖安装将替换该配置全部数据（含本地修改）。`, { title: '覆盖配置包', confirmText: '覆盖安装', danger: true })) return
      await api.importPackageArchive(bytes, { expectedSha256, overwrite: true })
    }
    toast(`配置已安装：${entry.id}@${entry.version || '?'}`, 'success')
    await refreshPackages()
  } catch (e) {
    toast(`安装失败：${e?.message || '请重试'}`, 'error')
  } finally {
    installBusyId.value = ''
  }
}

onMounted(() => { loadPackages(); loadRemoteRegistry() })
</script>

<style scoped>
.market-section { display:flex; flex-direction:column; gap:10px; }
.market-section-head { display:flex; align-items:center; justify-content:space-between; gap:10px; }
.market-section-head h3 { margin:0; font-size:13px; font-weight:600; }.market-section-head h3 span { margin-left:10px; font-weight:400; }.market-head-actions { display:flex; gap:6px; }
.market-hint { font-size:12px; color:var(--text-2); }.market-error { font-size:12px; color:var(--danger); }
.market-remote-list { border:1px solid var(--border); border-radius:3px; overflow:hidden; }
.market-remote-card { display:flex; justify-content:space-between; align-items:center; gap:24px; padding:14px 16px; background:var(--bg-1); border-bottom:1px solid color-mix(in srgb,var(--border) 65%,transparent); }.market-remote-card:last-child { border-bottom:0; }.market-remote-card:hover { background:color-mix(in srgb,var(--bg-2) 45%,var(--bg-1)); }
.market-remote-main { min-width:0; flex:1; }.market-remote-title { display:flex; align-items:baseline; gap:10px; flex-wrap:wrap; }.market-remote-title strong { font-size:14px; font-weight:600; }.market-remote-title .mono,.market-remote-id { font-size:12px; color:var(--text-2); overflow-wrap:anywhere; }.market-remote-id { margin-top:5px; }.market-state { font-size:12px; color:var(--ok); }
.market-remote-facts { display:flex; flex-wrap:wrap; gap:4px 18px; font-size:12px; line-height:1.6; color:var(--text-1); overflow-wrap:anywhere; margin-top:8px; }
.market-remote-actions { flex-shrink:0; }.market-remote-actions .btn { min-width:80px; justify-content:center; }
@container (max-width:600px) { .market-remote-card { flex-wrap:wrap; gap:12px; }.market-remote-main { flex-basis:100%; }.market-remote-actions { margin-left:auto; } }
</style>
