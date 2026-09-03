// @vitest-environment happy-dom
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import PluginPanelHost from './workspace/PluginPanelHost.vue'
import { manifestPanels } from './workspace/contribution-manager'

// declarative 面板的按钮动作直接调服务端 plugin.call 端点（api.callExtension）
vi.mock('./api', () => ({ api: { callExtension: vi.fn().mockResolvedValue({ ok: true }) } }))
import { api } from './api'

const SCHEMA = {
  description: '可选说明',
  fields: [
    { type: 'text', name: 'api_key', label: 'API Key', placeholder: 'sk-...', default: 'abc' },
    { type: 'number', name: 'threads', label: '线程数', default: 4 },
    { type: 'boolean', name: 'enabled', label: '启用', default: true },
    {
      type: 'select', name: 'mode', label: '模式', default: 'fast',
      options: [{ value: 'fast', label: '快速' }, { value: 'slow', label: '慢速' }],
    },
    { type: 'button', label: '刷新', action: 'refresh' },
  ],
}

function declarativeContribution(overrides = {}) {
  return {
    pluginId: 'com.demo.plugin',
    panelId: 'settings',
    title: '设置',
    location: 'console.right',
    runtime: 'declarative',
    schema: SCHEMA,
    ...overrides,
  }
}

function mountHost(overrides = {}) {
  const bridge = { dispatch: vi.fn().mockResolvedValue({ ok: true }) }
  const wrapper = mount(PluginPanelHost, {
    props: { contribution: declarativeContribution(overrides), bridge },
  })
  return { wrapper, bridge }
}

describe('Declarative 插件面板 Host', () => {
  beforeEach(() => {
    api.callExtension.mockClear()
    api.callExtension.mockResolvedValue({ ok: true })
  })

  it('manifest declarative 贡献的 schema 原样进入面板注册表', () => {
    const panels = manifestPanels({
      id: 'com.demo.plugin',
      ui: { contributions: [{ panel_id: 'settings', title: '设置', runtime: 'declarative', schema: SCHEMA }] },
    })
    expect(panels[0].runtime).toBe('declarative')
    expect(panels[0].schema.fields).toHaveLength(5)
    expect(panels[0].schema.fields[4].action).toBe('refresh')
  })

  it('按 schema 原生渲染表单并用默认值初始化控件', () => {
    const { wrapper } = mountHost()
    const inputs = wrapper.findAll('input')
    // text + number + boolean（checkbox）
    expect(inputs).toHaveLength(3)
    expect(inputs[0].element.value).toBe('abc')
    expect(inputs[0].element.placeholder).toBe('sk-...')
    expect(Number(inputs[1].element.value)).toBe(4)
    expect(inputs[2].element.checked).toBe(true)
    const select = wrapper.find('select')
    expect(select.findAll('option')).toHaveLength(2)
    expect(select.element.value).toBe('fast')
    expect(wrapper.find('form .declarative-description').text()).toBe('可选说明')
    expect(wrapper.find('button.declarative-button').text()).toContain('刷新')
  })

  it('按钮点击调服务端 plugin.call 端点，发送 action 与收集的控件值', async () => {
    const { wrapper } = mountHost()
    const inputs = wrapper.findAll('input')
    await inputs[0].setValue('new-key')
    await inputs[1].setValue('8')
    await inputs[2].setValue(false)
    await wrapper.find('select').setValue('slow')
    await wrapper.find('button.declarative-button').trigger('click')

    expect(api.callExtension).toHaveBeenCalledTimes(1)
    const [pluginId, action, values] = api.callExtension.mock.calls[0]
    expect(pluginId).toBe('com.demo.plugin')
    expect(action).toBe('refresh')
    expect(values).toEqual({
      api_key: 'new-key', threads: 8, enabled: false, mode: 'slow',
    })
    // guest 返回的 JSON 结果展示在面板内
    await vi.waitFor(() => {
      expect(wrapper.find('.plugin-action-result').exists()).toBe(true)
      expect(wrapper.find('.plugin-action-result').text()).toContain('ok')
    })
  })

  it('plugin.call 失败时在面板内显示错误，不抛出未捕获异常', async () => {
    api.callExtension.mockRejectedValue(new Error('插件后端未响应'))
    const { wrapper } = mountHost()
    await wrapper.find('button.declarative-button').trigger('click')
    await vi.waitFor(() => {
      expect(wrapper.find('.plugin-bridge-error').text()).toContain('插件后端未响应')
    })
  })

  it('declarative 无 schema 时保留占位提示', () => {
    const { wrapper } = mountHost({ schema: undefined })
    expect(wrapper.find('.declarative-form').exists()).toBe(false)
    expect(wrapper.text()).toContain('该 declarative 面板未声明表单 schema')
  })
})
