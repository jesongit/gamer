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

describe('Market 页挂载冒烟（T5b：分区渲染 插件市场/配置市场 + Package 远端源）', () => {
  it('配置市场分区：registry.json 无 packages 段（现网形态）显示空态不抛错', async () => {
    const { default: MarketView } = await import('./workspace/MarketView.vue')
    const { flushPromises } = await import('@vue/test-utils')
    const originalFetch = globalThis.fetch
    globalThis.fetch = async (url) => {
      if (String(url).includes('registry.json')) {
        return {
          ok: true,
          status: 200,
          headers: { get: () => 'application/json' },
          json: async () => ({ schema_version: 1, plugins: [] }),
        }
      }
      return originalFetch(url)
    }
    const wrapper = mount(MarketView)
    try {
      await flushPromises()
      expect(wrapper.text()).toContain('发现配置')
      // api stub 返回空集：已装配置为空态提示
      expect(wrapper.text()).not.toContain('本地已装')
      // registry.json 无 packages 段 = 「远端源暂无配置」，不抛错不阻塞页面
      expect(wrapper.text()).toContain('市场暂无可用配置包')
    } finally {
      globalThis.fetch = originalFetch
      wrapper.unmount()
    }
  })

  it('配置市场分区：远端源含 packages 段时按 §21 字段渲染卡片并给出安装入口', async () => {
    const { default: MarketView } = await import('./workspace/MarketView.vue')
    const { flushPromises } = await import('@vue/test-utils')
    const originalFetch = globalThis.fetch
    globalThis.fetch = async (url) => {
      if (String(url).includes('registry.json')) {
        return {
          ok: true,
          status: 200,
          headers: { get: () => 'application/json' },
          json: async () => ({
            schema_version: 1,
            plugins: [],
            packages: [{
              id: 'official.hsr.daily',
              name: '星铁日常包',
              version: '1.2.0',
              download_url: '/packages/official.hsr.daily-1.2.0.gamerpkg',
              android_targets: ['com.MiHoYo.hkrpg'],
              required_plugins: ['gamer-yaml'],
              author: 'gamer.dev',
            }],
          }),
        }
      }
      return originalFetch(url)
    }
    const wrapper = mount(MarketView)
    try {
      await flushPromises()
      expect(wrapper.text()).toContain('星铁日常包')
      expect(wrapper.text()).toContain('official.hsr.daily')
      expect(wrapper.text()).toContain('v1.2.0')
      expect(wrapper.text()).toContain('com.MiHoYo.hkrpg')
      expect(wrapper.text()).toContain('gamer-yaml')
      expect(wrapper.text()).toContain('gamer.dev')
      expect(wrapper.text()).toContain('安装')
    } finally {
      globalThis.fetch = originalFetch
      wrapper.unmount()
    }
  })
})
