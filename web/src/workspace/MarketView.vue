<template>
  <div class="market-view">
    <!-- 插件市场（plan §20）：搜索/安装/升级/卸载/能力查看全部由插件中心承载，
         本页保留入口与已装插件清单（只读概览，管理动作不在此重复）。
         主导航「市场」为下拉二级菜单，section 决定渲染哪个分区。 -->
    <section v-if="section === 'plugin'" class="market-section">
      <div class="market-section-head">
        <div>
          <h3>插件市场</h3>
          <p>搜索、安装、升级、卸载插件与查看插件能力，统一在插件中心完成。</p>
        </div>
        <div class="market-head-actions">
          <button class="btn btn-sm" :disabled="extensionsLoading" @click="loadExtensions">刷新清单</button>
          <button type="button" class="btn btn-primary" @click="centerOpen = true">打开插件市场</button>
        </div>
      </div>
      <p v-if="extensionsError" class="market-error">已装插件读取失败：{{ extensionsError }}</p>
      <div v-else-if="extensions.length" class="market-ext-list">
        <div v-for="ext in extensions" :key="ext.id" class="market-ext-item">
          <span class="market-ext-name">{{ ext.name || ext.id }}</span>
          <span class="mono">{{ ext.id }}</span>
          <span class="mono">v{{ ext.active_version || ext.version || '?' }}</span>
          <span class="market-state" :class="extStateClass(ext.state)">{{ extStateLabel(ext.state) }}</span>
        </div>
      </div>
      <p v-else class="market-hint">尚未安装任何插件。</p>
    </section>

    <!-- Package 市场（plan §21）：远端源（registry.json packages 段）+ 本地已装清单；
         本地文件导入仍走右侧 Package 栏「导入」 -->
    <section v-else-if="section === 'package'" class="market-section">
      <div class="market-section-head">
        <div>
          <h3>配置市场</h3>
          <p>远端源来自静态 registry.json 的 packages 段；安装 = 下载归档并导入本地配置。</p>
        </div>
        <button class="btn btn-sm" :disabled="remoteLoading" @click="loadRemoteRegistry">刷新远端源</button>
      </div>

      <div class="market-subhead">远端源</div>
      <p v-if="remoteLoading" class="market-hint">正在读取远端源…</p>
      <p v-else-if="remoteError" class="market-error">远端源读取失败：{{ remoteError }}</p>
      <p v-else-if="!remotePackages.length" class="market-hint">远端源暂无配置。</p>
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
              <span>Android Targets：{{ formatList(entry.android_targets) }}</span>
              <span>Required Plugins：{{ formatList(entry.required_plugins) }}</span>
              <span v-if="hasList(entry.optional_plugins)">Optional Plugins：{{ formatList(entry.optional_plugins) }}</span>
              <span>Author：{{ entry.author || entry.publisher || '未声明' }}</span>
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

      <div class="market-subhead">本地已装</div>
      <div v-if="packages.length" class="market-pkg-list">
        <div v-for="p in packages" :key="p.id" class="market-pkg-item">
          <span class="mono">{{ p.id }}</span>
          <span class="market-pkg-name">{{ p.name || '' }}</span>
          <span class="market-pkg-ver mono">v{{ p.version }}</span>
        </div>
      </div>
      <p v-else class="market-hint">尚未安装任何配置。</p>
    </section>
    <PluginCenter :open="centerOpen" @close="centerOpen = false" @changed="emit('extensions-changed')" />
  </div>
</template>

<script setup>
// 市场导航（plan §19/§30）：插件市场复用 PluginCenter（安装/升级/卸载/能力查看）；
// Package 市场展示本地已装包清单 + 远端源（web/public/registry.json 的 packages
// 段，静态文件相对路径裸 fetch，404/缺段 = 无远端源）。远端包安装 = fetch
// download_url 字节 → api.importPackageArchive（X-Expected-Sha256 校验），409 =
// 已存在 → confirm 覆盖后 overwrite 重试（与 usePackageContext 覆盖语义一致）。
import { computed, onMounted, ref } from 'vue'
import PluginCenter from './plugin-center/PluginCenter.vue'
import { api } from '../api'
import { useToast } from '../store'
import { loadPackages, packageStore, refreshPackages } from '../package-store'

const emit = defineEmits(['extensions-changed'])
// section：'plugin' = 插件市场 / 'package' = 配置市场（主导航「市场」下拉二级菜单选定）
defineProps({
  section: { type: String, default: 'plugin' },
})
const toast = useToast()
const centerOpen = ref(false)
const packages = computed(() => packageStore.packages)

// ---------- 已装插件清单（GET /api/extensions；启停/卸载等动作归 PluginCenter） ----------
const extensions = ref([])
const extensionsLoading = ref(false)
const extensionsError = ref('')

async function loadExtensions() {
  if (extensionsLoading.value) return
  extensionsLoading.value = true
  extensionsError.value = ''
  try {
    const rep = await api.listExtensions()
    extensions.value = Array.isArray(rep?.extensions) ? rep.extensions : []
  } catch (e) {
    extensionsError.value = e?.message || '请重试'
  } finally {
    extensionsLoading.value = false
  }
}

const EXT_STATES = {
  installed: { label: '已安装', cls: '' },
  enabled: { label: '已启用', cls: 'ok' },
  running: { label: '运行中', cls: 'run' },
  disabled: { label: '已停用', cls: 'warn' },
  failed: { label: '失败', cls: 'err' },
}
function extStateLabel(state) { return EXT_STATES[state]?.label || state || '未知' }
function extStateClass(state) { return EXT_STATES[state]?.cls || '' }

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
      if (!window.confirm(`配置 ${entry.id} 已存在${detail}，覆盖安装将替换该配置全部数据（含本地修改），继续？`)) return
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

onMounted(() => {
  loadPackages()
  loadExtensions()
  loadRemoteRegistry()
})
</script>

<style scoped>
.market-view { flex:1; min-height:0; overflow-y:auto; display:flex; flex-direction:column; gap:12px; padding:4px; }
.market-section { border:1px solid var(--border); border-radius:var(--radius); background:var(--bg-2); padding:14px; display:flex; flex-direction:column; gap:8px; }
.market-section-head { display:flex; align-items:flex-start; justify-content:space-between; gap:12px; }
.market-section-head h3 { margin:0; font-size:14px; }
.market-section-head p { margin:2px 0 0; font-size:12px; color:var(--text-2); }
.market-head-actions { display:flex; gap:8px; flex-shrink:0; }
.market-hint { margin:0; font-size:12px; color:var(--text-2); }
.market-error { margin:0; font-size:12px; color:var(--danger); }
.market-subhead { margin-top:4px; font-size:11px; color:var(--text-2); letter-spacing:.04em; border-bottom:1px dashed var(--border); padding-bottom:4px; }

/* 已装插件清单 */
.market-ext-list { display:flex; flex-direction:column; gap:4px; }
.market-ext-item { display:flex; gap:10px; align-items:baseline; font-size:12px; border-top:1px solid var(--border); padding-top:4px; }
.market-ext-item .mono { font-size:11px; color:var(--text-2); }
.market-ext-name { font-size:12px; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }

/* 状态徽章（插件 state / 已装包标记共用） */
.market-state { font-size:10px; line-height:16px; padding:0 6px; border:1px solid var(--border); border-radius:8px; color:var(--text-2); flex-shrink:0; }
.market-state.ok { color:var(--ok); border-color:rgba(34,197,94,.4); }
.market-state.run { color:var(--accent-2); border-color:rgba(56,189,248,.4); }
.market-state.warn { color:var(--warn); border-color:rgba(251,191,36,.4); }
.market-state.err { color:var(--danger); border-color:rgba(248,113,113,.4); }

/* 远端 Package 卡片（plan §21：Name/ID/Version/Android Targets/Required Plugins/Author） */
.market-remote-list { display:flex; flex-direction:column; gap:8px; }
.market-remote-card { display:flex; justify-content:space-between; gap:12px; padding:10px 12px; background:var(--bg-1); border:1px solid var(--border); border-radius:var(--radius-sm); }
.market-remote-main { min-width:0; flex:1; display:flex; flex-direction:column; gap:4px; }
.market-remote-title { display:flex; align-items:baseline; gap:8px; flex-wrap:wrap; }
.market-remote-title strong { font-size:13px; }
.market-remote-title .mono { font-size:11px; color:var(--text-2); }
.market-remote-id { font-size:11px; color:var(--accent-2); word-break:break-all; }
.market-remote-facts { display:flex; flex-wrap:wrap; gap:2px 14px; font-size:11px; color:var(--text-2); }
.market-remote-actions { display:flex; align-items:center; flex-shrink:0; }

/* 本地已装清单 */
.market-pkg-list { display:flex; flex-direction:column; gap:4px; }
.market-pkg-item { display:flex; gap:10px; align-items:baseline; font-size:12px; border-top:1px solid var(--border); padding-top:4px; }
.market-pkg-name { color:var(--text-2); flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
@media (max-width: 640px) {
  .market-remote-card { flex-direction:column; }
}
</style>
