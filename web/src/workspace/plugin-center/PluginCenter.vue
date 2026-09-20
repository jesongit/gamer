<template>
  <section class="plugin-center" aria-label="插件">
    <div class="plugin-toolbar">
      <input v-model="query" class="input plugin-search" type="search" aria-label="搜索插件" placeholder="搜索插件名称、ID 或描述" />
      <span class="muted plugin-count">{{ filteredPlugins.length }} 个插件</span>
      <button class="btn btn-sm" :disabled="busy || loading" @click="refresh">刷新</button>
      <button class="btn btn-sm" :disabled="busy" @click="fileInput?.click()">本地导入</button>
      <button class="btn btn-sm" :aria-expanded="urlOpen" @click="urlOpen = !urlOpen">URL 导入</button>
      <input ref="fileInput" type="file" accept=".gplugin,.zip,application/zip" hidden @change="onLocalFile" />
    </div>
    <div v-if="urlOpen" class="url-import">
      <div class="url-row"><input v-model.trim="url" class="input" type="url" aria-label="插件归档地址" placeholder="https://example.com/plugin.gplugin" @keyup.enter="onUrlImport" /><button class="btn" :disabled="busy || !url" @click="onUrlImport">下载并安装</button></div>
      <p class="muted">下载后核对来源与权限。</p>
    </div>
        <div class="plugin-center-body">
          <div v-if="error" class="plugin-alert error" role="alert">{{ error }}</div>
          <div v-if="operationResult" class="plugin-result" role="status" aria-live="polite">
            <div class="plugin-result-operation">{{ operationResult.operation.text }}</div>
            <div v-if="operationResult.detail" class="plugin-result-detail" :class="`result-${typeof operationResult.detail === 'object' ? operationResult.detail.tone : 'info'}`">
              {{ typeof operationResult.detail === 'object' ? operationResult.detail.text : operationResult.detail }}
            </div>
            <div v-if="operationResult.refreshError" class="plugin-result-detail result-warning">
              刷新插件状态失败：{{ operationResult.refreshError }}；变更本身已完成，可稍后手动刷新。
            </div>
          </div>
          <div v-else-if="notice" class="plugin-alert info" role="status">{{ notice }}</div>
          <div v-if="busy" class="plugin-operation-loading" role="status" aria-live="polite">
            正在{{ operationLabel(activeOperation) }}，请稍候…
          </div>
          <div v-if="loading" class="plugin-center-loading">正在读取插件信息…</div>

          <template v-else>
            <div v-if="!filteredPlugins.length" class="plugin-empty">{{ query ? '没有匹配的插件，请调整搜索。' : '暂无可用插件，可通过本地或 URL 导入。' }}</div>
            <div class="plugin-list-head" aria-hidden="true"><span>插件 / 说明</span><span>状态 / 版本</span><span>操作</span></div>
            <div class="plugin-grid" aria-label="插件列表">
              <article v-for="item in filteredPlugins" :key="item.id" class="plugin-card" :class="{ 'installed-card': item.installed }" :data-plugin-id="item.id">
                <div class="plugin-card-main">
                  <div class="plugin-identity"><div class="plugin-title-row">
                    <h3>{{ item.info.name || item.id }}</h3>
                    <span class="plugin-version mono">{{ item.info.active_version || item.info.version || '未知版本' }}</span>
                  </div>
                  <div class="plugin-meta"><code>{{ item.id }}</code><span>来源：{{ item.installed ? sourceLabel(item.installed.source) : '官方市场' }}</span><span>{{ item.info.publisher || item.entry?.publisher || '发布者未声明' }}</span></div>
                  </div>
                  <p v-if="item.info.description || item.entry?.description" class="plugin-description">{{ item.info.description || item.entry.description }}</p>
                  <template v-if="item.installed">
                    <div v-if="item.installed.last_error" class="dependency-line danger-text">失败：{{ item.installed.last_error }}</div>
                    <div v-if="dependencyState(item.installed).missing.length" class="dependency-line danger-text">缺少必需依赖：{{ dependencyState(item.installed).missing.map(dep => dep.id).join('、') }}（请搜索并安装对应插件）</div>
                    <div v-if="dependencyState(item.installed).disabled.length" class="dependency-line warn-text">必需依赖未启用：{{ dependencyState(item.installed).disabled.map(dep => dep.id + '（' + dep.state + '）').join('、') }}</div>
                    <div v-for="dep in optionalDependencyNotes(item.installed)" :key="dep.id" class="dependency-line warn-text">可选依赖 {{ dep.id }}{{ dep.note ? `（${dep.note}）` : '' }}：相关功能入口已降级</div>
                    <div v-if="dependentItems(item.installed).length" class="dependency-line warn-text">正被使用：{{ dependentItems(item.installed).map(dep => `${dep.name || dep.id}${dep.state ? `（${dep.state}）` : ''}`).join('、') }}</div>
                  </template>
                  <div v-else-if="dependencyNames(item.entry).length" class="dependency-line">依赖：{{ dependencyNames(item.entry).join('、') }}</div>
                  <div v-if="item.relation?.kind === 'incompatible'" class="dependency-line warn-text">{{ item.relation.reason }}</div>
                  <div v-if="item.entry && ['install','update'].includes(item.relation?.kind) && !canInstallMarket(item.entry)" class="dependency-line warn-text">缺少固定 SHA-256，无法安全下载</div>
                  <details class="plugin-details"><summary>详情 <UiIcon name="down" /></summary>
                  <div class="plugin-facts">
                    <span>执行形态：{{ executionLabel(item.info.execution) }}</span>
                    <span>权限：{{ item.info.permissions?.length ? item.info.permissions.join('、') : '无' }}</span>
                    <span v-if="hostVersionLabel(item.info.execution)">宿主版本要求：{{ hostVersionLabel(item.info.execution) }}</span>
                    <span v-if="item.installed && !item.entry">未收录于市场</span>
                  </div>
                    <template v-if="item.installed">
                    <div v-if="switchableVersions(item.installed).length" class="version-switch-line">历史版本：{{ switchableVersions(item.installed).join('、') }}</div>
                    <div v-if="dependencyItems(item.installed).length" class="dependency-line">依赖：{{ dependencyItems(item.installed).map(dep => dep.name || dep.id).join('、') }}</div>
                      <div class="plugin-remove-actions"><button class="btn btn-sm btn-ghost" :disabled="busy || !installedKnown" @click="uninstall(item.installed, false)"><UiIcon name="trash" />卸载</button><button class="btn btn-sm btn-ghost danger-text" :disabled="busy || !installedKnown" @click="uninstall(item.installed, true)">删除数据并卸载</button></div>
                    </template>
                  </details>
                </div>
                <div class="plugin-status">
                    <span class="tag" :class="installedKnown && item.installed ? stateClass(item.installed.state) : ''">{{ !installedKnown ? '状态未确认' : item.installed ? stateLabel(item.installed.state) : '未安装' }}</span>
                    <span v-if="item.relation?.kind === 'update'" class="tag info">有更新 · {{ item.entry.version }}</span>
                    <span v-else-if="item.relation?.kind === 'latest'" class="tag ok">已是最新</span>
                    <span v-else-if="item.relation?.kind === 'newer_installed'" class="tag warn">已安装更高版本</span>
                    <span v-else-if="item.relation?.kind === 'incompatible'" class="tag warn">版本不兼容</span>
                </div>
                <div class="plugin-card-actions">
                  <template v-if="item.installed">
                    <button class="btn btn-sm" :disabled="busy || !installedKnown" @click="runAction(item.installed.state === 'running' ? 'disable' : 'enable', item.installed)">{{ item.installed.state === 'running' ? '停用' : '启用' }}</button>
                    <button v-if="item.relation?.kind === 'update'" class="btn btn-sm btn-primary" :disabled="busy || !installedKnown || !canInstallMarket(item.entry)" @click="installMarket(item.entry, item.installed)">更新到 {{ item.entry.version }}</button>
                  </template>
                  <button v-else class="btn btn-sm btn-primary" :disabled="busy || !installedKnown || !marketActionAllowed(item.entry)" @click="installMarket(item.entry)">安装</button>
                </div>
              </article>
            </div>
          </template>
        </div>
      </section>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { api } from '../../api'
import UiIcon from '../../components/ui/UiIcon.vue'
import { useConfirmDialog } from '../../components/ui/useConfirmDialog'
import { downloadDirectUrl, downloadFixedVersion, fetchRegistry, findRegistryPlugin } from './registry-client'
import {
  dependencyRefsFor,
  dependencyStatus,
  executionChangeDetail,
  executionLabel,
  hostVersionLabel,
  installErrorText,
  installPolicy,
  installSummary,
  lifecyclePrompt,
  describeExtensionMutation,
  mergeManagementResponse,
  marketVersionRelation,
  normalizeExecution,
  readPluginSourceMetadata,
  rememberPluginSource,
  sourceLabel as sourceText,
  uninstallPrompt,
} from './plugin-service'

const props = defineProps({
  apiClient: { type: Object, default: () => api },
  registryUrl: { type: String, default: '/registry.json' },
})
const emit = defineEmits(['changed'])
const confirmDialog = useConfirmDialog()

const query = ref('')
const urlOpen = ref(false)
const registry = ref({ schema_version: 1, plugins: [] })
const installed = ref([])
const installedKnown = ref(false)
const loading = ref(false)
const busy = ref(false)
const activeOperation = ref('')
const error = ref('')
const notice = ref('')
const operationResult = ref(null)
const sourceMetadata = ref(readPluginSourceMetadata())
const url = ref('')
const fileInput = ref(null)

const market = computed(() => registry.value.plugins || [])
// ID 是唯一身份；市场有多个版本时只呈现最新条目，运行状态与权限以已安装快照为准。
const plugins = computed(() => {
  const ids = [...new Set([...installed.value.map(item => item.id), ...market.value.map(item => item.id)])]
  return ids.map(id => {
    const current = installedPlugin(id)
    const entry = findRegistryPlugin(registry.value, id)
    return { id, installed: current, entry, info: current || entry, relation: entry ? marketVersionRelation(entry, current) : null }
  })
})
const filteredPlugins = computed(() => {
  const needle = query.value.trim().toLowerCase()
  return plugins.value.filter(item => [item.id, item.info.name, item.info.description, item.entry?.name, item.entry?.description].some(value => String(value || '').toLowerCase().includes(needle)))
})

function clearMessages() { error.value = ''; notice.value = ''; operationResult.value = null }
function messageFor(errorValue) { return String(errorValue?.message || errorValue || '操作失败') }

function beginOperation(key) {
  if (busy.value) return false
  busy.value = true
  activeOperation.value = key
  return true
}

function endOperation(key) {
  if (activeOperation.value === key) {
    activeOperation.value = ''
    busy.value = false
  }
}

function operationLabel(key) {
  if (String(key).startsWith('market:')) return '更新插件'
  if (String(key).startsWith('uninstall:')) return '卸载插件'
  if (String(key).startsWith('enable:')) return '启用插件'
  if (String(key).startsWith('disable:')) return '停用插件'
  if (key === 'local-import') return '导入插件'
  if (key === 'url-import') return '导入插件'
  return '处理插件'
}

function showMutationResult(description, refreshResult) {
  operationResult.value = {
    ...description,
    refreshError: refreshResult?.ok ? '' : (refreshResult?.error || '未知错误'),
  }
  notice.value = ''
}

async function loadInstalled() {
  const client = props.apiClient
  // The management endpoint is the current contract: its outer dependencies
  // map describes dependents, while each extension entry is the authoritative
  // lifecycle snapshot. Do not silently downgrade to the base list response.
  const response = await client.getExtensionManagement()
  installed.value = mergeManagementResponse(response, market.value, sourceMetadata.value)
  installedKnown.value = true
}

let refreshSerial = 0
async function refresh() {
  const serial = ++refreshSerial
  installedKnown.value = false
  loading.value = true
  error.value = ''
  const failures = []
  try {
    registry.value = await fetchRegistry(globalThis.fetch, props.registryUrl)
  } catch (errorValue) {
    failures.push(messageFor(errorValue))
  }
  if (serial !== refreshSerial) return { ok: false, stale: true, error: '刷新请求已过期' }
  try {
    await loadInstalled()
  } catch (errorValue) {
    failures.push(messageFor(errorValue))
  }
  if (serial !== refreshSerial) return { ok: false, stale: true, error: '刷新请求已过期' }
  if (failures.length) error.value = failures.join('\n')
  loading.value = false
  return failures.length ? { ok: false, error: failures.join('\n') } : { ok: true }
}

onMounted(() => { void refresh() })

function installedPlugin(id) { return installed.value.find(item => item.id === id) }
function marketRelation(entry) { return marketVersionRelation(entry, installedPlugin(entry.id)) }
function marketActionAllowed(entry) {
  const relation = marketRelation(entry)
  return ['install', 'update'].includes(relation.kind) && canInstallMarket(entry)
}
function canInstallMarket(entry) {
  // 官方固定版本下载强制 SHA-256（registry-client downloadFixedVersion 门禁）；
  // 签名/proof 不再参与安装决策，builtin 包同样走下载校验 + 服务端宿主注册表门禁。
  return !!entry?.sha256
}
function dependencyNames(entry) { return dependencyRefsFor(entry).map(item => item.name || item.id) }
function dependencyItems(plugin) { return dependencyRefsFor(plugin) }
/** 依赖满足状态（简化计划 Phase 3）：优先用快照携带的服务端实时依赖状态
 *  （含 required/satisfied），旧形态快照回退本地推导。只对必需依赖报缺失/未启用。 */
function dependencyState(plugin) {
  const serverDeps = Array.isArray(plugin.dependencies) ? plugin.dependencies : []
  if (serverDeps.length) {
    const missing = serverDeps.filter(dep => dep.required && !dep.installed).map(dep => ({ id: dep.id, state: dep.version_req || '' }))
    const disabled = serverDeps.filter(dep => dep.required && dep.installed && !dep.satisfied).map(dep => ({ id: dep.id, state: dep.state || '未启用' }))
    return { ok: missing.length === 0 && disabled.length === 0, missing, disabled }
  }
  const extensionDependencies = dependencyItems(plugin).filter(item => item.kind === 'extension' || !item.kind)
  return dependencyStatus(extensionDependencies, installed.value)
}
/** 可选依赖未满足的降级提示（基础功能不受影响；对应功能入口由消费方隐藏）。 */
function optionalDependencyNotes(plugin) {
  const serverDeps = Array.isArray(plugin.dependencies) ? plugin.dependencies : []
  return serverDeps.filter(dep => !dep.required && !dep.satisfied)
}
function dependentItems(plugin) {
  const dependent = plugin.dependent || {}
  return [...(dependent.app_packages || []), ...(dependent.tasks || []), ...(dependent.workflows || [])]
}
function stateClass(state) { return state === 'running' ? 'run' : state === 'enabled' ? 'ok' : state === 'failed' ? 'err' : 'idle' }
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
  const accepted = await confirmDialog(inspection.name || inspection.id, {
    title, confirmText: current ? '确认更新' : '确认安装',
    fields: [
      { label: '插件 ID', value: inspection.id },
      { label: '版本', value: current ? `${current.active_version || current.version} → ${inspection.version}` : inspection.version },
      { label: '来源', value: `${sourceLabel(source.kind)}${source.publisher ? ` · ${source.publisher}` : ''}` },
      { label: '执行方式', value: executionLabel(execution) },
      ...(hostVersion ? [{ label: '宿主要求', value: hostVersion }] : []),
    ],
    sections: [
      { title: '请求权限', text: requestedPermissions.length ? requestedPermissions.join('、') : '无' },
      ...(requestedPermissions.includes('ui.host') ? [{ title: '宿主界面权限', text: '此插件的界面与 Gamer 页面在同一环境运行，可访问当前会话和工作区。仅安装你信任的插件；更新界面后需刷新页面生效。', tone: 'warn' }] : []),
      ...(current ? [{ title: '权限变化', text: formatPermissionDiff(summary.diff), tone: summary.diff.added.length ? 'warn' : '' }] : []),
      ...(executionChangeLine ? [{ title: '执行方式变化', text: executionChangeLine, tone: 'warn' }] : []),
    ],
    warning: policy.requiresWarning || summary.diff.added.length ? '请确认来源与权限后继续。' : '',
  })
  if (!accepted) return null
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
  let snapshot = await operation
  sourceMetadata.value = rememberPluginSource(sourceMetadata.value, result.inspection.id, result.inspection.version, source)
  if (!existing || existing.state !== 'disabled') {
    if (snapshot?.state === 'installed') snapshot = await props.apiClient.enableExtension(snapshot.id || result.inspection.id)
  }
  emit('changed')
  const refreshResult = await refresh()
  showMutationResult(
    describeExtensionMutation(existing ? 'update' : 'install', snapshot, { plugin: existing }),
    refreshResult,
  )
  return true
}

async function installMarket(entry, current = installedPlugin(entry.id)) {
  const relation = marketVersionRelation(entry, current)
  if (!['install', 'update'].includes(relation.kind)) {
    error.value = relation.kind === 'latest'
      ? '该插件已是最新版本，无需重复更新。'
      : relation.kind === 'newer_installed'
        ? '当前已安装更高版本，无需降级。'
        : relation.reason || '该市场版本与当前安装状态不兼容，已阻止更新。'
    return
  }
  if (!canInstallMarket(entry)) {
    error.value = '该市场条目缺少固定版本 SHA-256，无法校验完整性，已阻止下载。'
    return
  }
  const operationKey = `market:${entry.id}@${entry.version}`
  if (!beginOperation(operationKey)) return
  clearMessages()
  try {
    const downloaded = await downloadFixedVersion(entry)
    await installArchive(downloaded.file, {
      kind: 'official', label: '官方市场', publisher: entry.publisher, execution: entry.execution, registryEntry: entry,
    }, current)
  } catch (errorValue) { error.value = installErrorText(errorValue) } finally { endOperation(operationKey) }
}

async function onLocalFile(event) {
  const file = event.target.files?.[0]
  if (!file) return
  const operationKey = 'local-import'
  if (!beginOperation(operationKey)) return
  clearMessages()
  try {
    await installArchive(file, { kind: 'local', label: '本地文件' }, installedPlugin(''))
  } catch (errorValue) { error.value = installErrorText(errorValue) } finally {
    endOperation(operationKey)
    event.target.value = ''
  }
}

async function onUrlImport() {
  const operationKey = 'url-import'
  if (!beginOperation(operationKey)) return
  clearMessages()
  try {
    const downloaded = await downloadDirectUrl(url.value)
    await installArchive(downloaded.file, { kind: 'url', label: url.value }, undefined)
    url.value = ''
  } catch (errorValue) { error.value = installErrorText(errorValue) } finally { endOperation(operationKey) }
}

async function runAction(action, plugin) {
  if (!await confirmDialog(lifecyclePrompt(action, plugin), { title: action === 'enable' ? '启用插件' : '停用插件', confirmText: action === 'enable' ? '启用' : '停用' })) return
  const operationKey = `${action}:${plugin.id}`
  if (!beginOperation(operationKey)) return
  clearMessages()
  try {
    const method = { enable: 'enableExtension', disable: 'disableExtension' }[action]
    await props.apiClient[method](plugin.id)
    emit('changed')
    const refreshResult = await refresh()
    notice.value = `${plugin.name || plugin.id}：${action === 'enable' ? '已启用' : '已停用'}。`
    if (!refreshResult.ok && !refreshResult.stale) {
      error.value = `刷新插件状态失败：${refreshResult.error}；操作本身已完成，可稍后手动刷新。`
    }
  } catch (errorValue) { error.value = messageFor(errorValue) } finally { endOperation(operationKey) }
}

/** 可切换（含回滚）的历史版本：已安装列表里排除当前活动版本。 */
function switchableVersions(plugin) {
  const active = plugin.active_version || plugin.version || ''
  return (plugin.installed_versions || []).filter(version => version && version !== active)
}

async function uninstall(plugin, deleteData) {
  if (!await confirmDialog(uninstallPrompt(plugin, deleteData), { title: '卸载插件', confirmText: deleteData ? '删除数据并卸载' : '卸载', danger: true })) return
  const operationKey = `uninstall:${plugin.id}`
  if (!beginOperation(operationKey)) return
  clearMessages()
  try {
    const response = await props.apiClient.uninstallExtension(plugin.id, plugin.active_version || plugin.version, { deleteData })
    emit('changed')
    const refreshResult = await refresh()
    showMutationResult(describeExtensionMutation('uninstall', response, { plugin, deleteData }), refreshResult)
  } catch (errorValue) { error.value = messageFor(errorValue) } finally { endOperation(operationKey) }
}
</script>

<style scoped>
.plugin-center { flex:1; min-width:0; min-height:0; display:flex; flex-direction:column; overflow:hidden; padding:18px 24px; background:var(--bg-0); container-type:inline-size; }
.plugin-toolbar { display:flex; gap:8px; align-items:center; flex-wrap:wrap; padding-bottom:16px; flex-shrink:0; }
.plugin-search { width:320px; max-width:100%; }.plugin-count { margin-left:auto; white-space:nowrap; }
.url-import { padding:12px; margin-bottom:14px; background:var(--bg-1); border:1px solid var(--border); flex-shrink:0; }.url-import p { margin-top:6px; }.url-row { display:flex; gap:8px; }.url-row .input { flex:1; min-width:0; }
.plugin-center-body { min-height:0; overflow:auto; }
.plugin-list-head,.plugin-card { display:grid; grid-template-columns:minmax(0,1fr) 180px 260px; gap:24px; }
.plugin-list-head { padding:9px 16px; background:var(--bg-1); border:1px solid var(--border); color:var(--text-2); font-size:12px; border-radius:3px 3px 0 0; }
.plugin-list-head span:last-child { text-align:right; }.plugin-grid { border:1px solid var(--border); border-top:0; border-radius:0 0 3px 3px; background:var(--bg-1); }
.plugin-card { padding:16px; border-bottom:1px solid color-mix(in srgb,var(--border) 65%,transparent); align-items:start; }.plugin-card:last-child { border-bottom:0; }.plugin-card:hover { background:color-mix(in srgb,var(--bg-2) 45%,var(--bg-1)); }
.plugin-card-main { min-width:0; overflow-wrap:anywhere; display:grid; grid-template-columns:minmax(180px,.8fr) minmax(0,1fr); gap:0 24px; }.plugin-card-main>.dependency-line,.plugin-details { grid-column:1/-1; }.plugin-title-row { display:flex; align-items:baseline; flex-wrap:wrap; gap:10px; }.plugin-title-row h3 { font-size:14px; font-weight:600; }.plugin-version { font-size:12px; color:var(--text-2); }
.plugin-meta { display:flex; flex-wrap:wrap; gap:5px 12px; margin-top:5px; color:var(--text-2); font-size:12px; }.plugin-meta code { color:var(--text-2); font-family:var(--mono); }
.plugin-description { margin-top:0; align-self:center; color:var(--text-1); font-size:13px; line-height:1.6; }
.plugin-status { display:flex; flex-direction:column; align-items:flex-start; gap:7px; padding-top:2px; }.plugin-status .tag { padding:0; background:none; border:0; font-size:12px; color:var(--text-1); }.plugin-status .tag:first-child::before { content:''; display:inline-block; width:5px; height:5px; margin-right:7px; border-radius:50%; background:currentColor; }.plugin-status .ok,.plugin-status .run { color:var(--ok); }.plugin-status .info,.plugin-status .warn { color:var(--warn); }.plugin-status .err { color:var(--danger); }
.plugin-card-actions { display:flex; gap:6px; justify-content:flex-end; flex-wrap:wrap; }.plugin-card-actions .btn { min-width:66px; justify-content:center; }
.plugin-details { margin-top:8px; font-size:12px; }.plugin-details summary { display:inline-flex; gap:4px; align-items:center; cursor:pointer; list-style:none; color:var(--text-2); }.plugin-details summary:hover { color:var(--text-0); }.plugin-details summary::-webkit-details-marker { display:none; }.plugin-details summary .ui-icon { width:12px; height:12px; }.plugin-details[open] summary .ui-icon { transform:rotate(180deg); }
.plugin-facts { display:flex; flex-direction:column; gap:5px; margin-top:10px; color:var(--text-1); font-size:12px; line-height:1.6; }.version-switch-line { margin-top:7px; color:var(--text-2); }.plugin-remove-actions { display:flex; gap:8px; flex-wrap:wrap; margin-top:10px; border-top:1px solid var(--border); padding-top:8px; }
.dependency-line { margin-top:7px; font-size:12px; color:var(--text-1); line-height:1.6; }.danger-text { color:var(--danger); }.warn-text { color:var(--warn); }.muted { color:var(--text-2); font-size:12px; }
.plugin-center-loading,.plugin-empty { padding:48px 16px; text-align:center; color:var(--text-2); font-size:13px; }
.plugin-alert,.plugin-result { margin-bottom:12px; padding:10px 12px; border-left:2px solid var(--border); background:var(--bg-1); font-size:12px; line-height:1.6; white-space:pre-line; }.plugin-alert.error { color:var(--danger); border-color:var(--danger); }.plugin-alert.info,.plugin-result-operation { color:var(--ok); }.plugin-result { border-color:var(--ok); }.result-warning { color:var(--warn); }.result-danger { color:var(--danger); }.plugin-operation-loading { margin-bottom:12px; font-size:12px; color:var(--text-1); }
@container (max-width:1150px) { .plugin-card-main { display:block; }.plugin-description { margin-top:7px; } }
@container (max-width:900px) { .plugin-list-head,.plugin-card { grid-template-columns:minmax(0,1fr) 130px 180px; gap:16px; } }
@container (max-width:650px) { .plugin-list-head { display:none; }.plugin-grid { border-top:1px solid var(--border); }.plugin-card { grid-template-columns:minmax(0,1fr) auto; gap:12px; padding:14px; }.plugin-card-main { grid-column:1/-1; }.plugin-status { flex-direction:row; flex-wrap:wrap; }.plugin-card-actions { align-self:end; }.plugin-search { flex:1; min-width:160px; }.url-row { flex-wrap:wrap; }.plugin-count { margin-left:0; } }
@media (max-width:700px) { .plugin-center { padding:12px; } }
</style>
