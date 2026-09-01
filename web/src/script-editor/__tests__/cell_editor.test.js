// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import CellEditor from '../components/CellEditor.vue'
import { lit } from '../model'

describe('CellEditor：模板匹配预览', () => {
  it('模板字段在框选后提供匹配按钮，并只调用宿主匹配工具', async () => {
    const matchTemplate = vi.fn().mockResolvedValue({ hit: true })
    const wrapper = mount(CellEditor, {
      props: { cell: lit('login.png'), type: 'tmpl', label: '主模板' },
      global: { provide: { seCellTools: { matchTemplate } } },
    })

    const button = wrapper.find('button[title*="按步骤实际匹配规则"]')
    expect(button.exists()).toBe(true)
    await button.trigger('click')

    expect(matchTemplate).toHaveBeenCalledTimes(1)
    expect(matchTemplate).toHaveBeenCalledWith('login.png')
  })

  it('模板下拉支持名称子串与中文拼音首字母搜索，并按命中位置排序', async () => {
    const wrapper = mount(CellEditor, {
      props: {
        cell: lit('rc'), type: 'tmpl', label: '主模板',
        templates: ['普通.png', '日常战斗.png', '日常遗器.png'],
      },
    })

    await wrapper.find('.tpl-toggle').trigger('click')
    expect(wrapper.findAll('.tpl-drop-row').map((row) => row.text())).toEqual(['日常战斗.png', '日常遗器.png'])

    await wrapper.setProps({ cell: lit('遗器') })
    expect(wrapper.findAll('.tpl-drop-row').map((row) => row.text())).toEqual(['日常遗器.png'])
  })
})
