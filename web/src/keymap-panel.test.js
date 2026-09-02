// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import KeymapPanel from './components/console/KeymapPanel.vue'

function keymapContext(overrides = {}) {
  return {
    pkg: 'com.demo',
    keymaps: [
      {
        name: '战斗方案',
        version: 3,
        keymap: {
          version: 1,
          name: '战斗方案',
          bindings: [{ key: 'Space', action: { type: 'tap', at: [0.7, 0.8] } }],
        },
      },
    ],
    selectedName: '战斗方案',
    usedName: '战斗方案',
    loading: false,
    saving: false,
    error: '',
    ...overrides,
  }
}

function button(wrapper, text) {
  return wrapper.findAll('button').find(item => item.text() === text)
}

describe('KeymapPanel', () => {
  it('uses the single context prop and shows the no-package state', () => {
    const wrapper = mount(KeymapPanel, { props: { context: { pkg: '', keymaps: [] } } })
    expect(wrapper.get('[data-testid="keymap-no-package"]').text()).toContain('选择包名')
    expect(wrapper.find('[data-testid="keymap-editor"]').exists()).toBe(false)
  })

  it('renders schemes and calls onSelect for the selected list row', async () => {
    const onSelect = vi.fn()
    const context = keymapContext({ onSelect })
    const wrapper = mount(KeymapPanel, { props: { context } })

    expect(wrapper.get('[data-testid="keymap-panel"]').text()).toContain('战斗方案')
    expect(wrapper.get('.using-tag').text()).toBe('使用中')
    await wrapper.get('[data-testid="keymap-scheme-row"]').trigger('click')
    expect(onSelect).toHaveBeenCalledWith(context.keymaps[0])
  })

  it('supports new binding capture and sends a validated save payload', async () => {
    const onNew = vi.fn()
    const onSave = vi.fn(() => true)
    const wrapper = mount(KeymapPanel, {
      props: { context: keymapContext({ onNew, onSave }) },
      attachTo: document.body,
    })

    await button(wrapper, '＋ 新增映射').trigger('click')
    expect(onNew).toHaveBeenCalled()
    await button(wrapper, '＋ 添加绑定').trigger('click')
    const binding = wrapper.get('[data-testid="keymap-binding"]')
    await button(wrapper, '录入按键').trigger('click')
    await new Promise(resolve => setTimeout(resolve, 0))
    expect(binding.get('.key-input').element).toBe(document.activeElement)
    await binding.get('.key-input').trigger('keydown', {
      code: 'KeyQ',
      preventDefault: vi.fn(),
    })
    expect(binding.get('.key-input').element.value).toBe('KeyQ')
    await wrapper.get('[data-testid="keymap-name"]').setValue('新战斗')
    await button(wrapper, '💾 保存方案').trigger('click')

    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      pkg: 'com.demo',
      name: '新战斗',
      model: {
        version: 1,
        name: '新战斗',
        bindings: [{ key: 'KeyQ', action: { type: 'tap', at: [0.5, 0.5] } }],
      },
    }))
    wrapper.unmount()
  })

  it('takes a point from the console and writes it into the draft action', async () => {
    const onRequestPoint = vi.fn(() => Promise.resolve({ x: 0.25, y: 0.75 }))
    const wrapper = mount(KeymapPanel, { props: { context: keymapContext({ onRequestPoint }) } })

    await button(wrapper, '编辑').trigger('click')
    await button(wrapper, '取点').trigger('click')
    await new Promise(resolve => setTimeout(resolve, 0))

    const inputs = wrapper.get('[data-testid="keymap-binding"]').findAll('input')
    expect(onRequestPoint).toHaveBeenCalledWith({ pkg: 'com.demo', index: 0, field: 'at' })
    expect(inputs[1].element.value).toBe('0.25')
    expect(inputs[2].element.value).toBe('0.75')
  })

  it('keeps the editor open and reports failure when save is not confirmed', async () => {
    const onSave = vi.fn(() => false)
    const wrapper = mount(KeymapPanel, { props: { context: keymapContext({ onSave }) } })

    await button(wrapper, '编辑').trigger('click')
    await button(wrapper, '💾 保存方案').trigger('click')

    expect(wrapper.find('[data-testid="keymap-editor"]').exists()).toBe(true)
    expect(wrapper.get('[data-testid="keymap-note"]').text()).toContain('保存失败')
    expect(wrapper.text()).not.toContain('等待服务端确认')
  })

  it('rejects invalid raw YAML before calling onSave and supports cancel', async () => {
    const onSave = vi.fn()
    const onCancel = vi.fn()
    const wrapper = mount(KeymapPanel, { props: { context: keymapContext({ onSave, onCancel }) } })

    await wrapper.get('[data-testid="keymap-scheme-row"]').get('button').trigger('click')
    await button(wrapper, '原文 YAML').trigger('click')
    await wrapper.get('[data-testid="keymap-raw"]').setValue(
      'version: 1\nname: bad\nbindings:\n  - key: Space\n    action:\n      type: tap\n      at: [2, 0]',
    )
    await button(wrapper, '💾 保存方案').trigger('click')
    expect(wrapper.get('[data-testid="keymap-diagnostics"]').text()).toContain('[0, 1]')
    expect(onSave).not.toHaveBeenCalled()

    await button(wrapper, '取消').trigger('click')
    expect(onCancel).toHaveBeenCalled()
    expect(wrapper.find('[data-testid="keymap-editor"]').exists()).toBe(false)
  })

  it('uses a two-step delete and calls onDelete with package and name', async () => {
    const onDelete = vi.fn()
    const wrapper = mount(KeymapPanel, { props: { context: keymapContext({ onDelete }) } })
    const deleteButton = wrapper.get('[data-testid="keymap-scheme-row"]').get('button.danger')

    await deleteButton.trigger('click')
    expect(wrapper.text()).toContain('再次点击确认删除')
    await deleteButton.trigger('click')
    expect(onDelete).toHaveBeenCalledWith(expect.objectContaining({
      pkg: 'com.demo',
      name: '战斗方案',
    }))
  })
})
