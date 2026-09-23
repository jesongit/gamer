// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import CellEditor from '../components/CellEditor.vue'
import { isRefCell, lit, ref as cellRef } from '../model'
import { createCallFromSchema } from '../factories'

describe('CellEditor：模板匹配预览', () => {
  it('find_any 的未填写模板列表首次显示模板控件，添加后可直接选择模板', async () => {
    const step = createCallFromSchema('find_any', [{ name: 'templates', type: 'list', required: true, default: null }])
    const cell = step.args.entries.templates
    const wrapper = mount(CellEditor, {
      props: { cell, type: 'list', itemType: 'template', label: 'templates', templates: ['登录.png'] },
    })
    expect(wrapper.find('textarea').exists()).toBe(false)
    expect(wrapper.text()).toContain('请填写此参数')
    expect(wrapper.emitted('change')).toBeUndefined()
    await wrapper.get('button[aria-label="添加templates"]').trigger('click')
    await wrapper.setProps({ cell: wrapper.emitted('change').at(-1)[0] })
    await wrapper.get('.tpl-toggle').trigger('click')
    await wrapper.get('.tpl-drop-row').trigger('click')
    expect(wrapper.emitted('change').at(-1)[0]).toEqual(lit(['登录.png']))
    // 撤销到未填写状态时，同样无需切换取值方式。
    await wrapper.setProps({ cell })
    expect(wrapper.find('.template-list').exists()).toBe(true)
    expect(wrapper.text()).toContain('请填写此参数')
    // 显式错误值不应被静默当作空列表，通用列表仍使用 JSON 编辑。
    await wrapper.setProps({ cell: lit('错误值') })
    expect(wrapper.find('textarea').exists()).toBe(true)
    await wrapper.setProps({ cell, itemType: '' })
    expect(wrapper.find('textarea').exists()).toBe(true)
    wrapper.unmount()
  })

  it('障碍列表复用模板搜索、悬停预览、匹配和框选，并只更新操作的列表项', async () => {
    const matchTemplate = vi.fn().mockResolvedValue({ hit: true })
    const captureTemplate = vi.fn().mockResolvedValue('新障碍.png')
    const wrapper = mount(CellEditor, {
      props: { cell: lit(['关闭公告.png', '']), type: 'list', itemType: 'template', label: '障碍', templates: ['关闭公告.png', '关闭提示.png'] },
      global: { provide: { seCellTools: { matchTemplate, captureTemplate }, tplPreviewUrl: name => `/thumb/${name}` } },
    })
    const row = wrapper.findAll('.template-list-row')[1]
    await row.get('input[aria-label="障碍 2"]').setValue('gbts')
    await wrapper.setProps({ cell: wrapper.emitted('change').at(-1)[0] })
    const option = row.get('.tpl-drop-row')
    expect(option.text()).toBe('关闭提示.png')
    await option.trigger('mouseenter')
    expect(option.get('img').attributes('src')).toBe('/thumb/关闭提示.png')
    await option.trigger('click')
    expect(wrapper.emitted('change').at(-1)[0]).toEqual(lit(['关闭公告.png', '关闭提示.png']))
    await wrapper.setProps({ cell: wrapper.emitted('change').at(-1)[0] })
    await row.get('button[title*="按步骤实际匹配规则"]').trigger('click')
    expect(matchTemplate).toHaveBeenCalledWith('关闭提示.png', {})
    await row.get('button[title*="在投屏画面框选新模板"]').trigger('click')
    await flushPromises()
    expect(wrapper.emitted('change').at(-1)[0]).toEqual(lit(['关闭公告.png', '新障碍.png']))
  })

  it('列表项编辑保留变量引用与美元字面量的表达式语义', async () => {
    const wrapper = mount(CellEditor, {
      props: { cell: lit(['$closing', '$$literal.png']), type: 'list', itemType: 'template', label: '障碍' },
    })
    await wrapper.get('input[aria-label="障碍 1引用"]').setValue('$popup.close')
    expect(wrapper.emitted('change').at(-1)[0]).toEqual(lit(['$popup.close', '$$literal.png']))
    await wrapper.setProps({ cell: wrapper.emitted('change').at(-1)[0] })
    const literal = wrapper.get('input[aria-label="障碍 2"]')
    expect(literal.element.value).toBe('$literal.png')
    await literal.setValue('$new.png')
    expect(wrapper.emitted('change').at(-1)[0]).toEqual(lit(['$popup.close', '$$new.png']))
  })

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
    expect(matchTemplate).toHaveBeenCalledWith('login.png', {})
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
