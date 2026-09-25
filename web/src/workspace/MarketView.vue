<template>
  <div class="market-view">
    <section class="market-section" aria-label="配置市场">
      <div class="market-section-head">
        <h3>发现配置 <span class="market-hint">{{ remotePackages.length }} 个</span></h3>
        <div class="market-head-actions"><button class="btn btn-sm" :disabled="remoteLoading || sourceBusy" @click="loadRemoteRegistry(true)">刷新</button></div>
      </div>
      <form class="source-add" @submit.prevent="addSource">
        <input v-model="repositoryUrl" class="input" aria-label="GitHub 配置仓库 URL" placeholder="https://github.com/owner/repo" :disabled="sourceBusy || remoteLoading" />
        <button class="btn btn-sm" :disabled="sourceBusy || remoteLoading || !repositoryUrl.trim()">添加仓库</button>
      </form>
      <p class="market-hint">支持公开 GitHub 仓库。最新正式 Release 需包含发布插件生成的配置目录。</p>
      <div v-for="source in sources" :key="source.id" class="source-row">
        <a :href="`https://github.com/${source.repository}`" target="_blank" rel="noopener noreferrer">{{ source.repository }}</a>
        <span class="market-hint">{{ source.enabled ? '已启用' : '已停用' }}</span>
        <button class="btn btn-sm" :disabled="sourceBusy || remoteLoading" @click="toggleSource(source)">{{ source.enabled ? '停用' : '启用' }}</button>
        <button class="btn btn-sm" :disabled="sourceBusy || remoteLoading" @click="removeSource(source)">移除</button>
        <span v-if="sourceWarnings[source.id]" class="market-error">{{ sourceWarnings[source.id] }}</span>
      </div>
      <p v-if="remoteLoading" class="market-hint">正在读取远端源…</p>
      <p v-else-if="remoteError" class="market-error">配置市场读取失败：{{ remoteError }}</p>
      <p v-else-if="!remotePackages.length" class="market-hint">市场暂无可用配置包。</p>
      <div v-else class="market-remote-list">
        <article v-for="entry in remotePackages" :key="`${entry.source_id}:${entry.id}`" class="market-remote-card">
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
              <span>来源：{{ entry.repository }}</span>
            </div>
          </div>
          <div class="market-remote-actions">
            <button
              class="btn btn-sm btn-primary"
              :disabled="!!installBusyId"
              @click="installRemotePackage(entry)"
            >{{ installBusyId === `${entry.source_id}:${entry.id}` ? '安装中…' : (installedPackageVersion(entry.id) ? '覆盖安装' : '安装') }}</button>
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

const sources = ref([])
const sourceWarnings = ref({})
const repositoryUrl = ref('')
const sourceBusy = ref(false)
const remotePackages = ref([])
const remoteLoading = ref(false)
const remoteError = ref('')
const installBusyId = ref('')

async function loadRemoteRegistry(force = true) {
  if (remoteLoading.value) return
  remoteLoading.value = true
  remoteError.value = ''
  try {
    sources.value = await api.packageSources()
    sourceWarnings.value = {}
    const catalogs = await Promise.all(sources.value.filter(s => s.enabled).map(async source => {
      try {
        const data = await api.packageSourceCatalog(source.id, force === true)
        if (data.warning) sourceWarnings.value[source.id] = `${data.cached ? '显示缓存：' : ''}${data.warning}`
        return (data.packages || []).map(p => ({ ...p, source_id: source.id, repository: source.repository }))
      } catch (e) { sourceWarnings.value[source.id] = e.message; return [] }
    }))
    remotePackages.value = catalogs.flat()
  } catch (e) {
    remotePackages.value = []
    remoteError.value = e?.message || '请重试'
  } finally {
    remoteLoading.value = false
  }
}

async function changeSources(change) {
  if (sourceBusy.value || remoteLoading.value) return
  sourceBusy.value = true
  try { await change(); await loadRemoteRegistry(false) }
  catch (e) { toast(e.message, 'error') }
  finally { sourceBusy.value = false }
}
const addSource = () => changeSources(async () => { await api.savePackageSource(repositoryUrl.value); repositoryUrl.value = '' })
const toggleSource = source => changeSources(() => api.savePackageSource(source.repository, !source.enabled))
const removeSource = source => changeSources(() => api.removePackageSource(source.id))

/** 已装同 id 包版本（卡片上的「已装 vX」标记 + 安装按钮文案切换） */
function installedPackageVersion(id) {
  return packages.value.find(p => p.id === id)?.version || ''
}

function hasList(v) { return Array.isArray(v) ? v.length > 0 : !!v }
function formatList(v) {
  if (!hasList(v)) return '未声明'
  return Array.isArray(v) ? v.join('、') : String(v)
}

async function installRemotePackage(entry) {
  const toast = beginReport()
  if (installBusyId.value || !entry?.source_id) return
  installBusyId.value = `${entry.source_id}:${entry.id}`
  try {
    const bytes = await api.downloadSourcePackage(entry.source_id, entry)
    const expectedSha256 = entry.sha256
    try {
      await api.importPackageArchive(bytes, { expectedSha256 })
    } catch (e) {
      // 409 = 同 id Package 已存在 → 明确提示覆盖（§9；与 usePackageContext 语义一致）
      if (e?.status !== 409) throw e
      const existing = e?.data?.existing
      const detail = existing?.version ? `（当前 v${existing.version}）` : ''
      if (!await confirmDialog(`来自 ${entry.repository} 的配置 ${entry.id} 与本地同 ID 配置冲突${detail}。覆盖安装将替换全部数据（含本地修改），建议先导出备份。`, { title: '覆盖配置包', confirmText: '覆盖安装', danger: true })) return
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

onMounted(() => { loadPackages(); loadRemoteRegistry(false) })
</script>

<style scoped>
.market-section { display:flex; flex-direction:column; gap:10px; }
.source-add,.source-row { display:flex; gap:8px; align-items:center; flex-wrap:wrap; }
.source-add .input { flex:1; min-width:220px; }
.source-row a { overflow-wrap:anywhere; }
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
