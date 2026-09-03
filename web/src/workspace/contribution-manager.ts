import type { DeclarativeUiSchema, PanelContribution, PanelRegistry, RegisteredPanel } from './registry'
import { CONSOLE_RIGHT_LOCATION } from './registry'
import type { PluginRuntimeLifecycle } from './lifecycle'

export interface ManifestUiContribution {
  panel_id: string
  title: string
  icon?: string
  order?: number
  location?: string
  runtime: 'iframe' | 'declarative'
  requires_device?: boolean
  preferred_width?: number
  entry?: string
  /** declarative 面板的表单 schema（服务端 manifest.rs 校验后原样透传） */
  schema?: DeclarativeUiSchema
}

export interface ExtensionUiManifest {
  id: string
  ui?: { contributions?: ManifestUiContribution[] }
}

/** JSON shape returned by GET /api/extensions/ui. */
export interface ServerUiContribution extends ManifestUiContribution {
  plugin_id: string
  version?: string
}

export interface ExtensionRuntime {
  start?: () => unknown
  stop?: () => unknown
}

export interface ManifestContributionOptions {
  runtime?: ExtensionRuntime
  resolveEntry?: (manifest: ExtensionUiManifest, entry: string) => string
  aliases?: (contribution: ManifestUiContribution) => string[]
}

export interface InstalledContribution {
  pluginId: string
  panels: RegisteredPanel[]
  start(): Promise<unknown>
  stop(): Promise<unknown>
  uninstall(): Promise<void>
}

export interface ContributionManager {
  install(manifest: ExtensionUiManifest, options?: ManifestContributionOptions): InstalledContribution
  start(pluginId: string): Promise<unknown>
  stop(pluginId: string): Promise<unknown>
  uninstall(pluginId: string): Promise<void>
}

function panelFromManifest(
  manifest: ExtensionUiManifest,
  item: ManifestUiContribution,
  options: ManifestContributionOptions,
): PanelContribution {
  const pluginId = String(manifest?.id || '').trim()
  const panelId = String(item?.panel_id || '').trim()
  const title = String(item?.title || '').trim()
  if (!pluginId || !panelId || !title) {
    throw new Error('manifest UI contribution requires id, panel_id and title')
  }
  const location = item.location || CONSOLE_RIGHT_LOCATION
  if (location !== CONSOLE_RIGHT_LOCATION) {
    throw new Error(`Unsupported panel location: ${location}`)
  }
  if (item.runtime === 'iframe' && !item.entry) {
    throw new Error(`iframe panel requires entry: ${pluginId}:${panelId}`)
  }
  const aliases = options.aliases?.(item) || []
  const contribution: PanelContribution = {
    pluginId,
    panelId,
    title,
    icon: item.icon,
    order: Number.isFinite(item.order) ? Number(item.order) : 1000,
    location,
    runtime: item.runtime,
    requiresDevice: item.requires_device === true,
    preferredWidth: item.preferred_width,
    keepAlive: item.runtime === 'iframe' ? 'session' : 'none',
    aliases: [...new Set(aliases.map(String).filter(Boolean))],
  }
  if (item.runtime === 'iframe') {
    const resolveEntry = options.resolveEntry || ((_, entry) => entry)
    contribution.iframe = {
      src: resolveEntry(manifest, item.entry!),
      title,
    }
  }
  if (item.schema) contribution.schema = item.schema
  return contribution
}

/** Convert the server manifest contract to the Workspace registry. */
export function manifestPanels(
  manifest: ExtensionUiManifest,
  options: ManifestContributionOptions = {},
): PanelContribution[] {
  return (manifest?.ui?.contributions || []).map(item => panelFromManifest(manifest, item, options))
}

/**
 * Register the currently visible server contributions as ordinary panels.
 * The returned disposer is UI-only; stopping/uninstalling the WASM runtime
 * remains a server lifecycle operation and never follows a tab close or a
 * registry refresh.
 */
export function registerServerUiContributions(
  registry: PanelRegistry,
  contributions: ServerUiContribution[] = [],
  options: Omit<ManifestContributionOptions, 'runtime'> = {},
) {
  const unregister: Array<() => void> = []
  const panels: RegisteredPanel[] = []
  try {
    for (const item of contributions) {
      const pluginId = String(item?.plugin_id || '').trim()
      if (!pluginId) throw new Error('server UI contribution requires plugin_id')
      const entries = manifestPanels({
        id: pluginId,
        ui: { contributions: [item] },
      }, options)
      for (const entry of entries) {
        unregister.push(registry.register(entry))
        panels.push(registry.get({ pluginId, panelId: entry.panelId })!)
      }
    }
  } catch (error) {
    unregister.reverse().forEach(remove => remove())
    throw error
  }
  return {
    panels,
    dispose() { unregister.reverse().forEach(remove => remove()) },
  }
}

/**
 * UI visibility and extension runtime have separate owners. Closing a panel
 * only unmounts UI; uninstall is the operation that stops and unregisters the
 * extension.
 */
export function createContributionManager(
  registry: PanelRegistry,
  lifecycle: { runtime: PluginRuntimeLifecycle },
): ContributionManager {
  const installed = new Map<string, InstalledContribution>()

  function install(manifest: ExtensionUiManifest, options: ManifestContributionOptions = {}) {
    const pluginId = String(manifest?.id || '').trim()
    if (!pluginId) throw new Error('manifest id is required')
    if (installed.has(pluginId)) throw new Error(`Extension already installed: ${pluginId}`)

    const entries = manifestPanels(manifest, options)
    lifecycle.runtime.register(pluginId, options.runtime || {})
    const unregister = [] as Array<() => void>
    try {
      for (const entry of entries) unregister.push(registry.register(entry))
    } catch (error) {
      unregister.reverse().forEach(remove => remove())
      lifecycle.runtime.unregister(pluginId)
      throw error
    }

    let removed = false
    const handle: InstalledContribution = {
      pluginId,
      panels: entries.map(entry => registry.get({ pluginId, panelId: entry.panelId })!).filter(Boolean),
      start: () => lifecycle.runtime.start(pluginId),
      stop: () => lifecycle.runtime.stop(pluginId),
      async uninstall() {
        if (removed) return
        removed = true
        let stopError: unknown
        try {
          await lifecycle.runtime.stop(pluginId)
        } catch (error) {
          // Always remove registrations, then surface the guest failure.
          stopError = error
        } finally {
          registry.unregisterPlugin(pluginId)
          lifecycle.runtime.unregister(pluginId)
          installed.delete(pluginId)
        }
        if (stopError) throw stopError
      },
    }
    installed.set(pluginId, handle)
    return handle
  }

  return {
    install,
    start: pluginId => lifecycle.runtime.start(pluginId),
    stop: pluginId => lifecycle.runtime.stop(pluginId),
    async uninstall(pluginId) {
      const handle = installed.get(pluginId)
      if (handle) await handle.uninstall()
      else {
        await lifecycle.runtime.stop(pluginId)
        registry.unregisterPlugin(pluginId)
        lifecycle.runtime.unregister(pluginId)
      }
    },
  }
}
