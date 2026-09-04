// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'

// Console 壳挂载冒烟：phase-05 拆分后 Console.vue 只保留装配接线，
// 这里用 stub 依赖整体挂载一次，捕获 setup 阶段的引用错误/TDZ/组合顺序问题。
// （视图不连真机：api 全部 stub、不触发 WebRTC 连接。）

vi.mock('./api', () => {
  const listResponses = {
    listDevices: [],
    listScripts: [],
    listTemplates: [],
  }
  return {
    api: new Proxy({}, {
      get(_target, name) {
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

import { mount } from '@vue/test-utils'
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
      expect(wrapper.text()).toContain('🔌 连接')
      expect(wrapper.text()).toContain('启动应用')
      // DeviceStage 绑定来自各拆分模块：渲染后必须拿到结构化值（而非 undefined）
      const stage = wrapper.findComponent(ConsoleVideoStage)
      expect(stage.exists()).toBe(true)
      expect(stage.props('loupe')).toEqual({ show: false, x: 0, y: 0, zoom: 2.5 })
      expect(stage.props('keymapStatus')).toEqual({ name: '', inactive: false })
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
      // 默认面板 = gamer.core:tasks（裸 Core 兜底）
      expect(wrapper.text()).toContain('新建任务')
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
