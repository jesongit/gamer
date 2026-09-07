// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { defineComponent, nextTick } from 'vue'
import PluginWorkspace from './workspace/PluginWorkspace.vue'

// 回归锁定（浏览器冒烟发现）：业务面板 key（gamer.<plugin>:<panel>）激活时
// 必须解析渲染面板组件本体，而不是退化到插件选择器兜底（activeTop 旧逻辑把
// 所有非 core key 兜底成 'plugins'，导致面板永不渲染，仅选择器可见）。

const VideoPanelStub = defineComponent({
  name: 'VideoPanelStub',
  template: '<div class="video-stub">VIDEO_PANEL_OK</div>',
})

function makeRegistry() {
  const panels = [
    { key: 'gamer.core:tasks', title: '任务', pluginId: 'gamer.core', runtime: 'core', component: defineComponent({ template: '<div>TASKS_OK</div>' }) },
    { key: 'gamer.core:logs', title: '日志', pluginId: 'gamer.core', runtime: 'core', component: defineComponent({ template: '<div>LOGS_OK</div>' }) },
    { key: 'gamer.core:settings', title: '设置', pluginId: 'gamer.core', runtime: 'core', component: defineComponent({ template: '<div>SETTINGS_OK</div>' }) },
    { key: 'gamer.video:video', title: '视频', pluginId: 'gamer.video', runtime: 'core', component: VideoPanelStub, panelClass: 'video-tab' },
  ]
  return {
    getPanels: () => panels,
    resolve: key => panels.find(p => p.key === key) || null,
    defaultPanel: () => panels[0],
  }
}

async function mountWorkspace(activePanel) {
  const wrapper = mount(PluginWorkspace, {
    props: { registry: makeRegistry(), activePanel },
    global: {
      stubs: {
        // PluginCenter 弹窗与 MarketView 不在本测试范围
        PluginCenter: { template: '<div/>' },
        MarketView: { template: '<div/>' },
        teleport: true,
      },
    },
  })
  await nextTick()
  return wrapper
}

describe('PluginWorkspace 业务面板激活（下拉二级菜单回归）', () => {
  it('业务面板 key 激活：渲染面板组件本体，不显示插件选择器兜底', async () => {
    const wrapper = await mountWorkspace('gamer.video:video')
    expect(wrapper.find('.plugin-picker').exists()).toBe(false)
    expect(wrapper.find('.video-stub').exists()).toBe(true)
    expect(wrapper.text()).toContain('VIDEO_PANEL_OK')
    // 二级导航出现且该面板高亮
    expect(wrapper.find('.plugin-subnav').exists()).toBe(true)
    expect(wrapper.find('.plugin-subnav-title').text()).toBe('gamer.video')
    const activeSub = wrapper.find('.workspace-subtab.active')
    expect(activeSub.exists()).toBe(true)
    expect(activeSub.text()).toBe('视频')
    // 主导航「插件」页签高亮（activeTab 映射回挂靠页签）
    const activeTop = wrapper.find('.workspace-tab.active')
    expect(activeTop.exists()).toBe(true)
    expect(activeTop.text()).toContain('插件')
    wrapper.unmount()
  })

  it('显式 plugins 视图：插件选择器列出已启用插件分组', async () => {
    const wrapper = await mountWorkspace('plugins')
    expect(wrapper.find('.plugin-picker').exists()).toBe(true)
    const cards = wrapper.findAll('.plugin-card')
    expect(cards.length).toBe(1)
    expect(cards[0].text()).toContain('gamer.video')
    wrapper.unmount()
  })

  it('core 面板路径不回归：任务面板照常渲染', async () => {
    const wrapper = await mountWorkspace('gamer.core:tasks')
    expect(wrapper.find('.plugin-picker').exists()).toBe(false)
    expect(wrapper.text()).toContain('TASKS_OK')
    wrapper.unmount()
  })
})
