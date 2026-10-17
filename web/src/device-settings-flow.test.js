// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { effectScope, reactive, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { useConsoleDeviceManager } from './components/console/useConsoleDeviceManager'
import DeviceSettingsModal from './components/console/DeviceSettingsModal.vue'
import { api } from './api'
vi.mock('./api', () => ({ api:{ updateDevice:vi.fn(), createDevice:vi.fn() } }))
let scope
afterEach(() => { scope?.stop(); vi.resetAllMocks(); vi.unstubAllGlobals() })
function setup() {
  const device = { id:'phone', name:'手机', addr:'emulator-5554', pkg:'com.example.game', screen_mode:'virtual', vd_res:'1920x1080', vd_dpi:0, fps:30 }
  const options = { toast:vi.fn(), store:reactive({ deviceId:'phone' }), devicesData:ref([device]), consoleRuntime:{ scanning:ref(false), reconnectTimer:ref(null), cancelReconnect:vi.fn() }, connected:ref(false), errorMsg:ref(''), appHintDismissed:ref(false), loadData:vi.fn(), connect:vi.fn(), cleanup:vi.fn(), sendControl:vi.fn(), selectTarget:vi.fn() }
  vi.stubGlobal('localStorage',{ setItem:vi.fn() })
  scope = effectScope()
  const manager = scope.run(() => useConsoleDeviceManager(options))
  return { ...options, manager }
}
describe('设备设置流程', () => {
  it('地址变更显示未保存状态，失败保留输入与应用目标并在表单显示原因', async () => {
    const ctx = setup()
    ctx.manager.openSettings()
    ctx.manager.form.addr = '192.168.1.8:5555'
    expect(ctx.manager.formDirty.value).toBe(true)
    api.updateDevice.mockRejectedValue(new Error('请先停止当前任务'))
    await ctx.manager.saveSettings()
    expect(api.updateDevice.mock.calls[0][1].pkg).toBe('com.example.game')
    expect(ctx.manager.settingsOpen.value).toBe(true)
    expect(ctx.manager.form.addr).toBe('192.168.1.8:5555')
    expect(ctx.manager.formError.value).toContain('请先停止当前任务')
    ctx.manager.cancelSettings()
    expect(ctx.manager.form.addr).toBe('emulator-5554')
  })
  it('新增设备只提交一次，内部选择新目标时不被保存状态拦截', async () => {
    const ctx = setup()
    ctx.manager.startAdd()
    ctx.manager.form.name = '模拟器二'
    ctx.manager.form.addr = 'emulator-5556'
    let complete
    api.createDevice.mockImplementation(() => new Promise(resolve => { complete = resolve }))
    ctx.selectTarget.mockImplementation(async id => {
      expect(ctx.manager.configApplying.value).toBe(false)
      ctx.store.deviceId = id
    })
    ctx.loadData.mockImplementation(async () => { ctx.devicesData.value.push({ id:'new', ...ctx.manager.form }) })
    const pending = ctx.manager.saveSettings()
    await ctx.manager.saveSettings()
    expect(api.createDevice).toHaveBeenCalledTimes(1)
    expect(ctx.manager.configApplying.value).toBe(true)
    complete({ id:'new' })
    await pending
    expect(ctx.selectTarget).toHaveBeenCalledWith('new')
    expect(ctx.manager.settingsOpen.value).toBe(false)
    expect(ctx.manager.configApplying.value).toBe(false)
  })
  it('镜像主屏保留帧率操作，屏幕模式使用原生单选控件', async () => {
    const ctx = setup()
    ctx.manager.openSettings()
    const wrapper = mount(DeviceSettingsModal,{ props:{ context:reactive(ctx.manager.deviceSettingsContext) } })
    await flushPromises()
    await wrapper.get('input[type="radio"][value="mirror"]').setValue()
    expect(ctx.manager.form.screen_mode).toBe('mirror')
    expect(wrapper.text()).toContain('帧率')
    expect(wrapper.find('[aria-label="虚拟屏分辨率"]').exists()).toBe(false)
    wrapper.unmount()
  })
})
