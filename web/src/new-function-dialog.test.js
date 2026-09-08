// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import NewFunctionDialog from './components/console/NewFunctionDialog.vue'

async function mountDialog(props = {}) {
  const wrapper = mount(NewFunctionDialog, {
    props: { open: true, categories: ['登录', '日常'], defaultCategory: '日常', ...props },
  })
  await nextTick()
  return wrapper
}

describe('NewFunctionDialog：新建函数弹窗（分类 + 函数名，无文件概念）', () => {
  it('打开时默认选中指定分类，聚焦函数名；提交回传分类与函数名', async () => {
    const wrapper = await mountDialog()
    const selects = wrapper.findAll('select')
    expect(selects).toHaveLength(1)
    expect(selects[0].element.value).toBe('日常')
    // 提交：分类 + 函数名
    await wrapper.findAll('input')[0].setValue('领取奖励')
    await wrapper.find('.btn-primary').trigger('click')
    expect(wrapper.emitted('create')?.[0]).toEqual([{ category: '日常', name: '领取奖励' }])
    wrapper.unmount()
  })

  it('切「＋ 新分类…」变输入框，输入新分类可提交', async () => {
    const wrapper = await mountDialog()
    const select = wrapper.find('select')
    await select.findAll('option').at(-1).setSelected()
    await nextTick()
    // select 消失，出现新分类输入框
    const inputs = wrapper.findAll('input')
    expect(inputs).toHaveLength(2)
    await inputs[0].setValue('战斗')
    await inputs[1].setValue('func1')
    await wrapper.find('.btn-primary').trigger('click')
    expect(wrapper.emitted('create')?.[0]).toEqual([{ category: '战斗', name: 'func1' }])
    wrapper.unmount()
  })

  it('分类或函数名为空时禁止提交；close 经取消按钮', async () => {
    const wrapper = await mountDialog()
    // 函数名为空 → 按钮禁用
    expect(wrapper.find('.btn-primary').attributes('disabled')).toBeDefined()
    await wrapper.findAll('input')[0].setValue('fn')
    expect(wrapper.find('.btn-primary').attributes('disabled')).toBeUndefined()
    await wrapper.find('.nfd-actions .btn:not(.btn-primary)').trigger('click')
    expect(wrapper.emitted('close')).toHaveLength(1)
    wrapper.unmount()
  })

  it('无已有分类时打开直接是新分类输入模式', async () => {
    const wrapper = await mountDialog({ categories: [], defaultCategory: '' })
    await nextTick()
    expect(wrapper.find('select').exists()).toBe(false)
    expect(wrapper.findAll('input')).toHaveLength(2)
    wrapper.unmount()
  })
})
