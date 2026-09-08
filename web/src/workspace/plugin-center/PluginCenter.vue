<template>
  <Teleport to="body">
    <div v-if="open" class="plugin-center-mask" @click.self="close">
      <section class="plugin-center" role="dialog" aria-modal="true" aria-labelledby="plugin-center-title">
        <header class="plugin-center-head">
          <div>
            <h2 id="plugin-center-title">插件中心</h2>
            <p>插件会先下载到本地并校验，再由 Gamer 的本地扩展服务加载。</p>
          </div>
          <button class="btn btn-ghost" type="button" aria-label="关闭插件中心" @click="close">✕</button>
        </header>

        <nav class="plugin-center-tabs" aria-label="插件中心分类">
          <button v-for="item in tabs" :key="item.key" type="button" :class="{ active: tab === item.key }" @click="tab = item.key">
            {{ item.label }}<span v-if="item.key === 'installed'" class="tab-count">{{ installed.length }}</span>
          </button>
        </nav>

        <div class="plugin-center-body">
          <div v-if="error" class="plugin-alert error" role="alert">{{ error }}</div>
          <div v-if="notice" class="plugin-alert info" role="status">{{ notice }}</div>
          <div v-if="loading" class="plugin-center-loading">正在读取插件信息…</div>

          <template v-else-if="tab === 'market'">
            <div class="section-head">
              <div><strong>市场</strong><span class="muted">固定版本 · SHA-256 校验 · 本地安装</span></div>
              <button class="btn btn-sm" type="button" :disabled="busy" @click="loadRegistry">刷新</button>
            </div>
            <div v-if="!market.length" class="plugin-empty">市场暂无可用插件，或 registry.json 尚未配置。</div>
            <article v-for="entry in market" :key="`${entry.id}@${entry.version}`" class="plugin-card">
              <div class="plugin-card-main">
                <div class="plugin-title-row">
                  <h3>{{ entry.name }}</h3>
                  <span class="tag info">{{ entry.version }}</span>
                  <span class="tag" :class="entry.execution?.kind === 'builtin' ? 'warn' : ''">{{ executionLabel(entry.execution) }}</span>
                  <span v-if="installedVersion(entry.id)" class="tag ok">已安装 v{{ installedVersion(entry.id) }}</span>
                </div>
                <div class="plugin-meta"><code>{{ entry.id }}</code><span>{{ entry.publisher || '发布者未声明' }}</span></div>
                <p class="plugin-description">{{ entry.description || '暂无描述。' }}</p>
                <div class="plugin-facts">
                  <span>来源：官方市场</span>
                  <span v-if="hostVersionLabel(entry.execution)">宿主版本要求：{{ hostVersionLabel(entry.execution) }}</span>
                  <span>权限：{{ (entry.permissions || []).length ? entry.permissions.join('、') : '无' }}</span>
                  <span>UI：{{ uiType(entry) }}</span>
                </div>
                <div v-if="entry.dependencies?.length || entry.required_extensions?.length || entry.app_packages?.length" class="dependency-line">
                  依赖：{{ dependencyNames(entry).join('、') }}
                </div>
              </div>
              <div class="plugin-card-actions">
                <button class="btn btn-sm btn-primary" type="button" :disabled="busy || !canInstallMarket(entry)" @click="installMarket(entry)">
                  {{ installedVersion(entry.id) ? '更新' : '安装' }}
                </button>
                <span v-if="!canInstallMarket(entry)" class="action-hint">缺少固定 SHA-256，无法安全下载</span>
              </div>
            </article>
          </template>

          <template v-else-if="tab === 'installed'">
            <div class="section-head">
              <div><strong>已安装</strong><span class="muted">运行状态与依赖影响来自服务端管理契约</span></div>
              <button class="btn btn-sm" type="button" :disabled="busy" @click="refresh">刷新</button>
            </div>
            <div v-if="!installed.length" class="plugin-empty">还没有安装插件。</div>
            <article v-for="plugin in installed" :key="plugin.id" class="plugin-card installed-card">
              <div class="plugin-card-main">
                <div class="plugin-title-row">
                  <h3>{{ plugin.name || plugin.id }}</h3>
                  <span class="tag">{{ plugin.active_version || plugin.version || '未知版本' }}</span>
                  <span class="tag" :class="stateClass(plugin.state)">{{ stateLabel(plugin.state) }}</span>
                </div>
                <div class="plugin-meta"><code>{{ plugin.id }}</code><span>来源：{{ sourceLabel(plugin.source) }}</span><span>{{ plugin.publisher || '发布者未知' }}</span></div>
                <div class="plugin-facts">
                  <span>执行形态：{{ executionLabel(plugin.execution) }}</span>
                  <span>权限：{{ (plugin.permissions || []).length ? plugin.permissions.join('、') : '无' }}</span>
                  <span>已保留版本：{{ (plugin.installed_versions || []).join('、') || '无' }}</span>
                </div>
                <!-- V1 生命周期收敛：历史版本仅展示（回退 = 卸载后重装旧版本归档） -->
                <div v-if="switchableVersions(plugin).length" class="version-switch-line">
                  <span class="version-switch-label">历史版本：</span>
                  <span v-for="version in switchableVersions(plugin)" :key="version" class="version-switch-item">
                    <code>{{ version }}</code>
                  </span>
                </div>
                <div v-if="plugin.last_error" class="dependency-line danger-text">失败：{{ plugin.last_error }}</div>
                <div v-if="dependencyItems(plugin).length" class="dependency-line">
                  依赖：{{ dependencyItems(plugin).map(item => item.name || item.id).join('、') }}
                </div>
                <div v-if="dependencyState(plugin).missing.length" class="dependency-line danger-text">
                  缺少扩展依赖：{{ dependencyState(plugin).missing.map(item => item.id).join('、') }}
                </div>
                <div v-if="dependencyState(plugin).disabled.length" class="dependency-line warn-text">
                  依赖未启用：{{ dependencyState(plugin).disabled.map(item => item.id + '（' + item.state + '）').join('、') }}
                </div>
                <div v-if="dependentItems(plugin).length" class="dependency-line warn-text">
                  正被使用：{{ dependentItems(plugin).map(item => `${item.name || item.id}${item.state ? `（${item.state}）` : ''}`).join('、') }}
                </div>
              </div>
              <div class="plugin-card-actions installed-actions">
                <!-- V1 用户操作收敛：安装（自动启用）/ 启用 / 停用 / 更新 / 卸载 -->
                <button v-if="plugin.state !== 'running'" class="btn btn-sm" type="button" :disabled="busy" @click="runAction('enable', plugin)">启用</button>
                <button v-if="plugin.state === 'running'" class="btn btn-sm" type="button" :disabled="busy" @click="runAction('disable', plugin)">停用</button>
                <button v-if="marketUpdate(plugin)" class="btn btn-sm btn-primary" type="button" :disabled="busy || !canInstallMarket(marketUpdate(plugin))" @click="installMarket(marketUpdate(plugin), plugin)">更新到 {{ marketUpdate(plugin).version }}</button>
                <button class="btn btn-sm btn-danger" type="button" :disabled="busy" @click="uninstall(plugin, false)">卸载</button>
                <button class="btn btn-sm btn-danger" type="button" :disabled="busy" @click="uninstall(plugin, true)">删除数据并卸载</button>
              </div>
            </article>
          </template>

          <template v-else-if="tab === 'local'">
            <div class="import-pane">
              <h3>本地导入</h3>
              <p>选择 <code>.gplugin</code> 文件。本地导入不要求签名；安装前会展示插件 ID、执行形态、权限与来源供确认。</p>
              <label class="file-picker btn btn-primary">
                选择 .gplugin
                <input ref="fileInput" type="file" accept=".gplugin,.zip,application/zip" @change="onLocalFile" />
              </label>
              <div v-if="localFileName" class="selected-file">已选择：{{ localFileName }}</div>
            </div>
          </template>

          <template v-else>
            <div class="import-pane">
              <h3>URL 导入</h3>
              <p>仅下载固定归档到本地安装；远程地址永远不会被用作生产 iframe。</p>
              <div class="url-row">
                <input v-model.trim="url" class="input" type="url" placeholder="https://example.com/plugin.gplugin" @keyup.enter="onUrlImport" />
                <button class="btn btn-primary" type="button" :disabled="busy || !url" @click="onUrlImport">下载并安装</button>
              </div>
              <div class="plugin-alert warning">URL 导入来源不属于官方市场，请在确认框中核对发布者、权限与来源。</div>
            </div>
          </template>
        </div>
      </section>
    </div>
  </Teleport>
</template>

<script setup>
import { computed, onMounted, ref, watch } from 'vue'
import { api } from '../../api'
import { compareVersions, downloadDirectUrl, downloadFixedVersion, fetchRegistry, findRegistryPlugin } from './registry-client'
import {
  activateVersionErrorText,
  activateVersionPrompt,
  dependencyRefsFor,
  dependencyStatus,
  executionChangeDetail,
  executionLabel,
  hostVersionLabel,
  installErrorText,
  installPolicy,
  installSummary,
  lifecyclePrompt,
  mergeManagementResponse,
  normalizeExecution,
  readPluginSourceMetadata,
  rememberPluginSource,
  sourceLabel as sourceText,
  uninstallPrompt,
} from './plugin-service'

const props = defineProps({
  open: { type: Boolean, default: false },
  apiClient: { type: Object, default: () => api },
  registryUrl: { type: String, default: '/registry.json' },
})
const emit = defineEmits(['close', 'changed'])

const tabs = [
  { key: 'market', label: '市场' },
  { key: 'installed', label: '已安装' },
  { key: 'local', label: '本地导入' },
  { key: 'url', label: 'URL 导入' },
]
const tab = ref('market')
const registry = ref({ schema_version: 1, plugins: [] })
const installed = ref([])
const loading = ref(false)
const busy = ref(false)
const error = ref('')
const notice = ref('')
const sourceMetadata = ref(readPluginSourceMetadata())
const url = ref('')
const localFileName = ref('')
const fileInput = ref(null)

const market = computed(() => registry.value.plugins || [])

function close() { emit('close') }
function clearMessages() { error.value = ''; notice.value = '' }
function messageFor(errorValue) { return String(errorValue?.message || errorValue || '操作失败') }

async function loadRegistry() {
  try {
    registry.value = await fetchRegistry(globalThis.fetch, props.registryUrl)
  } catch (errorValue) {
    error.value = messageFor(errorValue)
    registry.value = { schema_version: 1, plugins: [] }
  }
}

async function loadInstalled() {
  const client = props.apiClient
  let response
  try {
    response = await client.getExtensionManagement()
  } catch (errorValue) {
    // Older Phase 6 servers still provide the base list; management fields are
    // additive and must not make the center unusable during rollout.
    response = await client.listExtensions()
  }
  installed.value = mergeManagementResponse(response, market.value, sourceMetadata.value)
}

async function refresh() {
  if (!props.open) return
  loading.value = true
  clearMessages()
  try {
    await loadRegistry()
    await loadInstalled()
  } catch (errorValue) {
    error.value = messageFor(errorValue)
  } finally { loading.value = false }
}

watch(() => props.open, value => { if (value) void refresh() })
onMounted(() => { if (props.open) void refresh() })

function installedPlugin(id) { return installed.value.find(item => item.id === id) }
function installedVersion(id) { return installedPlugin(id)?.active_version || installedPlugin(id)?.version || '' }
function marketUpdate(plugin) {
  const entry = findRegistryPlugin(registry.value, plugin.id)
  if (!entry || compareVersions(entry.version, installedVersion(plugin.id)) <= 0) return null
  return entry
}
function canInstallMarket(entry) {
  // 官方固定版本下载强制 SHA-256（registry-client downloadFixedVersion 门禁）；
  // 签名/proof 不再参与安装决策，builtin 包同样走下载校验 + 服务端宿主注册表门禁。
  return !!entry?.sha256
}
function uiType(entry) {
  const contributions = entry.ui?.contributions
  if (!Array.isArray(contributions) || !contributions.length) return 'none'
  // runtime 三档如实展示（core=宿主预置组件）；未知值退化为 declarative
  if (contributions.some(item => item && item.runtime === 'core')) return 'core（宿主组件）'
  if (contributions.some(item => item && item.runtime === 'iframe')) return 'iframe'
  return 'declarative'
}
function dependencyNames(entry) { return dependencyRefsFor(entry).map(item => item.name || item.id) }
function dependencyItems(plugin) { return dependencyRefsFor(plugin) }
function dependencyState(plugin) {
  const extensionDependencies = dependencyItems(plugin).filter(item => item.kind === 'extension' || !item.kind)
  return dependencyStatus(extensionDependencies, installed.value)
}
function dependentItems(plugin) {
  const dependent = plugin.dependent || {}
  return [...(dependent.app_packages || []), ...(dependent.tasks || []), ...(dependent.workflows || [])]
}
function stateClass(state) { return state === 'running' ? 'run' : state === 'enabled' ? 'ok' : state === 'failed' ? 'err' : 'warn' }
function stateLabel(state) { return ({ installed: '已安装', enabled: '已启用', running: '运行中', disabled: '已停用', failed: '失败' })[state] || state || '未知' }
function sourceLabel(source) { return sourceText(source || 'unknown') }

function formatPermissionDiff(diff) {
  const lines = []
  if (diff.added.length) lines.push(`新增权限：${diff.added.join('、')}`)
  if (diff.removed.length) lines.push(`移除权限：${diff.removed.join('、')}`)
  return lines.length ? lines.join('\n') : '权限无变化。'
}

async function inspectAndConfirm(file, source, current, providedInspection = null) {
  const inspection = providedInspection || await props.apiClient.inspectExtension(file)
  const entry = source.registryEntry
  if (entry && (inspection.id !== entry.id || inspection.version !== entry.version)) {
    throw new Error(`固定版本校验失败：期望 ${entry.id}@${entry.version}，归档是 ${inspection.id}@${inspection.version}`)
  }
  const requestedPermissions = inspection.permissions || entry?.permissions || []
  const summary = installSummary(source, current, requestedPermissions)
  const policy = installPolicy(source)
  // 执行形态以服务端 inspect 结果为准（包内真实 manifest），registry 条目兜底；
  // host_version 缺失不展示。普通用户界面不出现密钥/签名/proof 概念。
  const execution = normalizeExecution(inspection.execution || source.execution || entry?.execution)
  const hostVersion = execution.host_version || ''
  // Phase 8 遗留（D2）：wasm↔builtin 等执行形态变化必须明确提示——形态决定
  // 插件的执行方式与信任边界（宿主预置 vs guest WASM），更新前由用户确认。
  // 文案唯一实现在 plugin-service.executionChangeDetail（可单测）。
  const executionChangeLine = executionChangeDetail(inspection.execution_change)
  const title = current ? '确认更新插件' : '确认安装插件'
  const details = [
    `${title}：${inspection.name || inspection.id}`,
    `ID：${inspection.id}`,
    `版本：${inspection.version}`,
    `来源：${sourceLabel(source.kind)}${source.publisher ? `；发布者：${source.publisher}` : ''}`,
    `执行形态：${executionLabel(execution)}`,
    ...(executionChangeLine ? [executionChangeLine] : []),
    ...(hostVersion ? [`宿主版本要求：${hostVersion}`] : []),
    `权限：${requestedPermissions.length ? requestedPermissions.join('、') : '无'}`,
    formatPermissionDiff(summary.diff),
  ]
  if (policy.requiresWarning || summary.diff.added.length) {
    details.push('请确认来源与权限后继续。')
  }
  if (!globalThis.confirm(details.join('\n'))) return null
  return { inspection, summary }
}

async function installArchive(file, source, current) {
  // official 来源只用于服务端来源记录（审计/更新判断），不再触发签名/proof 门禁。
  const uploadOptions = source.kind === 'official' ? { source: 'official' } : {}
  const inspection = await props.apiClient.inspectExtension(file, uploadOptions)
  const existing = current || installedPlugin(inspection.id)
  const result = await inspectAndConfirm(file, source, existing, inspection)
  if (!result) return false
  const confirmedOptions = { ...uploadOptions, permissionConfirmed: true }
  const operation = existing
    ? props.apiClient.updateExtension(existing.id, file, confirmedOptions)
    : props.apiClient.installExtension(file, confirmedOptions)
  const snapshot = await operation
  sourceMetadata.value = rememberPluginSource(sourceMetadata.value, result.inspection.id, result.inspection.version, source)
  if (!existing || existing.state !== 'disabled') {
    if (snapshot?.state === 'installed') await props.apiClient.enableExtension(snapshot.id || result.inspection.id)
  }
  emit('changed')
  await refresh()
  // notice 放在 refresh 之后：refresh 内部 clearMessages 会清掉先行的提示（同 activateVersion）
  notice.value = `${result.inspection.name || result.inspection.id}@${result.inspection.version} 已${existing ? '更新' : '安装'}。`
  return true
}

async function installMarket(entry, current = installedPlugin(entry.id)) {
  if (!canInstallMarket(entry)) {
    error.value = '该市场条目缺少固定版本 SHA-256，无法校验完整性，已阻止下载。'
    return
  }
  busy.value = true
  clearMessages()
  try {
    const downloaded = await downloadFixedVersion(entry)
    await installArchive(downloaded.file, {
      kind: 'official', label: '官方市场', publisher: entry.publisher, execution: entry.execution, registryEntry: entry,
    }, current)
  } catch (errorValue) { error.value = installErrorText(errorValue) } finally { busy.value = false }
}

async function onLocalFile(event) {
  const file = event.target.files?.[0]
  if (!file) return
  localFileName.value = file.name
  busy.value = true
  clearMessages()
  try {
    await installArchive(file, { kind: 'local', label: '本地文件' }, installedPlugin(''))
  } catch (errorValue) { error.value = installErrorText(errorValue) } finally {
    busy.value = false
    event.target.value = ''
  }
}

async function onUrlImport() {
  busy.value = true
  clearMessages()
  try {
    const downloaded = await downloadDirectUrl(url.value)
    await installArchive(downloaded.file, { kind: 'url', label: url.value }, undefined)
    url.value = ''
  } catch (errorValue) { error.value = installErrorText(errorValue) } finally { busy.value = false }
}

async function runAction(action, plugin) {
  if (!globalThis.confirm(lifecyclePrompt(action, plugin))) return
  busy.value = true
  clearMessages()
  try {
    const method = { enable: 'enableExtension', disable: 'disableExtension' }[action]
    await props.apiClient[method](plugin.id)
    notice.value = `${plugin.name || plugin.id}：${action === 'enable' ? '已启用' : '已停用'}。`
    emit('changed')
    await refresh()
  } catch (errorValue) { error.value = messageFor(errorValue) } finally { busy.value = false }
}

/** 可切换（含回滚）的历史版本：已安装列表里排除当前活动版本。 */
function switchableVersions(plugin) {
  const active = plugin.active_version || plugin.version || ''
  return (plugin.installed_versions || []).filter(version => version && version !== active)
}

async function uninstall(plugin, deleteData) {
  if (!globalThis.confirm(uninstallPrompt(plugin, deleteData))) return
  busy.value = true
  clearMessages()
  try {
    await props.apiClient.uninstallExtension(plugin.id, plugin.active_version || plugin.version, { deleteData })
    notice.value = deleteData ? '插件及其用户数据已删除。' : '插件已卸载，用户数据已保留。'
    emit('changed')
    await refresh()
  } catch (errorValue) { error.value = messageFor(errorValue) } finally { busy.value = false }
}
</script>

<style scoped>
.plugin-center-mask { position:fixed; inset:0; z-index:210; display:flex; align-items:center; justify-content:center; padding:24px; background:rgba(4,6,10,.76); backdrop-filter:blur(4px); }
.plugin-center { width:min(900px, 96vw); max-height:90vh; display:flex; flex-direction:column; overflow:hidden; background:var(--bg-2); border:1px solid var(--border); border-radius:14px; box-shadow:var(--shadow); }
.plugin-center-head { display:flex; justify-content:space-between; gap:20px; padding:18px 22px; border-bottom:1px solid var(--border); }
.plugin-center-head h2 { font-size:18px; }
.plugin-center-head p { margin-top:5px; color:var(--text-2); font-size:12px; }
.plugin-center-tabs { display:flex; gap:4px; padding:10px 20px 0; border-bottom:1px solid var(--border); }
.plugin-center-tabs button { padding:8px 12px 10px; border:0; border-bottom:2px solid transparent; background:transparent; color:var(--text-1); cursor:pointer; font-size:13px; }
.plugin-center-tabs button:hover { color:var(--text-0); }
.plugin-center-tabs button.active { border-color:var(--accent); color:var(--accent); font-weight:600; }
.tab-count { margin-left:5px; color:var(--text-2); }
.plugin-center-body { min-height:300px; overflow:auto; padding:18px 20px 22px; }
.plugin-center-loading, .plugin-empty { display:flex; justify-content:center; align-items:center; min-height:220px; color:var(--text-2); }
.section-head { display:flex; align-items:center; justify-content:space-between; gap:12px; margin-bottom:12px; }
.section-head > div { display:flex; align-items:baseline; gap:10px; }
.muted { color:var(--text-2); font-size:12px; }
.plugin-card { display:flex; justify-content:space-between; gap:18px; margin-bottom:10px; padding:14px; background:var(--bg-1); border:1px solid var(--border); border-radius:var(--radius); }
.plugin-card-main { min-width:0; flex:1; }
.plugin-title-row { display:flex; align-items:center; flex-wrap:wrap; gap:7px; }
.plugin-title-row h3 { font-size:14px; }
.plugin-meta, .plugin-facts { display:flex; flex-wrap:wrap; gap:5px 14px; margin-top:7px; color:var(--text-2); font-size:11px; }
.plugin-meta code, .selected-file code, .import-pane code { color:var(--accent-2); font-family:var(--mono); }
.plugin-description { margin-top:9px; color:var(--text-1); font-size:12px; line-height:1.5; }
.dependency-line { margin-top:8px; color:var(--text-1); font-size:11px; line-height:1.45; }
.version-switch-line { display:flex; align-items:center; flex-wrap:wrap; gap:6px 10px; margin-top:8px; font-size:11px; }
.version-switch-label { color:var(--text-2); }
.version-switch-item { display:inline-flex; align-items:center; gap:6px; }
.danger-text { color:var(--danger); }
.warn-text { color:var(--warn); }
.plugin-card-actions { display:flex; flex-direction:column; align-items:flex-end; justify-content:flex-start; gap:7px; flex-shrink:0; }
.installed-actions { max-width:260px; flex-direction:row; flex-wrap:wrap; align-content:flex-start; justify-content:flex-end; }
.action-hint { color:var(--warn); font-size:10px; white-space:nowrap; }
.tag.ok { color:var(--ok); }
.tag.err { color:var(--danger); }
.tag.warn { color:var(--warn); }
.import-pane { max-width:680px; margin:25px auto; padding:24px; background:var(--bg-1); border:1px solid var(--border); border-radius:var(--radius); }
.import-pane h3 { font-size:15px; }
.import-pane p { margin:10px 0 16px; color:var(--text-1); font-size:12px; line-height:1.6; }
.file-picker { position:relative; overflow:hidden; }
.file-picker input { position:absolute; inset:0; width:100%; height:100%; cursor:pointer; opacity:0; }
.selected-file { margin-top:12px; color:var(--text-1); font-size:12px; }
.url-row { display:flex; gap:8px; }
.url-row .input { flex:1; }
.plugin-alert { margin-bottom:12px; padding:9px 11px; border:1px solid var(--border); border-radius:var(--radius-sm); font-size:12px; line-height:1.5; white-space:pre-line; }
.plugin-alert.error { color:var(--danger); border-color:rgba(248,113,113,.45); background:rgba(248,113,113,.08); }
.plugin-alert.info { color:var(--accent-2); border-color:rgba(56,189,248,.35); background:rgba(56,189,248,.08); }
.plugin-alert.warning { margin-top:16px; color:var(--warn); border-color:rgba(251,191,36,.35); background:rgba(251,191,36,.08); }
@media (max-width: 700px) {
  .plugin-center-mask { padding:8px; }
  .plugin-card { flex-direction:column; }
  .plugin-card-actions, .installed-actions { align-items:flex-start; justify-content:flex-start; max-width:none; }
  .url-row { flex-direction:column; }
}
</style>
