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
