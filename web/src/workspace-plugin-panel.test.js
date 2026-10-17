// @vitest-environment happy-dom
import { afterEach, describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { defineComponent, nextTick, shallowRef } from 'vue'
import PluginWorkspace from './workspace/PluginWorkspace.vue'

afterEach(() => localStorage.removeItem('gamer.workbench.tab-order'))

it('同一插件的页签排序重挂载后保留，恢复默认不切换面板', async () => {
  let wrapper = await mountWorkspace('gamer-yaml:templates')
  const tabs = wrapper.findAll('.workspace-subtab')
  await tabs[1].trigger('dragstart', { dataTransfer: { setData() {} } })
  await tabs[0].trigger('drop', { clientX: -1 })
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['模板', '自动化'])
  expect(wrapper.emitted('select')).toBeUndefined()
  wrapper.unmount()
  wrapper = await mountWorkspace('gamer-yaml:templates')
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['模板', '自动化'])
  await wrapper.get('[aria-label="恢复默认页签顺序"]').trigger('click')
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['自动化', '模板'])
  expect(wrapper.find('.templates-stub').exists()).toBe(true)
  wrapper.unmount()
})

it('键盘排序只在当前插件内移动，应用过滤后重新出现仍保留顺序', async () => {
  const wrapper = await mountWorkspace('gamer-yaml:automation', { pluginTargets: { 'gamer-yaml': ['game.app'] }, androidPackageName: 'game.app' })
  await wrapper.findAll('.workspace-subtab')[0].trigger('keydown', { key: 'ArrowRight', altKey: true, shiftKey: true })
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['模板', '自动化'])
  await wrapper.setProps({ androidPackageName: 'other.app' })
  expect(wrapper.find('.automation-stub').exists()).toBe(false)
  expect(wrapper.emitted('fallback')?.at(-1)).toEqual(['gamer-video:video'])
  await wrapper.setProps({ activePanel: 'gamer-video:video' })
  expect(wrapper.find('.plugin-subnav-tabs').exists()).toBe(false)
  await wrapper.setProps({ androidPackageName: 'game.app', activePanel: 'gamer-yaml:automation' })
  expect(wrapper.findAll('.workspace-subtab').map(tab => tab.text())).toEqual(['模板', '自动化'])
  wrapper.unmount()
})

// 回归锁定（浏览器冒烟发现）：业务面板 key（gamer.<plugin>:<panel>）激活时
// 必须解析渲染面板组件本体，而不是退化到插件选择器兜底（activeTop 旧逻辑把
// 所有非 core key 兜底成 'plugins'，导致面板永不渲染，仅选择器可见）。
// 导航契约：插件入口列「插件本体」（显示名），选中插件后多功能插件在主导航
// 下一行渲染功能子页签条，单面板插件不显示子页签行。

const VideoPanelStub = defineComponent({
  name: 'VideoPanelStub',
  data: () => ({ draft: '' }),
  template: '<div class="video-stub">VIDEO_PANEL_OK<input v-model="draft" aria-label="插件草稿" /></div>',
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
    { key: 'gamer-video:video', title: '视频', pluginId: 'gamer-video', runtime: 'core', component: VideoPanelStub, panelClass: 'video-tab', keepAlive: 'session' },
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
    expect(wrapper.find('.plugin-picker').exists()).toBe(true)
    expect(wrapper.find('.video-stub').exists()).toBe(true)
    expect(wrapper.text()).toContain('VIDEO_PANEL_OK')
    // 主导航「插件」页签高亮（activeTab 映射回挂靠页签）
    const activeTop = wrapper.find('.workspace-tab.active')
    expect(activeTop.exists()).toBe(true)
    expect(activeTop.text()).toContain('工作台')
    wrapper.unmount()
  })

  it('单功能插件保留命名选择器，不占用重复的功能页签', async () => {
    const wrapper = await mountWorkspace('gamer-video:video')
    expect(wrapper.get('[aria-label="当前插件"]').element.value).toBe('gamer-video')
    expect(wrapper.find('.plugin-subnav-tabs').exists()).toBe(false)
    wrapper.unmount()
  })

  it('多面板插件激活：主导航下一行渲染功能子页签条且当前功能高亮', async () => {
    const wrapper = await mountWorkspace('gamer-yaml:templates')
    expect(wrapper.find('.plugin-picker').exists()).toBe(true)
    const subtabs = wrapper.findAll('.workspace-subtab')
    expect(subtabs.length).toBe(2)
    expect(subtabs.map(node => node.text())).toEqual(['自动化', '模板'])
    expect(wrapper.find('.workspace-subtab.active').text()).toBe('模板')
    expect(wrapper.find('.templates-stub').exists()).toBe(true)
    // 子页签可切换功能
    await subtabs[0].trigger('click')
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
    expect(wrapper.findAll('.workspace-tab').map(tab => tab.text())).toEqual(['工作台', '任务', '运行记录', '配置包', '插件', '设置'])
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
    let items = wrapper.findAll('[aria-label="当前插件"] option')
    expect(items.map(node => node.text()))
      .toEqual(['视频工作台', '自动化'])
    // 切换到不匹配的应用：仅 * 插件留在入口
    await wrapper.setProps({ androidPackageName: 'com.other.game' })
    items = wrapper.findAll('[aria-label="当前插件"] option')
    expect(items.map(node => node.text()))
      .toEqual(['视频工作台'])
    wrapper.unmount()
  })

  it('未选择应用：仅通用（*）插件显示，具体目标插件不出现', async () => {
    const wrapper = await mountWorkspace('gamer-video:video', {
      pluginTargets: TARGETS,
      androidPackageName: '',
    })
    const items = wrapper.findAll('[aria-label="当前插件"] option')
    expect(items.map(node => node.text()))
      .toEqual(['视频工作台'])
    wrapper.unmount()
  })

  it('targets 数据未到达（未声明）：不误隐藏插件（fail-open）', async () => {
    const wrapper = await mountWorkspace('gamer-video:video')
    const items = wrapper.findAll('[aria-label="当前插件"] option')
    expect(items.map(node => node.text()))
      .toEqual(['视频工作台', '自动化'])
    wrapper.unmount()
  })
})

it('插件选择回到该插件最近面板，放大与还原不切换或停止当前功能', async () => {
  const wrapper = await mountWorkspace('gamer-yaml:templates')
  await wrapper.get('[aria-label="当前插件"]').setValue('gamer-video')
  expect(wrapper.emitted('select')?.at(-1)).toEqual(['gamer-video:video'])
  await wrapper.setProps({ activePanel: 'gamer-video:video' })
  await wrapper.get('[aria-label="当前插件"]').setValue('gamer-yaml')
  expect(wrapper.emitted('select')?.at(-1)).toEqual(['gamer-yaml:templates'])
  expect(wrapper.find('[aria-label="收起插件区"]').exists()).toBe(false)
  await wrapper.get('[aria-label="放大插件区"]').trigger('click')
  expect(wrapper.emitted('expand')).toHaveLength(1)
  expect(wrapper.find('.video-stub').exists()).toBe(true)
  await wrapper.setProps({ expanded: true })
  expect(wrapper.find('[aria-label="还原插件区"]').exists()).toBe(true)
  wrapper.unmount()
})

it('切换到声明式及全局页保留会话草稿，移除贡献后缓存随之释放', async () => {
  const base = makeRegistry()
  const video = base.resolve('gamer-video:video')
  const panels = shallowRef([...base.getPanels(), { key: 'sample:form', title: '表单', pluginId: 'sample', runtime: 'declarative', keepAlive: 'none', schema: { fields: [{ name: 'note', label: '备注', type: 'text' }] } }])
  const registry = { getPanels: () => panels.value, resolve: key => panels.value.find(item => item.key === key) || null }
  const wrapper = await mountWorkspace('gamer-video:video', { registry, context: { uiBridge: { call: async () => ({}) } } })
  await wrapper.get('[aria-label="插件草稿"]').setValue('保留草稿')
  await wrapper.setProps({ activePanel: 'sample:form' })
  expect(wrapper.find('.declarative-form').exists()).toBe(true)
  await wrapper.setProps({ activePanel: 'gamer.core:tasks' })
  expect(wrapper.text()).toContain('TASKS_OK')
  await wrapper.setProps({ activePanel: 'gamer-video:video' })
  expect(wrapper.get('[aria-label="插件草稿"]').element.value).toBe('保留草稿')
  panels.value = panels.value.filter(item => item.key !== video.key)
  await nextTick()
  expect(wrapper.find('.video-stub').exists()).toBe(false)
  panels.value = [...panels.value, video]
  await nextTick()
  expect(wrapper.get('[aria-label="插件草稿"]').element.value).toBe('')
  wrapper.unmount()
})
