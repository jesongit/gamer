import { createUiBridge, type UiBridgeOptions } from './bridge'
import { createDeviceStageBridge, type DeviceStageBridge } from './stage-bridge'

export const WORKSPACE_CONTEXT_KEY = Symbol('gamer.workspace.context')
export const PANEL_REGISTRY_KEY = Symbol('gamer.workspace.panel-registry')
export const DEVICE_STAGE_BRIDGE_KEY = Symbol('gamer.device-stage.bridge')

function readValue<T>(value: T | { value?: T } | (() => T)): T {
  if (typeof value === 'function') return (value as () => T)()
  if (value && typeof value === 'object' && 'value' in value) return (value as { value?: T }).value as T
  return value as T
}

function deviceSnapshot(value: unknown) {
  const device = readValue(value as never) as Record<string, unknown> | null | undefined
  if (!device) return null
  return { id: device.id || null, name: device.name || device.id || null, status: device.status || null, kind: device.kind || null, addr: device.addr || null }
}

export interface WorkspaceContextOptions {
  device?: unknown
  deviceId?: unknown
  activePackage?: unknown
  connected?: unknown
  stage?: Partial<DeviceStageBridge>
  openPanel?: (panel: unknown) => unknown | Promise<unknown>
  toast?: (message: string, type?: string) => unknown | Promise<unknown>
  dialogConfirm?: (message: string, options?: unknown) => unknown | Promise<unknown>
  pluginCall?: (payload: unknown, meta: { pluginId: string; panelId: string }) => unknown | Promise<unknown>
  storage?: UiBridgeOptions['storage']
  core?: Record<string, unknown>
}

/** Host-owned context; `uiBridge` is the only part that crosses into iframe UI. */
export function createWorkspaceContext(options: WorkspaceContextOptions = {}) {
  const getSnapshot = () => ({
    device: deviceSnapshot(options.device),
    deviceId: readValue(options.deviceId as never) || null,
    connected: !!readValue(options.connected as never),
    app: { package: readValue(options.activePackage as never) || null },
  })
  const stage = createDeviceStageBridge(options.stage || {})
  const uiBridge = createUiBridge({
    getContext: getSnapshot, openPanel: options.openPanel, toast: options.toast,
    dialogConfirm: options.dialogConfirm, pluginCall: options.pluginCall,
    selectRegion: stage.selectRegion, pickPoint: stage.pickPoint,
    showOverlay: stage.overlay.show, clearOverlay: stage.overlay.clear, storage: options.storage,
  })
  return Object.freeze({
    version: uiBridge.version, getSnapshot, stage, uiBridge,
    // Core Vue prop factories only. This is never serialized or passed to iframe.
    core: options.core || {},
  })
}
