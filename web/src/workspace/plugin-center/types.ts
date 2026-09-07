export type PluginSource = 'official' | 'local' | 'url' | 'unknown'

/**
 * 后端执行类型（Phase 1 契约）：kind = "wasm"（受控 WASM guest）| "builtin"（宿主预置实现）。
 * registry v1 条目/缺失字段按 wasm 处理；builtin 需要宿主注册表支持（服务端 host_feature_unavailable 门禁）。
 */
export interface PluginExecution {
  kind: 'wasm' | 'builtin'
  /** 宿主版本要求（如 ">=1.3.0"）；缺失表示无额外要求，UI 不展示。 */
  host_version?: string
}

export interface PluginPermissionSet {
  added: string[]
  removed: string[]
  unchanged: string[]
}

export interface PluginDependencyRef {
  id: string
  version?: string
  kind?: 'extension' | 'app_package' | 'task' | 'workflow' | string
  name?: string
  state?: string
}

export interface RegistryPluginVersion {
  id: string
  version: string
  name: string
  description?: string
  publisher?: string
  source?: PluginSource
  download_url: string
  sha256?: string
  size?: number
  execution: PluginExecution
  permissions?: string[]
  host_api?: Record<string, string> | string
  dependencies?: PluginDependencyRef[]
  required_extensions?: PluginDependencyRef[]
  app_packages?: PluginDependencyRef[]
  ui?: Record<string, unknown>
}

export interface PluginRegistryDocument {
  schema_version: number
  generated_at?: string
  host_api?: string
  plugins: RegistryPluginVersion[]
}

export interface InstalledPluginSnapshot {
  id: string
  version?: string
  active_version?: string
  installed_versions?: string[]
  name?: string
  description?: string
  state: string
  last_error?: string | null
  permissions?: string[]
  host_api?: Record<string, string>
  ui?: Array<Record<string, unknown>>
  source?: PluginSource
  publisher?: string
  execution?: PluginExecution
  dependencies?: PluginDependencyRef[]
  dependent?: {
    app_packages?: PluginDependencyRef[]
    tasks?: PluginDependencyRef[]
    workflows?: PluginDependencyRef[]
  }
}

export interface ExtensionManagementResponse {
  schema_version?: number
  host_api?: string
  runtime_available?: boolean
  extensions: InstalledPluginSnapshot[]
  dependencies?: Record<string, InstalledPluginSnapshot['dependent']>
}

export interface PluginInspection {
  id: string
  version: string
  name?: string
  description?: string
  archive_sha256?: string
  source?: PluginSource
  publisher?: string
  execution?: PluginExecution
  permissions?: string[]
  host_api?: Record<string, string>
  ui?: Array<Record<string, unknown>>
  permission_diff?: PluginPermissionSet
  already_installed?: boolean
}

export interface PluginInstallSource {
  kind: Exclude<PluginSource, 'unknown'>
  label?: string
  publisher?: string
  execution?: PluginExecution
  registryEntry?: RegistryPluginVersion
}
