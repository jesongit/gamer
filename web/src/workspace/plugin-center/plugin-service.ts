import type {
  ExtensionManagementResponse,
  InstalledPluginSnapshot,
  PluginDependencyRef,
  PluginInstallSource,
  PluginPermissionSet,
  PluginSignature,
  PluginSource,
  RegistryPluginVersion,
} from './types'

const SOURCE_METADATA_KEY = 'gamer.plugin-center.source.v1'
type PluginSourceMetadata = Record<string, {
  kind?: PluginSource
  label?: string
  publisher?: string
  signature?: PluginSignature
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
      ...(source.signature ? { signature: source.signature } : {}),
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

export function normalizeSignature(value: unknown, fallback: PluginSignature['status'] = 'unknown'): PluginSignature {
  if (!value || typeof value !== 'object') return { status: fallback }
  const item = value as Record<string, unknown>
  return {
    status: String(item.status || fallback),
    ...(item.key_id ? { key_id: String(item.key_id) } : {}),
    ...(item.algorithm ? { algorithm: String(item.algorithm) } : {}),
    ...(item.verified_at ? { verified_at: String(item.verified_at) } : {}),
    ...(item.value ? { value: String(item.value) } : {}),
    ...(item.signature ? { signature: String(item.signature) } : {}),
  }
}

/** Build the signed Registry claim sent to the server-side install gate. */
/**
 * Build the signed Registry claim sent to the server-side install gate.
 *
 * 两种签名形态（与 signer 产物对齐）：
 * - `value`：signer 输出的完整 `base64(RegistryProof JSON)` 信封——原样透传
 *   字符串即可（api.js 对 string 不再包装；服务端 `from_base64` 直接吃这个形态）。
 * - 仅 `signature`（纯 64 字节签名 base64）：返回对象，由 api.js 序列化打包。
 */
export function registryProofFor(
  entry?: RegistryPluginVersion,
): string | {
  id: string
  version: string
  download_url: string
  sha256: string
  key_id: string
  signature: string
} | null {
  const signature = normalizeSignature(entry?.signature)
  if (!entry?.id || !entry.version || !entry.download_url || !entry.sha256 || !signature.key_id) {
    return null
  }
  if (signature.value) return signature.value
  if (!signature.signature) return null
  return {
    id: entry.id,
    version: entry.version,
    download_url: entry.download_url,
    sha256: entry.sha256.toLowerCase(),
    key_id: signature.key_id,
    signature: signature.signature,
  }
}

export function signatureLabel(signature: unknown): string {
  switch (normalizeSignature(signature).status) {
    case 'valid': return '已签名 · 已验证'
    case 'unverified': return '已签名 · 未验证'
    case 'unsigned': return '未签名'
    case 'invalid': return '签名无效'
    default: return '签名状态未知'
  }
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
      signature: item.signature?.status && item.signature.status !== 'unknown'
        ? item.signature
        : remembered.signature || market?.signature || { status: 'unknown' },
      dependencies: item.dependencies?.length ? item.dependencies : dependencyRefsFor(market),
      dependent,
    }
  })
}

export function installPolicy(source: PluginInstallSource, signature: unknown = source.signature) {
  const normalized = normalizeSignature(signature, source.kind === 'local' || source.kind === 'url' ? 'unsigned' : 'unknown')
  const official = source.kind === 'official'
  return {
    allowed: !(official && normalized.status !== 'valid'),
    requiresWarning: normalized.status !== 'valid' || source.kind !== 'official',
    signature: normalized,
    warning: official && normalized.status !== 'valid'
      ? '官方插件签名未通过验证，已阻止安装。'
      : normalized.status !== 'valid'
        ? '来源未知或未签名。安装前请确认插件发布者与请求权限。'
        : '',
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
