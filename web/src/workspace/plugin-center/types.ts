export type PluginSource = 'official' | 'local' | 'url' | 'unknown'
export type SignatureStatus = 'valid' | 'unsigned' | 'invalid' | 'unknown' | 'unverified'

export interface PluginSignature {
  status: SignatureStatus | string
  key_id?: string
  algorithm?: string
  verified_at?: string
  /** Detached Ed25519 value for official Registry proof or package metadata. */
  value?: string
  signature?: string
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
  signature?: PluginSignature
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
  signature?: PluginSignature
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
  signature?: PluginSignature
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
  signature?: PluginSignature
  registryEntry?: RegistryPluginVersion
}
