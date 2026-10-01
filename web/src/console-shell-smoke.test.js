// @vitest-environment happy-dom
import './test-plugin-modules'
import { describe, expect, it, vi } from 'vitest'

// Console 壳挂载冒烟：phase-05 拆分后 Console.vue 只保留装配接线，
// 这里用 stub 依赖整体挂载一次，捕获 setup 阶段的引用错误/TDZ/组合顺序问题。
// （视图不连真机：api 全部 stub、不触发 WebRTC 连接。）

vi.mock('./api', () => {
  const listResponses = {
    listDevices: [],
    listScripts: [],
    listTemplates: [],
    packageSources: [],
  }
  return {
    api: new Proxy({}, {
      get(_target, name) {
        if (name === 'listDevices') return _target[name] ||= vi.fn().mockResolvedValue([])
        if (name === 'getInputControl') return _target[name] ||= vi.fn().mockResolvedValue({ phase: 'idle', owner: null, generation: 0, manual_allowed: true })
        if (name in listResponses) return vi.fn().mockResolvedValue(listResponses[name])
        if (name === 'listExtensions') return vi.fn().mockResolvedValue({ extensions: [], ui_contributions: [] })
        return vi.fn().mockResolvedValue({})
      },
    }),
  }
})

vi.mock('vue-router', () => ({
  useRoute: () => ({ path: '/console', query: {} }),
  useRouter: () => ({
    push: () => Promise.resolve(),
    replace: () => Promise.resolve(),
  }),
}))

import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { store } from './store'
import { api } from './api'
import Console from './views/Console.vue'
import ConsoleVideoStage from './components/console/ConsoleVideoStage.vue'

describe('Console 壳挂载冒烟（拆分后装配接线）', () => {
  it('无设备环境下可完整挂载：工具条、投屏占位与右侧 Workspace 页签就绪', async () => {
    vi.useFakeTimers()
    const warnings = []
    const warn = vi.spyOn(console, 'warn').mockImplementation((...args) => {
      warnings.push(args.map(String).join(' '))
    })
    let wrapper
    try {
      wrapper = mount(Console, {
        global: {
          stubs: {
            // 只挡住弹窗/iframe 子树；DeviceStage 真实渲染以覆盖壳里全部投屏绑定
            Teleport: true,
          },
        },
      })
      await vi.advanceTimersByTimeAsync(2100)
      expect(wrapper.exists()).toBe(true)
      expect(wrapper.text()).toContain('选择设备…')
      expect(wrapper.text()).toContain('连接')
      // 应用区 = 应用下拉（未配置时占位「未选择应用」）+ 读取 + 启动 + 停止应用；
      // 粘贴/按键等收进操控区的更多下拉（Teleport 在此已 stub，
      // 菜单内容不渲染，只断言两个触发按钮），菜单结构由 console-components 静态回归锁定
      expect(wrapper.find('button[title^="新增 / 设置"]').exists()).toBe(true)
      expect(wrapper.text()).toContain('未选择应用')
      expect(wrapper.text()).toContain('读取')
      expect(wrapper.text()).toContain('启动')
      expect(wrapper.find('[aria-label="更多投屏功能"]').exists()).toBe(true)
      const toolbar = wrapper.find('.toolbar')
      expect(toolbar.find('.keymap-select').exists()).toBe(false)
      expect(toolbar.find('.stage-source-btn').exists()).toBe(false)
      expect(toolbar.find('.stage-record-btn').exists()).toBe(false)
      expect(toolbar.text()).not.toContain('视频')
      expect(toolbar.text()).not.toContain('录制')
      expect(toolbar.text()).not.toContain('映射')
      // DeviceStage 绑定来自各拆分模块：渲染后必须拿到结构化值（而非 undefined）
      const stage = wrapper.findComponent(ConsoleVideoStage)
      expect(stage.exists()).toBe(true)
      expect(stage.props('loupe')).toEqual({ show: false, x: 0, y: 0, zoom: 2.5 })
      expect(stage.find('.keymap-status').exists()).toBe(false)
      expect(stage.props('bridgeOverlays')).toEqual([])
      expect(stage.props('scriptFx')).toEqual({
        tap: { show: false, x: 0, y: 0 },
        swipe: { show: false, x: 0, y: 0, w: 0, h: 0 },
        hit: { show: false, x: 0, y: 0, w: 0, h: 0, label: '', miss: false },
      })
      // 右侧 Workspace 由 PanelRegistry 驱动：裸 Core（listExtensions 空）
      // 只有 任务/日志/设置 三个自有页签（P11.5：业务面板全部 manifest 驱动）
      const tabTexts = wrapper.findAll('.workspace-tab').map(tab => tab.text())
      for (const title of ['任务', '日志', '设置']) {
        expect(tabTexts.some(text => text.includes(title))).toBe(true)
      }
      for (const gone of ['模板', '脚本', '映射']) {
        expect(tabTexts.some(text => text.includes(gone))).toBe(false)
      }
      expect(wrapper.findAll('.workspace-tab').map(tab => tab.text())).toEqual(['工作台', '任务', '日志', '配置包', '插件', '设置'])
      expect(wrapper.find('.workspace-tab.active').text()).toBe('工作台')
      expect(wrapper.find('.workspace-dd').exists()).toBe(false)
      expect(wrapper.text()).toContain('启用插件后')
    } finally {
      warn.mockRestore()
      vi.useRealTimers()
      wrapper?.unmount()
    }
    // setup/template 引用错误会以 Vue warn 形式出现（解析失败的绑定等）
    const fatal = warnings.filter(text => text.includes('is not defined') || text.includes('Properties that start with $'))
    expect(fatal).toEqual([])
  })
})

describe('配置市场挂载冒烟', () => {
  it('未添加仓库时显示空态和添加入口', async () => {
    const { default: MarketView } = await import('./workspace/MarketView.vue')
    const { flushPromises } = await import('@vue/test-utils')
    const wrapper = mount(MarketView)
    await flushPromises()
    expect(wrapper.text()).toContain('市场暂无可用配置包')
    expect(wrapper.text()).toContain('添加仓库')
    wrapper.unmount()
  })
})

describe('浏览器投屏的真实舞台输入接线', () => {
  it('刷新时恢复已有设备并完成初始化，不因 loadForm 缺失中断挂载', async () => {
    vi.useFakeTimers()
    api.listDevices.mockResolvedValue([{ id: 'browser-existing', name: '已保存浏览器', status: 'offline' }])
    localStorage.setItem('gb_device_id', 'browser-existing')
    let wrapper
    try {
      wrapper = mount(Console, { global: { stubs: { Teleport: true } } })
      await vi.advanceTimersByTimeAsync(2100)
      expect(store.deviceId).toBe('browser-existing')
      expect(wrapper.findComponent(ConsoleVideoStage).props('currentName')).toBe('已保存浏览器')
      expect(wrapper.find('.tb-browser-group').exists()).toBe(true)
    } finally {
      wrapper?.unmount()
      api.listDevices.mockResolvedValue([])
      store.deviceId = null
      localStorage.removeItem('gb_device_id')
      vi.useRealTimers()
    }
  })
  it('用图像尺寸换算坐标，内部转移焦点不中断按住，离开窗口释放按键', async () => {
    vi.useFakeTimers()
    let socket
    class Socket {
      static OPEN = 1
      readyState = 1
      bufferedAmount = 0
      sent = []
      constructor() { socket = this }
      send(data) { this.sent.push(JSON.parse(data)) }
      close() { this.readyState = 3 }
    }
    vi.stubGlobal('WebSocket', Socket)
    const wrapper = mount(Console, { attachTo: document.body, global: { stubs: { Teleport: true } } })
    try {
      await vi.advanceTimersByTimeAsync(2100)
      store.deviceId = 'browser-check'
      await nextTick()
      await wrapper.find('.tb-device-group .btn-primary').trigger('click')
      await flushPromises()
      const stamp = { target: 'browser-check', epoch: 'one', revision: 1 }
      socket.onmessage({ data: JSON.stringify({ type: 'frame', id: 1, jpeg: 'AA==', stamp }) })
      await nextTick()
      const img = wrapper.find('img[alt="浏览器目标画面"]')
      Object.defineProperty(img.element, 'naturalWidth', { value: 1280 })
      Object.defineProperty(img.element, 'naturalHeight', { value: 720 })
      img.element.getBoundingClientRect = () => ({ left: 10, top: 20, width: 640, height: 480 })
      await img.trigger('load')
      await flushPromises()
      expect(wrapper.findAll('.toolbar > .tb-row')).toHaveLength(2)
      expect(wrapper.find('.tb-operation-row select[aria-label="目标标签页"]').exists()).toBe(true)
      expect(wrapper.find('.browser-status-row').exists()).toBe(false)
      const stage = wrapper.find('.stage')
      await img.trigger('mousedown', { clientX: 110, clientY: 190, button: 0 })
      expect(socket.sent.at(-1)).toMatchObject({ type: 'pointer', action: 'down', x: 200, y: 220, stamp })
      await stage.trigger('focusout', { relatedTarget: wrapper.find('.player-stage').element })
      expect(socket.sent.at(-1).action).toBe('down')
      await img.trigger('mouseup', { clientX: 110, clientY: 190, button: 0 })
      expect(socket.sent.at(-1)).toMatchObject({ type: 'pointer', action: 'up', x: 200, y: 220 })
      await stage.trigger('keydown', { key: 'w' })
      expect(socket.sent.at(-1)).toMatchObject({ type: 'key', action: 'down', key: 'w' })
      window.dispatchEvent(new Event('blur'))
      expect(socket.sent.at(-1)).toEqual({ type: 'release' })
      await stage.trigger('focusout', { relatedTarget: document.body })
      expect(socket.sent.at(-1)).toEqual({ type: 'release' })
      // UI 输入锁必须覆盖浏览器直接 WS 的指针和键盘路径，而不只是工具条按钮。
      api.getInputControl.mockResolvedValue({ phase: 'pausing', owner: 'ai-session', generation: 1, manual_allowed: false })
      await vi.advanceTimersByTimeAsync(1200)
      expect(wrapper.find('.input-control-hint').text()).toContain('完成前不能人工操作')
      const beforeLockedInput = socket.sent.length
      await img.trigger('mousedown', { clientX: 110, clientY: 190, button: 0 })
      await stage.trigger('keydown', { key: 'w' })
      expect(socket.sent.length).toBe(beforeLockedInput)
      api.getInputControl.mockResolvedValue({ phase: 'paused', owner: 'ai-session', generation: 2, manual_allowed: true })
      await vi.advanceTimersByTimeAsync(1200)
      expect(wrapper.find('.input-control-hint').text()).toContain('可以人工操作')
      await img.trigger('mousedown', { clientX: 110, clientY: 190, button: 0 })
      expect(socket.sent.at(-1)).toMatchObject({ type: 'pointer', action: 'down', x: 200, y: 220 })
    } finally {
      wrapper.unmount()
      store.deviceId = null
      api.getInputControl.mockResolvedValue({ phase: 'idle', owner: null, generation: 0, manual_allowed: true })
      vi.unstubAllGlobals()
      vi.useRealTimers()
    }
  })
})
