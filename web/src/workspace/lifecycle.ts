export type KeepAlivePolicy = 'none' | 'session'

export interface PanelUiLifecycle {
  open(panelKey: string): void
  close(panelKey: string): void
  isOpen(panelKey: string): boolean
  snapshot(): string[]
}

export interface PluginRuntimeLifecycle {
  register(pluginId: string, runtime?: { start?: () => unknown; stop?: () => unknown }): void
  unregister(pluginId: string): void
  start(pluginId: string): Promise<unknown>
  stop(pluginId: string): Promise<unknown>
  state(pluginId: string): string
  snapshot(): Record<string, string>
}

/** UI mount state is intentionally independent from plugin runtime state. */
export function createPanelUiLifecycle(): PanelUiLifecycle {
  const openPanels = new Set<string>()
  return {
    open(panelKey) { if (panelKey) openPanels.add(panelKey) },
    close(panelKey) { openPanels.delete(panelKey) },
    isOpen(panelKey) { return openPanels.has(panelKey) },
    snapshot() { return [...openPanels] },
  }
}

/** Placeholder contract for a later WASM host; no Wasmtime/WASM is loaded here. */
export function createPluginRuntimeLifecycle(): PluginRuntimeLifecycle {
  const runtimes = new Map<string, { start?: () => unknown; stop?: () => unknown; state: string }>()
  return {
    register(pluginId, runtime = {}) {
      if (!pluginId) throw new Error('pluginId is required')
      runtimes.set(pluginId, { ...runtime, state: 'registered' })
    },
    unregister(pluginId) { runtimes.delete(pluginId) },
    async start(pluginId) {
      const runtime = runtimes.get(pluginId)
      if (!runtime) throw new Error(`Unknown plugin runtime: ${pluginId}`)
      const result = await runtime.start?.()
      runtime.state = 'running'
      return result
    },
    async stop(pluginId) {
      const runtime = runtimes.get(pluginId)
      if (!runtime) return undefined
      const result = await runtime.stop?.()
      runtime.state = 'stopped'
      return result
    },
    state(pluginId) { return runtimes.get(pluginId)?.state || 'missing' },
    snapshot() { return Object.fromEntries([...runtimes].map(([id, runtime]) => [id, runtime.state])) },
  }
}

export function createWorkspaceLifecycle() {
  return { ui: createPanelUiLifecycle(), runtime: createPluginRuntimeLifecycle() }
}
