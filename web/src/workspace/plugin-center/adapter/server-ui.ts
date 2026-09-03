import type { PanelRegistry } from '../../registry'
import { registerServerUiContributions } from '../../contribution-manager'
import type { ServerUiContribution } from '../../contribution-manager'

export interface ServerUiListResponse {
  ui_contributions?: ServerUiContribution[]
  extensions?: unknown[]
  [key: string]: unknown
}

export interface ServerUiContributionAdapterOptions {
  load?: () => Promise<ServerUiListResponse>
  resolveEntry?: (pluginId: string, entry: string) => string
}

function defaultResolveEntry(pluginId: string, entry: string): string {
  const path = String(entry || '')
    .replace(/^ui\//, '')
    .split('/')
    .map(segment => encodeURIComponent(segment))
    .join('/')
  return `/api/extensions/${encodeURIComponent(pluginId)}/ui/${path}`
}

async function loadFromApi(): Promise<ServerUiListResponse> {
  const response = await fetch('/api/extensions', { headers: { Accept: 'application/json' } })
  if (!response.ok) throw new Error(`读取插件 UI 贡献失败：HTTP ${response.status}`)
  const value = await response.json()
  if (!value || typeof value !== 'object') throw new Error('插件 UI 响应无效')
  return value as ServerUiListResponse
}

/**
 * Owns only server-backed panel registrations. A successful refresh replaces
 * the previous set atomically from the registry consumer's perspective;
 * dispose is explicit so uninstall/disable and component unmount cannot
 * leave stale iframe panels behind.
 */
export function createServerUiContributionAdapter(
  registry: PanelRegistry,
  options: ServerUiContributionAdapterOptions = {},
) {
  let registration: ReturnType<typeof registerServerUiContributions> | null = null
  let generation = 0

  async function refresh(): Promise<ServerUiListResponse> {
    const requestGeneration = ++generation
    const response = await (options.load || loadFromApi)()
    if (requestGeneration !== generation) return response
    const contributions = Array.isArray(response.ui_contributions)
      ? response.ui_contributions
      : []
    const previous = registration
    registration = null
    previous?.dispose()
    try {
      registration = registerServerUiContributions(registry, contributions, {
        resolveEntry: (manifest, entry) => (options.resolveEntry || defaultResolveEntry)(manifest.id, entry),
      })
    } catch (error) {
      // The manager rolls back the partial registration. Keep the adapter
      // empty so a later refresh can recover deterministically.
      registration = null
      throw error
    }
    return response
  }

  function dispose() {
    generation += 1
    registration?.dispose()
    registration = null
  }

  return { refresh, dispose }
}
