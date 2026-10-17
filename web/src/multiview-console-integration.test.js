// @vitest-environment happy-dom
import './test-plugin-modules'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
const { devices } = vi.hoisted(() => ({ devices: Array.from({ length: 9 }, (_, i) => ({ id: `browser-${i}`, name: `目标${i}`, status: 'offline' })) }))
vi.mock('./api', () => ({ api: new Proxy({}, {
  get(target, key) {
    if (target[key]) return target[key]
    const responses = {
      listDevices: devices, getInputControl: { phase: 'idle', manual_allowed: true, generation: 0 },
      listPackages: { packages: [{ id: 'daily' }, { id: 'other' }] },
      listExtensions: { extensions: [], ui_contributions: [] },
      deviceRun: { active: false }, listScripts: [], listTemplates: [], packageSources: [],
      getMedia: { id: 'clip', name: 'clip.mp4', width: 1920, height: 1080, duration_us: 1000000 },
    }
    return target[key] = vi.fn(async () => responses[key] ?? {})
  },
}) }))
vi.mock('vue-router', () => ({ useRoute: () => ({ path: '/console', query: {} }), useRouter: () => ({ push: async () => {}, replace: async () => {} }) }))
import Console from './views/Console.vue'
import ConsoleVideoStage from './components/console/ConsoleVideoStage.vue'
import { multiviewWorkspace as workspace } from './console/multiview-workspace'
import { api } from './api'
import { store, runRegistry, resetStoreRunState, applyRunRecord } from './store'
import { packageStore } from './package-store'
import { requestStageMedia } from './components/console/useConsoleStage'
class Socket {
  static OPEN = 1
  static instances = []
  readyState = 1; bufferedAmount = 0; sent = []
  constructor(url) { this.url = url; Socket.instances.push(this) }
  send(raw) { this.sent.push(JSON.parse(raw)) }
  close() { this.readyState = 3 }
  receive(value) { this.onmessage?.({ data: JSON.stringify(value) }) }
}
let wrapper
beforeEach(() => {
  vi.useFakeTimers(); vi.clearAllMocks(); vi.stubGlobal('WebSocket', Socket); Socket.instances = []
  // Global settings use a separate API client; never let navigation tests reach a real service.
  vi.stubGlobal('fetch', vi.fn(async () => ({ ok: false, status: 503, headers: { get: () => 'application/json' }, json: async () => ({ error: 'test service unavailable' }), text: async () => 'test service unavailable' })))
  localStorage.clear(); sessionStorage.clear(); store.deviceId = null; resetStoreRunState()
  runRegistry.byId = {}; runRegistry.activeByDevice = {}; runRegistry.last = null
  workspace.stopPolling()
  Object.assign(workspace.state, { targetIds: [], configs: {}, selectedTargetId: null, gridSize: 4, pending: {}, errors: {} })
  packageStore.loaded = false; packageStore.loading = false; packageStore.currentPackageId = null; packageStore.packages = []
  for (const device of devices) workspace.addTarget(device.id)
  workspace.updateConfig('browser-0', { packageId: 'daily' })
  workspace.updateConfig('browser-1', { packageId: 'other' })
})
afterEach(() => { wrapper?.unmount(); wrapper = null; workspace.stopPolling(); vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals() })
async function start() {
  wrapper = mount(Console, { attachTo: document.body, global: { stubs: { Teleport: true } } })
  await flushPromises(); await nextTick()
  return wrapper.findAll('.target-cell')
}
async function connectCell(cell, index) {
  await cell.find('.v-overlay button').trigger('click'); await flushPromises()
  const socket = Socket.instances.filter(s => new URL(s.url).pathname.endsWith(`/browser-${index}`)).at(-1)
  socket.receive({ type: 'frame', id: 1, jpeg: `AAA${index}`, width: 1920, height: 1080,
    preview_width: 640, preview_height: 360, stamp: { target: `browser-${index}`, epoch: 'epoch', revision: 1 } })
  await nextTick()
  const img = cell.find('img')
  Object.defineProperty(img.element, 'naturalWidth', { value: 640, configurable: true })
  Object.defineProperty(img.element, 'naturalHeight', { value: 360, configurable: true })
  img.element.getBoundingClientRect = () => ({ left: 0, top: 0, width: 960, height: 540 })
  await img.trigger('load'); await flushPromises()
  return socket
}

describe('multiview actual Console assembly', () => {
  it('mode and document visibility pause hidden transports and preserve a manually closed tile', async () => {
    const visibility = vi.spyOn(document, 'hidden', 'get').mockReturnValue(false)
    const cells = await start(), a = await connectCell(cells[0], 0), b = await connectCell(cells[1], 1)
    const c = await connectCell(cells[2], 2)
    await cells[2].find('[aria-label="目标2 的画面更多操作"]').trigger('click')
    await wrapper.findAll('[role="menuitem"]').find(button => button.text() === '关闭预览').trigger('click'); await flushPromises()
    expect(c.readyState).toBe(3)
    const mode = wrapper.find('select[aria-label="画面模式"]')
    await mode.setValue('1'); await flushPromises()
    expect(a.readyState).toBe(1); expect(b.readyState).toBe(3)
    expect(cells[1].find('img').exists()).toBe(false)
    expect(cells[2].find('img').exists()).toBe(false)
    await mode.setValue('4'); await flushPromises()
    expect(Socket.instances).toHaveLength(4)
    expect(Socket.instances.at(-1).url).toContain('browser-1?mode=grid4')
    visibility.mockReturnValue(true)
    document.dispatchEvent(new Event('visibilitychange')); await flushPromises()
    expect(Socket.instances.every(socket => socket.readyState === 3)).toBe(true)
    expect(wrapper.findAll('img[alt="浏览器目标画面"]')).toHaveLength(0)
    visibility.mockReturnValue(false)
    document.dispatchEvent(new Event('visibilitychange')); await flushPromises()
    expect(Socket.instances.filter(socket => socket.readyState === 1)).toHaveLength(2)
    expect(Socket.instances.filter(socket => socket.readyState === 1).some(socket => socket.url.includes('browser-2'))).toBe(false)
    expect(api.cancelRun).not.toHaveBeenCalled()
    visibility.mockRestore()
  })
  it('keeps the grid unselected and shows an operation target only after entering single mode', async () => {
    const cells = await start()
    const context = wrapper.find('.panel-target-context')
    expect(context.text()).toContain('多画面预览')
    expect(context.text()).toContain('点击画面进入单屏操作')
    expect(wrapper.findAll('.target-cell.selected')).toHaveLength(0)
    expect(wrapper.findAll('.target-name[aria-pressed]')).toHaveLength(0)
    await cells[1].find('.target-select-shield').trigger('click'); await flushPromises()
    expect(wrapper.find('.panel-target-context').text()).toContain('当前操作目标：目标1')
    expect(wrapper.find('.panel-target-context strong').attributes('title')).toBe('目标1 · browser-1')
    expect(store.deviceId).toBe('browser-1')
  })

  it('asks to select a picture when there is no operation target', async () => {
    workspace.state.targetIds = []; workspace.state.selectedTargetId = null; workspace.state.gridSize = 1
    api.listDevices.mockResolvedValueOnce([])
    await start()
    expect(store.deviceId).toBeNull()
    expect(wrapper.find('.panel-target-context').text()).toContain('当前操作目标：未选择目标')
    expect(wrapper.find('.panel-target-context').text()).toContain('请先选择目标')
  })

  it('does not imply that global logs or settings follow the selected target', async () => {
    await start()
    const selectTab = async label => {
      await wrapper.findAll('.workspace-tab').find(tab => tab.text() === label).trigger('click')
      await flushPromises()
    }
    await selectTab('运行记录')
    expect(wrapper.findAll('select[aria-label="当前配置包"]')).toHaveLength(1)
    expect(wrapper.find('.tb-device-group').exists()).toBe(false)
    expect(wrapper.find('.panel-target-context').exists()).toBe(false)
    expect(wrapper.find('select[aria-label="日志设备"]').element.value).toBe('')
    expect(api.listLogs).toHaveBeenCalledWith(null, null, 200)
    await selectTab('设置')
    expect(wrapper.findAll('select[aria-label="当前配置包"]')).toHaveLength(1)
    expect(wrapper.find('.panel-target-context').exists()).toBe(false)
    await selectTab('工作台')
    expect(wrapper.find('.panel-target-context').text()).toContain('多画面预览')
  })

  it('grid rejects all input and the picture click enters single mode without forwarding that click', async () => {
    const cells = await start()
    expect(store.deviceId).toBe('browser-0'); expect(packageStore.currentPackageId).toBe('daily')
    const a = await connectCell(cells[0], 0), b = await connectCell(cells[1], 1)
    await cells[0].find('img').trigger('mousedown', { clientX: 100, clientY: 100, button: 0 })
    await cells[0].find('img').trigger('mousemove', { clientX: 200, clientY: 100 })
    await cells[0].find('img').trigger('mouseup', { clientX: 200, clientY: 100, button: 0 })
    await cells[0].find('img').trigger('wheel', { clientX: 100, clientY: 100, deltaY: 40 })
    await wrapper.find('.stage').trigger('keydown', { key: 'w', code: 'KeyW' })
    await wrapper.find('.stage').trigger('keyup', { key: 'w', code: 'KeyW' })
    expect(a.sent.filter(message => ['pointer', 'scroll', 'key', 'text', 'input_event'].includes(message.type))).toEqual([])
    await cells[1].find('.target-select-shield').trigger('mousedown', { button: 0 })
    await cells[1].find('.target-select-shield').trigger('mouseup', { button: 0 })
    await cells[1].find('.target-select-shield').trigger('click')
    await flushPromises()
    expect(workspace.state.gridSize).toBe(1)
    expect(store.deviceId).toBe('browser-1'); expect(packageStore.currentPackageId).toBe('other')
    expect(a.sent.some(message => message.type === 'release')).toBe(true)
    expect(b.sent.filter(message => ['pointer', 'key'].includes(message.type))).toEqual([])
    await cells[1].find('img').trigger('mousedown', { clientX: 100, clientY: 100, button: 0 })
    expect(b.sent.at(-1)).toMatchObject({ type: 'pointer', action: 'down', x: 200, y: 200, stamp: { target: 'browser-1' } })
  })

  it('clicking the selected grid picture enters single mode and switching back releases held input', async () => {
    const cells = await start(), socket = await connectCell(cells[0], 0)
    await cells[0].find('.target-select-shield').trigger('click'); await flushPromises()
    expect(workspace.state.gridSize).toBe(1)
    expect(socket.sent.filter(message => ['pointer', 'key', 'input_event'].includes(message.type))).toEqual([])
    await cells[0].find('img').trigger('mousedown', { clientX: 100, clientY: 100, button: 0 })
    await wrapper.find('.stage').trigger('keydown', { key: 'w', code: 'KeyW' })
    expect(socket.sent.some(message => message.type === 'pointer' && message.action === 'down')).toBe(true)
    await wrapper.find('select[aria-label="画面模式"]').setValue('9'); await flushPromises()
    expect(socket.sent.some(message => message.type === 'release')).toBe(true)
    const startSent = socket.sent.length
    await wrapper.find('.stage').trigger('keyup', { key: 'w', code: 'KeyW' })
    await cells[0].find('img').trigger('mouseup', { clientX: 100, clientY: 100, button: 0 })
    expect(socket.sent.slice(startSent).filter(message => ['pointer', 'key', 'input_event'].includes(message.type))).toEqual([])
    expect(wrapper.find('.input-control-hint').text()).toContain('多画面仅预览')
  })

  it('device controls switch single/4/9 modes, show only relevant actions and preserve every preview element', async () => {
    const cells = await start()
    const elements = cells.map(cell => cell.find('video').element)
    expect(cells.filter(cell => cell.isVisible())).toHaveLength(4)
    const mode = wrapper.find('.tb-device-group select[aria-label="画面模式"]')
    expect(mode.exists()).toBe(true)
    expect(wrapper.find('.multiview-toolbar').exists()).toBe(false)
    await mode.setValue('9'); await flushPromises()
    expect(wrapper.findAll('.target-cell').filter(cell => cell.isVisible())).toHaveLength(9)
    expect(wrapper.find('.device-run-actions').text()).toContain('批量运行')
    expect(wrapper.find('.device-run-actions').text()).not.toContain('停止')
    await cells[0].find('button[title="单屏查看此目标"]').trigger('click'); await flushPromises()
    expect(wrapper.findAll('.target-cell').filter(cell => cell.isVisible())).toHaveLength(1)
    expect(mode.element.value).toBe('1')
    expect(wrapper.find('.device-run-actions').exists()).toBe(false)
    expect(wrapper.findAll('.target-heading')).toHaveLength(0)
    expect(api.run).not.toHaveBeenCalled(); expect(api.cancelRun).not.toHaveBeenCalled()
    await mode.setValue('9'); await flushPromises()
    expect(wrapper.find('.device-run-actions').text()).toContain('批量运行')
    await mode.setValue('4'); await flushPromises()
    expect(wrapper.findAll('.target-cell').map(cell => cell.find('video').element)).toEqual(elements)
    expect(wrapper.findAll('.target-cell').filter(cell => cell.isVisible())).toHaveLength(4)
    expect(api.cancelRun).not.toHaveBeenCalled()
    expect(api.run).not.toHaveBeenCalled()
  })

  it('polling after reload updates stop actions without changing layout or focus', async () => {
    const cells = await start()
    expect(wrapper.find('.device-run-actions').text()).not.toContain('批量停止')
    api.deviceRun.mockImplementation(async id => id === 'browser-1'
      ? { active: true, run: { run_id: 'recovered', device_id: id, state: 'running' } }
      : { active: false })
    await workspace.refreshRunStates(); await flushPromises()
    expect(wrapper.find('.device-run-actions').text()).toContain('批量停止')
    expect(cells[1].find('[aria-label="停止 目标1 的脚本"]').exists()).toBe(true)
    expect(store.deviceId).toBe('browser-0')
    applyRunRecord({ run_id: 'recovered', device_id: 'browser-1', state: 'cancelled' })
    await nextTick()
    expect(wrapper.find('.device-run-actions').text()).not.toContain('批量停止')
    expect(cells[1].find('[aria-label="停止 目标1 的脚本"]').exists()).toBe(false)
  })

  it('browser settings preserve native editing and release held mapped keys when focus enters the form', async () => {
    api.listExtensions.mockResolvedValueOnce({ extensions: [{ id: 'gamer-keymap', state: 'running' }], ui_contributions: [] })
    const cells = await start(), socket = await connectCell(cells[0], 0)
    await cells[0].find('.target-select-shield').trigger('click'); await flushPromises()
    const stage = wrapper.find('.stage')
    stage.element.focus()
    await stage.trigger('keydown', { key: 'w', code: 'KeyW' })
    expect(socket.sent.some(message => message.type === 'input_event' && message.event.code === 'KeyW' && message.event.type === 'key_down')).toBe(true)
    await wrapper.find('[aria-label="设备更多操作"]').trigger('click')
    await wrapper.findAll('button').find(button => button.text() === '浏览器设置').trigger('click')
    const input = wrapper.find('.browser-form input')
    input.element.focus(); await flushPromises()
    expect(socket.sent.some(message => message.type === 'input_event' && message.event.code === 'KeyW' && message.event.type === 'key_up')).toBe(true)
    const startSent = socket.sent.length
    for (const [key, code, ctrlKey] of [['Control', 'ControlLeft', true], ['a', 'KeyA', true], ['Backspace', 'Backspace', false], ['Delete', 'Delete', false], ['Tab', 'Tab', false]]) {
      const event = new KeyboardEvent('keydown', { key, code, ctrlKey, bubbles: true, cancelable: true })
      input.element.dispatchEvent(event)
      expect(event.defaultPrevented).toBe(false)
      await input.trigger('keyup', { key, code, ctrlKey })
    }
    expect(socket.sent.slice(startSent).filter(message => ['key', 'text', 'input_event'].includes(message.type))).toEqual([])
    await wrapper.find('.browser-form h3').trigger('click')
    expect(document.activeElement).not.toBe(stage.element)
    await input.trigger('keydown', { key: 'Escape', code: 'Escape' })
    expect(wrapper.find('.browser-form').exists()).toBe(false)
  })

  it('video workbench request is routed to selected tile only and switching target invalidates pending media lookup', async () => {
    const cells = await start()
    let finish
    api.getMedia.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    requestStageMedia('clip')
    await nextTick()
    let stages = wrapper.findAllComponents(ConsoleVideoStage)
    expect(stages[0].props('stage').kind).toBe('media')
    expect(stages.slice(1).every(stage => stage.props('stage') === null)).toBe(true)
    await cells[1].find('.target-select-shield').trigger('click'); await flushPromises()
    finish({ id: 'clip', width: 1920, height: 1080 })
    await flushPromises()
    stages = wrapper.findAllComponents(ConsoleVideoStage)
    expect(stages[0].props('stage')).toBeNull()
    expect(stages[1].props('stage').kind).toBe('live')
    expect(stages[1].props('stage').mediaId).toBe('')
    expect(wrapper.findAll('.media-stream')).toHaveLength(0)
  })
  it('same-package dirty source editor vetoes actual tile selection without losing its draft', async () => {
    workspace.updateConfig('browser-1', { packageId: 'daily' })
    const cells = await start()
    wrapper.vm.scriptPanel.sourceEditorDirty.value = true
    await cells[1].find('.target-select-shield').trigger('click'); await flushPromises()
    expect(store.deviceId).toBe('browser-0')
    expect(workspace.state.selectedTargetId).toBe('browser-0')
    expect(packageStore.currentPackageId).toBe('daily')
    expect(wrapper.vm.scriptPanel.sourceEditorDirty.value).toBe(true)
    wrapper.vm.scriptPanel.sourceEditorDirty.value = false
    await cells[1].find('.target-select-shield').trigger('click'); await flushPromises()
    expect(store.deviceId).toBe('browser-1')
  })

  it('same-package selection closes previous crop and a crop save blocks switching until it settles', async () => {
    workspace.updateConfig('browser-1', { packageId: 'daily' })
    const cells = await start()
    wrapper.vm.crop.active = true
    wrapper.vm.saving = true
    await cells[1].find('.target-select-shield').trigger('click'); await flushPromises()
    expect(store.deviceId).toBe('browser-0')
    expect(wrapper.vm.crop.active).toBe(true)
    wrapper.vm.saving = false
    await cells[1].find('.target-select-shield').trigger('click'); await flushPromises()
    expect(store.deviceId).toBe('browser-1')
    expect(wrapper.vm.crop.active).toBe(false)
  })

  it.each(['listDevices', 'listPackages', 'listExtensions'])('unmount during %s prevents late polling, listeners and auto-connect resurrection', async (method) => {
    let finish
    api[method].mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const polling = vi.spyOn(workspace, 'startPolling')
    const addWindowListener = vi.spyOn(window, 'addEventListener')
    const addDocumentListener = vi.spyOn(document, 'addEventListener')
    const registeredAfterUnmount = []
    try {
      await start()
      wrapper.unmount(); wrapper = null
      polling.mockClear(); addWindowListener.mockClear(); addDocumentListener.mockClear()
      const response = { listDevices: devices, listPackages: { packages: [{ id: 'daily' }, { id: 'other' }] }, listExtensions: { extensions: [], ui_contributions: [] } }
      finish(response[method])
      await flushPromises(); await nextTick(); await flushPromises()
      registeredAfterUnmount.push(...addWindowListener.mock.calls.filter(([name]) => [
        'keydown', 'beforeunload', 'gamer-before-update-reload', 'gamer-update-reload', 'gamer-service-restored', 'blur',
      ].includes(name)).map(([name, handler]) => ({ target: window, name, handler })))
      registeredAfterUnmount.push(...addDocumentListener.mock.calls.filter(([name]) => name === 'visibilitychange')
        .map(([name, handler]) => ({ target: document, name, handler })))
      expect(polling).not.toHaveBeenCalled()
      expect(registeredAfterUnmount).toEqual([])
      expect(api.connectBrowser).not.toHaveBeenCalled()
      expect(api.connectDevice).not.toHaveBeenCalled()
    } finally {
      for (const { target, name, handler } of registeredAfterUnmount) target.removeEventListener(name, handler)
      workspace.stopPolling()
      polling.mockRestore(); addWindowListener.mockRestore(); addDocumentListener.mockRestore()
    }
  })

  it('unmount during run recovery prevents the runner continuation from restarting intervals', async () => {
    let finish
    api.deviceRun.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const intervals = vi.spyOn(globalThis, 'setInterval')
    const addWindowListener = vi.spyOn(window, 'addEventListener')
    const addDocumentListener = vi.spyOn(document, 'addEventListener')
    const lateListeners = []
    try {
      await start()
      wrapper.unmount(); wrapper = null
      intervals.mockClear(); addWindowListener.mockClear(); addDocumentListener.mockClear()
      finish({ active: true, run: { run_id: 'restored-after-unmount', device_id: 'browser-0', state: 'running', runner_id: 'gamer-yaml', entrypoint: 'daily/main.yaml' } })
      await flushPromises(); await nextTick(); await flushPromises()
      lateListeners.push(...addWindowListener.mock.calls.map(([name, handler]) => ({ target: window, name, handler })))
      lateListeners.push(...addDocumentListener.mock.calls.map(([name, handler]) => ({ target: document, name, handler })))
      expect(intervals).not.toHaveBeenCalled()
      expect(lateListeners).toEqual([])
      expect(api.connectBrowser).not.toHaveBeenCalled()
    } finally {
      for (const { target, name, handler } of lateListeners) target.removeEventListener(name, handler)
      intervals.mockRestore(); addWindowListener.mockRestore(); addDocumentListener.mockRestore()
    }
  })

})
