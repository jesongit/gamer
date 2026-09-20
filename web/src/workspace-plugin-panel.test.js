// @vitest-environment happy-dom
import { afterEach, describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { defineComponent, nextTick } from 'vue'
import PluginWorkspace from './workspace/PluginWorkspace.vue'

afterEach(() => localStorage.removeItem('gamer.workbench.tab-order'))

it('工作台页签拖动只改变顺序，重挂载保留，恢复默认不切换当前面板', async () => {
  let wrapper = await mountWorkspace('gamer-video:video')
  let tabs = wrapper.findAll('.workspace-subtab')
  await tabs[2].trigger('dragstart', { dataTransfer: { setData() {} } })
  await tabs[0].trigger('drop', { clientX: -1 })
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['模板', '视频', '自动化'])
  expect(wrapper.emitted('select')).toBeUndefined()
  wrapper.unmount()
  wrapper = await mountWorkspace('gamer-video:video')
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['模板', '视频', '自动化'])
  await wrapper.get('[aria-label="恢复默认页签顺序"]').trigger('click')
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['视频', '自动化', '模板'])
  expect(wrapper.find('.video-stub').exists()).toBe(true)
  wrapper.unmount()
})

it('键盘排序在隐藏插件重新出现后保留其位置', async () => {
  const wrapper = await mountWorkspace('gamer-video:video', { pluginTargets: { 'gamer-yaml': ['game.app'] }, androidPackageName: 'game.app' })
  await wrapper.findAll('.workspace-subtab')[0].trigger('keydown', { key: 'ArrowRight', altKey: true, shiftKey: true })
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['自动化', '视频', '模板'])
  await wrapper.setProps({ androidPackageName: 'other.app' })
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['视频'])
  await wrapper.setProps({ androidPackageName: 'game.app' })
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['自动化', '视频', '模板'])
  wrapper.unmount()
})

// 回归锁定（浏览器冒烟发现）：业务面板 key（gamer.<plugin>:<panel>）激活时
// 必须解析渲染面板组件本体，而不是退化到插件选择器兜底（activeTop 旧逻辑把
// 所有非 core key 兜底成 'plugins'，导致面板永不渲染，仅选择器可见）。
// 导航契约：插件入口列「插件本体」（显示名），选中插件后多功能插件在主导航
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
    { key: 'gamer-video:video', title: '视频', pluginId: 'gamer-video', runtime: 'core', component: VideoPanelStub, panelClass: 'video-tab' },
    { key: 'gamer-yaml:automation', title: '自动化', pluginId: 'gamer-yaml', runtime: 'core', component: AutomationPanelStub },
    { key: 'gamer-yaml:templates', title: '模板', pluginId: 'gamer-yaml', runtime: 'core', component: TemplatesPanelStub },
  ]
  return {
    getPanels: () => panels,
    resolve: key => panels.find(p => p.key === key) || null,
    defaultPanel: () => panels[0],
  }
}

const PLUGIN_NAMES = { 'gamer-video': '视频工作台', 'gamer-yaml': '自动化' }

async function mountWorkspace(activePanel, extraProps = {}) {
  const wrapper = mount(PluginWorkspace, {
    props: { registry: makeRegistry(), activePanel, pluginNames: PLUGIN_NAMES, ...extraProps },
    global: {
      stubs: {
        // PluginCenter 弹窗与 MarketView 不在本测试范围
        PluginCenter: { name: 'PluginCenter', template: '<div class="plugin-manager-stub">统一插件列表</div>' },
        MarketView: { name: 'MarketView', template: '<div class="market-stub">配置市场</div>' },
        PackageContextBar: { template: '<div class="package-bar-stub">配置包操作</div>' },
        teleport: true,
      },
    },
  })
  await nextTick()
  return wrapper
}

describe('PluginWorkspace 业务面板激活（入口二级菜单回归）', () => {
  it('业务面板 key 激活：渲染面板组件本体，不显示插件选择器兜底', async () => {
    const wrapper = await mountWorkspace('gamer-video:video')
    expect(wrapper.find('.plugin-picker').exists()).toBe(false)
    expect(wrapper.find('.video-stub').exists()).toBe(true)
    expect(wrapper.text()).toContain('VIDEO_PANEL_OK')
    // 主导航「插件」页签高亮（activeTab 映射回挂靠页签）
    const activeTop = wrapper.find('.workspace-tab.active')
    expect(activeTop.exists()).toBe(true)
    expect(activeTop.text()).toContain('工作台')
    wrapper.unmount()
  })

  it('工作台平铺所有可用功能，当前单面板插件也有切换入口', async () => {
    const wrapper = await mountWorkspace('gamer-video:video')
    expect(wrapper.find('.workspace-subtab.active').text()).toBe('视频')
    wrapper.unmount()
  })

  it('多面板插件激活：主导航下一行渲染功能子页签条且当前功能高亮', async () => {
    const wrapper = await mountWorkspace('gamer-yaml:templates')
    expect(wrapper.find('.plugin-picker').exists()).toBe(false)
    const subtabs = wrapper.findAll('.workspace-subtab')
    expect(subtabs.length).toBe(3)
    expect(subtabs.map(node => node.text())).toEqual(['视频', '自动化', '模板'])
    expect(wrapper.find('.workspace-subtab.active').text()).toBe('模板')
    expect(wrapper.find('.templates-stub').exists()).toBe(true)
    // 子页签可切换功能
    await subtabs[1].trigger('click')
    expect(wrapper.emitted('select')?.at(-1)).toEqual(['gamer-yaml:automation'])
    wrapper.unmount()
  })

  it('插件导航直接显示已安装管理，移除重复概览和加号入口', async () => {
    const wrapper = await mountWorkspace('gamer.core:tasks')
    await wrapper.findAll('.workspace-tab').find(node => node.text() === '插件').trigger('click')
    expect(wrapper.emitted('select')?.at(-1)).toEqual(['plugins'])
    await wrapper.setProps({ activePanel: 'plugins' })
    expect(wrapper.get('.plugin-manager-stub').text()).toContain('统一插件列表')
    expect(wrapper.find('.plugin-picker').exists()).toBe(false)
    expect(wrapper.find('.workspace-tab-add').exists()).toBe(false)
    wrapper.unmount()
  })

  it('仅六个主导航，配置包的管理和市场同页，无二级分类页签', async () => {
    const wrapper = await mountWorkspace('packages')
    expect(wrapper.findAll('.workspace-tab').map(tab => tab.text())).toEqual(['工作台', '任务', '日志', '配置包', '插件', '设置'])
    expect(wrapper.find('.package-installed').exists()).toBe(true)
    expect(wrapper.get('.market-stub').text()).toBe('配置市场')
    expect(wrapper.find('.workspace-dd').exists()).toBe(false)
    expect(wrapper.find('.plugin-subnav-tabs').exists()).toBe(false)
    wrapper.unmount()
  })

  it('首次进入工作台自动选择可用功能，无插件仍留在工作台', async () => {
    const wrapper = await mountWorkspace('workbench')
    expect(wrapper.emitted('fallback')?.at(-1)).toEqual(['gamer-video:video'])
    wrapper.unmount()
    const empty = await mountWorkspace('workbench', { registry: { getPanels: () => [], resolve: () => null } })
    expect(empty.find('.workspace-tab.active').text()).toBe('工作台')
    expect(empty.text()).toContain('启用插件后')
    expect(empty.emitted('fallback')).toBeUndefined()
    empty.unmount()
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
// 当前设备应用命中时出现在插件入口，不命中（含未选择应用）直接不显示。
describe('PluginWorkspace 插件入口按当前应用过滤（Android Targets）', () => {
  const TARGETS = {
    'gamer-video': ['*'],
    'gamer-yaml': ['com.miHoYo.hkrpg'],
  }

  it('通用（*）插件恒显示；具体目标插件当前应用命中才显示', async () => {
    const wrapper = await mountWorkspace('gamer-video:video', {
      pluginTargets: TARGETS,
      androidPackageName: 'com.miHoYo.hkrpg',
    })
    let items = wrapper.findAll('.workspace-subtab')
    expect(items.map(node => node.text()))
      .toEqual(['视频', '自动化', '模板'])
    // 切换到不匹配的应用：仅 * 插件留在入口
    await wrapper.setProps({ androidPackageName: 'com.other.game' })
    items = wrapper.findAll('.workspace-subtab')
    expect(items.map(node => node.text()))
      .toEqual(['视频'])
    wrapper.unmount()
  })

  it('未选择应用：仅通用（*）插件显示，具体目标插件不出现', async () => {
    const wrapper = await mountWorkspace('gamer-video:video', {
      pluginTargets: TARGETS,
      androidPackageName: '',
    })
    const items = wrapper.findAll('.workspace-subtab')
    expect(items.map(node => node.text()))
      .toEqual(['视频'])
    wrapper.unmount()
  })

  it('targets 数据未到达（未声明）：不误隐藏插件（fail-open）', async () => {
    const wrapper = await mountWorkspace('gamer-video:video')
    const items = wrapper.findAll('.workspace-subtab')
    expect(items.map(node => node.text()))
      .toEqual(['视频', '自动化', '模板'])
    wrapper.unmount()
  })
})
