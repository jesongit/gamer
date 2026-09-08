// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { defineComponent, nextTick } from 'vue'
import PluginWorkspace from './workspace/PluginWorkspace.vue'

// 回归锁定（浏览器冒烟发现）：业务面板 key（gamer.<plugin>:<panel>）激活时
// 必须解析渲染面板组件本体，而不是退化到插件选择器兜底（activeTop 旧逻辑把
// 所有非 core key 兜底成 'plugins'，导致面板永不渲染，仅选择器可见）。
// 导航契约：插件下拉列「插件本体」（显示名），选中插件后多功能插件在主导航
// 下一行渲染功能子页签条，单面板插件不显示子页签行。

const VideoPanelStub = defineComponent({
  name: 'VideoPanelStub',
  template: '<div class="video-stub">VIDEO_PANEL_OK</div>',
})
const AutomationPanelStub = defineComponent({
  name: 'AutomationPanelStub',
  template: '<div class="automation-stub">AUTOMATION_PANEL_OK</div>',
})
const TemplatesPanelStub = defineComponent({
  name: 'TemplatesPanelStub',
  template: '<div class="templates-stub">TEMPLATES_PANEL_OK</div>',
})

function makeRegistry() {
  const panels = [
    { key: 'gamer.core:tasks', title: '任务', pluginId: 'gamer.core', runtime: 'core', component: defineComponent({ template: '<div>TASKS_OK</div>' }) },
    { key: 'gamer.core:logs', title: '日志', pluginId: 'gamer.core', runtime: 'core', component: defineComponent({ template: '<div>LOGS_OK</div>' }) },
    { key: 'gamer.core:settings', title: '设置', pluginId: 'gamer.core', runtime: 'core', component: defineComponent({ template: '<div>SETTINGS_OK</div>' }) },
    { key: 'gamer.video:video', title: '视频', pluginId: 'gamer.video', runtime: 'core', component: VideoPanelStub, panelClass: 'video-tab' },
    { key: 'gamer.yaml:automation', title: '自动化', pluginId: 'gamer.yaml', runtime: 'core', component: AutomationPanelStub },
    { key: 'gamer.yaml:templates', title: '模板', pluginId: 'gamer.yaml', runtime: 'core', component: TemplatesPanelStub },
  ]
  return {
    getPanels: () => panels,
    resolve: key => panels.find(p => p.key === key) || null,
    defaultPanel: () => panels[0],
  }
}

const PLUGIN_NAMES = { 'gamer.video': '视频工作台', 'gamer.yaml': '自动化' }

async function mountWorkspace(activePanel, extraProps = {}) {
  const wrapper = mount(PluginWorkspace, {
    props: { registry: makeRegistry(), activePanel, pluginNames: PLUGIN_NAMES, ...extraProps },
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

/** 点开主导航「插件」下拉，返回菜单项按钮列表（重复调用会先收起已开的菜单）。 */
async function openPluginMenu(wrapper) {
  const mask = wrapper.find('.workspace-dd-mask')
  if (mask.exists()) {
    await mask.trigger('click')
    await nextTick()
  }
  const pluginTab = wrapper.findAll('.workspace-tab').find(node => node.text().includes('插件'))
  expect(pluginTab).toBeTruthy()
  await pluginTab.trigger('click')
  await nextTick()
  return wrapper.findAll('.workspace-dd-item')
}

describe('PluginWorkspace 业务面板激活（下拉二级菜单回归）', () => {
  it('业务面板 key 激活：渲染面板组件本体，不显示插件选择器兜底', async () => {
    const wrapper = await mountWorkspace('gamer.video:video')
    expect(wrapper.find('.plugin-picker').exists()).toBe(false)
    expect(wrapper.find('.video-stub').exists()).toBe(true)
    expect(wrapper.text()).toContain('VIDEO_PANEL_OK')
    // 主导航「插件」页签高亮（activeTab 映射回挂靠页签）
    const activeTop = wrapper.find('.workspace-tab.active')
    expect(activeTop.exists()).toBe(true)
    expect(activeTop.text()).toContain('插件')
    wrapper.unmount()
  })

  it('单面板插件：直接渲染面板本体，不显示功能子页签行', async () => {
    const wrapper = await mountWorkspace('gamer.video:video')
    expect(wrapper.find('.plugin-subnav-tabs').exists()).toBe(false)
    wrapper.unmount()
  })

  it('多面板插件激活：主导航下一行渲染功能子页签条且当前功能高亮', async () => {
    const wrapper = await mountWorkspace('gamer.yaml:templates')
    expect(wrapper.find('.plugin-picker').exists()).toBe(false)
    const subtabs = wrapper.findAll('.workspace-subtab')
    expect(subtabs.length).toBe(2)
    expect(subtabs.map(node => node.text())).toEqual(['自动化', '模板'])
    expect(wrapper.find('.workspace-subtab.active').text()).toBe('模板')
    expect(wrapper.find('.templates-stub').exists()).toBe(true)
    // 子页签可切换功能
    await subtabs[0].trigger('click')
    expect(wrapper.emitted('select')?.at(-1)).toEqual(['gamer.yaml:automation'])
    wrapper.unmount()
  })

  it('插件下拉列插件本体（显示名 + 功能清单 hint），选中即打开该插件面板', async () => {
    const wrapper = await mountWorkspace('gamer.core:tasks')
    const items = await openPluginMenu(wrapper)
    // 下拉项是插件而不是面板：显示名在前，hint 为功能清单
    expect(items.map(node => node.find('.workspace-dd-item-title').text())).toEqual(['视频工作台', '自动化'])
    expect(items[0].find('.workspace-dd-item-hint').text()).toBe('视频')
    expect(items[1].find('.workspace-dd-item-hint').text()).toBe('自动化 · 模板')
    // 点「自动化」插件 → 翻译为该插件第一个面板
    await items[1].trigger('click')
    expect(wrapper.emitted('select')?.at(-1)).toEqual(['gamer.yaml:automation'])
    wrapper.unmount()
  })

  it('再次从下拉选中同一插件：回到上次访问的面板', async () => {
    const wrapper = await mountWorkspace('gamer.core:tasks')
    let items = await openPluginMenu(wrapper)
    await items[1].trigger('click') // 自动化 → automation（第一个）
    await wrapper.setProps({ activePanel: 'gamer.yaml:automation' })
    // 插件内切到模板
    await wrapper.find('.workspace-subtab').trigger('click')
    await wrapper.setProps({ activePanel: 'gamer.yaml:templates' })
    // 切走再切回
    await wrapper.setProps({ activePanel: 'gamer.core:tasks' })
    items = await openPluginMenu(wrapper)
    await items[1].trigger('click')
    expect(wrapper.emitted('select')?.at(-1)).toEqual(['gamer.yaml:templates'])
    wrapper.unmount()
  })

  it('显式 plugins 视图：插件选择器列出已启用插件分组（显示名）', async () => {
    const wrapper = await mountWorkspace('plugins')
    expect(wrapper.find('.plugin-picker').exists()).toBe(true)
    const cards = wrapper.findAll('.plugin-card')
    expect(cards.length).toBe(2)
    expect(cards[0].text()).toContain('视频工作台')
    expect(cards[0].text()).toContain('gamer.video')
    wrapper.unmount()
  })

  it('core 面板路径不回归：任务面板照常渲染', async () => {
    const wrapper = await mountWorkspace('gamer.core:tasks')
    expect(wrapper.find('.plugin-picker').exists()).toBe(false)
    expect(wrapper.text()).toContain('TASKS_OK')
    expect(wrapper.find('.plugin-subnav-tabs').exists()).toBe(false)
    wrapper.unmount()
  })
})

// 插件 Android Targets（manifest [targets.android].packages，服务端把空声明
// 归一为 ['*']）：声明 `*`（或未声明）的插件恒显示；声明具体应用的插件只在
// 当前设备应用命中时出现在插件下拉，不命中（含未选择应用）直接不显示。
describe('PluginWorkspace 插件下拉按当前应用过滤（Android Targets）', () => {
  const TARGETS = {
    'gamer.video': ['*'],
    'gamer.yaml': ['com.miHoYo.hkrpg'],
  }

  it('通用（*）插件恒显示；具体目标插件当前应用命中才显示', async () => {
    const wrapper = await mountWorkspace('gamer.core:tasks', {
      pluginTargets: TARGETS,
      androidPackageName: 'com.miHoYo.hkrpg',
    })
    let items = await openPluginMenu(wrapper)
    expect(items.map(node => node.find('.workspace-dd-item-title').text()))
      .toEqual(['视频工作台', '自动化'])
    // 切换到不匹配的应用：仅 * 插件留在下拉
    await wrapper.setProps({ androidPackageName: 'com.other.game' })
    items = await openPluginMenu(wrapper)
    expect(items.map(node => node.find('.workspace-dd-item-title').text()))
      .toEqual(['视频工作台'])
    wrapper.unmount()
  })

  it('未选择应用：仅通用（*）插件显示，具体目标插件不出现', async () => {
    const wrapper = await mountWorkspace('gamer.core:tasks', {
      pluginTargets: TARGETS,
      androidPackageName: '',
    })
    const items = await openPluginMenu(wrapper)
    expect(items.map(node => node.find('.workspace-dd-item-title').text()))
      .toEqual(['视频工作台'])
    wrapper.unmount()
  })

  it('targets 数据未到达（未声明）：不误隐藏插件（fail-open）', async () => {
    const wrapper = await mountWorkspace('gamer.core:tasks')
    const items = await openPluginMenu(wrapper)
    expect(items.map(node => node.find('.workspace-dd-item-title').text()))
      .toEqual(['视频工作台', '自动化'])
    wrapper.unmount()
  })
})
