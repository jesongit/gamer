import type {
  ExtensionManagementResponse,
  InstalledPluginSnapshot,
  PluginDependencyRef,
  PluginExecution,
  PluginInstallSource,
  PluginPermissionSet,
  PluginSource,
  RegistryPluginVersion,
} from './types'
import { compareVersions } from './registry-client'

const SOURCE_METADATA_KEY = 'gamer.plugin-center.source.v1'
type PluginSourceMetadata = Record<string, {
  kind?: PluginSource
  label?: string
  publisher?: string
}>

export function readPluginSourceMetadata(storage: Storage | undefined = globalThis.localStorage): PluginSourceMetadata {
  try {
    const parsed = JSON.parse(storage?.getItem(SOURCE_METADATA_KEY) || '{}')
    return parsed && typeof parsed === 'object' ? parsed : {}
  } catch {
    return {}
  }
}

export function rememberPluginSource(
  metadata: PluginSourceMetadata,
  id: string,
  version: string,
  source: PluginInstallSource,
): PluginSourceMetadata {
  const next = {
    ...metadata,
    [id + '@' + version]: {
      kind: source.kind,
      ...(source.label ? { label: source.label } : {}),
      ...(source.publisher ? { publisher: source.publisher } : {}),
    },
  }
  try { globalThis.localStorage?.setItem(SOURCE_METADATA_KEY, JSON.stringify(next)) } catch { /* 隐私模式下仅保留本次页面状态 */ }
  return next
}

export function permissionDiff(before: string[] = [], after: string[] = []): PluginPermissionSet {
  const oldSet = new Set(before.map(String))
  const newSet = new Set(after.map(String))
  return {
    added: [...newSet].filter(value => !oldSet.has(value)).sort(),
    removed: [...oldSet].filter(value => !newSet.has(value)).sort(),
    unchanged: [...newSet].filter(value => oldSet.has(value)).sort(),
  }
}

/**
 * 执行形态归一：显式 builtin 才按宿主预置处理；缺失/v1 条目/未知值一律 wasm
 * （与 registry-client 读端容错一致）。
 */
export function normalizeExecution(value: unknown): PluginExecution {
  const raw = value && typeof value === 'object' ? value as Record<string, unknown> : {}
  const kind = String(raw.kind || '').trim().toLowerCase()
  const hostVersion = String(raw.host_version || '').trim()
  return {
    kind: kind === 'builtin' ? 'builtin' : 'wasm',
    ...(hostVersion ? { host_version: hostVersion } : {}),
  }
}

/**
 * 执行形态变化提示行（Phase 8 §11.2 / Phase 4 遗留 D2）：inspect 响应携带
 * `execution_change:{from,to}`（wasm↔builtin 迁移）时给出明确警示；无变化或
 * 字段缺失返回空串（UI 不加行）。
 */
export function executionChangeDetail(change: { from?: unknown; to?: unknown } | null | undefined): string {
  const from = String(change?.from ?? '').trim()
  const to = String(change?.to ?? '').trim()
  if (!from || !to || from === to) return ''
  return `⚠️ 执行形态变化：${from} → ${to}（更新将改变插件的执行方式，请确认宿主支持目标形态）`
}

export function executionLabel(value: unknown): string {
  return normalizeExecution(value).kind === 'builtin'
    ? '宿主预置（需要 Gamer 宿主支持）'
    : 'WASM 插件'
}

/** 宿主版本要求展示文案；缺失返回空串（UI 不显示该行）。 */
export function hostVersionLabel(value: unknown): string {
  return normalizeExecution(value).host_version || ''
}

export function sourceLabel(source: PluginSource = 'unknown'): string {
  return { official: '官方市场', local: '本地导入', url: 'URL 导入', unknown: '未知来源' }[source]
}

export function dependencyRefsFor(entry?: RegistryPluginVersion | InstalledPluginSnapshot): PluginDependencyRef[] {
  if (!entry) return []
  const value = entry as RegistryPluginVersion & InstalledPluginSnapshot
  const groups: Array<[unknown, PluginDependencyRef['kind']]> = [
    [value.dependencies, 'extension'],
    [value.required_extensions, 'extension'],
    [value.app_packages, 'app_package'],
  ]
  return groups.flatMap(([items, kind]) => (Array.isArray(items) ? items : []).map(item => {
    if (typeof item === 'string') {
      const raw = item.trim()
      const separator = kind === 'extension' ? raw.lastIndexOf('@') : -1
      if (separator > 0 && separator < raw.length - 1) {
        return { id: raw.slice(0, separator), version: raw.slice(separator + 1), kind }
      }
      return { id: raw, kind }
    }
    const value = item && typeof item === 'object' ? { ...(item as PluginDependencyRef) } : { id: String(item) }
    return { ...value, kind: value.kind || kind }
  }))
}

export function dependencyStatus(
  required: PluginDependencyRef[] = [],
  installed: InstalledPluginSnapshot[] = [],
) {
  const active = new Map(installed.map(item => [item.id, item]))
  const missing: PluginDependencyRef[] = []
  const disabled: PluginDependencyRef[] = []
  for (const dependency of required) {
    const target = active.get(dependency.id)
    if (!target) missing.push(dependency)
    else if (!versionMatches(target.active_version || target.version, dependency.version)) {
      missing.push({ ...dependency, state: 'version ' + (target.active_version || target.version || 'unknown') })
    }
    else if (!['enabled', 'running'].includes(target.state)) disabled.push({ ...dependency, state: target.state })
  }
  return { ok: missing.length === 0 && disabled.length === 0, missing, disabled }
}

function versionMatches(actual: unknown, requirement: unknown) {
  if (!requirement) return true
  const current = String(actual || '')
  const requested = String(requirement).trim().replace(/^v/, '')
  if (!current || !requested) return false
  if (/^\d+$/.test(requested)) return current.split('.')[0] === requested
  if (/^\d+\.\d+$/.test(requested)) return current.split('-')[0].split('.').slice(0, 2).join('.') === requested
  if (/^\d+\.\d+\.\d+$/.test(requested)) return current === requested
  if (/^[~^]\d+(?:\.\d+)?(?:\.\d+)?$/.test(requested)) {
    const baseline = requested.slice(1).split('.').map(Number)
    const currentParts = current.split('-')[0].split('.').map(Number)
    if (currentParts[0] !== baseline[0]) return false
    if (baseline.length > 1 && currentParts[1] !== baseline[1]) return false
    if (baseline.length > 2 && currentParts[2] !== baseline[2]) return false
    return true
  }
  return current === requested
}

const COMPARABLE_VERSION_RE = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/

function pluginVersion(plugin: InstalledPluginSnapshot | null | undefined): string {
  return String(plugin?.active_version || plugin?.version || '').trim()
}

/**
 * Market 与已安装快照的唯一版本关系判断。
 *
 * registry-client 已经拒绝非法市场版本，但已安装目录可能来自较早的
 * 存量数据；无法可靠比较时必须显示为不兼容并阻止更新，不能猜测一次
 * 更新方向。宿主 builtin → WASM 也与服务端的执行形态门禁一致，归为
 * 不兼容；WASM → builtin 仍交给服务端确认/校验。
 */
export function marketVersionRelation(
  entry: Pick<RegistryPluginVersion, 'id' | 'version' | 'execution'> | null | undefined,
  installed: InstalledPluginSnapshot | null | undefined,
) {
  if (!installed) {
    return { kind: 'install', installedVersion: '', marketVersion: String(entry?.version || '') }
  }

  const installedVersion = pluginVersion(installed)
  const marketVersion = String(entry?.version || '').trim()
  if (
    !entry
    || entry.id !== installed.id
    || !COMPARABLE_VERSION_RE.test(installedVersion)
    || !COMPARABLE_VERSION_RE.test(marketVersion)
  ) {
    return {
      kind: 'incompatible',
      installedVersion,
      marketVersion,
      reason: '无法安全比较市场版本与已安装版本',
    }
  }

  const installedKind = installed.execution?.kind
  const marketKind = entry.execution?.kind
  if (installedKind === 'builtin' && marketKind === 'wasm') {
    return {
      kind: 'incompatible',
      installedVersion,
      marketVersion,
      reason: '宿主预置插件不能替换为 WASM 包',
    }
  }

  const order = compareVersions(marketVersion, installedVersion)
  if (order > 0) return { kind: 'update', installedVersion, marketVersion }
  if (order === 0) return { kind: 'latest', installedVersion, marketVersion }
  return { kind: 'newer_installed', installedVersion, marketVersion }
}

export function marketVersionLabel(relation: ReturnType<typeof marketVersionRelation>): string {
  switch (relation.kind) {
    case 'install': return '安装'
    case 'update': return `更新到 ${relation.marketVersion}`
    case 'latest': return '已是最新'
    case 'newer_installed': return '已安装更高版本'
    case 'incompatible': return '版本不兼容'
    default: return '不可用'
  }
}

function snapshotResponse(value: unknown): InstalledPluginSnapshot | null {
  return value && typeof value === 'object' && typeof (value as { id?: unknown }).id === 'string'
    ? value as InstalledPluginSnapshot
    : null
}

function runtimeResult(snapshot: InstalledPluginSnapshot | null) {
  const state = String(snapshot?.state || '').trim()
  const labels: Record<string, { tone: string; text: string }> = {
    running: { tone: 'success', text: '运行状态：运行中，已恢复。' },
    enabled: { tone: 'info', text: '运行状态：已启用，等待运行。' },
    disabled: { tone: 'warning', text: '运行状态：已停用，保持原状态。' },
    installed: { tone: 'warning', text: '运行状态：已安装，未启用。' },
    failed: { tone: 'danger', text: '运行状态：启动失败，请查看失败诊断后重试。' },
  }
  return labels[state] || {
    tone: 'warning',
    text: state ? `运行状态：${state}。` : '运行状态：服务端未返回状态。',
  }
}

/**
 * 将后端真实的更新快照 / 卸载 204 归一为两层结果：变更结果与运行态
 * 结果。更新不能只显示“成功”，卸载也不能要求 204 携带 JSON 快照。
 */
export function describeExtensionMutation(
  action: 'install' | 'update' | 'uninstall',
  response: unknown,
  options: { plugin?: InstalledPluginSnapshot | null; deleteData?: boolean } = {},
) {
  const plugin = options.plugin || null
  const snapshot = snapshotResponse(response)
  const name = snapshot?.name || plugin?.name || snapshot?.id || plugin?.id || '插件'
  if (action === 'uninstall') {
    return {
      operation: { tone: 'success', text: `${name} 已卸载。` },
      detail: options.deleteData ? '数据结果：插件用户数据已删除。' : '数据结果：插件用户数据已保留。',
      snapshot: null,
    }
  }

  const version = snapshot?.active_version || snapshot?.version || '未知版本'
  return {
    operation: {
      tone: 'success',
      text: `${name} 已${action === 'update' ? '更新到' : '安装'} v${version}。`,
    },
    detail: runtimeResult(snapshot),
    snapshot,
  }
}

export function mergeManagementResponse(
  response: ExtensionManagementResponse | null | undefined,
  registry: RegistryPluginVersion[] = [],
  metadata: PluginSourceMetadata = {},
): InstalledPluginSnapshot[] {
  const registryByKey = new Map(registry.map(item => [`${item.id}@${item.version}`, item]))
  return (Array.isArray(response?.extensions) ? response.extensions : []).map(item => {
    const version = item.active_version || item.version || ''
    const market = registryByKey.get(`${item.id}@${version}`)
    const remembered = metadata[`${item.id}@${version}`] || {}
    const dependent = response?.dependencies?.[item.id] || item.dependent
    return {
      ...item,
      source: item.source && item.source !== 'unknown' ? item.source : remembered.kind || market?.source || 'unknown',
      publisher: item.publisher || remembered.publisher || market?.publisher,
      execution: item.execution || market?.execution,
      // Management API 的 outer `dependencies` 字段是 dependents 映射；
      // 插件自身依赖若由 snapshot 明确返回，即使是空数组也必须保留。
      dependencies: Array.isArray(item.dependencies) ? item.dependencies : dependencyRefsFor(market),
      dependent,
    }
  })
}

/**
 * 安装策略（Phase 1 无强制签名单一模型）：任何来源都允许安装，签名/proof 不再
 * 参与门禁；完整性与兼容性由 SHA-256、manifest 校验、权限确认与服务端宿主注册表
 * 分别承担。非官方来源仅产生「请确认来源与权限」的提示性警告。
 */
export function installPolicy(source: PluginInstallSource) {
  const official = source.kind === 'official'
  return {
    allowed: true,
    requiresWarning: !official,
    warning: official ? '' : '来源非官方市场。安装前请确认插件发布者与请求权限。',
  }
}

/**
 * 安装/下载链路错误分型（计划 §4.3）：把机器错误码翻成可理解、可行动的人话提示；
 * 未识别的错误原样透传服务端 message。哈希错误明确「不污染已装版本」；
 * host_feature_unavailable 明确指向「升级 Gamer」而不是可反复重试的无意义报错。
 */
export function installErrorText(errorValue: unknown): string {
  const error = errorValue as { code?: unknown; status?: unknown; message?: unknown; name?: unknown } | null
  const message = String(error?.message || '')
  // registry-client 抛出的 RegistryError 文案已面向用户（含插件 id/版本与行动指引），直接采用
  if (error?.name === 'RegistryError' && message) return message
  const code = String(error?.code || '')
  switch (code) {
    case 'registry_network_error':
    case 'registry_http_error':
      return '插件市场暂时不可用，请检查网络后刷新重试。'
    case 'download_network_error':
      return '插件下载失败：网络错误或超时，请检查网络后重试。'
    case 'download_not_found':
      return '插件安装包在发布源不存在（404），该版本可能已下架。'
    case 'download_http_error':
      return `插件下载失败（HTTP ${String(error?.status ?? '')}），请稍后重试。`
    case 'hash_mismatch':
      return '下载文件与索引不一致（SHA-256 校验失败）：已拒绝安装，不污染已装版本。'
    case 'missing_hash':
      return '该市场条目缺少固定版本 SHA-256，无法校验完整性，已阻止下载。'
    case 'archive_too_large':
    case 'empty_archive':
      return '插件归档无效或超过 20 MiB 限制，已拒绝安装。'
    case 'invalid_registry':
    case 'unsupported_registry':
      return '插件市场索引无效或不受支持：请更新 Gamer 后重试。'
    case 'host_feature_unavailable':
      return '该插件需要更新的 Gamer 宿主支持：请升级 Gamer 到新版本后再安装，重试当前版本不会成功。'
    case 'permission_confirmation_required':
      return '插件权限变更需要先在安装确认框中勾选确认。'
    case 'already_installed':
      return '该版本已安装，无需重复安装。'
    default:
      return String(error?.message || errorValue || '操作失败')
  }
}

export function installSummary(
  source: PluginInstallSource,
  current: InstalledPluginSnapshot | undefined,
  requestedPermissions: string[] = source.registryEntry?.permissions || [],
) {
  const diff = permissionDiff(current?.permissions || [], requestedPermissions)
  const policy = installPolicy(source)
  return { diff, policy, isUpdate: !!current }
}

export function uninstallPrompt(plugin: InstalledPluginSnapshot, deleteData = false): string {
  const dependent = plugin.dependent || {}
  const blockers = [...(dependent.app_packages || []), ...(dependent.tasks || []), ...(dependent.workflows || [])]
  const dependencyText = blockers.length
    ? `\n仍被 ${blockers.length} 个 App Package/任务/Workflow 引用，卸载后相关任务可能挂起。`
    : ''
  return deleteData
    ? `确认卸载 ${plugin.name || plugin.id}@${plugin.active_version || plugin.version}，并删除该插件的用户数据？${dependencyText}`
    : `确认卸载 ${plugin.name || plugin.id}@${plugin.active_version || plugin.version}？用户数据将保留，可在之后重新安装时继续使用。${dependencyText}`
}

export function lifecyclePrompt(action: 'enable' | 'disable' | 'start' | 'stop', plugin: InstalledPluginSnapshot): string {
  const name = `${plugin.name || plugin.id}@${plugin.active_version || plugin.version || '未知版本'}`
  const verb = { enable: '启用', disable: '停用', start: '启动', stop: '停止' }[action]
  const message = {
    enable: '启用后，插件可被 Workspace 加载并响应其扩展点。',
    disable: '停用后，插件不会被加载；正在运行的插件会先停止。',
    start: '启动后，插件运行时可以访问声明的 Host 能力。',
    stop: '停止后，插件运行时将退出，但已安装文件和用户数据会保留。',
  }[action]
  return `确认${verb} ${name}？\n${message}`
}

/** 版本切换（含回滚到旧版本）确认文案：明确目标版本与运行前提。 */
export function activateVersionPrompt(plugin: InstalledPluginSnapshot, version: string): string {
  const name = plugin.name || plugin.id
  const current = plugin.active_version || plugin.version || '未知版本'
  return `确认将 ${name} 从 ${current} 切换到 ${version}？\n切换后插件以该版本重新加载；插件运行中需先停止才能切换。`
}

/** activate 失败的友好提示：409（插件 Running）/404（版本未安装）映射为行动指引，其余原样透传。 */
export function activateVersionErrorText(errorValue: unknown): string {
  const status = (errorValue as { status?: unknown } | null)?.status
  if (status === 409) return '插件正在运行，请先停止插件再切换版本。'
  if (status === 404) return '目标版本未安装，无法切换。'
  return String((errorValue as { message?: unknown } | null)?.message || errorValue || '操作失败')
}
