// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import CellEditor from '../components/CellEditor.vue'
import { isRefCell, lit, ref as cellRef } from '../model'

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

describe('CellEditor：v3 引用（属性路径）', () => {
  it('值 ↔ 引用切换；引用为自由路径输入（支持 $前缀粘贴与点路径）', async () => {
    const wrapper = mount(CellEditor, {
      props: { cell: lit([0.5, 0.5]), type: 'coord', label: '坐标', params: [{ type: 'string', name: 'reward', remark: '', default: null, rawForm: false }] },
    })
    await wrapper.findAll('button.mode-btn')[1].trigger('click') // 切引用 → 默认取第一个声明
    expect(wrapper.emitted('change')[0]).toEqual([{ ref: 'reward' }])
    // 受控组件：宿主回写 props.cell 后引用输入框才渲染
    await wrapper.setProps({ cell: { ref: 'reward' } })
    const input = wrapper.find('input.ref-input')
    expect(input.exists()).toBe(true)
    await input.setValue('$reward.center')
    expect(wrapper.emitted('change').at(-1)).toEqual([{ ref: 'reward.center' }])
    // 切回字面量 → 类型默认值
    await wrapper.findAll('button.mode-btn')[0].trigger('click')
    expect(wrapper.emitted('change').at(-1)).toEqual([{ lit: [0.5, 0.5] }])
  })

  it('数组索引路径与非法路径', async () => {
    const wrapper = mount(CellEditor, {
      props: { cell: cellRef('a'), type: 'expr', label: '值' },
    })
    expect(isRefCell(wrapper.props('cell'))).toBe(true)
    await wrapper.find('input.ref-input').setValue('list[0].x')
    expect(wrapper.emitted('change').at(-1)).toEqual([{ ref: 'list[0].x' }])
  })

  it('expr 字面量输入自动识别 true/false/数字', async () => {
    const wrapper = mount(CellEditor, {
      props: { cell: lit(''), type: 'expr', label: '条件' },
    })
    await wrapper.find('input.cell-input').setValue('true')
    expect(wrapper.emitted('change').at(-1)).toEqual([{ lit: true }])
    await wrapper.find('input.cell-input').setValue('42')
    expect(wrapper.emitted('change').at(-1)).toEqual([{ lit: 42 }])
    await wrapper.find('input.cell-input').setValue('hello')
    expect(wrapper.emitted('change').at(-1)).toEqual([{ lit: 'hello' }])
  })

  it('非法引用路径给即时提示', async () => {
    const wrapper = mount(CellEditor, {
      props: { cell: { ref: '1bad' }, type: 'expr', label: '值' },
    })
    expect(wrapper.find('.cell-editor').classes()).toContain('cell-error')
    expect(wrapper.text()).toContain('不是合法属性路径')
  })
})
