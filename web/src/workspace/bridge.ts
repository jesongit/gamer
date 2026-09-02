export const UI_BRIDGE_VERSION = 'gamer-ui@1'
export const BRIDGE_CONNECT_TYPE = 'gamer-ui:connect'
export const BRIDGE_REQUEST_TYPE = 'gamer-ui:request'
export const BRIDGE_RESPONSE_TYPE = 'gamer-ui:response'

export const UI_BRIDGE_METHODS = Object.freeze([
  'context.get', 'plugin.call', 'toast.show', 'dialog.confirm', 'workspace.openPanel',
  'video.selectRegion', 'video.pickPoint', 'video.showOverlay', 'video.clearOverlay',
  'overlay.show', 'overlay.clear', 'storage.get', 'storage.set',
])

export class UiBridgeError extends Error {
  code: string
  constructor(code: string, message: string) {
    super(message)
    this.name = 'UiBridgeError'
    this.code = code
  }
}

function messageFrom(payload: unknown): { message: string; type?: string } {
  if (typeof payload === 'string') return { message: payload }
  const item = payload && typeof payload === 'object' ? payload as { message?: unknown; type?: unknown } : {}
  return { message: String(item.message || ''), type: item.type ? String(item.type) : undefined }
}

export function createMemoryStorage() {
  const data = new Map<string, Map<string, unknown>>()
  const bucket = (pluginId: string) => {
    const key = String(pluginId || 'anonymous')
    if (!data.has(key)) data.set(key, new Map())
    return data.get(key)!
  }
  return {
    get(pluginId: string, key: string) { return bucket(pluginId).get(String(key)) },
    set(pluginId: string, key: string, value: unknown) { bucket(pluginId).set(String(key), value); return true },
    clear(pluginId?: string) { if (pluginId) data.delete(pluginId); else data.clear() },
  }
}

export interface UiBridgeOptions {
  getContext?: () => unknown
  pluginCall?: (payload: unknown, meta: { pluginId: string; panelId: string }) => unknown | Promise<unknown>
  selectRegion?: (options?: unknown) => unknown | Promise<unknown>
  pickPoint?: (options?: unknown) => unknown | Promise<unknown>
  showOverlay?: (overlay: unknown, meta: { pluginId: string; panelId: string }) => unknown | Promise<unknown>
  clearOverlay?: (id?: unknown, meta?: { pluginId: string; panelId: string }) => unknown | Promise<unknown>
  openPanel?: (panel: unknown) => unknown | Promise<unknown>
  toast?: (message: string, type?: string) => unknown | Promise<unknown>
  dialogConfirm?: (message: string, options?: unknown) => unknown | Promise<unknown>
  storage?: ReturnType<typeof createMemoryStorage>
}

/** Capability facade for sandbox UI; it never carries store, DOM, fetch, or REST objects. */
export function createUiBridge(options: UiBridgeOptions = {}) {
  const storage = options.storage || createMemoryStorage()
  const supported = new Set(UI_BRIDGE_METHODS)

  async function dispatch(method: string, payload: unknown = undefined, meta: { pluginId?: string; panelId?: string } = {}) {
    const name = String(method || '')
    if (!supported.has(name)) throw new UiBridgeError('method_not_allowed', `Unsupported UI Bridge method: ${name}`)
    const scope = { pluginId: String(meta.pluginId || 'anonymous'), panelId: String(meta.panelId || '') }
    switch (name) {
      case 'context.get': return options.getContext?.() || {}
      case 'plugin.call':
        if (!options.pluginCall) throw new UiBridgeError('plugin_call_unavailable', 'Plugin backend is not available')
        return options.pluginCall(payload, scope)
      case 'toast.show': {
        const item = messageFrom(payload)
        return options.toast?.(item.message, item.type || 'info')
      }
      case 'dialog.confirm': {
        const item = messageFrom(payload)
        return options.dialogConfirm?.(item.message, payload) ?? false
      }
      case 'workspace.openPanel':
        if (!options.openPanel) throw new UiBridgeError('workspace_unavailable', 'Workspace navigation is not available')
        return options.openPanel(typeof payload === 'object' && payload !== null
          ? (payload as { panel?: unknown; key?: unknown }).panel ?? (payload as { key?: unknown }).key
          : payload)
      case 'video.selectRegion':
        if (!options.selectRegion) throw new UiBridgeError('stage_unavailable', 'DeviceStage is not available')
        return options.selectRegion(payload)
      case 'video.pickPoint':
        if (!options.pickPoint) throw new UiBridgeError('stage_unavailable', 'DeviceStage is not available')
        return options.pickPoint(payload)
      case 'video.showOverlay':
      case 'overlay.show':
        if (!options.showOverlay) throw new UiBridgeError('stage_unavailable', 'Overlay is not available')
        return options.showOverlay(payload, scope)
      case 'video.clearOverlay':
      case 'overlay.clear':
        if (!options.clearOverlay) throw new UiBridgeError('stage_unavailable', 'Overlay is not available')
        return options.clearOverlay(payload, scope)
      case 'storage.get': {
        const key = String((payload as { key?: unknown } | undefined)?.key || '')
        if (!key) throw new UiBridgeError('invalid_request', 'storage.get requires key')
        return storage.get(scope.pluginId, key)
      }
      case 'storage.set': {
        const item = payload as { key?: unknown; value?: unknown } | undefined
        const key = String(item?.key || '')
        if (!key) throw new UiBridgeError('invalid_request', 'storage.set requires key')
        return storage.set(scope.pluginId, key, item?.value)
      }
      default: throw new UiBridgeError('method_not_allowed', `Unsupported UI Bridge method: ${name}`)
    }
  }

  const call = (method: string, payload?: unknown) => dispatch(method, payload)
  return Object.freeze({
    version: UI_BRIDGE_VERSION,
    methods: UI_BRIDGE_METHODS,
    dispatch,
    // Convenience namespaces are for host-side adapters/tests; iframe calls
    // still use the versioned MessageChannel request envelope.
    context: Object.freeze({ get: () => call('context.get') }),
    plugin: Object.freeze({ call: (payload?: unknown) => call('plugin.call', payload) }),
    toast: Object.freeze({ show: (payload?: unknown) => call('toast.show', payload) }),
    dialog: Object.freeze({ confirm: (payload?: unknown) => call('dialog.confirm', payload) }),
    workspace: Object.freeze({ openPanel: (payload?: unknown) => call('workspace.openPanel', payload) }),
    video: Object.freeze({
      selectRegion: (payload?: unknown) => call('video.selectRegion', payload),
      pickPoint: (payload?: unknown) => call('video.pickPoint', payload),
      showOverlay: (payload?: unknown) => call('video.showOverlay', payload),
      clearOverlay: (payload?: unknown) => call('video.clearOverlay', payload),
    }),
    overlay: Object.freeze({ show: (payload?: unknown) => call('overlay.show', payload), clear: (payload?: unknown) => call('overlay.clear', payload) }),
    storage: Object.freeze({ get: (payload?: unknown) => call('storage.get', payload), set: (payload?: unknown) => call('storage.set', payload) }),
  })
}

export function isBridgeRequest(message: unknown): message is { type: string; version: string; id: string; method: string; params?: unknown } {
  if (!message || typeof message !== 'object') return false
  const item = message as Record<string, unknown>
  return item.type === BRIDGE_REQUEST_TYPE && item.version === UI_BRIDGE_VERSION && typeof item.id === 'string' && typeof item.method === 'string'
}

export async function replyToBridgeRequest(
  request: { id: string; method: string; params?: unknown },
  bridge: ReturnType<typeof createUiBridge>,
  port: { postMessage: (message: unknown) => void },
  meta: { pluginId?: string; panelId?: string } = {},
) {
  try {
    const result = await bridge.dispatch(request.method, request.params, meta)
    port.postMessage({ type: BRIDGE_RESPONSE_TYPE, version: UI_BRIDGE_VERSION, id: request.id, ok: true, result })
  } catch (error) {
    const item = error as { code?: string; message?: string }
    port.postMessage({ type: BRIDGE_RESPONSE_TYPE, version: UI_BRIDGE_VERSION, id: request.id, ok: false, error: { code: item.code || 'bridge_error', message: item.message || String(error) } })
  }
}
