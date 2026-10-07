import { reactive } from 'vue'
import { api } from '../api'
import { runRegistry, applyRunRecord, beginCancel, devicesData } from '../store'
import { createMultiviewWorkspace } from './multiview-state.mjs'
let storage
try { storage = window.localStorage } catch {}
export const multiviewWorkspace = createMultiviewWorkspace({
  reactive, storage, api, registry: runRegistry, applyRunRecord, beginCancel,
  isOnline: id => {
    const device = devicesData.value.find(device => device.id === id)
    return !!device && device.status === 'online'
  },
})
export { createMultiviewWorkspace } from './multiview-state.mjs'
