import { ref } from 'vue'

export const CONSOLE_RIGHT_LOCATION = 'console.right'
export const DEFAULT_PANEL_KEY = 'gamer.yaml:scripts'

export type PanelRuntime = 'core' | 'iframe' | 'declarative'
export type PanelKeepAlive = 'none' | 'session'

export interface PanelContribution {
  pluginId: string
  panelId: string
  title: string
  icon?: string
  order?: number
  location: typeof CONSOLE_RIGHT_LOCATION | string
  runtime: PanelRuntime
  requiresDevice?: boolean
  preferredWidth?: number
  keepAlive?: PanelKeepAlive
  aliases?: string[]
  component?: unknown
  panelClass?: string
  iframe?: { src?: string; title?: string }
  getProps?: (context: Record<string, unknown>) => Record<string, unknown>
}

export interface RegisteredPanel extends PanelContribution {
  key: string
  sequence: number
}

export function makePanelKey(pluginId: string, panelId: string): string {
  return `${String(pluginId || '').trim()}:${String(panelId || '').trim()}`
}

export function parsePanelKey(value: unknown): { pluginId: string; panelId: string } | null {
  const key = String(value || '').trim()
  const separator = key.lastIndexOf(':')
  if (separator <= 0 || separator === key.length - 1) return null
  return { pluginId: key.slice(0, separator), panelId: key.slice(separator + 1) }
}

function normaliseContribution(input: PanelContribution, sequence: number): RegisteredPanel {
  const pluginId = String(input?.pluginId || '').trim()
  const panelId = String(input?.panelId || '').trim()
  const title = String(input?.title || '').trim()
  if (!pluginId || !panelId || !title) throw new Error('PanelContribution requires pluginId, panelId and title')
  if (input.location !== CONSOLE_RIGHT_LOCATION) throw new Error(`Unsupported panel location: ${String(input.location)}`)
  if (!['core', 'iframe', 'declarative'].includes(input.runtime)) throw new Error(`Unsupported panel runtime: ${String(input.runtime)}`)
  const key = makePanelKey(pluginId, panelId)
  return Object.freeze({
    ...input,
    pluginId,
    panelId,
    title,
    key,
    order: Number.isFinite(input.order) ? Number(input.order) : 1000,
    keepAlive: input.keepAlive || (input.runtime === 'iframe' ? 'session' : 'none'),
    aliases: Array.isArray(input.aliases) ? [...new Set(input.aliases.map(String))] : [],
    sequence,
  })
}

/** Registry owns only the contribution index; routes and device state stay outside. */
export class PanelRegistry {
  readonly revision = ref(0)
  private readonly entries = new Map<string, RegisteredPanel>()
  private readonly aliases = new Map<string, string>()
  private sequence = 0
  private readonly configuredDefault: string

  constructor(options: { defaultPanelKey?: string } = {}) {
    this.configuredDefault = String(options.defaultPanelKey || DEFAULT_PANEL_KEY)
  }

  register(input: PanelContribution): () => void {
    const next = normaliseContribution(input, this.sequence++)
    const previous = this.entries.get(next.key)
    if (previous) this.removeAliases(previous)
    this.entries.set(next.key, next)
    for (const alias of next.aliases || []) this.aliases.set(alias, next.key)
    this.revision.value += 1
    return () => this.unregister(next.key)
  }

  unregister(panel: string | { pluginId?: string; panelId?: string; key?: string }): boolean {
    const key = this.keyFor(panel)
    const current = key ? this.entries.get(key) : null
    if (!current) return false
    this.removeAliases(current)
    this.entries.delete(current.key)
    this.revision.value += 1
    return true
  }

  unregisterPlugin(pluginId: string): number {
    const keys = [...this.entries.values()].filter(entry => entry.pluginId === pluginId).map(entry => entry.key)
    keys.forEach(key => this.unregister(key))
    return keys.length
  }

  getPanels(): RegisteredPanel[] {
    void this.revision.value
    return [...this.entries.values()].sort((a, b) => ((a.order || 1000) - (b.order || 1000)) || (a.sequence - b.sequence) || a.key.localeCompare(b.key))
  }

  get(panel: string | { pluginId?: string; panelId?: string; key?: string }): RegisteredPanel | null {
    const key = this.keyFor(panel)
    return key ? this.entries.get(key) || null : null
  }

  resolve(panel: unknown): RegisteredPanel | null {
    const key = this.keyFor(panel)
    if (key) return this.entries.get(key) || this.entries.get(this.aliases.get(key) || '') || null
    return null
  }

  defaultPanel(): RegisteredPanel | null {
    return this.resolve(this.configuredDefault) || this.getPanels()[0] || null
  }

  has(panel: unknown): boolean { return !!this.resolve(panel) }

  keyFor(panel: unknown): string | null {
    if (typeof panel === 'string') return panel.trim() || null
    if (!panel || typeof panel !== 'object') return null
    const value = panel as { key?: unknown; pluginId?: unknown; panelId?: unknown }
    if (value.key) return String(value.key).trim() || null
    if (value.pluginId && value.panelId) return makePanelKey(String(value.pluginId), String(value.panelId))
    return null
  }

  private removeAliases(panel: RegisteredPanel) {
    for (const alias of panel.aliases || []) if (this.aliases.get(alias) === panel.key) this.aliases.delete(alias)
  }
}

export function createPanelRegistry(options: { defaultPanelKey?: string } = {}): PanelRegistry {
  return new PanelRegistry(options)
}
