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

function optionalId(value: unknown): string | null {
  const resolved = readValue(value as never)
  if (resolved === null || resolved === undefined) return null
  const id = String(resolved).trim()
  return id || null
}

function deviceSnapshot(value: unknown) {
  const device = readValue(value as never) as Record<string, unknown> | null | undefined
  if (!device) return null
  return { id: device.id || null, name: device.name || device.id || null, status: device.status || null, kind: device.kind || null, addr: device.addr || null }
}

function sizeSnapshot(value: unknown) {
  const size = readValue(value as never) as Record<string, unknown> | null | undefined
  if (!size || typeof size !== 'object') return null
  const width = Number(size.width)
  const height = Number(size.height)
  return {
    width: Number.isFinite(width) && width > 0 ? width : 0,
    height: Number.isFinite(height) && height > 0 ? height : 0,
  }
}

/** Stage 状态只读快照；动作仍由 DeviceStageBridge 提供，不混入设备/应用上下文。 */
function stageSnapshot(value: unknown) {
  const source = readValue(value as never) as Record<string, unknown> | null | undefined
  if (!source || typeof source !== 'object') return null
  const kind = String(readValue(source.kind as never) || '').trim()
  const sourceId = optionalId(source.sourceId)
  const generation = Number(readValue(source.generation as never))
  const readBoolean = (key: string) => {
    const current = readValue(source[key] as never)
    return current === undefined || current === null ? null : !!current
  }
  return {
    kind: kind === 'media' ? 'media' : kind === 'live' ? 'live' : null,
    sourceId,
    generation: Number.isFinite(generation) ? generation : 0,
    displaySize: sizeSnapshot(source.displaySize),
    referenceSize: sizeSnapshot(source.referenceSize),
    canDeviceInput: readBoolean('canDeviceInput'),
    stageReady: readBoolean('stageReady'),
    mediaId: optionalId(source.mediaId),
  }
}

export interface WorkspaceContextOptions {
  device?: unknown
  deviceId?: unknown
  /** Android 运行目标；只来自设备配置，不从 currentPackageId 推导。 */
  androidPackageName?: unknown
  /** Package 数据上下文；只用于资源/内容寻址。 */
  currentPackageId?: unknown
  /** 当前工作区查看的插件；不代表运行时调用方身份。 */
  activePluginId?: unknown
  connected?: unknown
  stage?: Partial<DeviceStageBridge>
  /** Stage 只读状态，和 stage 动作桥分开传入。 */
  stageContext?: unknown
  openPanel?: (panel: unknown) => unknown | Promise<unknown>
  toast?: (message: string, type?: string) => unknown | Promise<unknown>
  dialogConfirm?: (message: string, options?: unknown) => unknown | Promise<unknown>
  pluginCall?: (payload: unknown, meta: { pluginId: string; panelId: string }) => unknown | Promise<unknown>
  storage?: UiBridgeOptions['storage']
  core?: Record<string, unknown>
}

/** Host-owned context; `uiBridge` is the only part that crosses into iframe UI. */
export function createWorkspaceContext(options: WorkspaceContextOptions = {}) {
  const getSnapshot = () => {
    const androidPackageName = optionalId(options.androidPackageName)
    const currentPackageId = optionalId(options.currentPackageId)
    const activePluginId = optionalId(options.activePluginId)
    return {
      // 四个运行上下文保持平级且语义明确：Android 包名与 Package ID 永不互相兜底。
      device: deviceSnapshot(options.device),
      deviceId: optionalId(options.deviceId),
      app: {
        package: androidPackageName,
        packageName: androidPackageName,
        android_package: androidPackageName,
      },
      androidPackageName,
      package: {
        id: currentPackageId,
        content_package: currentPackageId,
      },
      currentPackageId,
      plugin: { id: activePluginId },
      activePluginId,
      connected: !!readValue(options.connected as never),
      stage: stageSnapshot(options.stageContext),
    }
  }
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
