import type { PluginRegistryDocument, RegistryPluginVersion } from './types'

export const REGISTRY_SCHEMA_VERSION = 1
export const DEFAULT_REGISTRY_URL = '/registry.json'
export const MAX_PLUGIN_ARCHIVE_BYTES = 20 * 1024 * 1024

const VERSION_RE = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/
const SHA256_RE = /^[a-f0-9]{64}$/i

export class RegistryError extends Error {
  code: string
  constructor(code: string, message: string) {
    super(message)
    this.name = 'RegistryError'
    this.code = code
  }
}

function text(value: unknown, field: string, required = true): string {
  const output = String(value ?? '').trim()
  if (required && !output) throw new RegistryError('invalid_registry', `registry ${field} 不能为空`)
  return output
}

function normaliseSignature(value: unknown) {
  if (!value || typeof value !== 'object') return undefined
  const signature = value as Record<string, unknown>
  return {
    status: text(signature.status, 'signature.status'),
    ...(signature.key_id ? { key_id: String(signature.key_id) } : {}),
    ...(signature.algorithm ? { algorithm: String(signature.algorithm) } : {}),
    ...(signature.verified_at ? { verified_at: String(signature.verified_at) } : {}),
  }
}

export function normalizeRegistry(input: unknown): PluginRegistryDocument {
  if (!input || typeof input !== 'object') throw new RegistryError('invalid_registry', 'registry 必须是 JSON 对象')
  const raw = input as Record<string, unknown>
  const schemaVersion = Number(raw.schema_version ?? raw.version ?? 0)
  if (schemaVersion !== REGISTRY_SCHEMA_VERSION) {
    throw new RegistryError('unsupported_registry', `registry schema_version=${schemaVersion} 不受支持`)
  }
  const rawPlugins = Array.isArray(raw.plugins) ? raw.plugins : Array.isArray(raw.extensions) ? raw.extensions : null
  if (!rawPlugins) throw new RegistryError('invalid_registry', 'registry.plugins 必须是数组')
  const seen = new Set<string>()
  const plugins = rawPlugins.map((item, index) => {
    if (!item || typeof item !== 'object') throw new RegistryError('invalid_registry', `registry.plugins[${index}] 无效`)
    const value = item as Record<string, unknown>
    const id = text(value.id, `plugins[${index}].id`)
    const version = text(value.version, `plugins[${index}].version`)
    if (!VERSION_RE.test(version)) throw new RegistryError('invalid_registry', `插件 ${id} 版本不是 SemVer: ${version}`)
    const key = `${id}@${version}`
    if (seen.has(key)) throw new RegistryError('invalid_registry', `registry 存在重复版本: ${key}`)
    seen.add(key)
    const downloadUrl = text(value.download_url ?? value.archive_url, `plugins[${index}].download_url`)
    if (!isDownloadUrl(downloadUrl)) throw new RegistryError('invalid_registry', `插件 ${key} 下载地址必须是 http(s) 或同源绝对路径`)
    if (value.sha256 && !SHA256_RE.test(String(value.sha256))) {
      throw new RegistryError('invalid_registry', `插件 ${key} sha256 无效`)
    }
    return {
      id,
      version,
      name: text(value.name ?? id, `plugins[${index}].name`),
      ...(value.description ? { description: String(value.description) } : {}),
      ...(value.publisher ? { publisher: String(value.publisher) } : {}),
      source: 'official' as const,
      download_url: downloadUrl,
      ...(value.sha256 ? { sha256: String(value.sha256).toLowerCase() } : {}),
      ...(value.size !== undefined ? { size: Number(value.size) } : {}),
      ...(normaliseSignature(value.signature) ? { signature: normaliseSignature(value.signature) } : {}),
      ...(Array.isArray(value.permissions) ? { permissions: value.permissions.map(String) } : {}),
      ...(value.host_api && typeof value.host_api === 'object' ? { host_api: value.host_api as Record<string, string> } : {}),
      ...(Array.isArray(value.dependencies) ? { dependencies: value.dependencies as never } : {}),
      ...(Array.isArray(value.required_extensions) ? { required_extensions: value.required_extensions as never } : {}),
      ...(Array.isArray(value.app_packages) ? { app_packages: value.app_packages as never } : {}),
      ...(value.ui && typeof value.ui === 'object' ? { ui: value.ui as Record<string, unknown> } : {}),
    }
  })
  return {
    schema_version: REGISTRY_SCHEMA_VERSION,
    ...(raw.generated_at ? { generated_at: String(raw.generated_at) } : {}),
    ...(raw.host_api ? { host_api: String(raw.host_api) } : {}),
    plugins,
  }
}

export function isDownloadUrl(value: string): boolean {
  try {
    const url = new URL(value, globalThis.location?.origin || 'http://localhost')
    return (url.protocol === 'http:' || url.protocol === 'https:') && (value.startsWith('/') || /^https?:\/\//i.test(value))
  } catch {
    return false
  }
}

export async function fetchRegistry(
  fetchImpl: typeof fetch = globalThis.fetch,
  url = DEFAULT_REGISTRY_URL,
): Promise<PluginRegistryDocument> {
  let response: Response
  try {
    response = await fetchImpl(url, { headers: { Accept: 'application/json' } })
  } catch (error) {
    throw new RegistryError('registry_network_error', `插件市场不可用：${String((error as Error)?.message || error)}`)
  }
  if (!response.ok) throw new RegistryError('registry_http_error', `插件市场返回 HTTP ${response.status}`)
  let body: unknown
  try { body = await response.json() } catch { throw new RegistryError('invalid_registry', 'registry 不是有效 JSON') }
  return normalizeRegistry(body)
}

export function findRegistryPlugin(registry: PluginRegistryDocument, id: string, version?: string): RegistryPluginVersion | null {
  const candidates = registry.plugins.filter(item => item.id === id)
  if (version) return candidates.find(item => item.version === version) || null
  return candidates.slice().sort((a, b) => compareVersions(b.version, a.version))[0] || null
}

export function compareVersions(left: string, right: string): number {
  const parse = (value: string) => {
    const [core, pre = ''] = String(value).split('-', 2)
    return { core: core.split('.').map(Number), pre }
  }
  const a = parse(left)
  const b = parse(right)
  for (let index = 0; index < 3; index += 1) {
    if ((a.core[index] || 0) !== (b.core[index] || 0)) return (a.core[index] || 0) - (b.core[index] || 0)
  }
  if (!a.pre && b.pre) return 1
  if (a.pre && !b.pre) return -1
  return a.pre.localeCompare(b.pre)
}

async function archiveBytes(response: Response): Promise<Uint8Array> {
  const declared = Number(response.headers.get('content-length') || 0)
  if (declared > MAX_PLUGIN_ARCHIVE_BYTES) throw new RegistryError('archive_too_large', '插件归档超过 20 MiB 限制')
  const bytes = new Uint8Array(await response.arrayBuffer())
  if (bytes.byteLength === 0) throw new RegistryError('empty_archive', '插件归档不能为空')
  if (bytes.byteLength > MAX_PLUGIN_ARCHIVE_BYTES) throw new RegistryError('archive_too_large', '插件归档超过 20 MiB 限制')
  return bytes
}

export async function sha256Hex(bytes: ArrayBuffer | Uint8Array): Promise<string> {
  const subtle = globalThis.crypto?.subtle
  if (!subtle) throw new RegistryError('crypto_unavailable', '当前浏览器不支持 SHA-256 校验')
  const input = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes)
  const owned = new ArrayBuffer(input.byteLength)
  new Uint8Array(owned).set(input)
  const digest = await subtle.digest('SHA-256', owned)
  return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('')
}

export async function downloadFixedVersion(
  entry: RegistryPluginVersion,
  options: { fetchImpl?: typeof fetch; verifyHash?: boolean } = {},
) {
  if (!entry || !entry.id || !entry.version) throw new RegistryError('invalid_download', '固定版本元数据不完整')
  if (!isDownloadUrl(entry.download_url)) throw new RegistryError('invalid_download', '插件下载地址无效')
  if (entry.source === 'official' && !entry.sha256) {
    throw new RegistryError('missing_hash', '官方插件缺少固定版本 SHA-256，已阻止下载')
  }
  const fetchImpl = options.fetchImpl || globalThis.fetch
  let response: Response
  try { response = await fetchImpl(entry.download_url, { headers: { Accept: 'application/octet-stream' } }) } catch (error) {
    throw new RegistryError('download_network_error', `插件下载失败：${String((error as Error)?.message || error)}`)
  }
  if (!response.ok) throw new RegistryError('download_http_error', `插件下载返回 HTTP ${response.status}`)
  const bytes = await archiveBytes(response)
  const digest = await sha256Hex(bytes)
  if (options.verifyHash !== false && entry.sha256 && digest.toLowerCase() !== entry.sha256.toLowerCase()) {
    throw new RegistryError('hash_mismatch', `插件 ${entry.id}@${entry.version} SHA-256 校验失败`)
  }
  return { bytes, sha256: digest, file: new Blob([toOwnedBuffer(bytes)], { type: 'application/zip' }) }
}

export async function downloadDirectUrl(url: string, options: { fetchImpl?: typeof fetch } = {}) {
  if (!/^https?:\/\//i.test(String(url || '').trim())) {
    throw new RegistryError('invalid_download', 'URL 导入只允许 http(s) 地址')
  }
  const fetchImpl = options.fetchImpl || globalThis.fetch
  let response: Response
  try { response = await fetchImpl(url, { headers: { Accept: 'application/octet-stream' } }) } catch (error) {
    throw new RegistryError('download_network_error', `插件下载失败：${String((error as Error)?.message || error)}`)
  }
  if (!response.ok) throw new RegistryError('download_http_error', `插件下载返回 HTTP ${response.status}`)
  const bytes = await archiveBytes(response)
  return { bytes, sha256: await sha256Hex(bytes), file: new Blob([toOwnedBuffer(bytes)], { type: 'application/zip' }) }
}

function toOwnedBuffer(bytes: Uint8Array): ArrayBuffer {
  const owned = new ArrayBuffer(bytes.byteLength)
  new Uint8Array(owned).set(bytes)
  return owned
}

/** A contribution can only point at the authenticated local asset endpoint. */
export function isProductionRemoteUi(value: unknown): boolean {
  return typeof value === 'string' && /^https?:\/\//i.test(value)
}
