export interface DeviceStageBridge {
  selectRegion: (options?: unknown) => unknown | Promise<unknown>
  pickPoint: (options?: unknown) => unknown | Promise<unknown>
  overlay: { show: (overlay: unknown) => unknown | Promise<unknown>; clear: (id?: unknown) => unknown | Promise<unknown> }
}

/** Minimal interaction contract; implementations remain inside DeviceStage. */
export function createDeviceStageBridge(options: Partial<DeviceStageBridge> = {}): DeviceStageBridge {
  const selectRegion = options.selectRegion || (() => Promise.resolve(null))
  const pickPoint = options.pickPoint || (() => Promise.resolve(null))
  const show = options.overlay?.show || (() => null)
  const clear = options.overlay?.clear || (() => null)
  return Object.freeze({ selectRegion, pickPoint, overlay: Object.freeze({ show, clear }) })
}
